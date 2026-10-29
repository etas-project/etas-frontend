use super::*;

pub(super) fn validate(
    environment: &ProjectEnvironmentInput,
    declarations: &BTreeMap<
        Vec<String>,
        (ExternalPackageId, &crate::ProjectExternalSpecSignatureInput),
    >,
) -> Result<(), ExternalEnvironmentValidationError> {
    let origins = environment
        .external_definition_origins
        .iter()
        .flat_map(|input| &input.origins.declarations)
        .map(|binding| (binding.path(), &binding.definition))
        .collect::<BTreeMap<_, _>>();
    let mut identities = BTreeMap::new();
    for metadata in &environment.external_public_metadata {
        let fail = |reason| ExternalEnvironmentValidationError::package(metadata.package, reason);
        for implementation in &metadata.spec_impls {
            if implementation.methods.is_empty() {
                if declarations
                    .get(&implementation.spec)
                    .is_some_and(|(_, spec)| !spec.methods.is_empty())
                {
                    return Err(fail(
                        "behavioral implementation is missing its declared method evidence"
                            .to_owned(),
                    ));
                }
                continue;
            }
            let (_, spec) = declarations.get(&implementation.spec).ok_or_else(|| {
                fail("behavioral implementation spec declaration is missing".to_owned())
            })?;
            if spec.kind != crate::ProjectExternalSpecKindInput::Type
                || spec.methods.len() != implementation.methods.len()
            {
                return Err(fail(
                    "behavioral implementation has the wrong spec kind or method set".to_owned(),
                ));
            }
            let subject_path = subject_path(&implementation.self_type).ok_or_else(|| {
                fail("behavioral implementation has no nominal subject".to_owned())
            })?;
            let subject = origins.get(subject_path).ok_or_else(|| {
                fail("behavioral implementation subject has no canonical origin".to_owned())
            })?;
            let selected = origins.get(&implementation.spec).ok_or_else(|| {
                fail("behavioral implementation spec has no canonical origin".to_owned())
            })?;
            let owner = environment
                .external_definition_origins
                .iter()
                .find(|input| input.package == metadata.package)
                .ok_or_else(|| {
                    fail("behavioral implementation package has no canonical origins".to_owned())
                })?;
            let headers = owner
                .descriptors
                .as_ref()
                .and_then(|set| set.headers.as_ref())
                .ok_or_else(|| {
                    fail(
                        "behavioral implementation method has no canonical descriptor headers"
                            .to_owned(),
                    )
                })?;
            let mut names = BTreeSet::new();
            for method in &implementation.methods {
                let identity = &method.identity;
                let key = identity.implementation();
                if !names.insert(identity.name())
                    || !spec
                        .methods
                        .iter()
                        .any(|declared| declared.name == identity.name())
                {
                    return Err(fail(
                        "behavioral implementation method identity is duplicate or undeclared"
                            .to_owned(),
                    ));
                }
                if let Some((subject, graph)) =
                    identities.insert(identity, (&implementation.self_type, &method.callable))
                    && (subject != &implementation.self_type || graph != &method.callable)
                {
                    return Err(fail("behavioral implementation method identity has conflicting behavior evidence".to_owned()));
                }
                if key.package() != owner.origins.package
                    || key.subject() != *subject
                    || key
                        .selected_spec()
                        .is_some_and(|selection| selection != *selected)
                    || !owner
                        .origins
                        .modules
                        .iter()
                        .any(|module| module.canonical_path == key.module())
                {
                    return Err(fail("behavioral implementation method identity disagrees with its package, subject or spec".to_owned()));
                }
                if !headers
                    .callables
                    .iter()
                    .any(|header| header.id == etas_code::CallableId::method(identity.clone()))
                {
                    return Err(fail("behavioral implementation method is absent from canonical descriptor headers".to_owned()));
                }
            }
        }
    }
    Ok(())
}

fn subject_path(ty: &ProjectExternalTypeInput) -> Option<&Vec<String>> {
    match ty {
        ProjectExternalTypeInput::Applied { path, .. }
        | ProjectExternalTypeInput::Nominal { path, .. }
        | ProjectExternalTypeInput::Enum { path, .. }
        | ProjectExternalTypeInput::Named(path) => Some(path),
        ProjectExternalTypeInput::Alias { target, .. } => subject_path(target),
        _ => None,
    }
}
