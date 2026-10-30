use std::collections::{BTreeMap, BTreeSet};

use etas_code::{DefinitionKey, DefinitionKind};

use crate::{ExternalPackageId, ProjectEnvironmentInput, ProjectExternalPublicMetadataInput};

use super::{ExternalEnvironmentValidationError as Error, MAX_EXTERNAL_METADATA_GRAPH_NODES};

struct Binding<'a> {
    package: ExternalPackageId,
    definition: &'a DefinitionKey,
    source: Option<&'a [String]>,
    direct: bool,
}

pub(super) fn validate(
    environment: &ProjectEnvironmentInput,
    packages: &BTreeMap<ExternalPackageId, &crate::ProjectExternalPackageInput>,
    modules: &BTreeMap<Vec<String>, ExternalPackageId>,
    exports: &BTreeMap<ExternalPackageId, BTreeSet<Vec<String>>>,
    std_registry: &etas_std::StdRegistry,
) -> Result<(), Error> {
    validate_resource_bounds(environment)?;
    let metadata = environment
        .external_public_metadata
        .iter()
        .map(|metadata| (metadata.package, metadata))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut bindings = BTreeMap::new();
    let mut descriptor_comparison_budget =
        etas_code::descriptor::DescriptorLimits::default().max_edges;
    let mut identity_index = None;
    for input in &environment.external_definition_origins {
        let fail = |reason| Error::package(input.package, reason);
        if !packages.contains_key(&input.package) || !seen.insert(input.package) {
            return Err(fail(
                "definition origins have an unknown or duplicate package binding".to_owned(),
            ));
        }
        input
            .origins
            .validate()
            .map_err(|error| fail(error.to_string()))?;
        if let Some(descriptors) = &input.descriptors {
            let declarations = input
                .origins
                .declarations
                .iter()
                .map(|binding| &binding.definition)
                .collect::<BTreeSet<_>>();
            for binding in &descriptors.declarations {
                if !declarations.contains(&binding.definition) {
                    return Err(fail(
                        "descriptor root has no matching public definition origin".to_owned(),
                    ));
                }
            }
            if let Some(bodies) = descriptors
                .headers
                .as_ref()
                .and_then(|headers| headers.bodies.as_ref())
            {
                bodies
                    .validate_origins(&input.origins)
                    .map_err(|error| fail(error.to_string()))?;
                for body in &bodies.entries {
                    crate::code::service::validate_retained_source(&body.source).map_err(fail)?;
                }
            }
        }
        let metadata = metadata
            .get(&input.package)
            .ok_or_else(|| fail("definition origins have no public metadata record".to_owned()))?;
        validate_agent_entries(input, metadata)?;
        let kinds = declaration_kinds(metadata);
        let mut re_exports = BTreeMap::new();
        for export in &metadata.re_exports {
            if re_exports
                .insert(export.exported.as_slice(), export.from.as_slice())
                .is_some()
            {
                return Err(fail(
                    "definition has duplicate re-export targets".to_owned(),
                ));
            }
        }
        let mut canonical_modules = BTreeMap::new();
        for module in &input.origins.modules {
            if modules.get(&module.path) != Some(&input.package) {
                return Err(fail(format!(
                    "definition module `{}` does not match its package module identity",
                    module.path.join(".")
                )));
            }
            canonical_modules.insert(module.path.as_slice(), module.canonical_path.as_slice());
        }
        for binding in &input.origins.declarations {
            let path = binding.path();
            if !exports
                .get(&input.package)
                .is_some_and(|exports| exports.contains(&path))
            {
                return Err(fail(format!(
                    "definition `{}` has no matching module export",
                    path.join(".")
                )));
            }
            let source = re_exports.get(path.as_slice()).copied();
            let expected = kinds.get(path.as_slice());
            if expected.is_some_and(|kind| *kind != binding.definition.kind()) {
                return Err(fail(format!(
                    "definition `{}` kind disagrees with its checked public signature",
                    path.join(".")
                )));
            }
            let direct = source.is_none()
                && expected.is_some()
                && binding.definition.package() == input.origins.package
                && canonical_modules.get(binding.module.as_slice()).copied()
                    == Some(binding.definition.module())
                && binding.definition.declaration() == binding.declaration;
            if bindings
                .insert(
                    path.clone(),
                    Binding {
                        package: input.package,
                        definition: &binding.definition,
                        source,
                        direct,
                    },
                )
                .is_some()
            {
                return Err(fail(format!(
                    "duplicate definition lookup identity `{}`",
                    path.join(".")
                )));
            }
        }
    }
    validate_chains(&bindings)?;
    for input in &environment.external_definition_origins {
        let metadata = metadata.get(&input.package).ok_or_else(|| {
            Error::package(
                input.package,
                "definition origins have no public metadata record",
            )
        })?;
        validate_alias_descriptors(
            input,
            metadata,
            std_registry,
            environment,
            &mut descriptor_comparison_budget,
            &mut identity_index,
        )?;
        validate_callable_definition_parameters(
            input,
            metadata,
            std_registry,
            environment,
            &mut descriptor_comparison_budget,
            &mut identity_index,
        )?;
    }
    Ok(())
}

