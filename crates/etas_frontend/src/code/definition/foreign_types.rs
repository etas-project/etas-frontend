use std::{collections::HashMap, sync::Arc};

use etas_code::DefinitionKey;
use etas_types::{CheckedSpecRef, TypeId, TypeStore};
use etas_utils::PipelineExecutionControl;

use super::DefinitionCatalog;
use crate::code::descriptor::ProjectionError;

/// An internal checked-origin proof; it does not introduce lexical names.
#[derive(Clone, Default, etas_core::RetainedSize)]
pub(crate) struct ForeignTypeBindings {
    bindings: HashMap<TypeId, ForeignTypeBinding>,
    specs: HashMap<DefinitionKey, ForeignSpecBinding>,
}

#[derive(Clone, etas_core::RetainedSize)]
struct ForeignTypeBinding {
    identity: DefinitionKey,
    source: TypeId,
    catalog: Arc<DefinitionCatalog>,
}

#[derive(Clone, etas_core::RetainedSize)]
struct ForeignSpecBinding {
    source: CheckedSpecRef,
    catalog: Arc<DefinitionCatalog>,
}

impl std::fmt::Debug for ForeignTypeBindings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ForeignTypeBindings")
            .field("count", &self.bindings.len())
            .field("spec_count", &self.specs.len())
            .finish()
    }
}

impl ForeignTypeBindings {
    pub(crate) fn is_empty(&self) -> bool {
        self.bindings.is_empty() && self.specs.is_empty()
    }

    pub(crate) fn relocate_checked(
        &self,
        declarations: &HashMap<TypeId, TypeId>,
        target: &TypeStore,
        control: &dyn PipelineExecutionControl,
    ) -> Result<Self, ProjectionError> {
        let mut result = Self::default();
        for (source, binding) in &self.bindings {
            control.checkpoint().map_err(|error| {
                ProjectionError::Syntax(etas_code::syntax::SyntaxOperationError::Interrupted(error))
            })?;
            let Some(target_id) = declarations.get(source).copied() else {
                continue;
            };
            let Some(target_identity) = target.nominal_identity(target_id) else {
                continue;
            };
            if target_identity != &binding.identity
                || binding.catalog.native_type_declaration(binding.source)
                    != Some(&binding.identity)
            {
                return Err(ProjectionError::MissingIdentity(format!(
                    "foreign checked type {source:?}"
                )));
            }
            if result.bindings.insert(target_id, binding.clone()).is_some() {
                return Err(ProjectionError::Invalid(
                    "duplicate foreign type relocation".into(),
                ));
            }
        }
        for (identity, binding) in &self.specs {
            control.checkpoint().map_err(|error| {
                ProjectionError::Syntax(etas_code::syntax::SyntaxOperationError::Interrupted(error))
            })?;
            result.specs.insert(identity.clone(), binding.clone());
        }
        Ok(result)
    }

    pub(crate) fn contains(&self, ty: TypeId) -> bool {
        self.bindings.contains_key(&ty)
    }

    pub(crate) fn extend_checked(
        &self,
        catalog: &Arc<DefinitionCatalog>,
        identities: &HashMap<TypeId, DefinitionKey>,
        declarations: &HashMap<TypeId, TypeId>,
        target: &TypeStore,
        control: &dyn PipelineExecutionControl,
    ) -> Result<Self, ProjectionError> {
        let mut result = Self::default();
        for (id, binding) in &self.bindings {
            control.checkpoint().map_err(|error| {
                ProjectionError::Syntax(etas_code::syntax::SyntaxOperationError::Interrupted(error))
            })?;
            result.bindings.insert(*id, binding.clone());
        }
        for (identity, binding) in &self.specs {
            control.checkpoint().map_err(|error| {
                ProjectionError::Syntax(etas_code::syntax::SyntaxOperationError::Interrupted(error))
            })?;
            result.specs.insert(identity.clone(), binding.clone());
        }
        for (source, identity) in identities {
            control.checkpoint().map_err(|error| {
                ProjectionError::Syntax(etas_code::syntax::SyntaxOperationError::Interrupted(error))
            })?;
            let Some(target_id) = declarations.get(source).copied() else {
                continue;
            };
            // Declarations already present in the destination are represented
            // by its own checked index, not by a foreign binding.
            match target.nominal_identity(target_id) {
                None => continue,
                Some(actual) if actual == identity => {}
                Some(_) => {
                    return Err(ProjectionError::Invalid(
                        "conflicting foreign type identities".into(),
                    ));
                }
            }
            if catalog.native_type_declaration(*source) != Some(identity) {
                return Err(ProjectionError::MissingIdentity(format!(
                    "foreign checked declaration {source:?}"
                )));
            }
            if let Some(existing) = result.bindings.get(&target_id) {
                if existing.identity != *identity {
                    return Err(ProjectionError::Invalid(
                        "conflicting foreign type identities".into(),
                    ));
                }
                continue;
            }
            result.bindings.insert(
                target_id,
                ForeignTypeBinding {
                    identity: identity.clone(),
                    source: *source,
                    catalog: catalog.clone(),
                },
            );
        }
        Ok(result)
    }

