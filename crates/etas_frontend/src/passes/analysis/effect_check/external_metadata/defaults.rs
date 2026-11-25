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
                let Some(graph) = &default.callable_value else {
                    continue;
                };
                graph
                    .validate_references(Some((arity, &kinds)))
                    .map_err(|error| error.to_string())?;
                let fingerprint = etas_package_metadata::callable_value_graph_fingerprint(graph)
                    .map_err(|error| error.to_string())?;
                let reference = default.reference.fingerprint();
                let graph_key = etas_code::CodeFingerprint::of_parts(
                    "etas.effects.parameter-default.v1",
                    [
                        reference.as_bytes().as_slice(),
                        fingerprint.as_bytes().as_slice(),
                    ],
                );
                let value = etas_effects::ExternalParameterDefaultMetadata {
                    reference: default.reference.clone(),
                    graph_key,
                    graph: graph.clone(),
                    owner_params: owner_params.to_vec(),
                    generics: generics.clone(),
                    arity,
                    values: crate::project::lower_callable_graph(graph, types)?,
                    objects: crate::project::lower_callable_objects(graph, types)?,
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
            for signature in spec.callable.iter().chain(
                spec.methods
                    .iter()
                    .filter_map(|method| method.signature.as_ref()),
            ) {
                add(
                    &signature.parameter_defaults,
                    split_external_generic_params(&signature.generic_params).ordered,
                    signature.params.len(),
                    &[],
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
            add(
                &method.parameter_defaults,
                generics,
                root.params.len(),
                owner_params,
            )?;
        }
    }
    Ok(result.into_values().collect())
}
