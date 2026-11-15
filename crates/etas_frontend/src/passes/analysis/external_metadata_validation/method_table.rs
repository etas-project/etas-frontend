use super::*;

#[cfg(test)]
mod tests;

pub(super) fn validate(
    environment: &ProjectEnvironmentInput,
    std_registry: &etas_std::StdRegistry,
) -> Result<(), ExternalEnvironmentValidationError> {
    let mut subject_paths = std::collections::BTreeMap::<_, BTreeSet<Vec<String>>>::new();
    for binding in environment
        .external_definition_origins
        .iter()
        .flat_map(|origin| &origin.origins.declarations)
    {
        subject_paths
            .entry(&binding.definition)
            .or_default()
            .insert(binding.path().to_vec());
    }
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
        let descriptors = origin
            .descriptors
            .as_ref()
            .ok_or_else(|| fail("published methods require canonical descriptors"))?;
        let parameters =
            etas_package_metadata::MetadataTypeParameters::from_descriptors(descriptors)
                .map_err(|error| fail(&error.to_string()))?;
        let headers = descriptors
            .headers
            .as_ref()
            .ok_or_else(|| fail("published methods require canonical callable headers"))?;
        let mut ids = BTreeSet::new();
        for method in &metadata.methods {
            parameters
                .validate_method(method)
                .map_err(|error| fail(&error.to_string()))?;
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
            let paths = subject_paths
                .get(owner.subject())
                .ok_or_else(|| fail("published method subject has no declaration origin"))?;
            let builtin_path = match method.self_type.kind {
                etas_package_metadata::TypeKind::Array => Some(["std", "collections", "Array"]),
                etas_package_metadata::TypeKind::List => Some(["std", "collections", "List"]),
                etas_package_metadata::TypeKind::Map => Some(["std", "collections", "Map"]),
                etas_package_metadata::TypeKind::Set => Some(["std", "collections", "Set"]),
                etas_package_metadata::TypeKind::Range => Some(["std", "collections", "Range"]),
                etas_package_metadata::TypeKind::Slice => Some(["std", "collections", "Slice"]),
                etas_package_metadata::TypeKind::Result => Some(["std", "result", "Result"]),
                etas_package_metadata::TypeKind::Option => Some(["std", "option", "Option"]),
                _ => None,
            };
            let source_role = builtin_path.is_some_and(|path| {
                std_registry
                    .lookup_qualified(&path)
                    .and_then(|symbol| std_registry.declaration_key(symbol.id))
                    == Some(owner.subject())
            });
            if !paths.contains(&method.self_type.path) && !source_role {
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