fn validate_alias_descriptors<'a>(
    input: &crate::ProjectExternalDefinitionOriginsInput,
    metadata: &ProjectExternalPublicMetadataInput,
    std_registry: &'a etas_std::StdRegistry,
    environment: &ProjectEnvironmentInput,
    comparison_budget: &mut usize,
    identity_index: &mut Option<etas_types::ExternalDescriptorIdentities<'a>>,
) -> Result<(), Error> {
    use etas_code::descriptor::TypeInfo;
    let Some(set) = input.descriptors.as_ref() else {
        return Ok(());
    };
    let roots = set
        .declarations
        .iter()
        .map(|root| (&root.definition, root.ty))
        .collect::<BTreeMap<_, _>>();
    let public_types = metadata
        .types
        .iter()
        .map(|item| (item.path.as_slice(), item))
        .collect::<BTreeMap<_, _>>();
    let re_exports = metadata
        .re_exports
        .iter()
        .map(|export| export.exported.as_slice())
        .collect::<BTreeSet<_>>();
    for binding in &input.origins.declarations {
        let path = binding.path();
        let Some(item) = public_types.get(path.as_slice()) else {
            continue;
        };
        let public_alias = match &item.ty {
            Some(crate::ProjectExternalTypeInput::Alias { target, .. }) => Some(target.as_ref()),
            _ => None,
        };
        let Some(root) = roots.get(&binding.definition) else {
            if public_alias.is_some()
                && binding.definition.package() == input.origins.package
                && !re_exports.contains(path.as_slice())
            {
                return Err(Error::package(
                    input.package,
                    "public alias has no checked descriptor root",
                ));
            }
            continue;
        };
        let (target, definition, parameters, expected) =
            match (public_alias, set.graph.types.get(root.0 as usize)) {
                (
                    Some(target),
                    Some(TypeInfo::Alias {
                        definition,
                        parameters,
                        target: expected,
                    }),
                ) => (target, definition, parameters, expected),
                (Some(_), _) | (None, Some(TypeInfo::Alias { .. })) => {
                    return Err(Error::package(
                        input.package,
                        "alias descriptor and public signature disagree on declaration kind",
                    ));
                }
                _ => continue,
            };
        if definition != &binding.definition {
            return Err(Error::package(
                input.package,
                "alias descriptor identity differs from its public origin",
            ));
        }
        if identity_index.is_none() {
            let identities =
                super::super::type_check_bridge::reflection::signature_input_for_environment(
                    std_registry,
                    Some(environment),
                )
                .map_err(|error| Error::package(input.package, error.to_string()))?;
            *identity_index = Some(
                etas_types::ExternalDescriptorIdentities::new(std_registry, &identities)
                    .map_err(|error| Error::package(input.package, error))?,
            );
        }
        let identities = identity_index.as_ref().ok_or_else(|| {
            Error::package(input.package, "missing external descriptor identities")
        })?;
        let generics =
            super::super::type_check_bridge::convert_external_generic_params(&item.generic_params);
        let mut matcher = etas_types::ExternalDescriptorMatcher::new(
            &set.graph,
            identities,
            &generics,
            parameters,
            comparison_budget,
        )
        .map_err(|error| Error::package(input.package, error))?;
        if !matcher
            .matches(
                &super::super::type_check_bridge::convert_external_type(target),
                *expected,
            )
            .map_err(|error| Error::package(input.package, error))?
        {
            return Err(Error::package(
                input.package,
                "alias descriptor and public signature disagree on target type",
            ));
        }
    }
    Ok(())
}

