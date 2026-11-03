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
    /// Publishes a checked application of a portable flow snapshot. The
    /// resulting definition remains metadata-backed; it is not executable
    /// until the pinned source environment is independently restored.
    pub fn reflect_external_callable_application(
        &self,
        expression: etas_hir::HirExprId,
        lexical: Option<&crate::CheckedCallableApplication>,
        limits: etas_code::descriptor::DescriptorLimits,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let application =
            self.checked_external_callable_application(expression, lexical, limits)?;
        let access = self
            .catalog
            .checked_program()
            .checked_reflections()
            .get(&expression)
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?
            .access_module;
        self.reflect_external_application_from_catalog(&self.catalog, application, access, limits)
    }

    pub(super) fn reflect_external_application_from_catalog(
        &self,
        catalog: &Arc<crate::code::definition::DefinitionCatalog>,
        application: super::CheckedExternalCallableApplication,
        access: Option<etas_hir::HirModuleId>,
        limits: etas_code::descriptor::DescriptorLimits,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let invalid = || NativeDefinitionError::CandidateInputMismatch;
        let external = self
            .catalog
            .external_callables
            .get(application.callable())
            .ok_or_else(invalid)?;
        if catalog.external_callables.get(application.callable()) != Some(external) {
            return Err(invalid());
        }
        let original = external.snapshot.as_ref().ok_or_else(invalid)?;
        let context = original.generic_environment_context.ok_or_else(invalid)?;
        if etas_code::descriptor::DefinitionSnapshotInfo::generic_environment_fingerprint(
            context,
            original.revision.contract,
        ) != original.revision.environment
        {
            return Err(invalid());
        }
        let application_fingerprint = catalog
            .external_callable_application_fingerprint(&application, limits)
            .map_err(NativeDefinitionError::Projection)?;
        let (graph, header) = catalog
            .external_callable_application_description(&application, limits)
            .map_err(NativeDefinitionError::Projection)?;
        if header.id != external.canonical {
            return Err(invalid());
        }
        let fingerprint = crate::code::definition::contract::applied_contract_fingerprint(
            original.revision.contract,
            application_fingerprint,
            &graph,
            &header,
        )
        .map_err(NativeDefinitionError::Projection)?;
        let mut snapshot = original.clone();
        snapshot.revision.contract = fingerprint;
        snapshot.revision.environment =
            etas_code::descriptor::DefinitionSnapshotInfo::generic_environment_fingerprint(
                context,
                fingerprint,
            );
        snapshot.dependencies.environment = snapshot.revision.environment;
        let contract = FrozenDefinitionContract::from_flow_description(header, graph, fingerprint)
            .map_err(NativeDefinitionError::Projection)?;
        self.reflect_metadata(
            &snapshot,
            contract,
            BTreeMap::new(),
            access,
            Some(Arc::new(super::ExternalApplicationEvidence {
                application,
                catalog: catalog.clone(),
            })),
        )
    }

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
            None,
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
        self.reflect_metadata(snapshot, contract, BTreeMap::new(), access, None)
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
        application: Option<Arc<super::ExternalApplicationEvidence>>,
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
            external_application: application,
            external_restoration: None,
        })
        .map_err(Into::into)
    }
}
