#[test]
fn source_symbol_identity_follows_only_explicit_edges_and_rejects_corrupt_chains() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(7),
        None,
        "module app; import app.leaf as imported; flow leaf() -> unit { return; }",
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut hir = crate::lower_program(&parsed.value);
    let (item, leaf) = hir
        .items
        .iter()
        .find_map(|(item, data)| {
            let crate::HirItem::Flow(flow) = data else {
                return None;
            };
            Some((item, flow.symbol))
        })
        .unwrap();
    let alias = hir
        .symbols
        .iter()
        .find(|symbol| {
            matches!(
                symbol.def,
                crate::SymbolDef::ImportAlias {
                    origin: crate::ImportAliasOrigin::SourceImport,
                    ..
                }
            )
        })
        .unwrap()
        .id;
    assert_eq!(
        hir.canonical_source_symbol(alias),
        Some(alias),
        "matching path text is not resolver evidence"
    );
    hir.source_import_targets.insert(alias, leaf);
    assert_eq!(hir.canonical_source_symbol(alias), Some(leaf));
    let template = hir.symbols.get(alias).unwrap().clone();
    let mut outer = alias;
    for index in 0..10_000 {
        let next = hir.symbols.alloc(crate::SymbolData {
            name: format!("alias_{index}"),
            kind: template.kind,
            visibility: template.visibility,
            defining_module: template.defining_module,
            defining_item: None,
            def: template.def.clone(),
            declared_type: None,
            definition_span: template.definition_span,
        });
        hir.source_import_targets.insert(next, outer);
        outer = next;
    }
    assert_eq!(hir.canonical_source_symbol(outer), Some(leaf));
    hir.source_import_targets.insert(alias, outer);
    assert_eq!(hir.canonical_source_symbol(outer), None);
    hir.source_import_targets
        .insert(alias, crate::SymbolId(u32::MAX));
    assert_eq!(hir.canonical_source_symbol(outer), None);
    hir.source_import_targets.insert(alias, leaf);
    hir.source_import_targets.insert(leaf, alias);
    assert_eq!(
        hir.canonical_source_symbol(outer),
        None,
        "a source declaration is not an alias edge"
    );
    assert!(
        matches!(hir.symbols.get(leaf).unwrap().def, crate::SymbolDef::Item { item: actual } if actual == item)
    );
}
