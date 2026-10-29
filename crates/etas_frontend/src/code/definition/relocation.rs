use std::collections::{BTreeMap, HashMap};

use etas_types::TypeId;

use super::DefinitionCatalog;
use crate::code::{
    descriptor::{DescriptorInput, ProjectionError},
    identity::DefinitionIdentityIndex,
};

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
        Self::from_inputs(
            source.checked_program().as_ref().into(),
            &source.identities,
            target.checked_program().as_ref().into(),
            &target.identities,
        )
    }

    /// Build the same canonical correspondence from signature-stage facts.
    /// This descriptive mapping is not a checked-program or execution permit.
    pub(crate) fn from_inputs(
        source: DescriptorInput<'_>,
        source_identities: &DefinitionIdentityIndex,
        target: DescriptorInput<'_>,
        target_identities: &DefinitionIdentityIndex,
    ) -> Result<Self, ProjectionError> {
        let mut declarations = BTreeMap::new();
        for (ty, identity) in native_type_declarations(target.type_store, target_identities) {
            if declarations
                .insert(identity, ty)
                .is_some_and(|old| old != ty)
            {
                return Err(ProjectionError::Invalid(
                    "ambiguous target type declaration".into(),
                ));
            }
        }
        let declarations = native_type_declarations(source.type_store, source_identities)
            .filter_map(|(ty, identity)| declarations.get(identity).map(|target| (ty, *target)))
            .collect();
        let from = source;
        let to = target;
        let mut tags = BTreeMap::new();
        for tag in to.effect_registry.tags() {
            let identity = target_identities.effect_tag(tag.id)?;
            if tags.insert(identity, tag.id).is_some() {
                return Err(ProjectionError::Invalid(
                    "ambiguous target effect identity".into(),
                ));
            }
        }
        let mut effect_names = HashMap::new();
        for (name, tag) in from.effect_registry.named_tags() {
            let identity = source_identities.effect_tag(tag)?;
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
            let identity = target_identities.effect_tag(action.owner)?;
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
            let owner = source_identities.effect_tag(action.tag)?;
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

pub(super) fn native_type_declarations<'a>(
    store: &'a etas_types::TypeStore,
    identities: &'a DefinitionIdentityIndex,
) -> impl Iterator<Item = (TypeId, &'a etas_code::DefinitionKey)> + 'a {
    store.iter().filter_map(|(ty, _)| {
        let declaration = identities.types.get(&ty)?;
        declaration
            .alias
            .is_none()
            .then_some((ty, &declaration.identity))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use etas_code::{CodeFingerprint, PackageKey, descriptor::DescriptorLimits};
    use etas_types::{SymbolTypeFact, Type, TypeId};

    use super::{DescriptorInput, NativeTypeCorrespondence, native_type_declarations};
    use crate::{
        CheckedProgram, EntryPolicy, ProjectContext, ProjectInput, SourceInput,
        code::{definition::DefinitionCatalog, identity::DefinitionOrigins},
    };

    fn catalog(source: &str) -> (CheckedProgram, DefinitionCatalog, etas_types::TypeOutput) {
        let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
        input.options.entry_policy = EntryPolicy::Optional;
        let mut context = ProjectContext::new(input);
        let mut pipeline = crate::pipeline::project_pipeline();
        let run = etas_utils::PassManager::new().run_pipeline(&mut pipeline, &mut context);
        assert!(
            matches!(run.control, etas_utils::PassControl::Continue),
            "{:?}",
            run.control
        );
        assert!(context.diagnostics.is_empty(), "{:?}", context.diagnostics);
        let signatures = context.signature_types.take().unwrap();
        let checked = context.checked.take().unwrap();
        let package = |name: &[u8]| {
            PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
                "test.signature-stage-correspondence.v1",
                name,
            ))
        };
        let origins = DefinitionOrigins {
            source_modules: DefinitionOrigins::declared_source_modules(&checked, package(b"app"))
                .unwrap(),
            standard_package: package(b"std"),
            external_definitions: BTreeMap::new(),
        };
        let catalog =
            DefinitionCatalog::build(checked.clone(), &origins, DescriptorLimits::default())
                .unwrap();
        (checked, catalog, signatures)
    }

    fn nominal(checked: &CheckedProgram, name: &str) -> TypeId {
        let symbol = checked
            .symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .unwrap()
            .id;
        match &checked.types.symbol_types[&symbol] {
            SymbolTypeFact::NominalType { constructor, .. }
            | SymbolTypeFact::Type { constructor } => TypeId(constructor.0),
            _ => panic!("expected nominal type"),
        }
    }

    #[test]
    fn signature_stage_correspondence_preserves_nominal_and_effect_identities() {
        let (source, source_catalog, source_signatures) = catalog(
            "module app; type A; type B; flow run() -> unit ![Console.stdout_write] { perform Console.stdout_write(\"a\"); return; }",
        );
        let (target, target_catalog, target_signatures) = catalog(
            "module app; type B; type A; flow run() -> unit ![Console.stdout_write] { perform Console.stdout_write(\"b\"); return; }",
        );
        let mut source_input: DescriptorInput<'_> = source.as_ref().into();
        let mut target_input: DescriptorInput<'_> = target.as_ref().into();
        source_input.effects = None;
        target_input.effects = None;
        source_input.types = &source_signatures.facts;
        source_input.type_store = &source_signatures.store;
        target_input.types = &target_signatures.facts;
        target_input.type_store = &target_signatures.store;
        let source_identities =
            crate::code::identity::DefinitionIdentityIndex::from_input_with_limits(
                source_input,
                &source_catalog.origins,
                DescriptorLimits::default(),
            )
            .unwrap();
        let target_identities =
            crate::code::identity::DefinitionIdentityIndex::from_input_with_limits(
                target_input,
                &target_catalog.origins,
                DescriptorLimits::default(),
            )
            .unwrap();
        let correspondence = NativeTypeCorrespondence::from_inputs(
            source_input,
            &source_identities,
            target_input,
            &target_identities,
        )
        .unwrap();
        for name in ["A", "B"] {
            assert_eq!(
                correspondence.declarations().get(&nominal(&source, name)),
                Some(&nominal(&target, name))
            );
        }
        assert_eq!(
            correspondence
                .effect_names()
                .get("Console.stdout_write")
                .map(String::as_str),
            Some("Console.stdout_write")
        );
        let complete = NativeTypeCorrespondence::build(&source_catalog, &target_catalog).unwrap();
        for name in ["A", "B"] {
            assert_eq!(
                correspondence.declarations().get(&nominal(&source, name)),
                complete.declarations().get(&nominal(&source, name))
            );
        }
        assert_eq!(correspondence.effect_names(), complete.effect_names());
    }

    #[test]
    fn an_unresolved_same_named_type_is_not_a_declaration_identity() {
        let (checked, catalog, _) = catalog("module app; type A; flow run() -> unit { return; }");
        let id = nominal(&checked, "A");
        let Some(Type::Nominal(known)) = checked.type_store.get(id) else {
            panic!("nominal type");
        };
        let mut types = etas_types::TypeInterner::from_store((*checked.type_store).clone());
        let unresolved = types.intern(Type::Named(etas_types::NamedTypeRef {
            name: known.name.clone(),
            source_parameter: None,
        }));
        let types = types.into_store();
        let mapped = native_type_declarations(&types, &catalog.identities).collect::<Vec<_>>();
        assert!(mapped.iter().any(|(ty, _)| *ty == id));
        assert!(!mapped.iter().any(|(ty, _)| *ty == unresolved));
        let mut target = etas_types::TypeInterner::from_store(types.clone());
        let declarations = mapped.into_iter().map(|(ty, _)| (ty, ty)).collect();
        let effects = std::collections::HashMap::new();
        let mut relocation = etas_types::TypeSubstitutionEngine::relocating_materialized(
            etas_types::ty::TypeRelocationMaterializationInput {
                source: &types,
                target: &mut target,
                declaration_types: &declarations,
                effect_names: &effects,
            },
        );
        assert_eq!(
            relocation.substitute(unresolved),
            Err(etas_types::TypeSubstitutionError::MissingTypeIdentity(
                unresolved
            ))
        );
    }
}
