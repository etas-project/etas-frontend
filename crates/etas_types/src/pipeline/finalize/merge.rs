pub fn merge_body_deltas<'a>(
    base: &crate::SignatureEnvironment,
    bodies: impl IntoIterator<Item = &'a crate::BodyTypeOutput>,
) -> Result<crate::TypeOutput, String> {
    let mut facts = (*base.facts).clone();
    let mut diagnostics = base.diagnostics.to_vec();
    let mut interner =
        crate::TypeInterner::from_store(crate::TypeStore::with_base(base.types.clone()));
    for body in bodies {
        let body_facts = super::remap::remap_body_delta(body, base, &mut interner)?;
        merge_facts(&mut facts, body_facts);
        diagnostics.extend(body.diagnostics.iter().cloned());
    }
    Ok(crate::TypeOutput {
        facts: std::sync::Arc::new(facts),
        store: std::sync::Arc::new(interner.into_store()),
        diagnostics,
    })
}

fn merge_facts(facts: &mut crate::TypeFacts, body: crate::TypeFacts) {
    facts.expr_types.extend(body.expr_types);
    facts.expr_memory_places.extend(body.expr_memory_places);
    facts.stmt_types.extend(body.stmt_types);
    facts.pattern_types.extend(body.pattern_types);
    facts.type_refs.extend(body.type_refs);
    facts.enum_layouts.extend(body.enum_layouts);
    facts.symbol_types.extend(body.symbol_types);
    if facts.known_std_types.index_error.is_none() {
        facts.known_std_types.index_error = body.known_std_types.index_error;
    }
    facts
        .known_std_types
        .support_constraints
        .extend(body.known_std_types.support_constraints);
    facts
        .known_std_types
        .iterables
        .extend(body.known_std_types.iterables);
    facts.resource_handles.extend(body.resource_handles);
    facts.item_signatures.extend(body.item_signatures);
    facts.action_signatures.extend(body.action_signatures);
    facts
        .qualified_action_signatures
        .extend(body.qualified_action_signatures);
    facts.index_facts.extend(body.index_facts);
    facts.field_projections.extend(body.field_projections);
    facts.slice_facts.extend(body.slice_facts);
    facts.checked_index_errors.extend(body.checked_index_errors);
    facts
        .generic_instantiations
        .extend(body.generic_instantiations);
    facts.try_facts.extend(body.try_facts);
}
