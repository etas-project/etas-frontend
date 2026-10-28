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
) -> Result<(), Error> {
    validate_resource_bounds(environment)?;
    let metadata = environment
        .external_public_metadata
        .iter()
        .map(|metadata| (metadata.package, metadata))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut bindings = BTreeMap::new();
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
    validate_chains(&bindings)
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
