use super::*;

/// Generic payloads can carry caller-owned effects even when the SDK body
/// only returns that payload. Retain their checked identity in its type arena.
pub(super) fn import(
    caller: &Arc<DefinitionCatalog>,
    program: &Arc<CheckedProgramData>,
) -> Result<Arc<CheckedProgramData>, String> {
    let target = catalog(program.clone())?;
    let mut known = program
        .effect_registry
        .tags()
        .map(|tag| {
            target
                .native_effect_identity(tag.id)
                .map(|identity| (identity.clone(), tag.id))
                .map_err(|e| e.to_string())
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?;
    let mut registry = program.effect_registry.as_ref().clone();
    let mut bindings = program.foreign_type_bindings.as_ref().clone();
    let mut tags = std::collections::BTreeMap::new();
    for tag in caller.checked_program().effect_registry.tags() {
        let identity = caller
            .native_effect_identity(tag.id)
            .map_err(|e| e.to_string())?;
        let target_tag = if let Some(target) = known.get(identity) {
            *target
        } else {
            let target = registry
                .register_carried_effect(tag, format!("$etas.effect.{}", identity.fingerprint()));
            bindings
                .bind_effect(caller, tag.id, target)
                .map_err(|e| e.to_string())?;
            known.insert(identity.clone(), target);
            target
        };
        tags.insert(tag.id, target_tag);
    }
    let source = &caller.checked_program().effect_registry;
    for (action, name) in source.action_identities() {
        registry.register_carried_action(
            tags[&action.tag],
            name,
            source
                .action_selectors(&action)
                .ok_or("carried action has no checked selector shape")?,
        );
    }
    let mut result = program.as_ref().clone();
    result.effect_registry = Arc::new(registry);
    result.foreign_type_bindings = Arc::new(bindings);
    Ok(Arc::new(result))
}
