use std::{collections::HashMap, sync::Arc};

use super::{
    super::DefinitionCatalog, NativeDefinitionError, NativeDefinitionStore, SourceReflectionKey,
};
use crate::{
    CheckedProgram,
    code::{descriptor::ProjectionError, service::CheckedCandidate},
};
use etas_code::{
    CallableId,
    definition::DefinitionRef,
    descriptor::{AgentHeaderInfo, CallableHeaderInfo},
    syntax::SyntaxBlockRef,
};
use etas_hir::{HirExprId, HirItemId, HirModuleId};

mod build;

pub enum NativeSourceReflection {
    Agent(DefinitionRef),
    Callable(Box<NativeCallableReflection>),
}

pub struct NativeCallableReflection {
    pub header: CallableHeaderInfo,
    pub graph: etas_code::descriptor::DescriptorGraph,
    pub body: Option<SyntaxBlockRef>,
    pub definition: Option<DefinitionRef>,
    pub method_snapshot: Option<super::NativeMethodSnapshot>,
}

enum SourceTarget {
    CallableValue,
    Agent {
        header: AgentHeaderInfo,
        item: HirItemId,
    },
    Callable {
        header: CallableHeaderInfo,
        canonical: CallableId,
    },
    ExternalCallable(Box<super::super::external::ExternalCallable>),
    ExternalAgent(Box<super::super::external::ExternalAgent>),
}

struct Binding {
    candidate_agent: Option<HirItemId>,
    access: Option<HirModuleId>,
    target: SourceTarget,
}

/// Native source bindings verified once against an emitted candidate. HIR IDs
/// remain local to their owning programs; reflection does not rebind a source
/// declaration to whichever implementation happens to be executing.
pub struct FrozenReflectionBindings {
    program: CheckedProgram,
    source: Arc<NativeDefinitionStore>,
    source_applications: Arc<NativeDefinitionStore>,
    bindings: HashMap<HirExprId, Binding>,
}

impl FrozenReflectionBindings {
    pub fn reflect_external_callable_application(
        &self,
        candidate: &NativeDefinitionStore,
        expression: HirExprId,
        lexical: Option<&crate::CheckedCallableApplication>,
        limits: etas_code::descriptor::DescriptorLimits,
    ) -> Result<(Arc<NativeDefinitionStore>, DefinitionRef), NativeDefinitionError> {
        let invalid = || NativeDefinitionError::CandidateInputMismatch;
        if !Arc::ptr_eq(candidate.catalog().checked_program(), &self.program) {
            return Err(invalid());
        }
        let binding = self.bindings.get(&expression).ok_or_else(invalid)?;
        let SourceTarget::ExternalCallable(external) = &binding.target else {
            return Err(invalid());
        };
        let application =
            candidate.checked_external_callable_application(expression, lexical, limits)?;
        if application.callable() != &external.canonical {
            return Err(invalid());
        }
        let definition = self.source.reflect_external_application_from_catalog(
            candidate.catalog(),
            application,
            binding.access,
            limits,
        )?;
        Ok((self.source.clone(), definition))
    }

    pub fn build(
        source: Arc<NativeDefinitionStore>,
        candidate: &CheckedCandidate,
        catalog: &DefinitionCatalog,
    ) -> Result<Self, NativeDefinitionError> {
        build::build(source, candidate, catalog).map_err(NativeDefinitionError::Projection)
    }
    pub fn source(&self) -> &Arc<NativeDefinitionStore> {
        &self.source
    }

