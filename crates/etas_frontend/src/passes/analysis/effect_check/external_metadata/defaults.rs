use super::*;
use etas_package_metadata::{GenericParamKind, ParameterDefault};

pub(in crate::passes::analysis::effect_check) fn external_parameter_defaults(
    environment: &ProjectEnvironmentInput,
    types: &TypeOutput,
    anchors: &ExternalImportAnchorIndex,
) -> Result<
    Vec<etas_effects::AnchoredExternalMetadata<etas_effects::ExternalParameterDefaultMetadata>>,
    String,
> {
    let mut result = BTreeMap::new();
    for metadata in &environment.external_public_metadata {
        let Some(span) = anchors.package_span(metadata.package) else {
            continue;
        };
        let mut add = |defaults: &[ParameterDefault],
                       mut generics: Vec<(String, etas_types::CallableGenericParamKind)>,
                       arity: usize,
                       owner_params: &[etas_types::CallableGenericParam]|
         -> Result<(), String> {
            generics.extend(
                owner_params
                    .iter()
                    .map(|param| (param.name.clone(), param.kind)),
            );
            let kinds = generics
                .iter()
                .map(|(_, kind)| match kind {
                    etas_types::CallableGenericParamKind::Type => GenericParamKind::Type,
                    etas_types::CallableGenericParamKind::Effect => GenericParamKind::Effect,
                })
                .collect::<Vec<_>>();
            for default in defaults {
                if default.callable_value.is_none() && default.binding.is_none() {
                    continue;
                }
                let graph = default.callable_value.as_ref();
                if let Some(graph) = graph {
                    graph
                        .validate_references(Some((arity, &kinds)))
                        .map_err(|error| error.to_string())?;
                }
                let fingerprint = graph
                    .map(etas_package_metadata::callable_value_graph_fingerprint)
                    .transpose()
                    .map_err(|error| error.to_string())?
                    .unwrap_or(default.reference.fingerprint());
                let reference = default.reference.fingerprint();
                let graph_key = etas_code::CodeFingerprint::of_parts(
                    "etas.effects.parameter-default.v1",
                    [
                        reference.as_bytes().as_slice(),
                        fingerprint.as_bytes().as_slice(),
                    ],
                );
                let value = etas_effects::ExternalParameterDefaultMetadata {
                    binding: default
                        .binding
                        .as_ref()
                        .map(|binding| {
                            crate::project::lower_parameter_binding(binding, types, &generics)
                        })
                        .transpose()?,
                    reference: default.reference.clone(),
                    graph_key,
                    graph: graph.cloned(),
                    owner_params: owner_params.to_vec(),
                    generics: generics.clone(),
                    arity,
                    values: graph
                        .map(|graph| crate::project::lower_callable_graph(graph, types))
                        .transpose()?
                        .unwrap_or_default(),
                    objects: graph
                        .map(|graph| crate::project::lower_callable_objects(graph, types))
                        .transpose()?
                        .unwrap_or_default(),
                };
                if let Some(previous) = result.get(&default.reference) {
                    let previous: &etas_effects::AnchoredExternalMetadata<
                        etas_effects::ExternalParameterDefaultMetadata,
                    > = previous;
                    if previous.metadata != value {
                        return Err(
                            "default expression has conflicting checked value graphs".into()
                        );
                    }
                } else {
                    result.insert(
                        default.reference.clone(),
                        etas_effects::AnchoredExternalMetadata {
                            package: metadata.package.0,
                            span,
                            metadata: value,
                        },
                    );
                }
            }
            Ok(())
        };
        for signature in &metadata.flows {
            add(
                &signature.parameter_defaults,
                split_external_generic_params(&signature.generic_params).ordered,
                signature.params.len(),
                &[],
            )?;
        }
        for signature in &metadata.agents {
            add(
                &signature.parameter_defaults,
                split_external_generic_params(&signature.generic_params).ordered,
                signature.input.len(),
                &[],
            )?;
        }
        for signature in &metadata.tools {
            add(
                &signature.parameter_defaults,
                split_external_generic_params(&signature.generic_params).ordered,
                signature.input.len(),
                &[],
            )?;
        }
        for signature in &metadata.actions {
            add(
                &signature.parameter_defaults,
                split_external_generic_params(&signature.generic_params).ordered,
                signature.params.len(),
                &[],
            )?;
        }
        for spec in &metadata.spec_signatures {
            if !spec
                .callable
                .iter()
                .chain(
                    spec.methods
                        .iter()
                        .filter_map(|method| method.signature.as_ref()),
                )
                .any(|signature| !signature.parameter_defaults.is_empty())
            {
                continue;
            }
            let owner_params = types
                .facts
                .external_specs
                .signatures
                .iter()
                .find(|(identity, _)| {
                    identity.package == metadata.package.0 && identity.path == spec.path
                })
                .map(|(_, signature)| signature.generic_params.as_slice())
                .ok_or("spec default has no checked owner parameter interface")?;
            for signature in spec.callable.iter().chain(
                spec.methods
                    .iter()
                    .filter_map(|method| method.signature.as_ref()),
            ) {
                add(
                    &signature.parameter_defaults,
                    split_external_generic_params(&signature.generic_params).ordered,
                    signature.params.len(),
                    owner_params,
                )?;
            }
        }
        for method in &metadata.methods {
            if method.parameter_defaults.is_empty() {
                continue;
            }
            let root = match method.callable.sources.as_slice() {
                [etas_package_metadata::CallableValueSource::Value(root)] => {
                    &method.callable.values[*root as usize]
                }
                _ => return Err("method default has no checked declaration root".into()),
            };
            let generics = root
                .generic_params
                .iter()
                .map(|parameter| {
                    (
                        parameter.name.clone(),
                        match parameter.kind {
                            GenericParamKind::Type => etas_types::CallableGenericParamKind::Type,
                            GenericParamKind::Effect => {
                                etas_types::CallableGenericParamKind::Effect
                            }
                        },
                    )
                })
                .collect();
            let owner_params = types
                .facts
                .external_methods
                .iter()
                .find(|fact| fact.identity == method.identity)
                .ok_or("default method has no checked owner parameter interface")?
                .owner_params
                .as_slice();
            // Inherited bindings retain their defining spec's generic scope.
            // That canonical contract is loaded from the spec, once.
            let defaults = method
                .parameter_defaults
                .iter()
                .filter(|default| {
                    matches!(
                        default.reference.owner,
                        etas_code::descriptor::ParameterDefaultOwner::Method(_)
                    )
                })
                .cloned()
                .collect::<Vec<_>>();
            add(&defaults, generics, root.params.len(), owner_params)?;
        }
    }
    Ok(result.into_values().collect())
}