    pub(crate) fn extend_specs_checked(
        &mut self,
        catalog: &Arc<DefinitionCatalog>,
        mapping: &HashMap<CheckedSpecRef, CheckedSpecRef>,
        control: &dyn PipelineExecutionControl,
    ) -> Result<(), ProjectionError> {
        for (source, target) in mapping {
            control.checkpoint().map_err(|error| {
                ProjectionError::Syntax(etas_code::syntax::SyntaxOperationError::Interrupted(error))
            })?;
            let CheckedSpecRef::Canonical(identity) = target else {
                continue;
            };
            if identity.kind() != etas_code::DefinitionKind::Spec
                || catalog.native_spec_identity(source)? != identity
            {
                return Err(ProjectionError::Invalid(
                    "foreign spec has conflicting checked identity".into(),
                ));
            }
            if let CheckedSpecRef::Canonical(previous) = source {
                let origin = catalog
                    .checked_program()
                    .foreign_type_bindings
                    .specs
                    .get(previous)
                    .ok_or_else(|| {
                        ProjectionError::MissingIdentity("foreign spec origin proof".into())
                    })?;
                self.specs
                    .entry(identity.clone())
                    .or_insert_with(|| origin.clone());
            } else {
                self.specs
                    .entry(identity.clone())
                    .or_insert_with(|| ForeignSpecBinding {
                        source: source.clone(),
                        catalog: catalog.clone(),
                    });
            }
        }
        Ok(())
    }

    pub(crate) fn verifies_spec(&self, identity: &DefinitionKey) -> bool {
        self.specs.get(identity).is_some_and(|binding| {
            binding.catalog.native_spec_identity(&binding.source).ok() == Some(identity)
        })
    }

    pub(crate) fn verifies_marker_spec(&self, identity: &DefinitionKey) -> bool {
        let Some(binding) = self.specs.get(identity) else {
            return false;
        };
        if binding.catalog.native_spec_identity(&binding.source).ok() != Some(identity) {
            return false;
        }
        let checked = binding.catalog.checked_program();
        match &binding.source {
            CheckedSpecRef::Source(symbol) => checked
                .types
                .spec_signatures
                .get(symbol)
                .is_some_and(|signature| {
                    signature.kind == etas_types::SpecKind::TypeSpec
                        && signature.callable.is_none()
                        && signature.methods.is_empty()
                }),
            CheckedSpecRef::External(spec) => {
                let Some(checked_signature) = checked.types.external_specs.signatures.get(spec)
                else {
                    return false;
                };
                if checked_signature.kind != etas_types::SpecKind::TypeSpec
                    || !checked_signature.methods.is_empty()
                {
                    return false;
                }
                let mut signatures = checked
                    .definition_environment
                    .environment()
                    .external_public_metadata
                    .iter()
                    .filter(|package| package.package.0 == spec.package)
                    .flat_map(|package| package.spec_signatures.iter())
                    .filter(|signature| signature.path == spec.path);
                matches!((signatures.next(), signatures.next()), (Some(signature), None)
                    if signature.kind == crate::ProjectExternalSpecKindInput::Type
                        && signature.callable.is_none() && signature.methods.is_empty())
            }
            CheckedSpecRef::Std(_) | CheckedSpecRef::Canonical(_) => false,
        }
    }

    pub(crate) fn iter(
        &self,
    ) -> impl Iterator<Item = (TypeId, &DefinitionKey, TypeId, &DefinitionCatalog)> {
        self.bindings.iter().map(|(target, binding)| {
            (
                *target,
                &binding.identity,
                binding.source,
                binding.catalog.as_ref(),
            )
        })
    }
}