    pub fn reflect_callable_application(
        &self,
        candidate: &NativeDefinitionStore,
        expression: HirExprId,
        application: &crate::CheckedCallableApplication,
        lexical: Option<&crate::CheckedCallableApplication>,
        limits: etas_code::descriptor::DescriptorLimits,
    ) -> Result<(Arc<NativeDefinitionStore>, DefinitionRef), NativeDefinitionError> {
        let invalid = || NativeDefinitionError::CandidateInputMismatch;
        let candidate_catalog = candidate.catalog();
        if !Arc::ptr_eq(candidate_catalog.checked_program(), &self.program) {
            return Err(invalid());
        }
        let interface = candidate.checked_reflection_interface(expression, application, lexical)?;
        let binding = self.bindings.get(&expression).ok_or_else(invalid)?;
        let canonical = candidate_catalog
            .ordinary_flow_identity(application.owner())
            .ok_or_else(invalid)?;
        match &binding.target {
            SourceTarget::Callable {
                canonical: expected,
                ..
            } if expected == canonical => {}
            SourceTarget::ExternalCallable(external) if &external.canonical == canonical => {}
            SourceTarget::CallableValue => {
                let original = self.frozen_flow_item(candidate_catalog, application.owner())?;
                if self.source_applications.catalog.item_owner(canonical) != Some(original) {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
        let original = self
            .source_applications
            .catalog
            .item_owner(canonical)
            .ok_or_else(invalid)?;
        let source = self.source_applications.catalog.checked_program();
        let source_signature = crate::project::instantiation::signature(&source.types, original)
            .ok_or_else(invalid)?;
        let candidate_signature =
            crate::project::instantiation::signature(&self.program.types, application.owner())
                .ok_or_else(invalid)?;
        if source_signature.generic_params.len() != application.arguments().len()
            || source_signature.generic_params.len() != candidate_signature.generic_params.len()
            || source_signature
                .generic_params
                .iter()
                .zip(&candidate_signature.generic_params)
                .any(|(a, b)| a.kind != b.kind || a.name != b.name)
        {
            return Err(invalid());
        }
        let correspondence = crate::code::definition::NativeTypeCorrespondence::build(
            candidate_catalog,
            &self.source_applications.catalog,
        )
        .map_err(NativeDefinitionError::Projection)?;
        let target_types = etas_types::ty::TypeLookupIndex::new(&source.type_store);
        let budget =
            etas_types::TypeSubstitutionBudget::new(limits.max_nodes, limits.max_edges, None);
        let mut relocation =
            etas_types::TypeSubstitutionEngine::relocating(etas_types::ty::TypeRelocationInput {
                source: &self.program.type_store,
                target: &source.type_store,
                declaration_types: correspondence.declarations(),
                target_types: &target_types,
                effect_names: correspondence.effect_names(),
            })
            .with_expansion_depth(limits.max_expansion_depth)
            .with_budget(&budget);
        let mut types = Vec::new();
        let mut rows = Vec::new();
        for (param, arg) in source_signature
            .generic_params
            .iter()
            .zip(application.arguments())
        {
            match arg {
                etas_types::CallableGenericArg::Type(ty) => types.push((
                    param.name.clone(),
                    relocation.substitute(*ty).map_err(|_| invalid())?,
                )),
                etas_types::CallableGenericArg::EffectRow(row) => rows.push((
                    param.name.clone(),
                    relocation
                        .substitute_effect_row(row.clone())
                        .map_err(|_| invalid())?,
                )),
            }
        }
        let source_interface = relocation.substitute(interface).map_err(|_| invalid())?;
        let original_application = source
            .checked_callable_application(original, &types, &rows)
            .map_err(|error| {
                NativeDefinitionError::Projection(ProjectionError::Invalid(format!(
                    "frozen callable application is unavailable: {error}"
                )))
            })?;
        let original_application = original_application
            .with_checked_interface(source_interface, limits)
            .map_err(|_| invalid())?;
        let definition = self.source_applications.reflect_checked_application(
            &original_application,
            binding.access,
            limits,
        )?;
        Ok((self.source_applications.clone(), definition))
    }

    pub fn frozen_flow_item(
        &self,
        catalog: &DefinitionCatalog,
        item: HirItemId,
    ) -> Result<HirItemId, NativeDefinitionError> {
        let invalid = || NativeDefinitionError::CandidateInputMismatch;
        if !Arc::ptr_eq(catalog.checked_program(), &self.program) {
            return Err(invalid());
        }
        let identity = catalog.ordinary_flow_identity(item).ok_or_else(invalid)?;
        let original = self
            .source
            .catalog
            .item_owner(identity)
            .ok_or_else(invalid)?;
        let source = self.source.catalog.checked_program();
        let before = source
            .definition_sources
            .body(original)
            .ok_or_else(invalid)?;
        let after = self
            .program
            .definition_sources
            .body(item)
            .ok_or_else(invalid)?;
        if before != after {
            return Err(invalid());
        }
        // Identity alone cannot substitute the edited entry for its old body.
        // Any retained overlay must still be the very same native revision.
        match (
            source.definition_sources.overlaid_body(original),
            self.program.definition_sources.overlaid_body(item),
        ) {
            (None, None) => {}
            (Some(before), Some(after)) if Arc::ptr_eq(before, after) => {}
            _ => return Err(invalid()),
        }
        Ok(original)
    }

    pub fn reflect_flow_binding(
        &self,
        catalog: &DefinitionCatalog,
        expression: HirExprId,
        original: HirItemId,
    ) -> Result<NativeCallableReflection, NativeDefinitionError> {
        let invalid = || NativeDefinitionError::CandidateInputMismatch;
        if !Arc::ptr_eq(catalog.checked_program(), &self.program) {
            return Err(invalid());
        }
        let binding = self.bindings.get(&expression).ok_or_else(invalid)?;
        if !matches!(binding.target, SourceTarget::CallableValue) {
            return Err(invalid());
        }
        let identity = self
            .source
            .catalog
            .ordinary_flow_identity(original)
            .ok_or_else(invalid)?;
        let item = catalog.item_owner(identity).ok_or_else(invalid)?;
        catalog
            .callable_value_reflection(expression, item)
            .map_err(NativeDefinitionError::Projection)?;
        let super::super::StaticReflectionView::Callable(header) = self
            .source
            .catalog
            .callable_view(identity, binding.access)
            .map_err(NativeDefinitionError::Projection)?
        else {
            return Err(invalid());
        };
        Ok(NativeCallableReflection {
            header: header.clone(),
            graph: self.source.catalog.descriptors().clone(),
            body: self
                .source
                .catalog
                .retained_body_at_access(identity, binding.access, &self.source.syntax)
                .map_err(NativeDefinitionError::Projection)?,
            definition: Some(self.source.reflect_flow_at_access(
                header,
                original,
                binding.access,
            )?),
            method_snapshot: None,
        })
    }

    pub fn reflect(
        &self,
        program: &CheckedProgram,
        expression: HirExprId,
        candidate_agent: Option<HirItemId>,
    ) -> Result<NativeSourceReflection, NativeDefinitionError> {
        let invalid = || NativeDefinitionError::CandidateInputMismatch;
        if !Arc::ptr_eq(program, &self.program) {
            return Err(invalid());
        }
        let binding = self.bindings.get(&expression).ok_or_else(invalid)?;
        if binding.candidate_agent != candidate_agent {
            return Err(invalid());
        }
        match &binding.target {
            SourceTarget::CallableValue => Err(invalid()),
            SourceTarget::ExternalAgent(external) => {
                let key = SourceReflectionKey::External {
                    component: external.component,
                    access: binding.access,
                };
                if let Some(reference) = self.source.source_view(&key)? {
                    return Ok(NativeSourceReflection::Agent(reference));
                }
                let reference = self.source.reflect_external(external, binding.access)?;
                self.source
                    .remember_source_view(key, reference)
                    .map(NativeSourceReflection::Agent)
                    .map_err(Into::into)
            }
            SourceTarget::Agent { header, item } => {
                let key = SourceReflectionKey::Native {
                    item: *item,
                    access: binding.access,
                };
                if let Some(reference) = self.source.source_view(&key)? {
                    return Ok(NativeSourceReflection::Agent(reference));
                }
                let environment = self
                    .source
                    .catalog
                    .freeze_agent_view(header.clone(), *item)
                    .map_err(NativeDefinitionError::Projection)?;
                let body = header
                    .default_entry
                    .as_ref()
                    .map(|callable| {
                        self.source.catalog.retained_body_at_access(
                            callable,
                            binding.access,
                            &self.source.syntax,
                        )
                    })
                    .transpose()
                    .map_err(NativeDefinitionError::Projection)?
                    .flatten();
                let reference = self
                    .source
                    .reflect_frozen(environment, body, binding.access)?;
                self.source
                    .remember_source_view(key, reference)
                    .map(NativeSourceReflection::Agent)
                    .map_err(Into::into)
            }
            SourceTarget::Callable { header, canonical } => {
                let body = self
                    .source
                    .catalog
                    .retained_body_at_access(canonical, binding.access, &self.source.syntax)
                    .map_err(NativeDefinitionError::Projection)?;
                let definition = if canonical
                    .as_declaration()
                    .is_some_and(|identity| identity.kind() == etas_code::DefinitionKind::Flow)
                {
                    let item = self
                        .source
                        .catalog
                        .item_owner(canonical)
                        .ok_or_else(invalid)?;
                    Some(
                        self.source
                            .reflect_flow_at_access(header, item, binding.access)?,
                    )
                } else {
                    None
                };
                Ok(NativeSourceReflection::Callable(Box::new(
                    NativeCallableReflection {
                        header: header.clone(),
                        graph: self.source.catalog.descriptors().clone(),
                        body,
                        definition,
                        method_snapshot: canonical
                            .as_method()
                            .map(|_| {
                                self.source
                                    .method_snapshot_at_access(canonical, binding.access)
                            })
                            .transpose()?,
                    },
                )))
            }
            SourceTarget::ExternalCallable(external) => {
                let body = self
                    .source
                    .catalog
                    .retained_body_at_access(
                        &external.canonical,
                        binding.access,
                        &self.source.syntax,
                    )
                    .map_err(NativeDefinitionError::Projection)?;
                let definition = if external
                    .canonical
                    .as_declaration()
                    .is_some_and(|id| id.kind() == etas_code::DefinitionKind::Flow)
                    && external.snapshot.is_some()
                {
                    Some(
                        self.source
                            .reflect_external_flow_at_access(external, binding.access)?,
                    )
                } else {
                    None
                };
                Ok(NativeSourceReflection::Callable(Box::new(
                    NativeCallableReflection {
                        header: external.header.clone(),
                        graph: external.graph.clone(),
                        body,
                        definition,
                        method_snapshot: external
                            .canonical
                            .as_method()
                            .map(|_| {
                                self.source
                                    .method_snapshot_at_access(&external.canonical, binding.access)
                            })
                            .transpose()?,
                    },
                )))
            }
        }
    }
}
