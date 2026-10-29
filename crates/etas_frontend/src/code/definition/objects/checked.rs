use std::{collections::BTreeMap, sync::Arc};

use etas_code::{
    definition::{
        AnalysisFingerprint, BaseRevisionInput, DefinitionAnalysisState, DefinitionRef,
        DefinitionSnapshot,
    },
    descriptor::DescriptorLimits,
};

use super::{DefinitionProvenance, NativeDefinition, NativeDefinitionError, NativeDefinitionStore};
use crate::code::{descriptor::ProjectionError, service::CheckedCandidate};

impl NativeDefinitionStore {
    /// Reflects an emitted implementation without re-freezing its inferred
    /// behavior as a new contract. A user-created report cannot enter here.
    pub fn reflect_checked(
        &self,
        definition: &DefinitionRef,
        candidate: &CheckedCandidate,
        limits: DescriptorLimits,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let original = self.resolve(definition)?;
        if !Arc::ptr_eq(original.environment()?, candidate.frozen())
            || original.identity() != candidate.identity()
            || original.revision() != candidate.revision()
        {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        }
        let catalog = candidate
            .definition_catalog(limits)
            .map_err(NativeDefinitionError::Projection)?;
        let default = original
            .0
            .data
            .base()
            .default_callable()
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
        let analysis = catalog.callable(default).ok_or_else(|| {
            NativeDefinitionError::Projection(ProjectionError::MissingIdentity(
                "checked default callable".into(),
            ))
        })?;
        let body = original
            .0
            .reflection_body_access
            .then(|| {
                original
                    .body()
                    .cloned()
                    .ok_or(NativeDefinitionError::CandidateInputMismatch)
            })
            .transpose()?;
        let data = DefinitionSnapshot::from_base(
            original.0.data.shared_base(),
            BaseRevisionInput {
                body,
                content: candidate.revision().inputs().content,
                parent: candidate.revision().inputs().parent,
                analysis_configuration: original.0.analysis_configuration,
                summaries: Some(Arc::new(analysis.analysis.clone())),
            },
        )
        .map_err(NativeDefinitionError::Definition)?;
        if data.revision() != candidate.revision() {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        }
        let mut auxiliary = BTreeMap::new();
        for id in original.environment()?.contract().methods() {
            let callable = catalog.callable(id).ok_or_else(|| {
                NativeDefinitionError::Projection(ProjectionError::MissingIdentity(
                    "checked auxiliary callable".into(),
                ))
            })?;
            auxiliary.insert(id.clone(), callable.analysis.clone());
        }
        let auxiliary = DefinitionAnalysisState::Checked {
            input: AnalysisFingerprint::new(
                data.revision().key(),
                original.0.analysis_configuration,
            ),
            summaries: Arc::new(auxiliary),
        };
        for (id, snapshot) in &original.0.method_snapshots {
            if snapshot.revision.content
                != catalog
                    .body_content(id, &self.syntax)
                    .map_err(NativeDefinitionError::Projection)?
            {
                return Err(NativeDefinitionError::CandidateInputMismatch);
            }
        }
        self.publish(NativeDefinition {
            data,
            environment: super::DefinitionEnvironment::Native(original.environment()?.clone()),
            auxiliary,
            method_snapshots: original.0.method_snapshots.clone(),
            analysis_configuration: original.0.analysis_configuration,
            analysis_graph: catalog.descriptors().clone(),
            reflection_body_access: original.0.reflection_body_access,
            reflection_access: original.0.reflection_access,
            provenance: DefinitionProvenance::Checked {
                snapshot: candidate.snapshot_fingerprint(),
            },
            implementation: Some(Arc::new(candidate.clone())),
        })
        .map_err(Into::into)
    }
}
