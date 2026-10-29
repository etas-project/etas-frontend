use std::sync::Arc;

use etas_hir::HirRevision;
use etas_types::{SignatureEnvironment, TypeOutput, TypeStore};
use etas_utils::PipelineExecutionControl;

use super::EffectPipelineInput;
use crate::{DependencyEffectMetadata, EffectPipelineError, EffectRegistry};

/// Session-local declaration registry, pinned to the exact checked HIR,
/// signature prefix, standard registry and dependency metadata. It is not a
/// serialized artifact or permission to skip final type validation.
pub struct PreparedEffectRegistry {
    hir: HirRevision,
    base: etas_types::ty::SharedTypeBase,
    facts: Arc<etas_types::TypeFacts>,
    std_registry: Arc<etas_std::StdRegistry>,
    dependencies: Arc<DependencyEffectMetadata>,
    registry: EffectRegistry,
}

impl PreparedEffectRegistry {
    pub fn build(
        hir: &HirRevision,
        signatures: &TypeOutput,
        environment: &SignatureEnvironment,
        std_registry: Arc<etas_std::StdRegistry>,
        dependencies: Arc<DependencyEffectMetadata>,
        control: Option<&dyn PipelineExecutionControl>,
    ) -> Result<Self, EffectPipelineError> {
        if !environment.owns_signature_facts(&signatures.facts) {
            return Err(invalid(
                "signature facts do not belong to the type environment",
            ));
        }
        let base = environment.type_base();
        verify_prefix(&signatures.store, base, true, control)?;
        let registry = EffectRegistry::from_hir_types_dependencies_and_std(
            hir,
            signatures,
            &dependencies,
            &std_registry,
        )?;
        checkpoint(control)?;
        Ok(Self {
            hir: hir.clone(),
            base: base.clone(),
            facts: signatures.facts.clone(),
            std_registry,
            dependencies,
            registry,
        })
    }

    pub fn registry(&self) -> &EffectRegistry {
        &self.registry
    }

    pub fn dependencies(&self) -> &Arc<DependencyEffectMetadata> {
        &self.dependencies
    }

    pub(super) fn finish(
        mut self,
        input: &EffectPipelineInput<'_>,
        control: Option<&dyn PipelineExecutionControl>,
    ) -> Result<EffectRegistry, EffectPipelineError> {
        checkpoint(control)?;
        if !Arc::ptr_eq(self.hir.shared_program(), input.hir.shared_program())
            || !std::ptr::eq(self.std_registry.as_ref(), input.std_registry)
            || input.dependency_metadata != Some(self.dependencies.as_ref())
        {
            return Err(invalid(
                "HIR, standard registry or dependency metadata changed",
            ));
        }
        verify_prefix(&input.types.store, &self.base, false, control)?;
        if self.facts.action_signatures != input.types.facts.action_signatures
            || self.facts.qualified_action_signatures
                != input.types.facts.qualified_action_signatures
        {
            return Err(invalid(
                "checked action signatures changed after registry preparation",
            ));
        }
        for entry in input.types.store.iter() {
            checkpoint(control)?;
            self.registry.register_memory_places(std::iter::once(entry));
        }
        checkpoint(control)?;
        Ok(self.registry)
    }
}

fn checkpoint(control: Option<&dyn PipelineExecutionControl>) -> Result<(), EffectPipelineError> {
    if let Some(control) = control {
        control
            .checkpoint()
            .map_err(EffectPipelineError::Interrupted)?;
    }
    Ok(())
}

fn verify_prefix(
    store: &TypeStore,
    base: &etas_types::ty::SharedTypeBase,
    exact: bool,
    control: Option<&dyn PipelineExecutionControl>,
) -> Result<(), EffectPipelineError> {
    if store.len() < base.len() || (exact && store.len() != base.len()) {
        return Err(invalid("signature type universe has a different size"));
    }
    let prefix = TypeStore::with_base(base.clone());
    for ((_, actual), (_, expected)) in store.iter().zip(prefix.iter()) {
        checkpoint(control)?;
        if actual != expected {
            return Err(invalid("signature type universe changed"));
        }
    }
    Ok(())
}

