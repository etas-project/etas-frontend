use std::sync::Arc;

use etas_code::{CodeFingerprint, DefinitionKey, RevisionInfo};
use etas_hir::HirItemId;

use crate::{CheckedProgram, code::definition::FrozenDefinitionEnvironment};

/// Native compiler evidence, not a serialized artifact or an execution grant.
/// Its fields and constructor are inaccessible to external callers. Only the
/// candidate emit pass constructs this after all required artifacts are valid.
#[derive(Clone, etas_core::RetainedSize)]
pub struct CheckedCandidate {
    pub(crate) descriptors: Arc<crate::code::definition::DefinitionCatalog>,
    pub(crate) descriptor_limits: etas_code::descriptor::DescriptorLimits,
    pub(crate) program: CheckedProgram,
    pub(crate) frozen: Arc<FrozenDefinitionEnvironment>,
    pub(crate) revision: RevisionInfo,
    pub(crate) snapshot: CodeFingerprint,
    pub(crate) entry: HirItemId,
}

pub(crate) enum CandidateEmission {
    Rejected,
    Ready(Box<CheckedCandidate>),
}

impl CheckedCandidate {
    pub fn program(&self) -> &CheckedProgram {
        &self.program
    }
    pub fn frozen(&self) -> &Arc<FrozenDefinitionEnvironment> {
        &self.frozen
    }
    pub fn identity(&self) -> &DefinitionKey {
        self.frozen.identity()
    }
    pub fn revision(&self) -> &RevisionInfo {
        &self.revision
    }
    pub fn snapshot_fingerprint(&self) -> CodeFingerprint {
        self.snapshot
    }
    pub fn entry(&self) -> HirItemId {
        self.entry
    }

    pub fn definition_catalog(
        &self,
        limits: etas_code::descriptor::DescriptorLimits,
    ) -> Result<
        Arc<crate::code::definition::DefinitionCatalog>,
        crate::code::descriptor::ProjectionError,
    > {
        if limits == self.descriptor_limits {
            return Ok(self.descriptors.clone());
        }
        let origins = self.frozen.origins_for(&self.program)?;
        crate::code::definition::DefinitionCatalog::build(self.program.clone(), &origins, limits)
            .map(Arc::new)
    }

    pub fn revision_catalogs(
        &self,
        limits: etas_code::descriptor::DescriptorLimits,
    ) -> Result<
        (
            Arc<crate::code::definition::DefinitionCatalog>,
            Arc<crate::code::definition::DefinitionCatalog>,
        ),
        crate::code::descriptor::ProjectionError,
    > {
        use crate::code::definition::DefinitionCatalog;
        let original = self.frozen.checked();
        let original_origins = self.frozen.origins_for(original)?;
        Ok((
            Arc::new(DefinitionCatalog::build(
                original.clone(),
                &original_origins,
                limits,
            )?),
            self.definition_catalog(limits)?,
        ))
    }
}

/// Runs on the engine's owning executor. Publication prepares an immutable
/// implementation; it must not evaluate it or replace a caller's binding.
pub trait CandidatePublisher {
    type Artifact;
    type Error;
    fn publish(&mut self, candidate: CheckedCandidate) -> Result<Self::Artifact, Self::Error>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandidateResolutionError {
    UnownedReference,
    Unavailable(String),
}

impl std::fmt::Display for CandidateResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "native checked candidate resolution failed: {self:?}")
    }
}
impl std::error::Error for CandidateResolutionError {}

/// Native composition port. A portable fingerprint is not evidence that the
/// service owns a candidate; the resolver must validate the actual lease.
pub trait CheckedCandidateResolver: Send + Sync {
    fn resolve_checked_candidate(
        &self,
        reference: &etas_code::artifact::CheckedCodeRef,
    ) -> Result<CheckedCandidate, CandidateResolutionError>;
}
