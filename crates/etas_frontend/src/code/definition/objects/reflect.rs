use std::{collections::BTreeMap, sync::Arc};

use etas_code::definition::{
    AnalysisFingerprint, BaseRevisionInput, DefinitionAnalysisState, DefinitionRef,
    DefinitionSnapshot, FrozenDefinition, FrozenDefinitionInput,
};

use super::{NativeDefinition, NativeDefinitionError, NativeDefinitionStore, SourceReflectionKey};

impl NativeDefinitionStore {
    pub fn reflect_flow_value(
        &self,
        expression: etas_hir::HirExprId,
        item: etas_hir::HirItemId,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let header = self
            .catalog
            .callable_value_reflection(expression, item)
            .map_err(NativeDefinitionError::Projection)?;
        let access = self
            .catalog
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?
            .access_module;
        self.reflect_flow_at_access(header, item, access)
    }

    pub fn reflect_flow(
        &self,
        expression: etas_hir::HirExprId,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let access = self
            .catalog
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?
            .access_module;
        if let super::super::StaticReflectionView::Callable(header) = self
            .catalog
            .static_reflection(expression)
            .map_err(NativeDefinitionError::Projection)?
            && let Some(external) = self.catalog.external_callables.get(&header.id)
        {
            return self.reflect_external_flow_at_access(external, access);
        }
        let (header, item) = self
            .catalog
            .editable_flow_reflection(expression)
            .map_err(NativeDefinitionError::Projection)?;
        self.reflect_flow_at_access(header, item, access)
    }

    pub(super) fn reflect_external_flow_at_access(
        &self,
        external: &super::super::external::ExternalCallable,
        access: Option<etas_hir::HirModuleId>,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let key = SourceReflectionKey::ExternalFlow {
            identity: external.canonical.clone(),
            access,
        };
        if let Some(reference) = self.source_view(&key)? {
            return Ok(reference);
        }
        self.remember_source_view(key, self.reflect_external_flow(external, access)?)
            .map_err(Into::into)
    }

    pub(super) fn reflect_flow_at_access(
        &self,
        header: &etas_code::descriptor::CallableHeaderInfo,
        item: etas_hir::HirItemId,
        access: Option<etas_hir::HirModuleId>,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let key = SourceReflectionKey::Native { item, access };
        if let Some(reference) = self.source_view(&key)? {
            return Ok(reference);
        }
        let frozen = self
            .catalog
            .freeze_flow_view(header, item)
            .map_err(NativeDefinitionError::Projection)?;
        let body = self
            .catalog
            .retained_body_at_access(&header.id, access, &self.syntax)
            .map_err(NativeDefinitionError::Projection)?;
        self.remember_source_view(key, self.reflect_frozen(frozen, body, access)?)
            .map_err(Into::into)
    }

    pub fn reflect_agent(
        &self,
        expression: etas_hir::HirExprId,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        if !self
            .catalog
            .checked
            .definition_sources
            .revisions()
            .is_empty()
        {
            return Err(NativeDefinitionError::MissingRevisionContext);
        }
        let fact = self
            .catalog
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
        if let etas_types::CheckedReflectionTarget::Agent {
            declaration,
            component,
        } = fact.target
            && let Some(identity) = self.catalog.agent_bindings.get(&declaration)
            && let Some(external) = self.catalog.external_agents.get(identity)
        {
            if component != external.component {
                return Err(NativeDefinitionError::CandidateInputMismatch);
            }
            let key = SourceReflectionKey::External {
                component,
                access: fact.access_module,
            };
            if let Some(reference) = self.source_view(&key)? {
                return Ok(reference);
            }
            return self
                .remember_source_view(key, self.reflect_external(external, fact.access_module)?)
                .map_err(Into::into);
        }
        let key = match fact.target {
            etas_types::CheckedReflectionTarget::Agent { declaration, .. } => {
                let Some(etas_hir::SymbolDef::Item { item }) = self
                    .catalog
                    .checked
                    .symbols
                    .get(declaration)
                    .map(|symbol| &symbol.def)
                else {
                    return Err(NativeDefinitionError::CandidateInputMismatch);
                };
                SourceReflectionKey::Native {
                    item: *item,
                    access: fact.access_module,
                }
            }
            _ => return Err(NativeDefinitionError::CandidateInputMismatch),
        };
        if let Some(reference) = self.source_view(&key)? {
            return Ok(reference);
        }
        let environment = self
            .catalog
            .freeze_agent(expression)
            .map_err(NativeDefinitionError::Projection)?;
        let body = self
            .catalog
            .retained_body(expression, &self.syntax)
            .map_err(NativeDefinitionError::Projection)?;
        let access = self
            .catalog
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?
            .access_module;
        self.remember_source_view(key, self.reflect_frozen(environment, body, access)?)
            .map_err(Into::into)
    }

