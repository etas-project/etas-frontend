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
            Ok((base, arguments)) => {
                context.type_environment = Some(base);
                let Some(candidate) = context.candidate.as_mut() else {
                    return PassResult::failed(
                        "candidate disappeared during bound application preparation",
                    );
                };
                candidate.bound_application = Some(BoundCandidateApplication {
                    owner: target,
                    arguments,
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

fn prepare(
    input: PreparationInput<'_>,
) -> Result<(etas_types::SignatureEnvironment, Vec<CallableGenericArg>), PreparationError> {
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
        types: &signatures.facts,
        type_store: &signatures.store,
        std_registry,
        resolved_imports: imports,
        module_index: modules,
        effect_registry: registry,
        effects: None,
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
    let mut interner = TypeInterner::from_store(TypeStore::with_base(base.type_base().clone()));
    let budget = TypeSubstitutionBudget::new(limits.max_nodes, limits.max_edges, Some(control));
    let mut relocation =
        TypeSubstitutionEngine::relocating_materialized(TypeRelocationMaterializationInput {
            source: &source.type_store,
            target: &mut interner,
            declaration_types: correspondence.declarations(),
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
        .map_err(|error| match error {
            etas_types::TypeSubstitutionError::Interrupted(interruption) => {
                PreparationError::Interrupted(interruption)
            }
            etas_types::TypeSubstitutionError::WorkLimit { .. }
            | etas_types::TypeSubstitutionError::ExpansionDepth(_) => {
                PreparationError::Interrupted(PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::ResourceLimit,
                    reason: error.to_string(),
                })
            }
            _ => PreparationError::Invalid(error.to_string()),
        })?;
        arguments.push(mapped);
    }
    let max_types = base.type_base().len().saturating_add(limits.max_nodes);
    let base = base
        .extend_type_base_controlled(interner.into_store(), max_types, control)
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
    Ok((base, arguments))
}
