use std::{collections::BTreeMap, sync::Arc};

use etas_code::definition::{
    AgentDefinition, AgentDefinitionRef, AnalysisFingerprint, BaseRevisionInput,
    DefinitionAnalysisState, FrozenDefinition, FrozenDefinitionInput,
};

use super::{
    DefinitionEnvironment, DefinitionProvenance, NativeAgentDefinition, NativeDefinitionError,
    NativeDefinitionStore,
};
use crate::code::definition::{FrozenAgentContract, external::ExternalAgent};

impl NativeDefinitionStore {
    pub(super) fn reflect_external(
        &self,
        external: &ExternalAgent,
        access: Option<etas_hir::HirModuleId>,
    ) -> Result<AgentDefinitionRef, NativeDefinitionError> {
        let snapshot = external.snapshot.as_ref().ok_or_else(|| {
            NativeDefinitionError::Projection(
                crate::code::descriptor::ProjectionError::MissingIdentity(
                    "external agent producer did not publish revision coordinates".into(),
                ),
            )
        })?;
        let revision = &snapshot.revision;
        let header = &external.header;
        let default = header
            .default_entry
            .as_ref()
            .map(|id| {
                external
                    .callables
                    .get(id)
                    .ok_or(NativeDefinitionError::CandidateInputMismatch)
            })
            .transpose()?;
        let contract = FrozenAgentContract::from_description(
            header.clone(),
            external.graph.clone(),
            external.callables.clone(),
            revision.contract,
        );
        let body = header
            .default_entry
            .as_ref()
            .map(|id| {
                self.catalog
                    .retained_body_at_access(id, access, &self.syntax)
            })
            .transpose()
            .map_err(NativeDefinitionError::Projection)?
            .flatten();
        let reflection_body_access = body.is_some();
        let base = FrozenDefinition::new(FrozenDefinitionInput {
            identity: header.identity.clone(),
            descriptors: external.graph.clone(),
            default_entry: default.map(|callable| callable.signature),
            default_callable: header.default_entry.clone(),
            environment: revision.environment,
            contract_fingerprint: revision.contract,
            contract,
            compilation: revision.compilation,
        })
        .map_err(NativeDefinitionError::Definition)?;
        let data = AgentDefinition::from_base(
            base,
            BaseRevisionInput {
                body,
                content: revision.content,
                parent: revision.parent,
                analysis_configuration: revision.compilation,
                summaries: default.map(|callable| Arc::new(callable.analysis.clone())),
            },
        )
        .map_err(NativeDefinitionError::Definition)?;
        if data.revision() != &snapshot.revision_info() {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        }
        let summaries = header
            .methods
            .iter()
            .map(|id| {
                external
                    .callables
                    .get(id)
                    .map(|callable| (id.clone(), callable.analysis.clone()))
                    .ok_or(NativeDefinitionError::CandidateInputMismatch)
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let auxiliary = DefinitionAnalysisState::Checked {
            input: AnalysisFingerprint::new(data.revision().key(), revision.compilation),
            summaries: Arc::new(summaries),
        };
        self.publish(NativeAgentDefinition {
            data,
            environment: DefinitionEnvironment::Metadata(Arc::new(snapshot.dependencies.clone())),
            auxiliary,
            analysis_configuration: revision.compilation,
            analysis_graph: external.graph.clone(),
            reflection_body_access,
            reflection_access: access,
            provenance: DefinitionProvenance::Metadata,
            implementation: None,
        })
        .map_err(Into::into)
    }
}