    pub fn reflect_agent_value(
        &self,
        expression: etas_hir::HirExprId,
        item: etas_hir::HirItemId,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let access = self
            .catalog
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?
            .access_module;
        let key = SourceReflectionKey::Native { item, access };
        let crate::code::definition::StaticReflectionView::Agent(header) = self
            .catalog
            .agent_value_reflection(expression, item)
            .map_err(NativeDefinitionError::Projection)?
        else {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        };
        if let Some(reference) = self.source_view(&key)? {
            return Ok(reference);
        }
        let body = header
            .default_entry
            .as_ref()
            .map(|callable| {
                self.catalog
                    .retained_callable_body(expression, callable, &self.syntax)
            })
            .transpose()
            .map_err(NativeDefinitionError::Projection)?
            .flatten();
        let environment = self
            .catalog
            .freeze_agent_view(header, item)
            .map_err(NativeDefinitionError::Projection)?;
        let access = self
            .catalog
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?
            .access_module;
        self.remember_source_view(key, self.reflect_frozen(environment, body, access)?)
            .map_err(Into::into)
    }

    pub(super) fn reflect_frozen(
        &self,
        environment: Arc<crate::code::definition::FrozenDefinitionEnvironment>,
        body: Option<etas_code::syntax::SyntaxBlockRef>,
        access: Option<etas_hir::HirModuleId>,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let contract = environment.contract();
        let default = contract
            .entry()
            .map(|id| {
                contract.callable(id).ok_or_else(|| {
                    NativeDefinitionError::Projection(
                        crate::code::descriptor::ProjectionError::MissingIdentity(
                            "default callable descriptor".into(),
                        ),
                    )
                })
            })
            .transpose()?;
        let reflection_body_access = body.is_some();
        let revision = self
            .catalog
            .source_revision(&environment, &self.syntax)
            .map_err(NativeDefinitionError::Projection)?;
        let configuration = environment.compilation_fingerprint();
        let base = FrozenDefinition::new(FrozenDefinitionInput {
            identity: environment.identity().clone(),
            descriptors: environment.contract().descriptors().clone(),
            default_entry: default.map(|callable| callable.signature),
            default_callable: contract.entry().cloned(),
            environment: environment.fingerprint(),
            contract_fingerprint: environment.contract().fingerprint(),
            contract: environment.contract_arc(),
            compilation: configuration,
        })
        .map_err(NativeDefinitionError::Definition)?;
        let data = DefinitionSnapshot::from_base(
            base,
            BaseRevisionInput {
                body,
                content: revision.content,
                parent: None,
                analysis_configuration: configuration,
                summaries: default.map(|callable| Arc::new(callable.analysis.clone())),
            },
        )
        .map_err(NativeDefinitionError::Definition)?;
        let mut auxiliary = BTreeMap::new();
        for id in contract.methods() {
            let callable = contract.callable(id).ok_or_else(|| {
                NativeDefinitionError::Projection(
                    crate::code::descriptor::ProjectionError::MissingIdentity(
                        "auxiliary callable descriptor".into(),
                    ),
                )
            })?;
            auxiliary.insert(id.clone(), callable.analysis.clone());
        }
        let auxiliary = DefinitionAnalysisState::Checked {
            input: AnalysisFingerprint::new(data.revision().key(), configuration),
            summaries: Arc::new(auxiliary),
        };
        let method_snapshots = if environment.identity().kind() == etas_code::DefinitionKind::Agent
        {
            self.catalog
                .source_method_snapshots(&environment, &self.syntax)
                .map_err(NativeDefinitionError::Projection)?
                .into_iter()
                .map(|snapshot| (snapshot.revision.definition.clone(), Arc::new(snapshot)))
                .collect()
        } else {
            BTreeMap::new()
        };
        let analysis_graph = contract.descriptors().clone();
        self.publish(NativeDefinition {
            data,
            environment: super::DefinitionEnvironment::Native(environment),
            auxiliary,
            method_snapshots,
            analysis_configuration: configuration,
            analysis_graph,
            reflection_body_access,
            reflection_access: access,
            provenance: super::DefinitionProvenance::Source,
            implementation: None,
        })
        .map_err(Into::into)
    }
}
