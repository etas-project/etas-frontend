use std::{collections::HashMap, sync::Arc};

use etas_types::method::MethodTarget;
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use super::effect_check::residual_identity::{
    CheckedResidualIdentities, CheckedResidualIdentityInput,
};
use crate::{
    ProjectContext,
    passes::artifacts::{
        BOUND_METHOD_ORIGINS, HIR_OUTPUT, MODULE_INDEX, PARSED_SOURCE_SET,
        PREPARED_EFFECT_REGISTRY, RESOLVED_IMPORTS, SIGNATURE_FACTS, SOURCE_SET, TYPE_ENVIRONMENT,
        VALIDATED_EXTERNAL_ENVIRONMENT,
    },
};

pub struct BindMethodOriginsPass;

impl Pass<ProjectContext> for BindMethodOriginsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("BindMethodOriginsPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                MODULE_INDEX,
                PARSED_SOURCE_SET,
                PREPARED_EFFECT_REGISTRY,
                RESOLVED_IMPORTS,
                SIGNATURE_FACTS,
                SOURCE_SET,
                TYPE_ENVIRONMENT,
                VALIDATED_EXTERNAL_ENVIRONMENT,
            ]))
            .produces(ArtifactSet::from([
                BOUND_METHOD_ORIGINS,
                TYPE_ENVIRONMENT,
                SIGNATURE_FACTS,
            ]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let (
            Some(hir),
            Some(modules),
            Some(signatures),
            Some(base),
            Some(imports),
            Some(environment),
            Some(sources),
            Some(registry),
        ) = (
            context.hir.as_ref(),
            context.modules.as_ref(),
            context.signature_types.as_ref(),
            context.type_environment.as_ref(),
            context.resolved_imports.as_ref(),
            context.validated_external_environment.as_ref(),
            context.sources.as_ref(),
            context.prepared_effect_registry.as_ref(),
        )
        else {
            return PassResult::failed(
                "method origins require prepared signature and import artifacts",
            );
        };
        let mut origins = HashMap::new();
        if !signatures.facts.external_methods.is_empty()
            && signatures.facts.methods.entries().any(|method| {
                matches!(
                    method.target,
                    MethodTarget::Source { .. } | MethodTarget::Agent { .. }
                )
            })
        {
            let identities = CheckedResidualIdentities::new(CheckedResidualIdentityInput {
                hir: &hir.hir,
                types: signatures,
                std_registry: &context.std_registry,
                imports,
                modules,
                sources,
                parsed: &context.parsed_sources,
                environment,
                overlay: context
                    .candidate
                    .as_ref()
                    .map(|candidate| &candidate.overlay),
                control: manager.execution_control(),
                foreign_type_bindings: None,
                limits: context
                    .candidate
                    .as_ref()
                    .map_or_else(Default::default, |candidate| candidate.descriptor_limits),
            });
            let index = match identities.index(registry.registry()) {
                Ok(index) => index,
                Err(error) => return PassResult::failed(error.to_string()),
            };
            for method in signatures.facts.methods.entries() {
                if let MethodTarget::Source { symbol, .. } | MethodTarget::Agent { symbol, .. } =
                    method.target
                {
                    let Some(identity) = index.method(symbol) else {
                        return PassResult::failed(
                            "source method has no canonical declaration identity",
                        );
                    };
                    origins.insert(symbol, identity.clone());
                }
            }
        }
        let methods = match signatures.facts.methods.bind_source_origins(
            &signatures.store,
            &signatures.facts,
            &origins,
        ) {
            Ok(methods) => methods,
            Err(error) => return PassResult::failed(error),
        };
        let base = base.with_method_index(Arc::new(methods));
        context
            .signature_types
            .as_mut()
            .expect("signature facts")
            .facts = base.signature_facts().clone();
        context.type_environment = Some(base);
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}
