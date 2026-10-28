use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, Weak},
};

use etas_code::{
    CodeFingerprint,
    diagnostic::{
        DiagnosticDetails, DiagnosticLocation, DiagnosticRecord, DiagnosticSeverity,
        RelatedLocation,
    },
    syntax::{
        CodeSyntaxOperations, ParseBodyFailure, ParseError, SyntaxBlockRef, SyntaxIdentity,
        SyntaxObjectLease, SyntaxOperationError, SyntaxResource,
    },
};
use etas_core::{SourceFile, SourceId, memory::MemoryFootprint};
use etas_syntax::{
    BodyFormatError, BodyFormatOptions, BodyParseLimits, BodyResourceKind, BodySyntax,
    BoundedBodyParseError, ProgramBodySelection, RetainedBodyError, SourceBodySyntax,
    SourceBodyView,
};

mod retained;
pub(crate) use retained::validate_retained_source;

#[derive(Clone, Copy, Debug)]
pub struct NativeSyntaxLimits {
    pub parse: BodyParseLimits,
    pub relocation: etas_syntax::BodyRelocationLimits,
    pub max_retained_objects: usize,
    pub max_retained_bytes: u64,
    pub max_formatted_bytes: usize,
}

impl Default for NativeSyntaxLimits {
    fn default() -> Self {
        Self {
            parse: BodyParseLimits::default(),
            relocation: etas_syntax::BodyRelocationLimits::default(),
            max_retained_objects: 256,
            max_retained_bytes: 32 * 1024 * 1024,
            max_formatted_bytes: 2 * 1024 * 1024,
        }
    }
}

/// Owns only immutable native syntax, never project files, compiler processes,
/// caller locals, authority, or an executable revision.
pub struct NativeCodeSyntaxOperations {
    limits: NativeSyntaxLimits,
    state: Mutex<SyntaxStore>,
}

#[derive(Default)]
struct SyntaxStore {
    objects: HashMap<SyntaxIdentity, StoredSyntax>,
    next_source: u32,
}

struct StoredSyntax {
    object: Weak<NativeSyntaxObject>,
    validated_parse: BodyParseLimits,
}

#[derive(Debug, etas_core::RetainedSize)]
struct NativeSyntaxObject {
    identity: SyntaxIdentity,
    syntax: NativeSyntaxPayload,
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
enum NativeSyntaxPayload {
    Fragment(Arc<BodySyntax>),
    Source(Arc<SourceBodySyntax>),
    Retained {
        syntax: Arc<etas_syntax::RetainedBodyFragment>,
        origin: retained::RetainedBodyOrigin,
    },
}

impl SyntaxObjectLease for NativeSyntaxObject {
    fn identity(&self) -> SyntaxIdentity {
        self.identity
    }
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct NativeBodyLease(Arc<NativeSyntaxObject>);

impl NativeBodyLease {
    /// SourceId belongs to this syntax backend. Candidate overlays must register
    /// and relocate it in the candidate source map before lowering into HIR.
    pub fn body(&self) -> Result<SourceBodyView<'_>, SyntaxOperationError> {
        match &self.0.syntax {
            NativeSyntaxPayload::Fragment(syntax) => Ok(SourceBodyView::Block(syntax.block())),
            NativeSyntaxPayload::Source(syntax) => syntax.body().map_err(retained_body_error),
            NativeSyntaxPayload::Retained { syntax, .. } => Ok(syntax.body()),
        }
    }

    pub fn source(&self) -> &SourceFile {
        match &self.0.syntax {
            NativeSyntaxPayload::Fragment(syntax) => syntax.source(),
            NativeSyntaxPayload::Source(syntax) => syntax.source(),
            NativeSyntaxPayload::Retained { syntax, .. } => syntax.source(),
        }
    }
}

impl NativeCodeSyntaxOperations {
    pub fn new(limits: NativeSyntaxLimits) -> Self {
        Self {
            limits,
            state: Mutex::new(SyntaxStore::default()),
        }
    }

    pub fn resolve(
        &self,
        reference: &SyntaxBlockRef,
    ) -> Result<NativeBodyLease, SyntaxOperationError> {
        let state = self.lock()?;
        let object = state
            .objects
            .get(&reference.identity())
            .and_then(|entry| entry.object.upgrade())
            .filter(|object| reference.is_backed_by(object))
            .ok_or(SyntaxOperationError::ForeignSyntaxObject)?;
        Ok(NativeBodyLease(object))
    }