fn validate_callable_definition_parameters<'a>(
    input: &crate::ProjectExternalDefinitionOriginsInput,
    metadata: &ProjectExternalPublicMetadataInput,
    std_registry: &'a etas_std::StdRegistry,
    environment: &ProjectEnvironmentInput,
    comparison_budget: &mut usize,
    identity_index: &mut Option<etas_types::ExternalDescriptorIdentities<'a>>,
) -> Result<(), Error> {
    use etas_code::descriptor::TypeInfo;
    let Some(set) = input.descriptors.as_ref() else {
        return Ok(());
    };
    let Some(headers) = set.headers.as_ref() else {
        return Ok(());
    };
    let by_callable = headers
        .callables
        .iter()
        .map(|header| (&header.id, header))
        .collect::<BTreeMap<_, _>>();
    let mut callable_headers = headers
        .callables
        .iter()
        .filter_map(|header| {
            header
                .id
                .as_declaration()
                .map(|identity| (identity, header))
        })
        .collect::<BTreeMap<_, _>>();
    for agent in &headers.agents {
        if let Some(default) = &agent.default_entry {
            let header = by_callable.get(default).ok_or_else(|| {
                Error::package(
                    input.package,
                    "agent default entry has no checked callable descriptor",
                )
            })?;
            callable_headers.insert(&agent.identity, *header);
        }
    }
    let mut public_signatures = BTreeMap::new();
    for signature in &metadata.flows {
        if public_signatures
            .insert(
                signature.path.as_slice(),
                (
                    signature.params.as_slice(),
                    signature.param_names.as_slice(),
                    signature.generic_params.as_slice(),
                    &signature.output,
                    signature.effects.as_ref(),
                ),
            )
            .is_some()
        {
            return Err(Error::package(
                input.package,
                "duplicate public callable signature",
            ));
        }
    }
    for signature in &metadata.tools {
        if public_signatures
            .insert(
                signature.path.as_slice(),
                (
                    signature.input.as_slice(),
                    signature.param_names.as_slice(),
                    signature.generic_params.as_slice(),
                    &signature.output,
                    signature.effects.as_ref(),
                ),
            )
            .is_some()
        {
            return Err(Error::package(
                input.package,
                "duplicate public callable signature",
            ));
        }
    }
    for signature in &metadata.agents {
        let Some(output) = &signature.output else {
            continue;
        };
        if public_signatures
            .insert(
                signature.path.as_slice(),
                (
                    signature.input.as_slice(),
                    signature.param_names.as_slice(),
                    signature.generic_params.as_slice(),
                    output,
                    signature.effects.as_ref(),
                ),
            )
            .is_some()
        {
            return Err(Error::package(
                input.package,
                "duplicate public callable signature",
            ));
        }
    }
    for binding in &input.origins.declarations {
        let Some(header) = callable_headers.get(&binding.definition) else {
            continue;
        };
        let path = binding.path();
        let Some(&(parameters, parameter_names, generic_parameters, output, effects)) =
            public_signatures.get(path.as_slice())
        else {
            continue;
        };
        let Some(TypeInfo::Function(signature)) = set.graph.types.get(header.signature.0 as usize)
        else {
            return Err(Error::package(
                input.package,
                "callable descriptor has no checked signature",
            ));
        };
        if signature.parameters.len() != parameters.len() {
            return Err(Error::package(
                input.package,
                "callable descriptor and public signature disagree on parameter count",
            ));
        }
        if parameter_names.len() != signature.parameters.len()
            || signature
                .parameters
                .iter()
                .zip(parameter_names)
                .any(|(parameter, name)| {
                    parameter.name.as_deref() != (name != "_").then_some(name.as_str())
                })
        {
            return Err(Error::package(
                input.package,
                "callable descriptor and public signature disagree on parameter names",
            ));
        }
        {
            let identities = match &mut *identity_index {
                Some(identities) => identities,
                slot @ None => {
                    let project_identities = super::super::type_check_bridge::reflection::signature_input_for_environment(
                        std_registry,
                        Some(environment),
                    )
                    .map_err(|error| Error::package(input.package, error.to_string()))?;
                    slot.insert(
                        etas_types::ExternalDescriptorIdentities::new(
                            std_registry,
                            &project_identities,
                        )
                        .map_err(|error| Error::package(input.package, error))?,
                    )
                }
            };
            let generics = super::super::type_check_bridge::convert_external_generic_params(
                generic_parameters,
            );
            let mut matcher = etas_types::ExternalDescriptorMatcher::new(
                &set.graph,
                identities,
                &generics,
                &signature.generics,
                comparison_budget,
            )
            .map_err(|error| Error::package(input.package, error))?;
            for (portable, public) in signature.parameters.iter().zip(parameters) {
                if matcher
                    .matches(
                        &super::super::type_check_bridge::convert_external_type(public),
                        portable.ty,
                    )
                    .map_err(|error| Error::package(input.package, error))?
                {
                    continue;
                }
                return Err(Error::package(
                    input.package,
                    "callable descriptor and public signature disagree on parameter type",
                ));
            }
            if !matcher
                .matches(
                    &super::super::type_check_bridge::convert_external_type(output),
                    signature.output,
                )
                .map_err(|error| Error::package(input.package, error))?
            {
                return Err(Error::package(
                    input.package,
                    "callable descriptor and public signature disagree on return type",
                ));
            }
            let effects = effects.map(super::super::type_check_bridge::convert_external_effect_row);
            if !matcher
                .matches_optional_effect_row(effects.as_ref(), signature.effects)
                .map_err(|error| Error::package(input.package, error))?
            {
                return Err(Error::package(
                    input.package,
                    "callable descriptor and public signature disagree on effect row",
                ));
            }
        }
    }
    Ok(())
}

