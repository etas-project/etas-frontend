use std::{collections::BTreeMap, sync::Arc};

use etas_code::{DefinitionKind, descriptor::DescriptorGraphInput};
use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_types::{ExternalSignatureInput, facts::reflection::ExternalAgentReflectionFact};

use crate::ProjectContext;

pub(super) fn signature_input(
    context: &ProjectContext,
) -> Result<etas_types::api::ReflectionSignatureInput, crate::code::descriptor::ProjectionError> {
    signature_input_for_environment(
        &context.std_registry,
        context
            .validated_external_environment
            .as_ref()
            .map(|validated| validated.environment()),
    )
}

pub(in crate::passes::analysis) fn signature_input_for_environment(
    registry: &etas_std::StdRegistry,
    environment: Option<&crate::ProjectEnvironmentInput>,
) -> Result<etas_types::api::ReflectionSignatureInput, crate::code::descriptor::ProjectionError> {
    let mut declarations = BTreeMap::new();
    let mut effect_targets = BTreeMap::new();
    if let Some(environment) = environment {
        for origin in &environment.external_definition_origins {
            let aliases = origin
                .descriptors
                .iter()
                .flat_map(|set| &set.graph.types)
                .filter_map(|ty| match ty {
                    etas_code::descriptor::TypeInfo::Alias { definition, .. } => Some(definition),
                    _ => None,
                })
                .collect::<std::collections::BTreeSet<_>>();
            for binding in &origin.origins.declarations {
                if !aliases.contains(&binding.definition) {
                    insert_declaration(
                        &mut declarations,
                        binding.path(),
                        binding.definition.clone(),
                    )?;
                }
            }
        }
    }
    let package = crate::code::identity::standard_package_key(registry)?;
    for symbol in registry.symbols() {
        if let Some(key) =
            crate::code::identity::standard_definition_key(registry, package, symbol)?
        {
            insert_declaration(&mut declarations, symbol.qualified_path.clone(), key)?;
        }
    }
    use etas_code::descriptor::EffectTargetInfo;
    for (path, identity) in &declarations {
        if identity.kind() == DefinitionKind::Effect {
            effect_targets.insert(path.join("."), EffectTargetInfo::Effect(identity.clone()));
        }
    }
    // Registry logical names are the canonical names used by typed std rows.
    // Source rows use resolved declaration paths, including when they shadow std.
    let mut standard_effects = BTreeMap::new();
    for symbol in registry.symbols() {
        if let etas_std::StdDecl::Effect(effect) = &symbol.decl {
            let key = declarations.get(&symbol.qualified_path).ok_or_else(|| {
                crate::code::descriptor::ProjectionError::MissingIdentity(
                    symbol.qualified_path.join("."),
                )
            })?;
            standard_effects.insert(effect.name.clone(), key.clone());
            effect_targets.insert(effect.name.clone(), EffectTargetInfo::Effect(key.clone()));
        }
    }
    for symbol in registry.symbols() {
        if let etas_std::StdDecl::EffectAction(action) = &symbol.decl {
            let key = standard_effects.get(&action.owner).ok_or_else(|| {
                crate::code::descriptor::ProjectionError::MissingIdentity(action.owner.clone())
            })?;
            let target = EffectTargetInfo::Action {
                effect: key.clone(),
                name: action.name.clone(),
            };
            effect_targets.insert(symbol.qualified_path.join("."), target.clone());
            let path = key
                .module()
                .iter()
                .chain(key.declaration())
                .map(String::as_str)
                .chain(std::iter::once(action.name.as_str()))
                .collect::<Vec<_>>()
                .join(".");
            effect_targets.insert(path, target.clone());
            effect_targets.insert(format!("{}.{}", action.owner, action.name), target);
        }
    }
    if let Some(environment) = environment {
        for metadata in &environment.external_public_metadata {
            for action in &metadata.actions {
                let Some((name, owner)) = action.path.split_last() else {
                    continue;
                };
                if let Some(key) = declarations.get(owner)
                    && key.kind() == DefinitionKind::Effect
                {
                    effect_targets.insert(
                        action.path.join("."),
                        EffectTargetInfo::Action {
                            effect: key.clone(),
                            name: name.clone(),
                        },
                    );
                }
            }
        }
    }
    Ok(etas_types::api::ReflectionSignatureInput {
        declarations: declarations.into_iter().collect(),
        effect_targets: effect_targets.into_iter().collect(),
    })
}

fn insert_declaration(
    declarations: &mut BTreeMap<Vec<String>, etas_code::DefinitionKey>,
    path: Vec<String>,
    identity: etas_code::DefinitionKey,
) -> Result<(), crate::code::descriptor::ProjectionError> {
    if let Some(existing) = declarations.get(&path)
        && existing != &identity
    {
        return Err(crate::code::descriptor::ProjectionError::Invalid(format!(
            "conflicting canonical reflection declarations for {}",
            path.join(".")
        )));
    }
    declarations.insert(path, identity);
    Ok(())
}

