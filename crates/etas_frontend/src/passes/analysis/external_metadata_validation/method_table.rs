use super::*;

pub(super) fn validate(
    environment: &ProjectEnvironmentInput,
) -> Result<(), ExternalEnvironmentValidationError> {
    for metadata in &environment.external_public_metadata {
        if metadata.methods.is_empty() {
            continue;
        }
        let fail =
            |reason: &str| ExternalEnvironmentValidationError::package(metadata.package, reason);
        let origin = environment
            .external_definition_origins
            .iter()
            .find(|origin| origin.package == metadata.package)
            .ok_or_else(|| fail("published methods require canonical package origins"))?;
        let headers = origin
            .descriptors
            .as_ref()
            .and_then(|set| set.headers.as_ref())
            .ok_or_else(|| fail("published methods require canonical callable headers"))?;
        let mut ids = BTreeSet::new();
        for method in &metadata.methods {
            let owner = method.identity.implementation();
            if !ids.insert(&method.identity)
                || owner.package() != origin.origins.package
                || !origin
                    .origins
                    .modules
                    .iter()
                    .any(|module| module.canonical_path == owner.module())
            {
                return Err(fail(
                    "published method has an invalid or duplicate canonical owner",
                ));
            }
            let subject = environment
                .external_definition_origins
                .iter()
                .flat_map(|origin| &origin.origins.declarations)
                .find(|binding| &binding.definition == owner.subject())
                .ok_or_else(|| fail("published method subject has no declaration origin"))?;
            if subject.path() != method.self_type.path {
                return Err(fail(
                    "published method receiver disagrees with its canonical subject",
                ));
            }
            if !headers
                .callables
                .iter()
                .any(|header| header.id == etas_code::CallableId::method(method.identity.clone()))
            {
                return Err(fail(
                    "published method is absent from canonical callable headers",
                ));
            }
            if owner.selected_spec().is_some()
                && !metadata
                    .spec_impls
                    .iter()
                    .any(|implementation| implementation.methods.contains(&method.identity))
            {
                return Err(fail("published spec method has no implementation evidence"));
            }
        }
    }
    Ok(())
}