fn validate_agent_entries(
    input: &crate::ProjectExternalDefinitionOriginsInput,
    metadata: &ProjectExternalPublicMetadataInput,
) -> Result<(), Error> {
    let Some(headers) = input
        .descriptors
        .as_ref()
        .and_then(|set| set.headers.as_ref())
    else {
        return Ok(());
    };
    let agents = headers
        .agents
        .iter()
        .map(|header| (&header.identity, header))
        .collect::<BTreeMap<_, _>>();
    let signatures = metadata
        .agents
        .iter()
        .map(|signature| (signature.path.as_slice(), signature))
        .collect::<BTreeMap<_, _>>();
    for binding in &input.origins.declarations {
        let Some(header) = agents.get(&binding.definition) else {
            continue;
        };
        let path = binding.path();
        let signature = signatures.get(path.as_slice()).ok_or_else(|| {
            Error::package(
                input.package,
                "agent descriptor has no matching public agent signature",
            )
        })?;
        if header.default_entry.is_some() != signature.output.is_some() {
            return Err(Error::package(
                input.package,
                "agent descriptor and public signature disagree on the default entry",
            ));
        }
    }
    Ok(())
}

fn validate_resource_bounds(environment: &ProjectEnvironmentInput) -> Result<(), Error> {
    let mut nodes = 0usize;
    for input in &environment.external_definition_origins {
        let mut charge = |count: usize| {
            nodes = nodes.saturating_add(count);
            if nodes > MAX_EXTERNAL_METADATA_GRAPH_NODES {
                Err(Error::package(
                    input.package,
                    "definition origin graph exceeds the node limit",
                ))
            } else {
                Ok(())
            }
        };
        charge(1)?;
        if let Some(descriptors) = &input.descriptors {
            let units = etas_package_metadata::validate_definition_descriptors(descriptors)
                .map_err(|error| Error::package(input.package, error.to_string()))?;
            charge(units)?;
        }
        for module in &input.origins.modules {
            charge(
                1usize
                    .saturating_add(module.path.len())
                    .saturating_add(module.canonical_path.len()),
            )?;
        }
        for binding in &input.origins.declarations {
            charge(
                2usize
                    .saturating_add(binding.module.len())
                    .saturating_add(binding.declaration.len())
                    .saturating_add(binding.definition.module().len())
                    .saturating_add(binding.definition.declaration().len()),
            )?;
        }
    }
    Ok(())
}