pub(super) fn apply(
    context: &ProjectContext,
    input: &mut ExternalSignatureInput,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(validated) = &context.validated_external_environment else {
        return;
    };
    let origins = &validated.environment().external_definition_origins;
    let mut graphs = BTreeMap::<usize, Arc<DescriptorGraphInput>>::new();
    for binding in &input.symbol_bindings {
        let Some(origin) = origins
            .iter()
            .find(|origin| origin.package.0 == binding.package.0)
        else {
            continue;
        };
        let Some(declaration) = origin
            .origins
            .declarations
            .iter()
            .find(|declaration| declaration.path() == binding.path)
        else {
            continue;
        };
        input
            .definition_bindings
            .push((binding.symbol, declaration.definition.clone()));
        if declaration.definition.kind() != DefinitionKind::Agent {
            continue;
        }
        let mut matches = origins.iter().enumerate().filter_map(|(index, origin)| {
            let set = origin.descriptors.as_ref()?;
            let header = set
                .headers
                .as_ref()?
                .agents
                .iter()
                .find(|header| header.identity == declaration.definition)?;
            Some((index, set, header))
        });
        let Some((index, set, header)) = matches.next() else {
            continue;
        };
        if matches
            .any(|(_, other, other_header)| other_header != header || other.graph != set.graph)
        {
            diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::IncompleteTypeFacts,
                binding.span,
                "external agent has conflicting reflection descriptions",
            ));
            continue;
        }
        let graph = graphs
            .entry(index)
            .or_insert_with(|| Arc::new(set.graph.clone()))
            .clone();
        input.agent_reflections.push((
            binding.symbol,
            ExternalAgentReflectionFact::Available {
                header: Arc::new(header.clone()),
                graph,
            },
        ));
    }
}

pub(super) fn canonicalize_re_exports(
    context: &ProjectContext,
    input: &mut ExternalSignatureInput,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(validated) = &context.validated_external_environment else {
        return;
    };
    let environment = validated.environment();
    let identities = environment
        .external_definition_origins
        .iter()
        .flat_map(|origin| {
            origin
                .origins
                .declarations
                .iter()
                .map(move |binding| (binding.path(), (origin.package, &binding.definition)))
        })
        .collect::<BTreeMap<_, _>>();
    let exports = environment
        .external_public_metadata
        .iter()
        .flat_map(|metadata| {
            metadata
                .re_exports
                .iter()
                .map(move |export| ((metadata.package.0, export.exported.clone()), &export.from))
        })
        .collect::<BTreeMap<_, _>>();
    let definitions = input
        .definition_bindings
        .iter()
        .map(|(symbol, key)| (*symbol, key))
        .collect::<std::collections::HashMap<_, _>>();
    for binding in &mut input.symbol_bindings {
        // Legacy type-only metadata cannot supply reflection identities. Leave
        // its existing type projection intact, without inventing an origin.
        if !definitions.contains_key(&binding.symbol) {
            continue;
        }
        let mut visited = std::collections::BTreeSet::new();
        while let Some(from) = exports.get(&(binding.package.0, binding.path.clone())) {
            let Some((package, identity)) = identities.get(*from) else {
                diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::IncompleteTypeFacts,
                    binding.span,
                    "re-export has no canonical declaration origin",
                ));
                break;
            };
            if !visited.insert((binding.package.0, binding.path.clone()))
                || definitions.get(&binding.symbol).copied() != Some(*identity)
            {
                diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::IncompleteTypeFacts,
                    binding.span,
                    "re-export has no consistent canonical declaration binding",
                ));
                break;
            }
            binding.package = etas_types::ExternalPackageKey(package.0);
            binding.path = (*from).clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use etas_code::{
        CodeFingerprint, DefinitionKey, PackageKey,
        identity::{DefinitionBindingOrigin, DefinitionModuleOrigin, PackageDefinitionOrigins},
    };

    #[test]
    fn reflection_identities_are_available_without_hir_and_preserve_package_identity() {
        let registry = etas_std::standard_registry();
        let mut environment = crate::ProjectEnvironmentInput::default();
        let mut expected = Vec::new();
        for (number, root) in [(1, "first"), (2, "second")] {
            let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
                "etas.test.reflection.identity",
                root.as_bytes(),
            ));
            let key = DefinitionKey::new(
                package,
                vec!["library".into()],
                vec!["Item".into()],
                DefinitionKind::Type,
            )
            .unwrap();
            let module = vec![root.to_owned(), "library".into()];
            let mut path = module.clone();
            path.push("Item".into());
            environment.external_definition_origins.push(
                crate::ProjectExternalDefinitionOriginsInput {
                    package: crate::ExternalPackageId(number),
                    origins: PackageDefinitionOrigins {
                        package,
                        modules: vec![DefinitionModuleOrigin {
                            path: module.clone(),
                            canonical_path: vec!["library".into()],
                        }],
                        declarations: vec![DefinitionBindingOrigin {
                            module,
                            declaration: vec!["Item".into()],
                            definition: key.clone(),
                        }],
                    },
                    descriptors: None,
                },
            );
            expected.push((path, key));
        }
        let input = signature_input_for_environment(&registry, Some(&environment)).unwrap();
        for entry in &expected {
            assert!(input.declarations.contains(entry));
        }
        assert_ne!(expected[0].1, expected[1].1);
        assert!(input.effect_targets.iter().any(|(path, target)| {
            path == "Console.stdout_write"
                && matches!(target, etas_code::descriptor::EffectTargetInfo::Action { name, .. } if name == "stdout_write")
        }));
    }
}
