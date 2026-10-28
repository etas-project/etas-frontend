use std::{collections::HashMap, sync::Arc};

use etas_types::{
    TypeInterner, TypeOutput, TypeSubstitutionEngine, ty::TypeRelocationMaterializationInput,
};
use etas_utils::{
    ArtifactKey, ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PipelineExecutionControl, PreservedArtifacts,
};

use super::{sources::FROZEN_DEFINITION_ENVIRONMENT, target::CANDIDATE_TARGET};
use crate::{
    CheckedProgram, ProjectContext,
    code::definition::{DefinitionCatalog, FrozenDefinitionEnvironment, NativeTypeCorrespondence},
    passes::artifacts::{CALLABLE_INSTANTIATIONS, CHECKED_PROJECT, TYPE_OUTPUT},
    project::instantiation::{
        self, InstantiationError, NativeGenericArgument, NativeInstantiation,
    },
};

pub(crate) const CANDIDATE_INVOCATION_TYPES: ArtifactKey =
    ArtifactKey::new("code", "candidate_invocation_types");
pub(crate) struct MaterializeCandidateInvocationTypesPass;

impl Pass<ProjectContext> for MaterializeCandidateInvocationTypesPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new(
            "MaterializeCandidateInvocationTypesPass",
            PassKind::Transform,
        )
        .requires(ArtifactSet::from([
            CHECKED_PROJECT,
            TYPE_OUTPUT,
            CALLABLE_INSTANTIATIONS,
            FROZEN_DEFINITION_ENVIRONMENT,
            CANDIDATE_TARGET,
        ]))
        .produces(ArtifactSet::from([
            CANDIDATE_INVOCATION_TYPES,
            CHECKED_PROJECT,
            TYPE_OUTPUT,
            CALLABLE_INSTANTIATIONS,
        ]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let Some(candidate) = &context.candidate else {
            return PassResult::failed("invocation types require a frozen candidate environment");
        };
        if let Some(checked) = &mut context.checked {
            let Some(types) = &mut context.types else {
                return PassResult::failed("candidate invocation types have no type output");
            };
            let prepared = match materialize(
                candidate.overlay.frozen(),
                checked,
                types,
                candidate.descriptor_limits,
                manager.execution_control(),
            ) {
                Ok(prepared) => prepared,
                Err(InstantiationError::Interrupted(interruption)) => {
                    return PassResult::interrupted(interruption);
                }
                Err(InstantiationError::Invalid(reason)) => return PassResult::failed(reason),
                Err(InstantiationError::Resource { reason, .. }) => {
                    return PassResult::interrupted(etas_utils::PipelineInterruption {
                        kind: etas_utils::PipelineInterruptionKind::ResourceLimit,
                        reason,
                    });
                }
            };
            types.store = prepared.store.clone();
            context.native_instantiations = Some(prepared.plan.clone());
            let checked = Arc::make_mut(checked);
            checked.type_store = prepared.store;
            checked.native_instantiations = prepared.plan;
        }
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}

fn materialize(
    frozen: &FrozenDefinitionEnvironment,
    target: &CheckedProgram,
    types: &TypeOutput,
    limits: etas_code::descriptor::DescriptorLimits,
    control: Option<&dyn PipelineExecutionControl>,
) -> Result<instantiation::PreparedInstantiations, InstantiationError> {
    let source = frozen.checked();
    if source.native_instantiations.instances.is_empty() {
        return Ok(instantiation::PreparedInstantiations {
            store: target.type_store.clone(),
            plan: target.native_instantiations.clone(),
        });
    }
    let source_catalog = DefinitionCatalog::build(
        source.clone(),
        &frozen
            .origins_for(source)
            .map_err(|error| error.to_string())?,
        limits,
    )
    .map_err(projection_failure)?;
    let target_catalog = DefinitionCatalog::build(
        target.clone(),
        &frozen
            .origins_for(target)
            .map_err(|error| error.to_string())?,
        limits,
    )
    .map_err(projection_failure)?;
    let correspondence = NativeTypeCorrespondence::build(&source_catalog, &target_catalog)
        .map_err(projection_failure)?;
    let callables = source_catalog
        .native_callables()
        .map(|(id, item)| (item, id))
        .collect::<HashMap<_, _>>();
    let mut interner = TypeInterner::from_store((*target.type_store).clone());
    let mut seeds = Vec::new();
    for instance in &source.native_instantiations.instances {
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(InstantiationError::Interrupted)?;
        }
        let identity = callables
            .get(&instance.owner)
            .ok_or("invocation has no canonical callable identity".to_owned())?;
        let item = target_catalog
            .item_owner(identity)
            .ok_or("candidate has no corresponding invocation method".to_owned())?;
        let from = instantiation::signature(&source.types, instance.owner)
            .ok_or("source invocation has no checked signature".to_owned())?;
        let to = instantiation::signature(&target.types, item)
            .ok_or("candidate invocation has no checked signature".to_owned())?;
        if from.generic_params.len() != to.generic_params.len()
            || from
                .generic_params
                .iter()
                .zip(&to.generic_params)
                .any(|(from, to)| from.kind != to.kind)
        {
            return Err(InstantiationError::Invalid(
                "candidate invocation changed frozen generic binders".into(),
            ));
        }
        let mut relocation =
            TypeSubstitutionEngine::relocating_materialized(TypeRelocationMaterializationInput {
                source: &source.type_store,
                target: &mut interner,
                declaration_types: correspondence.declarations(),
                effect_names: correspondence.effect_names(),
            })
            .with_expansion_depth(limits.max_expansion_depth);
        let arguments = instance
            .arguments
            .iter()
            .map(|argument| match argument {
                NativeGenericArgument::Type(ty) => {
                    relocation.substitute(*ty).map(NativeGenericArgument::Type)
                }
                NativeGenericArgument::EffectRow(row) => relocation
                    .substitute_effect_row(row.clone())
                    .map(NativeGenericArgument::EffectRow),
            })
            .collect::<Result<_, _>>()
            .map_err(InstantiationError::from)?;
        seeds.push(NativeInstantiation {
            owner: item,
            arguments,
        });
    }
    instantiation::prepare(
        instantiation::PreparationInput {
            hir: &target.hir,
            types,
            effects: &target.effects,
            limits,
            control,
        },
        &seeds,
        Some(interner),
    )
}

fn projection_failure(error: crate::code::descriptor::ProjectionError) -> InstantiationError {
    if error.resource_limit().is_some() {
        InstantiationError::Interrupted(etas_utils::PipelineInterruption {
            kind: etas_utils::PipelineInterruptionKind::ResourceLimit,
            reason: error.to_string(),
        })
    } else {
        InstantiationError::Invalid(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_type_preparation_requires_checked_pipeline_artifacts() {
        let mut context = ProjectContext::new(crate::ProjectInput::single_source(
            crate::SourceInput::anonymous(""),
        ));
        let mut pipeline = etas_utils::Pipeline::new("missing_invocation_types")
            .pass(MaterializeCandidateInvocationTypesPass);
        let run = PassManager::new().run_pipeline(&mut pipeline, &mut context);
        let etas_utils::PassControl::Failed(failure) = run.control else {
            panic!("missing input artifacts must fail");
        };
        assert!(
            MaterializeCandidateInvocationTypesPass
                .descriptor()
                .requires
                .contains(failure.missing_artifact.unwrap())
        );
        assert!(context.candidate.is_none());
    }
}