fn declaration_kinds(
    metadata: &ProjectExternalPublicMetadataInput,
) -> BTreeMap<&[String], DefinitionKind> {
    use DefinitionKind as K;
    metadata
        .types
        .iter()
        .map(|s| (s.path.as_slice(), K::Type))
        .chain(metadata.enums.iter().map(|s| (s.path.as_slice(), K::Type)))
        .chain(metadata.flows.iter().map(|s| (s.path.as_slice(), K::Flow)))
        .chain(
            metadata
                .agents
                .iter()
                .map(|s| (s.path.as_slice(), K::Agent)),
        )
        .chain(metadata.tools.iter().map(|s| (s.path.as_slice(), K::Tool)))
        .chain(
            metadata
                .values
                .iter()
                .map(|s| (s.path.as_slice(), K::Value)),
        )
        .chain(
            metadata
                .effects
                .iter()
                .map(|s| (s.path.as_slice(), K::Effect)),
        )
        .chain(
            metadata
                .spec_signatures
                .iter()
                .map(|s| (s.path.as_slice(), K::Spec)),
        )
        .chain(
            metadata
                .trace_specs
                .iter()
                .map(|s| (s.path.as_slice(), K::Spec)),
        )
        .collect()
}

fn validate_chains(bindings: &BTreeMap<Vec<String>, Binding<'_>>) -> Result<(), Error> {
    let mut verified = BTreeSet::new();
    for (root, binding) in bindings {
        let mut path = root.as_slice();
        let mut active = BTreeSet::new();
        // A re-export must terminate at a checked declaration, not another alias cycle.
        loop {
            if !active.insert(path) {
                return Err(Error::package(
                    binding.package,
                    format!("cyclic definition re-export `{}`", path.join(".")),
                ));
            }
            let current = bindings.get(path).ok_or_else(|| {
                Error::package(
                    binding.package,
                    format!(
                        "definition re-export `{}` has no canonical target binding",
                        path.join(".")
                    ),
                )
            })?;
            if current.definition != binding.definition {
                return Err(Error::package(
                    binding.package,
                    format!(
                        "definition re-export `{}` changes canonical identity",
                        path.join(".")
                    ),
                ));
            }
            if verified.contains(path) {
                break;
            }
            if current.direct {
                break;
            }
            path = current.source.ok_or_else(|| {
                Error::package(
                    binding.package,
                    format!(
                        "definition `{}` has no checked canonical declaration or re-export",
                        path.join(".")
                    ),
                )
            })?;
        }
        verified.extend(active);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
