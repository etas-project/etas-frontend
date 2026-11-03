use super::*;

pub(crate) struct BuildRegistryPass;

impl<'a> Pass<EffectPipelineContext<'a>> for BuildRegistryPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.build_registry", PassKind::Analysis)
            .produces(ArtifactSet::one(EFFECT_REGISTRY))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let empty_dependency_metadata = DependencyEffectMetadata::default();
        let source = std::mem::replace(
            &mut context.registry_source,
            super::super::context::RegistrySource::Consumed,
        );
        let result = match source {
            super::super::context::RegistrySource::CheckedTypes => {
                EffectRegistry::from_hir_types_dependencies_and_std(
                    context.input.hir,
                    context.input.types,
                    context
                        .input
                        .dependency_metadata
                        .unwrap_or(&empty_dependency_metadata),
                    context.input.std_registry,
                )
            }
            super::super::context::RegistrySource::Prepared(prepared) => {
                prepared.finish(&context.input, manager.execution_control())
            }
            super::super::context::RegistrySource::Consumed => {
                return context.fail(missing(EffectStage::BuildRegistry, "registry source"));
            }
        };
        let registry = match result {
            Ok(registry) => registry,
            Err(error) => return context.fail(error),
        };
        match dependency_registry_diagnostics(&registry, context.input.external_artifact_anchors) {
            Ok(diagnostics) => context.registry_diagnostics = diagnostics,
            Err(error) => return context.fail(error),
        }
        context.registry = Some(registry);
        produced(EFFECT_REGISTRY)
    }
}
