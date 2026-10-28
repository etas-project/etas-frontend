use super::*;
use etas_code::syntax::{RETAINED_BODY_SOURCE_SCHEMA, RetainedBodyForm as F, RetainedBodySource};

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub(super) struct RetainedBodyOrigin {
    schema: u32,
    form: F,
    source_unit: CodeFingerprint,
    source_length: u64,
    source_start: u64,
    content: CodeFingerprint,
}

impl RetainedBodyOrigin {
    fn from_source(source: &RetainedBodySource) -> Self {
        Self {
            schema: source.schema,
            form: source.form,
            source_unit: source.source_unit,
            source_length: source.source_length,
            source_start: source.source_start,
            content: source.content,
        }
    }
    fn with_text(&self, text: &str) -> RetainedBodySource {
        RetainedBodySource {
            schema: self.schema,
            form: self.form,
            source_unit: self.source_unit,
            source_length: self.source_length,
            source_start: self.source_start,
            content: self.content,
            text: text.to_owned(),
        }
    }
}

impl NativeCodeSyntaxOperations {
    pub(crate) fn matches_source_body(
        &self,
        body: &SyntaxBlockRef,
        source: Arc<SourceFile>,
        program: Arc<etas_syntax::Parse<etas_syntax::ast::Program>>,
        selection: ProgramBodySelection,
    ) -> Result<bool, SyntaxOperationError> {
        let original = SourceBodySyntax::new(source, program, selection, self.limits.parse)
            .map_err(retained_body_error)?;
        let original = source_parts(&original)?;
        let lease = self.resolve(body)?;
        let (form, text) = match &lease.0.syntax {
            NativeSyntaxPayload::Source(syntax) => {
                let parts = source_parts(syntax)?;
                (parts.form, parts.text)
            }
            NativeSyntaxPayload::Fragment(syntax) => (F::BlockContents, syntax.source().text()),
            NativeSyntaxPayload::Retained { syntax, .. } => {
                (retained_form(syntax), syntax.source().text())
            }
        };
        Ok(original.form == form && original.text == text)
    }

    pub fn export_body_source(
        &self,
        body: &SyntaxBlockRef,
    ) -> Result<RetainedBodySource, SyntaxOperationError> {
        self.export_body_source_bounded(body, self.limits.parse.max_source_bytes)
    }

    pub fn export_body_source_bounded(
        &self,
        body: &SyntaxBlockRef,
        max_source_bytes: usize,
    ) -> Result<RetainedBodySource, SyntaxOperationError> {
        let lease = self.resolve(body)?;
        let limit = max_source_bytes.min(self.limits.parse.max_source_bytes);
        let length = match &lease.0.syntax {
            NativeSyntaxPayload::Source(syntax) => source_parts(syntax)?.text.len(),
            NativeSyntaxPayload::Fragment(syntax) => syntax.source().text().len(),
            NativeSyntaxPayload::Retained { syntax, .. } => syntax.source().text().len(),
        };
        if length > limit {
            return Err(SyntaxOperationError::ResourceLimit {
                resource: SyntaxResource::SourceBytes,
                limit: limit as u64,
            });
        }
        let source = match &lease.0.syntax {
            NativeSyntaxPayload::Source(syntax) => source_info(syntax)?,
            NativeSyntaxPayload::Fragment(syntax) => {
                detached_source(syntax.source().text(), F::BlockContents)
            }
            NativeSyntaxPayload::Retained { syntax, origin } => {
                origin.with_text(syntax.source().text())
            }
        };
        if source.content != body.identity().content() {
            return Err(invalid(
                "exported source does not match its native syntax content",
            ));
        }
        Ok(source)
    }

    pub(crate) fn export_source_body(
        &self,
        source: Arc<SourceFile>,
        program: Arc<etas_syntax::Parse<etas_syntax::ast::Program>>,
        selection: ProgramBodySelection,
    ) -> Result<RetainedBodySource, SyntaxOperationError> {
        let syntax = SourceBodySyntax::new(source, program, selection, self.limits.parse)
            .map_err(retained_body_error)?;
        source_info(&syntax)
    }

    /// Parsing an approved body is pure. This creates no native compilation
    /// environment, resolves no imports, and cannot publish executable code.
    pub fn import_retained_body(
        &self,
        callable: &etas_code::CallableId,
        source: &RetainedBodySource,
    ) -> Result<SyntaxBlockRef, SyntaxOperationError> {
        self.import_retained_body_with_checkpoint(callable, source, &|| Ok(()))
    }