    /// Register the returned source in the candidate's source bundle before
    /// lowering. This is an AST transform, never a format/parse round trip.
    pub fn relocate_body(
        &self,
        reference: &SyntaxBlockRef,
        source: SourceId,
    ) -> Result<Arc<etas_syntax::RelocatedBodySyntax>, SyntaxOperationError> {
        let lease = self.resolve(reference)?;
        let handler = match &lease.0.syntax {
            NativeSyntaxPayload::Source(syntax) => syntax.trailing_handler(),
            NativeSyntaxPayload::Retained { syntax, .. } => syntax.trailing_handler(),
            NativeSyntaxPayload::Fragment(_) => None,
        };
        etas_syntax::RelocatedBodySyntax::new(
            lease.body()?,
            handler,
            lease.source(),
            source,
            self.limits.relocation,
        )
        .map_err(|error| match error {
            etas_syntax::RelocatedBodyError::Resource(error) => {
                SyntaxOperationError::ResourceLimit {
                    resource: match error.resource {
                        etas_syntax::BodyRelocationResource::SourceBytes => {
                            SyntaxResource::SourceBytes
                        }
                        etas_syntax::BodyRelocationResource::AstNodes => SyntaxResource::AstNodes,
                        etas_syntax::BodyRelocationResource::Nesting => SyntaxResource::Nesting,
                    },
                    limit: error.limit as u64,
                }
            }
            etas_syntax::RelocatedBodyError::InvalidSourceReference => {
                SyntaxOperationError::BackendUnavailable {
                    reason: "native body contains an invalid source reference".into(),
                }
            }
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, SyntaxStore>, SyntaxOperationError> {
        self.state
            .lock()
            .map_err(|_| SyntaxOperationError::BackendUnavailable {
                reason: "native syntax object store lock is poisoned".into(),
            })
    }

    fn publish(
        &self,
        identity: SyntaxIdentity,
        syntax: NativeSyntaxPayload,
    ) -> Result<SyntaxBlockRef, SyntaxOperationError> {
        self.publish_with_limits(identity, syntax, self.limits)
    }

    fn publish_with_limits(
        &self,
        identity: SyntaxIdentity,
        syntax: NativeSyntaxPayload,
        limits: NativeSyntaxLimits,
    ) -> Result<SyntaxBlockRef, SyntaxOperationError> {
        let object = Arc::new(NativeSyntaxObject { identity, syntax });
        let mut state = self.lock()?;
        state
            .objects
            .retain(|_, entry| entry.object.strong_count() > 0);
        let existing = state
            .objects
            .get(&identity)
            .and_then(|entry| entry.object.upgrade());
        if state.objects.len() + usize::from(existing.is_none()) > limits.max_retained_objects {
            return Err(SyntaxOperationError::ResourceLimit {
                resource: SyntaxResource::RetainedObjects,
                limit: limits.max_retained_objects as u64,
            });
        }
        let mut objects = state
            .objects
            .values()
            .filter_map(|entry| entry.object.upgrade())
            .collect::<Vec<_>>();
        if existing.is_none() {
            objects.push(object.clone());
        }
        let retained = MemoryFootprint::of(&objects)
            .estimated_bytes()
            .saturating_sub(
                (objects.capacity() * std::mem::size_of::<Arc<NativeSyntaxObject>>()) as u64,
            );
        if retained > limits.max_retained_bytes {
            return Err(SyntaxOperationError::ResourceLimit {
                resource: SyntaxResource::RetainedBytes,
                limit: limits.max_retained_bytes,
            });
        }
        if let Some(existing) = existing {
            let entry = state.objects.get_mut(&identity).ok_or_else(|| {
                SyntaxOperationError::BackendUnavailable {
                    reason: "native syntax entry disappeared during publication".into(),
                }
            })?;
            entry.validated_parse = stricter_parse(entry.validated_parse, limits.parse);
            return Ok(SyntaxBlockRef::from_backend(existing));
        }
        state.objects.insert(
            identity,
            StoredSyntax {
                object: Arc::downgrade(&object),
                validated_parse: limits.parse,
            },
        );
        Ok(SyntaxBlockRef::from_backend(object))
    }

    pub fn retain_source_body(
        &self,
        owner: CodeFingerprint,
        source: Arc<SourceFile>,
        program: Arc<etas_syntax::Parse<etas_syntax::ast::Program>>,
        selection: ProgramBodySelection,
    ) -> Result<SyntaxBlockRef, SyntaxOperationError> {
        let syntax = SourceBodySyntax::new(source, program, selection, self.limits.parse)
            .map_err(retained_body_error)?;
        let identity = source_body_identity(owner, &syntax)?;
        self.publish(identity, NativeSyntaxPayload::Source(syntax))
    }

    pub(crate) fn source_body_identity(
        &self,
        owner: CodeFingerprint,
        source: Arc<SourceFile>,
        program: Arc<etas_syntax::Parse<etas_syntax::ast::Program>>,
        selection: ProgramBodySelection,
    ) -> Result<SyntaxIdentity, SyntaxOperationError> {
        let syntax = SourceBodySyntax::new(source, program, selection, self.limits.parse)
            .map_err(retained_body_error)?;
        source_body_identity(owner, &syntax)
    }
}

fn source_body_identity(
    owner: CodeFingerprint,
    syntax: &SourceBodySyntax,
) -> Result<SyntaxIdentity, SyntaxOperationError> {
    retained::source_identity(owner, syntax)
}

impl Default for NativeCodeSyntaxOperations {
    fn default() -> Self {
        Self::new(NativeSyntaxLimits::default())
    }
}

impl CodeSyntaxOperations for NativeCodeSyntaxOperations {
    fn parse_body(&self, source: &str) -> Result<SyntaxBlockRef, ParseBodyFailure> {
        self.parse_body_bounded(source, self.limits)
    }

    fn parse_body_controlled(
        &self,
        source: &str,
        control: &dyn etas_utils::PipelineExecutionControl,
    ) -> Result<SyntaxBlockRef, ParseBodyFailure> {
        self.parse_body_bounded_controlled(source, self.limits, control)
    }

    fn format(&self, syntax: &SyntaxBlockRef) -> Result<String, SyntaxOperationError> {
        self.format_bounded(syntax, self.limits.max_formatted_bytes)
    }

    fn format_bounded(
        &self,
        syntax: &SyntaxBlockRef,
        max_output_bytes: usize,
    ) -> Result<String, SyntaxOperationError> {
        self.format_native(syntax, max_output_bytes)
    }

    fn format_bounded_controlled(
        &self,
        syntax: &SyntaxBlockRef,
        max_output_bytes: usize,
        control: &dyn etas_utils::PipelineExecutionControl,
    ) -> Result<String, SyntaxOperationError> {
        self.format_native_with_checkpoint(syntax, max_output_bytes, &|| {
            control
                .checkpoint()
                .map_err(SyntaxOperationError::Interrupted)
        })
    }
}

impl NativeCodeSyntaxOperations {
    pub fn parse_body_bounded(
        &self,
        source: &str,
        limits: NativeSyntaxLimits,
    ) -> Result<SyntaxBlockRef, ParseBodyFailure> {
        self.parse_native(source, limits, &|| Ok(()))
    }

    pub fn parse_body_bounded_controlled(
        &self,
        source: &str,
        limits: NativeSyntaxLimits,
        control: &dyn etas_utils::PipelineExecutionControl,
    ) -> Result<SyntaxBlockRef, ParseBodyFailure> {
        self.parse_native(source, limits, &|| {
            control
                .checkpoint()
                .map_err(SyntaxOperationError::Interrupted)
        })
    }

    fn parse_native(
        &self,
        source: &str,
        limits: NativeSyntaxLimits,
        checkpoint: &impl Fn() -> Result<(), SyntaxOperationError>,
    ) -> Result<SyntaxBlockRef, ParseBodyFailure> {
        checkpoint().map_err(ParseBodyFailure::Operation)?;
        let limits = NativeSyntaxLimits {
            parse: stricter_parse(self.limits.parse, limits.parse),
            max_retained_objects: self
                .limits
                .max_retained_objects
                .min(limits.max_retained_objects),
            max_retained_bytes: self
                .limits
                .max_retained_bytes
                .min(limits.max_retained_bytes),
            ..self.limits
        };
        let max_source_bytes = limits.parse.max_source_bytes.min(u32::MAX as usize);
        if source.len() > max_source_bytes {
            return Err(ParseBodyFailure::Operation(
                SyntaxOperationError::ResourceLimit {
                    resource: SyntaxResource::SourceBytes,
                    limit: max_source_bytes as u64,
                },
            ));
        }
        let identity = SyntaxIdentity::new(
            CodeFingerprint::of_bytes("etas.code.generated-source.v1", source.as_bytes()),
            CodeFingerprint::of_bytes("etas.code.body-content.v1", source.as_bytes()),
        );
        checkpoint().map_err(ParseBodyFailure::Operation)?;
        let cached = {
            let state = self.lock().map_err(ParseBodyFailure::Operation)?;
            state.objects.get(&identity).and_then(|entry| {
                let previous = entry.validated_parse;
                (previous.max_source_bytes <= limits.parse.max_source_bytes
                    && previous.max_tokens <= limits.parse.max_tokens
                    && previous.max_recursion <= limits.parse.max_recursion)
                    .then(|| entry.object.upgrade())
                    .flatten()
            })
        };
        if let Some(object) = cached {
            checkpoint().map_err(ParseBodyFailure::Operation)?;
            if limits.max_retained_objects == self.limits.max_retained_objects
                && limits.max_retained_bytes == self.limits.max_retained_bytes
            {
                // The immutable object already passed the store's unchanged
                // retention ceilings; only stricter invocation ceilings need a walk.
                return Ok(SyntaxBlockRef::from_backend(object));
            }
            return self
                .publish_with_limits(identity, object.syntax.clone(), limits)
                .map_err(ParseBodyFailure::Operation);
        }
        let source_id = self.next_source_id().map_err(ParseBodyFailure::Operation)?;
        let syntax = BodySyntax::parse_controlled(
            SourceFile::new(source_id, None, source),
            limits.parse,
            checkpoint,
        )
        .map_err(|error| match error {
            BoundedBodyParseError::Interrupted(error) => ParseBodyFailure::Operation(error),
            BoundedBodyParseError::Syntax(error) => parse_failure(error, identity),
            BoundedBodyParseError::Resource(error) => {
                ParseBodyFailure::Operation(SyntaxOperationError::ResourceLimit {
                    resource: match error.kind {
                        BodyResourceKind::SourceBytes => SyntaxResource::SourceBytes,
                        BodyResourceKind::Tokens => SyntaxResource::Tokens,
                        BodyResourceKind::Recursion => SyntaxResource::Nesting,
                    },
                    limit: error.limit as u64,
                })
            }
        })?;
        checkpoint().map_err(ParseBodyFailure::Operation)?;
        let result = self
            .publish_with_limits(identity, NativeSyntaxPayload::Fragment(syntax), limits)
            .map_err(ParseBodyFailure::Operation)?;
        checkpoint().map_err(ParseBodyFailure::Operation)?;
        Ok(result)
    }

    fn format_native(
        &self,
        syntax: &SyntaxBlockRef,
        max_output_bytes: usize,
    ) -> Result<String, SyntaxOperationError> {
        self.format_native_with_checkpoint(syntax, max_output_bytes, &|| Ok(()))
    }

    fn format_native_with_checkpoint(
        &self,
        syntax: &SyntaxBlockRef,
        max_output_bytes: usize,
        checkpoint: &impl Fn() -> Result<(), SyntaxOperationError>,
    ) -> Result<String, SyntaxOperationError> {
        checkpoint()?;
        let syntax = self.resolve(syntax)?;
        let options = BodyFormatOptions {
            max_output_bytes: self.limits.max_formatted_bytes.min(max_output_bytes),
            ..BodyFormatOptions::default()
        };
        match &syntax.0.syntax {
            NativeSyntaxPayload::Fragment(syntax) => syntax.format_controlled(options, checkpoint),
            NativeSyntaxPayload::Source(syntax) => syntax.format_controlled(options, checkpoint),
            NativeSyntaxPayload::Retained { syntax, .. } => {
                syntax.format_controlled(options, checkpoint)
            }
        }
        .map_err(|error| match error {
            etas_syntax::ControlledBodyFormatError::Interrupted(error) => error,
            etas_syntax::ControlledBodyFormatError::Format(BodyFormatError::OutputLimit {
                limit,
            }) => SyntaxOperationError::ResourceLimit {
                resource: SyntaxResource::FormattedBytes,
                limit: limit as u64,
            },
            etas_syntax::ControlledBodyFormatError::Format(
                BodyFormatError::InvalidSourceReference(_),
            ) => SyntaxOperationError::BackendUnavailable {
                reason: "retained body token references a different source".into(),
            },
        })
    }
}

fn stricter_parse(left: BodyParseLimits, right: BodyParseLimits) -> BodyParseLimits {
    BodyParseLimits {
        max_source_bytes: left.max_source_bytes.min(right.max_source_bytes),
        max_tokens: left.max_tokens.min(right.max_tokens),
        max_recursion: left.max_recursion.min(right.max_recursion),
    }
}

fn retained_body_error(error: RetainedBodyError) -> SyntaxOperationError {
    match error {
        RetainedBodyError::Resource(error) => SyntaxOperationError::ResourceLimit {
            resource: match error.kind {
                BodyResourceKind::SourceBytes => SyntaxResource::SourceBytes,
                BodyResourceKind::Tokens => SyntaxResource::Tokens,
                BodyResourceKind::Recursion => SyntaxResource::Nesting,
            },
            limit: error.limit as u64,
        },
        error => SyntaxOperationError::BackendUnavailable {
            reason: format!("retained native body is inconsistent: {error:?}"),
        },
    }
}

fn parse_failure(error: etas_syntax::BodyParseError, identity: SyntaxIdentity) -> ParseBodyFailure {
    let location = |span: etas_core::Span| {
        if span.source != error.source.id || span.range.end.to_usize() > error.source.text().len() {
            return Err(SyntaxOperationError::BackendUnavailable {
                reason: "native parser returned a diagnostic for another source".into(),
            });
        }
        Ok(DiagnosticLocation {
            source: identity.source(),
            revision: identity.content(),
            byte_start: span.range.start.to_usize(),
            byte_end: span.range.end.to_usize(),
        })
    };
    let records = error
        .diagnostics
        .into_iter()
        .map(|diagnostic| {
            let etas_core::DiagnosticCode::Syntax(code) = diagnostic.code else {
                return Err(SyntaxOperationError::BackendUnavailable {
                    reason: "native body parser returned a non-syntax diagnostic".into(),
                });
            };
            let related = diagnostic
                .labels
                .into_iter()
                .filter(|label| label.style == etas_core::LabelStyle::Secondary)
                .map(|label| {
                    Ok(RelatedLocation {
                        location: location(label.span)?,
                        message: label.message,
                    })
                })
                .collect::<Result<Vec<_>, SyntaxOperationError>>()?;
            Ok(DiagnosticRecord {
                code: syntax_diagnostic_code(code).into(),
                severity: match diagnostic.severity {
                    etas_core::Severity::Error => DiagnosticSeverity::Error,
                    etas_core::Severity::Warning => DiagnosticSeverity::Warning,
                    etas_core::Severity::Info | etas_core::Severity::Hint => {
                        DiagnosticSeverity::Note
                    }
                },
                message: diagnostic.message,
                location: Some(location(diagnostic.primary.span)?),
                details: (!related.is_empty()).then(|| {
                    Arc::new(DiagnosticDetails {
                        related,
                        ..DiagnosticDetails::default()
                    })
                }),
            })
        })
        .collect::<Result<Vec<_>, SyntaxOperationError>>();
    match records {
        Ok(diagnostics) => ParseBodyFailure::InvalidSyntax(ParseError { diagnostics }),
        Err(error) => ParseBodyFailure::Operation(error),
    }
}

fn syntax_diagnostic_code(code: etas_core::SyntaxDiagnosticCode) -> &'static str {
    use etas_core::SyntaxDiagnosticCode::*;
    match code {
        UnexpectedToken => "syntax::UnexpectedToken",
        UnexpectedEof => "syntax::UnexpectedEof",
        MissingToken => "syntax::MissingToken",
        UnclosedDelimiter => "syntax::UnclosedDelimiter",
        InvalidLiteral => "syntax::InvalidLiteral",
        UnterminatedString => "syntax::UnterminatedString",
        UnterminatedBlockComment => "syntax::UnterminatedBlockComment",
        InvalidItem => "syntax::InvalidItem",
        InvalidType => "syntax::InvalidType",
        InvalidExpression => "syntax::InvalidExpression",
        InvalidPattern => "syntax::InvalidPattern",
    }
}
