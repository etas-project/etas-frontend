use std::sync::Arc;

use etas_types::{
    CallableGenericArg, TypeInterner, TypeStore, TypeSubstitutionBudget, TypeSubstitutionEngine,
    ty::TypeRelocationMaterializationInput,
};
use etas_utils::{
    ArtifactKey, ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PipelineExecutionControl, PipelineInterruption, PreservedArtifacts,
};

use super::{sources::FROZEN_DEFINITION_ENVIRONMENT, target::CANDIDATE_TARGET};
use crate::code::pipeline::context::BoundCandidateApplication;
use crate::{
    ProjectContext,
    code::{
        definition::NativeTypeCorrespondence, descriptor::DescriptorInput,
        identity::DefinitionIdentityIndex,
    },
    passes::artifacts::{
        HIR_OUTPUT, MODULE_INDEX, PREPARED_EFFECT_REGISTRY, RESOLVED_IMPORTS, SIGNATURE_FACTS,
        TYPE_ENVIRONMENT,
    },
};

const BOUND_CANDIDATE_APPLICATION: ArtifactKey =
    ArtifactKey::new("code", "bound_candidate_application");

pub(crate) struct PrepareCandidateApplicationPass;

impl Pass<ProjectContext> for PrepareCandidateApplicationPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("PrepareCandidateApplicationPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                MODULE_INDEX,
                RESOLVED_IMPORTS,
                SIGNATURE_FACTS,
                TYPE_ENVIRONMENT,
                PREPARED_EFFECT_REGISTRY,
                FROZEN_DEFINITION_ENVIRONMENT,
                CANDIDATE_TARGET,
            ]))
            .produces(ArtifactSet::from([
                BOUND_CANDIDATE_APPLICATION,
                TYPE_ENVIRONMENT,
            ]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let Some(candidate) = context.candidate.as_ref() else {
            return PassResult::failed("bound candidate preparation requires a candidate");
        };
        let Some(application) = candidate.overlay.frozen().callable_application() else {
            return PassResult::changed(PreservedArtifacts::All, self.descriptor().produces);
        };
        let (
            Some(target),
            Some(hir),
            Some(signatures),
            Some(base),
            Some(imports),
            Some(modules),
            Some(registry),
        ) = (
            candidate.target,
            context.hir.as_ref(),
            context.signature_types.as_ref(),
            context.type_environment.as_ref(),
            context.resolved_imports.as_ref(),
            context.modules.as_ref(),
            context.prepared_effect_registry.as_ref(),
        )
        else {
            return PassResult::failed(
                "bound candidate preparation requires checked signature artifacts",
            );
        };
        let frozen = candidate.overlay.frozen();
        let source = frozen.checked();
        if !Arc::ptr_eq(application.program(), source)
            || application.owner() != frozen.declaration()
        {
            return PassResult::failed(
                "bound candidate application does not belong to its frozen declaration",
            );
        }
        let control = manager.execution_control().unwrap_or(&Continue);
        let result = prepare(PreparationInput {
            application,
            frozen,
            target,
            hir: &hir.hir,
            signatures,
            base,
            imports,
            modules,
            registry: registry.registry(),
            std_registry: &context.std_registry,
            limits: candidate.descriptor_limits,
            control,
        });
        match result {
            Ok(prepared) => {
                context.type_environment = Some(prepared.base);
                let Some(candidate) = context.candidate.as_mut() else {
                    return PassResult::failed(
                        "candidate disappeared during bound application preparation",
                    );
                };
                candidate.bound_application = Some(BoundCandidateApplication {
                    owner: target,
                    arguments: prepared.arguments,
                    presented_type: prepared.presented_type,
                    foreign_type_bindings: prepared.foreign_type_bindings,
                });
                PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
            }
            Err(PreparationError::Interrupted(interruption)) => {
                PassResult::interrupted(interruption)
            }
            Err(PreparationError::Invalid(reason)) => PassResult::failed(reason),
        }
    }
}

