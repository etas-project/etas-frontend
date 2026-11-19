use std::collections::BTreeMap;

use etas_code::{MethodId, descriptor::TypeInfo};
use etas_types::{ParameterDefaultRef, ParameterInterface};

use crate::{
    ProjectEnvironmentInput, ProjectExternalPublicMetadataInput, code::descriptor::ProjectionError,
};

type Interfaces = (
    BTreeMap<Vec<String>, Vec<ParameterInterface>>,
    BTreeMap<MethodId, Vec<ParameterInterface>>,
);

/// Descriptors supply canonical default revisions; import origins supply lookup
/// paths, including aliases. No defining-program SymbolId enters the caller.
pub(super) fn collect(
    environment: &ProjectEnvironmentInput,
    metadata: &ProjectExternalPublicMetadataInput,
) -> Result<Interfaces, ProjectionError> {
    let mut declarations = BTreeMap::new();
    let mut methods = BTreeMap::new();
    for signature in &metadata.flows {
        declarations.insert(
            signature.path.clone(),
            from_declaration(
                &signature.param_names,
                signature.params.len(),
                &signature.parameter_defaults,
            ),
        );
    }
    for signature in &metadata.agents {
        declarations.insert(
            signature.path.clone(),
            from_declaration(
                &signature.param_names,
                signature.input.len(),
                &signature.parameter_defaults,
            ),
        );
    }
    for signature in &metadata.tools {
        declarations.insert(
            signature.path.clone(),
            from_declaration(
                &signature.param_names,
                signature.input.len(),
                &signature.parameter_defaults,
            ),
        );
    }
    for spec in &metadata.spec_signatures {
        for signature in spec.callable.iter().chain(
            spec.methods
                .iter()
                .filter_map(|method| method.signature.as_ref()),
        ) {
            declarations.insert(
                signature.path.clone(),
                from_declaration(
                    &signature.param_names,
                    signature.params.len(),
                    &signature.parameter_defaults,
                ),
            );
        }
    }
    for signature in &metadata.actions {
        declarations.insert(
            signature.path.clone(),
            from_declaration(
                &signature.param_names,
                signature.params.len(),
                &signature.parameter_defaults,
            ),
        );
    }
    let Some(origin) = environment
        .external_definition_origins
        .iter()
        .find(|origin| origin.package == metadata.package)
    else {
        return Ok((declarations, methods));
    };
    let Some(descriptors) = &origin.descriptors else {
        return Ok((declarations, methods));
    };
    let Some(headers) = &descriptors.headers else {
        return Ok((declarations, methods));
    };
    for header in &headers.callables {
        let Some(TypeInfo::Function(signature)) =
            descriptors.graph.types.get(header.signature.0 as usize)
        else {
            return Err(ProjectionError::Invalid(
                "imported calling interface has no function descriptor".into(),
            ));
        };
        let parameters = signature
            .parameters
            .iter()
            .map(|parameter| ParameterInterface {
                name: parameter.name.clone(),
                default: parameter
                    .default
                    .clone()
                    .map(|reference| ParameterDefaultRef::External { reference }),
            })
            .collect::<Vec<_>>();
        if let Some(method) = header.id.as_method() {
            methods.insert(method.clone(), parameters);
        } else if let Some(definition) = header.id.as_declaration() {
            for binding in &origin.origins.declarations {
                if &binding.definition == definition {
                    declarations.insert(binding.path(), parameters.clone());
                }
            }
        }
    }
    Ok((declarations, methods))
}

pub(super) fn from_declaration(
    names: &[String],
    arity: usize,
    defaults: &[etas_package_metadata::ParameterDefault],
) -> Vec<ParameterInterface> {
    let mut parameters = (0..arity)
        .map(|slot| ParameterInterface {
            name: names.get(slot).cloned(),
            default: None,
        })
        .collect::<Vec<_>>();
    for default in defaults {
        parameters
            .get_mut(default.slot as usize)
            .expect("validated declaration default slot")
            .default = Some(ParameterDefaultRef::External {
            reference: default.reference.clone(),
        });
    }
    parameters
}
