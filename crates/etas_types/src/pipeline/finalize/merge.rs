pub fn merge_outputs(
    mut signatures: crate::TypeOutput,
    body_outputs: Vec<crate::TypeOutput>,
) -> crate::TypeOutput {
    let mut interner = crate::TypeInterner::from_store(signatures.store.clone());
    for body in body_outputs {
        let body = super::remap::remap_body_output(body, &mut interner);
        signatures.diagnostics.extend(body.diagnostics);
        signatures.facts.expr_types.extend(body.facts.expr_types);
        signatures
            .facts
            .expr_memory_places
            .extend(body.facts.expr_memory_places);
        signatures.facts.stmt_types.extend(body.facts.stmt_types);
        signatures
            .facts
            .pattern_types
            .extend(body.facts.pattern_types);
        signatures.facts.type_refs.extend(body.facts.type_refs);
        signatures
            .facts
            .symbol_types
            .extend(body.facts.symbol_types);
        if signatures.facts.known_std_types.index_error.is_none() {
            signatures.facts.known_std_types.index_error = body.facts.known_std_types.index_error;
        }
        signatures
            .facts
            .resource_handles
            .extend(body.facts.resource_handles);
        signatures
            .facts
            .item_signatures
            .extend(body.facts.item_signatures);
        signatures
            .facts
            .action_signatures
            .extend(body.facts.action_signatures);
        signatures
            .facts
            .qualified_action_signatures
            .extend(body.facts.qualified_action_signatures);
        signatures.facts.index_facts.extend(body.facts.index_facts);
        signatures.facts.slice_facts.extend(body.facts.slice_facts);
        signatures
            .facts
            .checked_index_errors
            .extend(body.facts.checked_index_errors);
        signatures.facts.try_facts.extend(body.facts.try_facts);
    }
    signatures.store = interner.into_store();
    signatures
}