    pub fn import_retained_body_controlled(
        &self,
        callable: &etas_code::CallableId,
        source: &RetainedBodySource,
        control: &dyn etas_utils::PipelineExecutionControl,
    ) -> Result<SyntaxBlockRef, SyntaxOperationError> {
        self.import_retained_body_with_checkpoint(callable, source, &|| {
            control
                .checkpoint()
                .map_err(SyntaxOperationError::Interrupted)
        })
    }

    fn import_retained_body_with_checkpoint(
        &self,
        callable: &etas_code::CallableId,
        source: &RetainedBodySource,
        checkpoint: &impl Fn() -> Result<(), SyntaxOperationError>,
    ) -> Result<SyntaxBlockRef, SyntaxOperationError> {
        checkpoint()?;
        let limit = self.limits.parse.max_source_bytes.min(u32::MAX as usize);
        if source.text.len() > limit {
            return Err(SyntaxOperationError::ResourceLimit {
                resource: SyntaxResource::SourceBytes,
                limit: limit as u64,
            });
        }
        let identity = source.identity(callable.fingerprint()).map_err(invalid)?;
        let cached = self
            .lock()?
            .objects
            .get(&identity)
            .and_then(|entry| entry.object.upgrade());
        if let Some(object) = cached {
            checkpoint()?;
            return Ok(SyntaxBlockRef::from_backend(object));
        }
        let source_id = self.next_source_id()?;
        let syntax = etas_syntax::RetainedBodyFragment::parse_controlled(
            SourceFile::new(source_id, None, source.text.clone()),
            fragment_form(source.form),
            self.limits.parse,
            checkpoint,
        )
        .map_err(|error| match error {
            BoundedBodyParseError::Interrupted(error) => error,
            BoundedBodyParseError::Syntax(_) => invalid("retained body source has syntax errors"),
            BoundedBodyParseError::Resource(error) => SyntaxOperationError::ResourceLimit {
                resource: match error.kind {
                    BodyResourceKind::SourceBytes => SyntaxResource::SourceBytes,
                    BodyResourceKind::Tokens => SyntaxResource::Tokens,
                    BodyResourceKind::Recursion => SyntaxResource::Nesting,
                },
                limit: error.limit as u64,
            },
        })?;
        checkpoint()?;
        self.publish(
            identity,
            NativeSyntaxPayload::Retained {
                syntax,
                origin: RetainedBodyOrigin::from_source(source),
            },
        )
    }

    // Compilation identity belongs to the frozen callable and portable source
    // provenance, not the transient parse/import lease representation.
    pub(crate) fn compilation_body_identity(
        &self,
        body: &SyntaxBlockRef,
        owner: CodeFingerprint,
    ) -> Result<SyntaxIdentity, SyntaxOperationError> {
        let lease = self.resolve(body)?;
        match &lease.0.syntax {
            NativeSyntaxPayload::Source(syntax) => source_identity(owner, syntax),
            NativeSyntaxPayload::Fragment(syntax) => {
                let text = syntax.source().text();
                RetainedBodySource::identity_for_text(
                    owner,
                    F::BlockContents,
                    CodeFingerprint::of_bytes("etas.code.source-unit.v1", text.as_bytes()),
                    text.len() as u64,
                    0,
                    text,
                )
                .map_err(invalid)
            }
            NativeSyntaxPayload::Retained { syntax, origin } => {
                RetainedBodySource::identity_for_text(
                    owner,
                    origin.form,
                    origin.source_unit,
                    origin.source_length,
                    origin.source_start,
                    syntax.source().text(),
                )
                .map_err(invalid)
            }
        }
    }

    pub(super) fn next_source_id(&self) -> Result<SourceId, SyntaxOperationError> {
        let mut state = self.lock()?;
        state.next_source = state
            .next_source
            .checked_add(1)
            .ok_or_else(|| invalid("native syntax source identity space exhausted"))?;
        Ok(SourceId(state.next_source))
    }
}

fn retained_form(syntax: &etas_syntax::RetainedBodyFragment) -> F {
    match (syntax.body(), syntax.trailing_handler().is_some()) {
        (SourceBodyView::Block(_), false) => F::BlockContents,
        (SourceBodyView::Expression(_), false) => F::Expression,
        (SourceBodyView::Block(_), true) => F::HandledBlock,
        (SourceBodyView::Expression(_), true) => F::HandledExpression,
    }
}