fn invalid(reason: &str) -> EffectPipelineError {
    EffectPipelineError::InvalidCallableFacts {
        reason: format!("prepared effect registry: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use etas_hir::HirProgram;
    use etas_types::{Type, ty::MemoryPlaceType};

    fn input<'a>(
        hir: &'a HirRevision,
        types: &'a TypeOutput,
        std_registry: &'a etas_std::StdRegistry,
        dependencies: Option<&'a DependencyEffectMetadata>,
    ) -> EffectPipelineInput<'a> {
        EffectPipelineInput {
            frozen_calls: None,
            hir,
            types,
            std_registry,
            dependency_metadata: dependencies,
            tool_bindings: &[],
            external_summaries: &[],
            external_trace_specs: &[],
            external_artifact_anchors: &[],
            reachable_items: None,
        }
    }

    #[test]
    fn prepared_registry_adds_body_memory_places_without_rebuilding_declarations() {
        let hir = HirRevision::try_new(HirProgram::default()).unwrap();
        let signatures = TypeOutput::default();
        let environment = SignatureEnvironment::new(&hir, &signatures);
        let registry = Arc::new(etas_std::standard_registry());
        let dependencies = Arc::new(DependencyEffectMetadata::default());
        let prepared = PreparedEffectRegistry::build(
            &hir,
            &signatures,
            &environment,
            registry.clone(),
            dependencies.clone(),
            None,
        )
        .unwrap();
        let mut store = TypeStore::with_base(environment.type_base().clone());
        let place = store.intern(Type::MemoryPlace(MemoryPlaceType {
            segments: vec!["app".into(), "cache".into()],
        }));
        assert!(prepared.registry().memory_place(place).is_none());
        let types = TypeOutput {
            store: Arc::new(store),
            ..signatures
        };
        let checked = input(&hir, &types, &registry, Some(&dependencies));
        let finished = prepared.finish(&checked, None).unwrap();
        assert_eq!(
            finished.memory_place(place).unwrap().segments,
            ["app", "cache"]
        );
    }

    #[test]
    fn prepared_registry_rejects_foreign_signature_hir_and_missing_dependencies() {
        let hir = HirRevision::try_new(HirProgram::default()).unwrap();
        let other_hir = HirRevision::try_new(HirProgram::default()).unwrap();
        let signatures = TypeOutput::default();
        let environment = SignatureEnvironment::new(&hir, &signatures);
        let registry = Arc::new(etas_std::standard_registry());
        let dependencies = Arc::new(DependencyEffectMetadata::default());
        let foreign_signatures = TypeOutput {
            facts: Arc::new((*signatures.facts).clone()),
            ..signatures.clone()
        };
        assert!(
            PreparedEffectRegistry::build(
                &hir,
                &foreign_signatures,
                &environment,
                registry.clone(),
                dependencies.clone(),
                None,
            )
            .is_err()
        );
        let prepare = || {
            PreparedEffectRegistry::build(
                &hir,
                &signatures,
                &environment,
                registry.clone(),
                dependencies.clone(),
                None,
            )
            .unwrap()
        };
        assert!(
            prepare()
                .finish(
                    &input(&other_hir, &signatures, &registry, Some(&dependencies)),
                    None
                )
                .is_err()
        );
        assert!(
            prepare()
                .finish(&input(&hir, &signatures, &registry, None), None)
                .is_err()
        );
        let other_registry = etas_std::standard_registry();
        assert!(
            prepare()
                .finish(
                    &input(&hir, &signatures, &other_registry, Some(&dependencies)),
                    None
                )
                .is_err()
        );
    }

    #[test]
    fn prepared_registry_rejects_changed_signature_type_prefix() {
        let hir = HirRevision::try_new(HirProgram::default()).unwrap();
        let mut signature_store = TypeStore::new();
        signature_store.intern(Type::Primitive(etas_types::PrimitiveType::Bool));
        let signatures = TypeOutput {
            store: Arc::new(signature_store),
            ..TypeOutput::default()
        };
        let environment = SignatureEnvironment::new(&hir, &signatures);
        let registry = Arc::new(etas_std::standard_registry());
        let dependencies = Arc::new(DependencyEffectMetadata::default());
        let prepared = PreparedEffectRegistry::build(
            &hir,
            &signatures,
            &environment,
            registry.clone(),
            dependencies.clone(),
            None,
        )
        .unwrap();
        let mut foreign_store = TypeStore::new();
        foreign_store.intern(Type::Primitive(etas_types::PrimitiveType::I32));
        let foreign = TypeOutput {
            store: Arc::new(foreign_store),
            ..signatures
        };
        assert!(
            prepared
                .finish(&input(&hir, &foreign, &registry, Some(&dependencies)), None)
                .is_err()
        );
    }
}
