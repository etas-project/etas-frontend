use super::*;

pub(super) fn validate(
    environment: &ProjectEnvironmentInput,
) -> Result<(), ExternalEnvironmentValidationError> {
    let mut declarations = BTreeMap::new();
    for metadata in &environment.external_public_metadata {
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
    for (path, (package, signature)) in &declarations {
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
