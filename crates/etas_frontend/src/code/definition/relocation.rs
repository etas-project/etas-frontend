use std::collections::{BTreeMap, HashMap};

use etas_types::TypeId;

use super::DefinitionCatalog;
use crate::code::descriptor::ProjectionError;

/// Correspondence between checked native universes, keyed by declaration identity.
pub struct NativeTypeCorrespondence {
    declarations: HashMap<TypeId, TypeId>,
    effect_names: HashMap<String, String>,
}

impl NativeTypeCorrespondence {
    pub fn build(
        source: &DefinitionCatalog,
        target: &DefinitionCatalog,
    ) -> Result<Self, ProjectionError> {
        let mut declarations = BTreeMap::new();
        for (ty, identity) in target.native_type_declarations() {
            if declarations
                .insert(identity, ty)
                .is_some_and(|old| old != ty)
            {
                return Err(ProjectionError::Invalid(
                    "ambiguous target type declaration".into(),
                ));
            }
        }
        let declarations = source
            .native_type_declarations()
            .filter_map(|(ty, identity)| declarations.get(identity).map(|target| (ty, *target)))
            .collect();
        let from = source.checked_program();
        let to = target.checked_program();
        let mut tags = BTreeMap::new();
        for tag in to.effect_registry.tags() {
            let identity = target.native_effect_identity(tag.id)?;
            if tags.insert(identity, tag.id).is_some() {
                return Err(ProjectionError::Invalid(
                    "ambiguous target effect identity".into(),
                ));
            }
        }
        let mut effect_names = HashMap::new();
        for (name, tag) in from.effect_registry.named_tags() {
            let identity = source.native_effect_identity(tag)?;
            if let Some(target_tag) = tags.get(identity) {
                let canonical = to.effect_registry.tag_name(*target_tag).ok_or_else(|| {
                    ProjectionError::Invalid("target effect has no canonical name".into())
                })?;
                let target_name = if to.effect_registry.tag_by_name(name) == Some(*target_tag) {
                    name
                } else {
                    canonical
                };
                effect_names.insert(name.into(), target_name.into());
            }
        }
        let mut actions = BTreeMap::new();
        for action in to.effect_registry.action_signatures() {
            let identity = target.native_effect_identity(action.owner)?;
            let reference = etas_effects::ActionRef {
                tag: action.owner,
                action: action.id,
            };
            if actions
                .insert((identity, action.name.as_str()), reference.clone())
                .is_some_and(|old| old != reference)
            {
                return Err(ProjectionError::Invalid(
                    "ambiguous target action identity".into(),
                ));
            }
        }
        for (name, action) in from.effect_registry.named_actions() {
            let signature = from
                .effect_registry
                .action_signature(action)
                .ok_or_else(|| {
                    ProjectionError::Invalid("source action alias has no descriptor".into())
                })?;
            let owner = source.native_effect_identity(action.tag)?;
            if let Some(target_action) = actions.get(&(owner, signature.name.as_str())) {
                let owner = to
                    .effect_registry
                    .tag_name(target_action.tag)
                    .ok_or_else(|| ProjectionError::Invalid("target action has no owner".into()))?;
                let canonical = format!("{owner}.{}", signature.name);
                let target_name =
                    if to.effect_registry.action_by_name(name).as_ref() == Some(target_action) {
                        name
                    } else {
                        canonical.as_str()
                    };
                effect_names.insert(name.into(), target_name.into());
            }
        }
        Ok(Self {
            declarations,
            effect_names,
        })
    }

    pub fn declarations(&self) -> &HashMap<TypeId, TypeId> {
        &self.declarations
    }

    pub fn effect_names(&self) -> &HashMap<String, String> {
        &self.effect_names
    }
}
