use std::{collections::BTreeMap, sync::Arc};

use etas_code::definition::{
    AnalysisFingerprint, BaseRevisionInput, DefinitionAnalysisState, DefinitionRef,
    DefinitionSnapshot, FrozenDefinition, FrozenDefinitionInput,
};

use super::{
    DefinitionEnvironment, DefinitionProvenance, NativeDefinition, NativeDefinitionError,
    NativeDefinitionStore,
};
use crate::code::definition::{
    FrozenDefinitionContract,
    external::{ExternalAgent, ExternalCallable},
};

impl NativeDefinitionStore {
    pub(super) fn reflect_external(
        &self,
        external: &ExternalAgent,
        access: Option<etas_hir::HirModuleId>,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let snapshot = external.snapshot.as_ref().ok_or_else(|| {
            NativeDefinitionError::Projection(
                crate::code::descriptor::ProjectionError::MissingIdentity(
                    "external agent producer did not publish revision coordinates".into(),
                ),
            )
        })?;
        let header = &external.header;
        let contract = FrozenDefinitionContract::from_agent_description(
            header.clone(),
            external.graph.clone(),
            external.callables.clone(),
            snapshot.revision.contract,
        );
        self.reflect_metadata(
            snapshot,
            contract,
            external
                .method_snapshots
                .iter()
                .map(|(id, snapshot)| (id.clone(), Arc::new(snapshot.clone())))
                .collect(),
            access,
        )
    }

    pub(super) fn reflect_external_flow(
        &self,
        external: &ExternalCallable,
        access: Option<etas_hir::HirModuleId>,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let snapshot = external.snapshot.as_ref().ok_or_else(|| {
            NativeDefinitionError::Projection(
                crate::code::descriptor::ProjectionError::MissingIdentity(
                    "external flow producer did not publish revision coordinates".into(),
                ),
            )
        })?;
        let contract = FrozenDefinitionContract::from_flow_description(
            external.header.clone(),
            external.graph.clone(),
            snapshot.revision.contract,
        )
        .map_err(NativeDefinitionError::Projection)?;
        self.reflect_metadata(snapshot, contract, BTreeMap::new(), access)
    }

    fn reflect_metadata(
        &self,
        snapshot: &etas_code::descriptor::DefinitionSnapshotInfo,
        contract: Arc<FrozenDefinitionContract>,
        method_snapshots: BTreeMap<
            etas_code::CallableId,
            Arc<etas_code::descriptor::DefinitionSnapshotInfo>,
        >,
        access: Option<etas_hir::HirModuleId>,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let revision = &snapshot.revision;
        let graph = contract.descriptors().clone();
        let default = contract
            .entry()
            .as_ref()
            .map(|id| {
                contract
                    .callable(id)
                    .ok_or(NativeDefinitionError::CandidateInputMismatch)
            })
            .transpose()?;
        let body = contract
            .entry()
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
            identity: snapshot.identity.clone(),
            descriptors: graph.clone(),
            default_entry: default.map(|callable| callable.signature),
            default_callable: contract.entry().cloned(),
            environment: revision.environment,
            contract_fingerprint: revision.contract,
            contract: contract.clone(),
            compilation: revision.compilation,
        })
        .map_err(NativeDefinitionError::Definition)?;
        let data = DefinitionSnapshot::from_base(
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
        let summaries = contract
            .methods()
            .iter()
            .map(|id| {
                contract
                    .callable(id)
                    .map(|callable| (id.clone(), callable.analysis.clone()))
                    .ok_or(NativeDefinitionError::CandidateInputMismatch)
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let auxiliary = DefinitionAnalysisState::Checked {
            input: AnalysisFingerprint::new(data.revision().key(), revision.compilation),
            summaries: Arc::new(summaries),
        };
        self.publish(NativeDefinition {
            data,
            environment: DefinitionEnvironment::Metadata(Arc::new(snapshot.dependencies.clone())),
            auxiliary,
            method_snapshots,
            analysis_configuration: revision.compilation,
            analysis_graph: graph,
            reflection_body_access,
            reflection_access: access,
            provenance: DefinitionProvenance::Metadata,
            implementation: None,
        })
        .map_err(Into::into)
    }
}
