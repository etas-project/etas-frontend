use std::collections::BTreeMap;

use crate::{ExternalPackageId, ModulePath, ProjectEnvironmentInput};

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
pub(crate) struct DependencyModuleNamespaces {
    packages: BTreeMap<ExternalPackageId, PackageNamespaces>,
}

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
struct PackageNamespaces {
    source_to_namespace: BTreeMap<ModulePath, ModulePath>,
    namespace_to_source: BTreeMap<ModulePath, ModulePath>,
}

impl DependencyModuleNamespaces {
    pub(crate) fn from_environment(environment: &ProjectEnvironmentInput) -> Result<Self, String> {
        let mut namespaces = Self::default();
        for input in &environment.external_definition_origins {
            let package = namespaces.packages.entry(input.package).or_default();
            for module in &input.origins.modules {
                let source = ModulePath {
                    segments: module.canonical_path.clone(),
                };
                let namespace = ModulePath {
                    segments: module.path.clone(),
                };
                if package
                    .source_to_namespace
                    .insert(source.clone(), namespace.clone())
                    .is_some_and(|previous| previous != namespace)
                    || package
                        .namespace_to_source
                        .insert(namespace.clone(), source.clone())
                        .is_some_and(|previous| previous != source)
                {
                    return Err(format!(
                        "dependency package {} has conflicting canonical module namespaces",
                        input.package.0
                    ));
                }
            }
        }
        Ok(namespaces)
    }

    pub(crate) fn namespace_for_source(
        &self,
        package: ExternalPackageId,
        source: &ModulePath,
    ) -> Result<ModulePath, String> {
        let Some(namespaces) = self.packages.get(&package) else {
            // Source-only inputs have no metadata namespace substitution.
            return Ok(source.clone());
        };
        namespaces.source_to_namespace.get(source).cloned().ok_or_else(|| format!(
            "dependency source module `{}` has no canonical namespace binding for package {}", source.segments.join("."), package.0
        ))
    }

    pub(crate) fn source_for_namespace(
        &self,
        package: ExternalPackageId,
        namespace: &ModulePath,
    ) -> Result<ModulePath, String> {
        let Some(namespaces) = self.packages.get(&package) else {
            return Ok(namespace.clone());
        };
        namespaces
            .namespace_to_source
            .get(namespace)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "runtime dependency module `{}` has no canonical source binding for package {}",
                    namespace.segments.join("."),
                    package.0
                )
            })
    }

    pub(crate) fn scoped_namespace(
        &self,
        package: ExternalPackageId,
        source: &ModulePath,
    ) -> Option<&ModulePath> {
        self.packages.get(&package)?.source_to_namespace.get(source)
    }
}