fn detached_source(text: &str, form: F) -> RetainedBodySource {
    RetainedBodySource {
        schema: RETAINED_BODY_SOURCE_SCHEMA,
        form,
        source_unit: CodeFingerprint::of_bytes("etas.code.source-unit.v1", text.as_bytes()),
        source_length: text.len() as u64,
        source_start: 0,
        content: CodeFingerprint::of_bytes("etas.code.body-content.v1", text.as_bytes()),
        text: text.to_owned(),
    }
}

pub(super) fn source_info(
    syntax: &SourceBodySyntax,
) -> Result<RetainedBodySource, SyntaxOperationError> {
    let parts = source_parts(syntax)?;
    Ok(RetainedBodySource {
        schema: RETAINED_BODY_SOURCE_SCHEMA,
        form: parts.form,
        source_unit: parts.source_unit,
        source_length: syntax.source().text().len() as u64,
        source_start: parts.start,
        content: CodeFingerprint::of_bytes("etas.code.body-content.v1", parts.text.as_bytes()),
        text: parts.text.into(),
    })
}

pub(super) fn source_identity(
    owner: CodeFingerprint,
    syntax: &SourceBodySyntax,
) -> Result<SyntaxIdentity, SyntaxOperationError> {
    let parts = source_parts(syntax)?;
    RetainedBodySource::identity_for_text(
        owner,
        parts.form,
        parts.source_unit,
        syntax.source().text().len() as u64,
        parts.start,
        parts.text,
    )
    .map_err(invalid)
}

struct SourceParts<'a> {
    form: F,
    source_unit: CodeFingerprint,
    start: u64,
    text: &'a str,
}

fn source_parts(syntax: &SourceBodySyntax) -> Result<SourceParts<'_>, SyntaxOperationError> {
    let span = syntax.content_span();
    let text = syntax
        .source()
        .text()
        .get(span.range.start.to_usize()..span.range.end.to_usize())
        .ok_or_else(|| invalid("retained body source range is invalid"))?;
    let form = match (
        syntax.body().map_err(retained_body_error)?,
        syntax.trailing_handler().is_some(),
    ) {
        (SourceBodyView::Block(_), false) => F::BlockContents,
        (SourceBodyView::Expression(_), false) => F::Expression,
        (SourceBodyView::Block(_), true) => F::HandledBlock,
        (SourceBodyView::Expression(_), true) => F::HandledExpression,
    };
    Ok(SourceParts {
        form,
        source_unit: CodeFingerprint::of_bytes(
            "etas.code.source-unit.v1",
            syntax.source().text().as_bytes(),
        ),
        start: span.range.start.to_usize() as u64,
        text,
    })
}

pub(crate) fn fragment_form(form: F) -> etas_syntax::RetainedFragmentForm {
    match form {
        F::BlockContents => etas_syntax::RetainedFragmentForm::BlockContents,
        F::Expression => etas_syntax::RetainedFragmentForm::Expression,
        F::HandledBlock => etas_syntax::RetainedFragmentForm::HandledBlock,
        F::HandledExpression => etas_syntax::RetainedFragmentForm::HandledExpression,
    }
}

pub(crate) fn validate_retained_source(source: &RetainedBodySource) -> Result<(), String> {
    let limits = BodyParseLimits::default();
    if source.text.len() > limits.max_source_bytes {
        return Err(format!(
            "retained body syntax exceeds SourceBytes limit {}",
            limits.max_source_bytes
        ));
    }
    source.validate().map_err(str::to_owned)?;
    etas_syntax::RetainedBodyFragment::parse_controlled(
        SourceFile::new(SourceId(1), None, source.text.clone()),
        fragment_form(source.form),
        limits,
        &|| Ok::<(), std::convert::Infallible>(()),
    )
    .map(|_| ())
    .map_err(|error| match error {
        BoundedBodyParseError::Interrupted(never) => match never {},
        BoundedBodyParseError::Syntax(error) => format!(
            "retained body syntax is invalid: {:?}",
            error
                .diagnostics
                .iter()
                .map(|diagnostic| &diagnostic.message)
                .collect::<Vec<_>>()
        ),
        BoundedBodyParseError::Resource(error) => format!(
            "retained body syntax exceeds {:?} limit {}",
            error.kind, error.limit
        ),
    })
}

pub(super) fn invalid(reason: &str) -> SyntaxOperationError {
    SyntaxOperationError::BackendUnavailable {
        reason: reason.into(),
    }
}