struct Continue;
impl PipelineExecutionControl for Continue {
    fn checkpoint(&self) -> Result<(), PipelineInterruption> {
        Ok(())
    }
}

enum PreparationError {
    Interrupted(PipelineInterruption),
    Invalid(String),
}

struct PreparationInput<'a> {
    application: &'a crate::CheckedCallableApplication,
    frozen: &'a crate::code::definition::FrozenDefinitionEnvironment,
    target: etas_hir::HirItemId,
    hir: &'a etas_hir::HirRevision,
    signatures: &'a etas_types::TypeOutput,
    base: &'a etas_types::SignatureEnvironment,
    imports: &'a crate::ResolvedImports,
    modules: &'a crate::ModuleIndex,
    registry: &'a etas_effects::EffectRegistry,
    std_registry: &'a etas_std::StdRegistry,
    limits: etas_code::descriptor::DescriptorLimits,
    control: &'a dyn PipelineExecutionControl,
}

struct PreparedCandidateApplication {
    base: etas_types::SignatureEnvironment,
    arguments: Vec<CallableGenericArg>,
    presented_type: Option<etas_types::TypeId>,
    foreign_type_bindings: Arc<crate::code::definition::ForeignTypeBindings>,
}

fn prepare(input: PreparationInput<'_>) -> Result<PreparedCandidateApplication, PreparationError> {
    let PreparationInput {
        application,
        frozen,
        target,
        hir,
        signatures,
        base,
        imports,
        modules,
        registry,
        std_registry,
        limits,
        control,
    } = input;
    control
        .checkpoint()
        .map_err(PreparationError::Interrupted)?;
    let source = frozen.checked();
    let source_input: DescriptorInput<'_> = source.as_ref().into();
    let target_input = DescriptorInput {
        hir,
        sources: None,
        types: &signatures.facts,
        type_store: &signatures.store,
        std_registry,
        resolved_imports: imports,
        module_index: modules,
        effect_registry: registry,
        effects: None,
        foreign_type_bindings: None,
    };
    let source_origins = frozen
        .origins_for(source)
        .map_err(|e| PreparationError::Invalid(e.to_string()))?;
    let target_origins = frozen
        .origins_for_hir(hir)
        .map_err(|e| PreparationError::Invalid(e.to_string()))?;
    let source_ids =
        DefinitionIdentityIndex::from_input_with_limits(source_input, &source_origins, limits)
            .map_err(|e| PreparationError::Invalid(e.to_string()))?;
    control
        .checkpoint()
        .map_err(PreparationError::Interrupted)?;
    let target_ids =
        DefinitionIdentityIndex::from_input_with_limits(target_input, &target_origins, limits)
            .map_err(|e| PreparationError::Invalid(e.to_string()))?;
    let correspondence =
        NativeTypeCorrespondence::from_inputs(source_input, &source_ids, target_input, &target_ids)
            .map_err(|e| PreparationError::Invalid(e.to_string()))?;
    let source_signature =
        crate::project::instantiation::signature(&source.types, application.owner()).ok_or_else(
            || PreparationError::Invalid("source application has no signature".into()),
        )?;
    let target_signature = crate::project::instantiation::signature(&signatures.facts, target)
        .ok_or_else(|| {
            PreparationError::Invalid("candidate application has no signature".into())
        })?;
    if source_signature.generic_params.len() != target_signature.generic_params.len()
        || source_signature
            .generic_params
            .iter()
            .zip(&target_signature.generic_params)
            .any(|(a, b)| a.kind != b.kind || a.name != b.name)
    {
        return Err(PreparationError::Invalid(
            "candidate changed its frozen generic binders".into(),
        ));
    }
    let interner = TypeInterner::from_store(TypeStore::with_base(base.type_base().clone()));
    let budget = TypeSubstitutionBudget::new(limits.max_nodes, limits.max_edges, Some(control));
    if application.arguments().len() > limits.max_nodes {
        return Err(PreparationError::Interrupted(PipelineInterruption {
            kind: etas_utils::PipelineInterruptionKind::ResourceLimit,
            reason: "candidate generic application exceeds the type limit".into(),
        }));
    }
    let mut roots = Vec::new();
    let mut rows = Vec::new();
    for argument in application.arguments() {
        control
            .checkpoint()
            .map_err(PreparationError::Interrupted)?;
        match argument {
            CallableGenericArg::Type(ty) => roots.push(*ty),
            CallableGenericArg::EffectRow(row) => rows.push(row),
        }
    }
    if let Some(interface) = application.presented_type() {
        roots.push(interface);
    }
    let (reachable, spec_plan) = crate::code::definition::checked_type_and_spec_plan(
        crate::code::definition::CheckedTypeAndSpecPlanInput {
            source,
            target: std_registry,
            roots: &roots,
            rows: &rows,
            declarations: correspondence.declarations(),
            identity: &|spec| match spec {
                etas_types::CheckedSpecRef::Source(symbol) => source_ids
                    .symbol(*symbol)
                    .cloned()
                    .ok_or(crate::code::descriptor::ProjectionError::MissingDeclaration(*symbol)),
                etas_types::CheckedSpecRef::External(external) => {
                    source_ids.path(&external.path).cloned().ok_or_else(|| {
                        crate::code::descriptor::ProjectionError::MissingIdentity(
                            "external spec".into(),
                        )
                    })
                }
                etas_types::CheckedSpecRef::Canonical(identity) => Ok(identity.clone()),
                etas_types::CheckedSpecRef::Std(_) => {
                    Err(crate::code::descriptor::ProjectionError::Invalid(
                        "standard spec requires registry identity".into(),
                    ))
                }
            },
            max_depth: limits.max_expansion_depth,
            control,
            budget: &budget,
        },
    )
    .map_err(|error| match error {
        crate::code::descriptor::ProjectionError::Syntax(
            etas_code::syntax::SyntaxOperationError::Interrupted(interruption),
        ) => PreparationError::Interrupted(interruption),
        other => PreparationError::Invalid(other.to_string()),
    })?;
    let mut identities = std::collections::HashMap::new();
    for ty in &reachable {
        if matches!(
            source.type_store.get(*ty),
            Some(etas_types::Type::Nominal(_) | etas_types::Type::Enum(_))
        ) {
            let declaration = source_ids
                .types
                .get(ty)
                .filter(|declaration| declaration.alias.is_none())
                .ok_or_else(|| {
                    PreparationError::Invalid(format!("missing checked type identity {ty:?}"))
                })?;
            identities.insert(*ty, declaration.identity.clone());
        }
    }
    let foreign = etas_types::ty::relocate_foreign_type_graph(
        interner,
        etas_types::ty::ForeignTypeGraphInput {
            source: &source.type_store,
            facts: &source.types,
            target_facts: &signatures.facts,
            reachable: &reachable,
            identities: &identities,
            declarations: correspondence.declarations(),
            effect_names: correspondence.effect_names(),
            specs: &spec_plan.mapping,
            max_depth: limits.max_expansion_depth,
            budget: &budget,
        },
    )
    .map_err(preparation_error)?;
    let mut foreign_type_bindings = source
        .foreign_type_bindings
        .relocate_checked(&foreign.declarations, foreign.interner.store(), control)
        .map_err(|error| match error {
            crate::code::descriptor::ProjectionError::Syntax(
                etas_code::syntax::SyntaxOperationError::Interrupted(interruption),
            ) => PreparationError::Interrupted(interruption),
            other => PreparationError::Invalid(other.to_string()),
        })?;
    if spec_plan
        .mapping
        .values()
        .any(|target| matches!(target, etas_types::CheckedSpecRef::Canonical(_)))
    {
        let catalog = frozen.proof_catalog().ok_or_else(|| {
            PreparationError::Invalid("missing checked source spec catalog".into())
        })?;
        foreign_type_bindings
            .extend_specs_checked(catalog, &spec_plan.mapping, control)
            .map_err(|error| match error {
                crate::code::descriptor::ProjectionError::Syntax(
                    etas_code::syntax::SyntaxOperationError::Interrupted(interruption),
                ) => PreparationError::Interrupted(interruption),
                other => PreparationError::Invalid(other.to_string()),
            })?;
    }
    for ty in foreign
        .type_generics
        .keys()
        .chain(foreign.enum_layouts.keys())
    {
        if !foreign_type_bindings.contains(*ty) {
            return Err(PreparationError::Invalid(format!(
                "missing checked foreign type evidence {ty:?}"
            )));
        }
    }
    let mut interner = foreign.interner;
    let declarations = foreign.declarations;
    let mut relocation =
        TypeSubstitutionEngine::relocating_materialized(TypeRelocationMaterializationInput {
            source: &source.type_store,
            target: &mut interner,
            declaration_types: &declarations,
            effect_names: correspondence.effect_names(),
        })
        .with_expansion_depth(limits.max_expansion_depth)
        .with_budget(&budget);
    let mut arguments = Vec::with_capacity(application.arguments().len());
    for argument in application.arguments() {
        let mapped = match argument {
            CallableGenericArg::Type(ty) => {
                relocation.substitute(*ty).map(CallableGenericArg::Type)
            }
            CallableGenericArg::EffectRow(row) => relocation
                .substitute_effect_row(row.clone())
                .map(CallableGenericArg::EffectRow),
        }
        .map_err(preparation_error)?;
        arguments.push(mapped);
    }
    let presented_type = application
        .presented_type()
        .map(|interface| relocation.substitute(interface))
        .transpose()
        .map_err(preparation_error)?;
    drop(relocation);
    let spec_facts = spec_plan
        .materialize(crate::code::definition::SpecMaterializationInput {
            source: &source.type_store,
            target: &mut interner,
            declarations: &declarations,
            effect_names: correspondence.effect_names(),
            reachable: &reachable,
            max_depth: limits.max_expansion_depth,
            budget: &budget,
        })
        .map_err(preparation_error)?;
    for identity in spec_facts.canonical.signatures.keys() {
        if !foreign_type_bindings.verifies_spec(identity) {
            return Err(PreparationError::Invalid(
                "missing canonical spec origin proof".into(),
            ));
        }
    }
    let max_types = base.type_base().len().saturating_add(limits.max_nodes);
    let base = base
        .extend_checked_declarations_controlled(
            interner.into_store(),
            foreign.type_generics,
            foreign.enum_layouts,
            spec_facts,
            max_types,
            control,
        )
        .map_err(|error| match error {
            etas_types::SignatureBaseExtensionError::Interrupted(interruption) => {
                PreparationError::Interrupted(interruption)
            }
            etas_types::SignatureBaseExtensionError::TypeLimit { .. } => {
                PreparationError::Interrupted(PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::ResourceLimit,
                    reason: error.to_string(),
                })
            }
            _ => PreparationError::Invalid(error.to_string()),
        })?;
    Ok(PreparedCandidateApplication {
        base,
        arguments,
        presented_type,
        foreign_type_bindings: Arc::new(foreign_type_bindings),
    })
}

fn preparation_error(error: etas_types::TypeSubstitutionError) -> PreparationError {
    match error {
        etas_types::TypeSubstitutionError::Interrupted(interruption) => {
            PreparationError::Interrupted(interruption)
        }
        etas_types::TypeSubstitutionError::WorkLimit { .. }
        | etas_types::TypeSubstitutionError::ExpansionDepth(_)
        | etas_types::TypeSubstitutionError::RepresentationLimit(_) => {
            PreparationError::Interrupted(PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::ResourceLimit,
                reason: error.to_string(),
            })
        }
        _ => PreparationError::Invalid(error.to_string()),
    }
}
