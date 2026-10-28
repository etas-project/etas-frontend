use super::*;

pub(super) fn validate(
    environment: &ProjectEnvironmentInput,
    packages: &BTreeMap<ExternalPackageId, &crate::ProjectExternalPackageInput>,
) -> Result<(), ExternalEnvironmentValidationError> {
    let mut edges = BTreeMap::<_, BTreeSet<_>>::new();
    for edge in &environment.external_package_dependencies {
        if !packages.contains_key(&edge.package) || !packages.contains_key(&edge.dependency) {
            return Err(ExternalEnvironmentValidationError::package(
                edge.package,
                "external dependency edge has an unknown package identity",
            ));
        }
        if !edges
            .entry(edge.package)
            .or_default()
            .insert(edge.dependency)
        {
            return Err(ExternalEnvironmentValidationError::package(
                edge.package,
                "external dependency edge is duplicated",
            ));
        }
    }
    let mut done = BTreeSet::new();
    for root in edges.keys() {
        let mut active = BTreeSet::new();
        let mut pending = vec![(*root, false)];
        while let Some((package, exit)) = pending.pop() {
            if exit {
                active.remove(&package);
                done.insert(package);
                continue;
            }
            if done.contains(&package) {
                continue;
            }
            if !active.insert(package) {
                return Err(ExternalEnvironmentValidationError::package(
                    package,
                    "external dependency graph is cyclic",
                ));
            }
            pending.push((package, true));
            pending.extend(
                edges
                    .get(&package)
                    .into_iter()
                    .flatten()
                    .rev()
                    .map(|dependency| (*dependency, false)),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(edges: &[(u32, u32)]) -> Result<(), ExternalEnvironmentValidationError> {
        let environment = ProjectEnvironmentInput {
            external_packages: (1..=4)
                .map(|id| crate::ProjectExternalPackageInput {
                    id: ExternalPackageId(id),
                    name: format!("dep{id}"),
                    version: "0.1.0".into(),
                    edition: "2026".into(),
                    import_root: format!("dep{id}"),
                })
                .collect(),
            external_package_dependencies: edges
                .iter()
                .map(
                    |(package, dependency)| crate::ProjectExternalPackageDependencyInput {
                        package: ExternalPackageId(*package),
                        dependency: ExternalPackageId(*dependency),
                    },
                )
                .collect(),
            ..Default::default()
        };
        let packages = environment
            .external_packages
            .iter()
            .map(|package| (package.id, package))
            .collect();
        validate(&environment, &packages)
    }

    #[test]
    fn dependency_diamond_is_valid_but_duplicate_dangling_and_cyclic_edges_are_rejected() {
        assert!(check(&[(1, 2), (1, 3), (2, 4), (3, 4)]).is_ok());
        for (edges, reason) in [
            (vec![(1, 2), (1, 2)], "duplicated"),
            (vec![(1, 5)], "unknown package"),
            (vec![(1, 2), (2, 3), (3, 1)], "cyclic"),
            (vec![(1, 1)], "cyclic"),
        ] {
            let error = check(&edges).unwrap_err();
            assert!(error.reason.contains(reason), "{error:?}");
        }
    }
}
