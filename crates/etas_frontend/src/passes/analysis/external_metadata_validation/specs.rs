use super::*;
use crate::ProjectExternalSpecKindInput;

pub(super) fn validate(
    environment: &ProjectEnvironmentInput,
) -> Result<(), ExternalEnvironmentValidationError> {
    let mut declarations = BTreeMap::new();
    for metadata in &environment.external_public_metadata {
        for implementation in &metadata.spec_impls {
            let module = &implementation.implementation_module;
            if (!implementation.methods.is_empty() && module.is_empty())
                || module
                    .iter()
                    .any(|segment| segment.is_empty() || segment.contains('\0'))
                || (!module.is_empty()
                    && !environment.external_modules.iter().any(|candidate| {
                        candidate.package == Some(metadata.package)
                            && candidate.path.segments == *module
                    }))
            {
                return Err(ExternalEnvironmentValidationError::package(
                    metadata.package,
                    "external spec implementation has no valid owned source module",
                ));
            }
        }
        for signature in &metadata.spec_signatures {
            if declarations
                .insert(signature.path.clone(), (metadata.package, signature))
                .is_some()
            {
                return Err(ExternalEnvironmentValidationError::package(
                    metadata.package,
                    "external spec path has multiple declaration owners",
                ));
            }
        }
    }
    let mut edges = BTreeMap::new();
    let dependencies = environment
        .external_package_dependencies
        .iter()
        .map(|edge| (edge.package, edge.dependency))
        .collect::<BTreeSet<_>>();
    for (path, (package, signature)) in &declarations {
        for bound in signature
            .generic_params
            .iter()
            .flat_map(|param| &param.bounds)
        {
            if bound.spec.first().is_some_and(|root| root == "std") {
                continue;
            }
            let Some((owner, declaration)) = declarations.get(&bound.spec) else {
                return Err(ExternalEnvironmentValidationError::package(
                    *package,
                    "external spec generic bound has no declared spec",
                ));
            };
            if declaration.kind != ProjectExternalSpecKindInput::Type
                || declaration.param_names.len() != bound.args.len()
                || (*owner != *package
                    && (declaration.visibility != "public"
                        || !dependencies.contains(&(*package, *owner))))
            {
                return Err(ExternalEnvironmentValidationError::package(
                    *package,
                    "external spec generic bound has an invalid kind, arity, or owner",
                ));
            }
        }
        let mut parents = Vec::new();
        for bound in &signature.super_specs {
            if bound.spec.first().is_some_and(|root| root == "std") {
                continue;
            }
            let Some((_, parent)) = declarations.get(&bound.spec) else {
                return Err(ExternalEnvironmentValidationError::package(
                    *package,
                    format!(
                        "external spec `{}` has no declared super spec `{}`",
                        path.join("."),
                        bound.spec.join(".")
                    ),
                ));
            };
            if parent.kind != signature.kind || parent.param_names.len() != bound.args.len() {
                return Err(ExternalEnvironmentValidationError::package(
                    *package,
                    "external super spec kind or generic arity does not match its declaration",
                ));
            }
            parents.push(bound.spec.clone());
        }
        edges.insert(path.clone(), parents);
    }
    let mut done = BTreeSet::new();
    for root in declarations.keys() {
        let mut active = BTreeSet::new();
        let mut pending = vec![(root.clone(), false)];
        while let Some((path, exit)) = pending.pop() {
            if exit {
                active.remove(&path);
                done.insert(path);
                continue;
            }
            if done.contains(&path) {
                continue;
            }
            if !active.insert(path.clone()) {
                return Err(ExternalEnvironmentValidationError::package(
                    declarations[&path].0,
                    format!("external spec entailment cycle at `{}`", path.join(".")),
                ));
            }
            pending.push((path.clone(), true));
            pending.extend(
                edges[&path]
                    .iter()
                    .rev()
                    .cloned()
                    .map(|parent| (parent, false)),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ProjectExternalCallableGenericParamInput, ProjectExternalCallableGenericParamKindInput,
        ProjectExternalPackageDependencyInput, ProjectExternalPublicMetadataInput,
        ProjectExternalSpecBoundInput, ProjectExternalSpecSignatureInput,
    };

    #[test]
    fn behavioral_implementation_requires_a_module_owned_by_its_package() {
        let mut package = metadata(1, vec![]);
        package
            .spec_impls
            .push(crate::ProjectExternalSpecImplInput {
                self_type: crate::ProjectExternalTypeInput::Primitive("i32".into()),
                spec: vec!["dep".into(), "Show".into()],
                args: vec![],
                methods: vec!["show".into()],
                implementation_module: vec!["dep".into(), "implementations".into()],
            });
        let mut environment = ProjectEnvironmentInput {
            external_public_metadata: vec![package],
            external_modules: vec![crate::ProjectExternalModuleInput {
                package: Some(ExternalPackageId(1)),
                id: crate::ExternalModuleId(1),
                path: crate::ModulePath {
                    segments: vec!["dep".into(), "implementations".into()],
                },
                exports: vec![],
            }],
            ..Default::default()
        };
        assert!(validate(&environment).is_ok());
        environment.external_modules[0].package = Some(ExternalPackageId(2));
        assert!(validate(&environment).is_err());
        environment.external_public_metadata[0].spec_impls[0]
            .implementation_module
            .clear();
        assert!(validate(&environment).is_err());
    }

    fn metadata(
        package: u32,
        specs: Vec<ProjectExternalSpecSignatureInput>,
    ) -> ProjectExternalPublicMetadataInput {
        ProjectExternalPublicMetadataInput {
            package: ExternalPackageId(package),
            types: vec![],
            values: vec![],
            enums: vec![],
            flows: vec![],
            agents: vec![],
            tools: vec![],
            tool_schemas: vec![],
            effects: vec![],
            actions: vec![],
            trace_specs: vec![],
            spec_signatures: specs,
            spec_impls: vec![],
            type_spec_satisfactions: vec![],
            callable_spec_satisfactions: vec![],
            trace_spec_conformances: vec![],
            effect_summaries: vec![],
            action_summaries: vec![],
            trace_spec_summaries: vec![],
            re_exports: vec![],
        }
    }

    fn spec(path: &[&str]) -> ProjectExternalSpecSignatureInput {
        ProjectExternalSpecSignatureInput {
            path: path.iter().map(|part| (*part).into()).collect(),
            visibility: "public".into(),
            kind: ProjectExternalSpecKindInput::Type,
            param_names: vec![],
            generic_params: vec![],
            callable: None,
            methods: vec![],
            super_specs: vec![],
        }
    }

    #[test]
    fn generic_spec_bounds_require_public_declared_dependency() {
        let mut local = spec(&["a", "Within"]);
        local.param_names.push("R".into());
        local
            .generic_params
            .push(ProjectExternalCallableGenericParamInput {
                name: "R".into(),
                kind: ProjectExternalCallableGenericParamKindInput::Type,
                bounds: vec![ProjectExternalSpecBoundInput {
                    spec: vec!["b".into(), "Region".into()],
                    args: vec![],
                }],
            });
        let mut environment = ProjectEnvironmentInput {
            external_public_metadata: vec![
                metadata(1, vec![local]),
                metadata(2, vec![spec(&["b", "Region"])]),
            ],
            ..Default::default()
        };
        assert!(validate(&environment).is_err());
        environment
            .external_package_dependencies
            .push(ProjectExternalPackageDependencyInput {
                package: ExternalPackageId(1),
                dependency: ExternalPackageId(2),
            });
        assert!(validate(&environment).is_ok());
        environment.external_public_metadata[1].spec_signatures[0].visibility = "private".into();
        assert!(validate(&environment).is_err());
        environment.external_public_metadata[1].spec_signatures[0].visibility = "public".into();
        environment.external_public_metadata[1].spec_signatures[0].kind =
            ProjectExternalSpecKindInput::Trace;
        assert!(validate(&environment).is_err());
    }
}
