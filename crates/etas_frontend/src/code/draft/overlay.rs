use std::sync::Arc;

use etas_code::{
    CodeFingerprint, RevisionInfo, RevisionInputs,
    syntax::{SyntaxBlockRef, SyntaxOperationError},
};
use etas_core::{SourceId, Span};
use etas_syntax::{RelocatedBodySyntax, ast};

use crate::{
    AstBodyRef,
    code::{
        definition::{BodyExposure, FrozenDefinitionEnvironment},
        service::NativeCodeSyntaxOperations,
    },
};

#[derive(Debug)]
pub enum OverlayError {
    MissingDefaultEntry,
    InvalidFrozenSlot,
    InvalidRevision,
    SourceIdentityExhausted,
    Syntax(SyntaxOperationError),
}

/// A native body transform, not permission to execute or a checked candidate.
/// The frozen header, lexical environment, and visibility remain authoritative.
#[derive(Clone, etas_core::RetainedSize)]
pub struct DefaultBodyOverlay {
    frozen: Arc<FrozenDefinitionEnvironment>,
    revision: Arc<super::NativeBodyRevision>,
    fingerprint: CodeFingerprint,
    exposure: BodyExposure,
    revision_info: RevisionInfo,
}

impl DefaultBodyOverlay {
    pub fn prepare(
        frozen: Arc<FrozenDefinitionEnvironment>,
        body: SyntaxBlockRef,
        syntax: &NativeCodeSyntaxOperations,
    ) -> Result<Self, OverlayError> {
        Self::prepare_inner(frozen, body, syntax, None)
    }

    pub(crate) fn prepare_revision(
        frozen: Arc<FrozenDefinitionEnvironment>,
        body: SyntaxBlockRef,
        syntax: &NativeCodeSyntaxOperations,
        revision: RevisionInfo,
    ) -> Result<Self, OverlayError> {
        Self::prepare_inner(frozen, body, syntax, Some(revision))
    }

    fn prepare_inner(
        frozen: Arc<FrozenDefinitionEnvironment>,
        body: SyntaxBlockRef,
        syntax: &NativeCodeSyntaxOperations,
        revision: Option<RevisionInfo>,
    ) -> Result<Self, OverlayError> {
        let checked = frozen.checked();
        let item = frozen
            .default_body()
            .ok_or(OverlayError::MissingDefaultEntry)?;
        let slot = checked
            .definition_sources
            .body(item)
            .cloned()
            .ok_or(OverlayError::InvalidFrozenSlot)?;
        let source = checked
            .definition_sources
            .source(slot.source)
            .ok_or(OverlayError::InvalidFrozenSlot)?;
        let unchanged = syntax
            .matches_source_body(
                &body,
                source.file.clone(),
                source.program.clone(),
                etas_syntax::ProgramBodySelection {
                    item: slot.item.index,
                    impl_member: slot.impl_member,
                },
            )
            .map_err(OverlayError::Syntax)?;
        let annotated = source
            .program
            .value
            .items
            .get(slot.item.index)
            .ok_or(OverlayError::InvalidFrozenSlot)?;
        let declaration = match (&annotated.item, slot.impl_member) {
            (ast::Item::Agent(agent), None) => agent.span,
            (ast::Item::Flow(flow), None) => flow.span,
            (ast::Item::Impl(implementation), Some(member)) => {
                match implementation.items.get(member) {
                    Some(ast::ImplItem::Flow(flow)) => flow.span,
                    _ => return Err(OverlayError::InvalidFrozenSlot),
                }
            }
            _ => return Err(OverlayError::InvalidFrozenSlot),
        };
        let next = checked
            .sources
            .sources
            .iter()
            .map(|source| source.id.0)
            .max()
            .ok_or(OverlayError::InvalidFrozenSlot)?
            .checked_add(1)
            .ok_or(OverlayError::SourceIdentityExhausted)?;
        let relocated = syntax
            .relocate_body(&body, SourceId(next))
            .map_err(OverlayError::Syntax)?;
        let environment = frozen.fingerprint();
        let exposure = checked.definition_sources.exposure(item);
        let callable = frozen
            .contract()
            .header()
            .default_entry
            .clone()
            .ok_or(OverlayError::InvalidFrozenSlot)?;
        let syntax_identity = syntax
            .compilation_body_identity(&body, callable.fingerprint())
            .map_err(OverlayError::Syntax)?;
        let parent = match &revision {
            Some(revision) => revision.inputs().parent,
            None => {
                let content = syntax
                    .source_body_identity(
                        callable.fingerprint(),
                        source.file.clone(),
                        source.program.clone(),
                        etas_syntax::ProgramBodySelection {
                            item: slot.item.index,
                            impl_member: slot.impl_member,
                        },
                    )
                    .map_err(OverlayError::Syntax)?
                    .content();
                Some(
                    RevisionInfo::new(RevisionInputs {
                        definition: callable.clone(),
                        content,
                        environment: frozen.fingerprint(),
                        contract: frozen.contract().fingerprint(),
                        compilation: frozen.compilation_fingerprint(),
                        parent: None,
                    })
                    .key(),
                )
            }
        };
        let revision_info = RevisionInfo::new(RevisionInputs {
            definition: callable,
            content: syntax_identity.content(),
            environment: frozen.fingerprint(),
            contract: frozen.contract().fingerprint(),
            compilation: frozen.compilation_fingerprint(),
            parent,
        });
        if revision
            .as_ref()
            .is_some_and(|revision| *revision != revision_info)
        {
            return Err(OverlayError::InvalidRevision);
        }
        let revision_fingerprint = revision_info.key().fingerprint();
        let fingerprint = CodeFingerprint::of_parts(
            "etas.code.default-body-overlay.v3",
            [
                environment.as_bytes().as_slice(),
                syntax_identity.source().as_bytes().as_slice(),
                syntax_identity.content().as_bytes().as_slice(),
                revision_fingerprint.as_bytes().as_slice(),
            ],
        );
        Ok(Self {
            frozen,
            revision: Arc::new(super::NativeBodyRevision {
                slot,
                declaration,
                syntax: body,
                relocated,
                unchanged,
            }),
            fingerprint,
            exposure,
            revision_info,
        })
    }

    pub fn frozen(&self) -> &Arc<FrozenDefinitionEnvironment> {
        &self.frozen
    }
    pub fn slot(&self) -> &AstBodyRef {
        &self.revision.slot
    }
    pub fn declaration(&self) -> Span {
        self.revision.declaration
    }
    pub fn body(&self) -> &SyntaxBlockRef {
        &self.revision.syntax
    }
    pub fn relocated(&self) -> &Arc<RelocatedBodySyntax> {
        &self.revision.relocated
    }
    pub fn fingerprint(&self) -> CodeFingerprint {
        self.fingerprint
    }
    pub fn exposure(&self) -> BodyExposure {
        self.exposure
    }
    pub fn revision(&self) -> &RevisionInfo {
        &self.revision_info
    }
    pub(crate) fn native_revision(&self) -> &Arc<super::NativeBodyRevision> {
        &self.revision
    }

    pub fn install(
        &self,
        lowering: &mut etas_hir::HirProjectLowering,
    ) -> Result<(), etas_hir::lower::BodyOverlayError> {
        lowering.replace_body(self.revision.declaration, self.revision.relocated.clone())
    }
}
