use etas_core::SourceId;
use etas_frontend::{
    EntryPolicy, ExternalPackageId, Frontend, ProjectExternalPackageInput, ProjectInput,
    SourceInput, SourceKind,
};
use etas_hir::{HirExpr, HirItem, ResolveResult, SymbolDef};

fn input(alias: &str, body: &str) -> ProjectInput {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(format!(
        "module app; import dep.left.Device{}; {body}",
        if alias == "Device" {
            String::new()
        } else {
            format!(" as {alias}")
        },
    )));
    input.options.entry_policy = EntryPolicy::Optional;
    for (index, side, payload) in [(1, "left", "string"), (2, "right", "i32")] {
        let package = ExternalPackageId(index);
        input.sources.push(SourceInput {
            id: SourceId(index), path: None,
            text: format!("module dep.{side}; public effect Device {{ action emit(value: {payload}) -> unit; }}"),
            kind: SourceKind::DependencySourceOverlay { package, import_root: format!("dep.{side}") },
        });
        input
            .environment
            .external_packages
            .push(ProjectExternalPackageInput {
                id: package,
                name: format!("{side}-package"),
                version: "1.0.0".into(),
                edition: "2026".into(),
                import_root: format!("dep.{side}"),
            });
    }
    input
}

#[test]
fn source_effect_actions_use_resolved_owner_identity_with_or_without_an_import_alias() {
    for alias in ["Device", "Selected"] {
        let output = Frontend.check_project(input(
            alias,
            &format!(
                "flow main() -> unit ![{alias}.emit] {{ perform {alias}.emit(\"hello\"); return; }}"
            ),
        ));
        assert!(
            output.diagnostics.is_empty(),
            "{alias}: {:?}",
            output.diagnostics
        );
        let checked = output.checked.unwrap();
        let left = checked
            .hir
            .items
            .iter()
            .find_map(|(_, item)| {
                let HirItem::Effect(effect) = item else {
                    return None;
                };
                let symbol = checked.symbols.get(effect.symbol).unwrap();
                let module = checked.modules_arena[symbol.defining_module]
                    .name
                    .as_ref()
                    .unwrap();
                module
                    .segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .eq(["dep", "left"])
                    .then_some(effect.symbol)
            })
            .unwrap();
        let mut count = 0;
        for (_, expr) in checked.hir.exprs.iter() {
            if let HirExpr::Perform { action, .. } = expr {
                let ResolveResult::Resolved(symbol) = action.action_symbol else {
                    panic!("resolved source action")
                };
                assert!(
                    matches!(checked.symbols.get(symbol).unwrap().def, SymbolDef::EffectAction { owner_effect: Some(owner), .. } if owner == left)
                );
                count += 1;
            }
        }
        assert_eq!(count, 1);
        let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
        assert_eq!(summary.requested_actions.effects.iter().count(), 1);
        assert_eq!(summary.escaping_effects.effects.iter().count(), 1);
        assert!(summary.requested_actions.open.is_none());
        assert!(summary.escaping_effects.open.is_none());
    }
}

#[test]
fn aliased_source_action_payloads_are_checked_against_the_selected_declaration() {
    let output = Frontend.check_project(input(
        "Selected",
        "flow main() -> unit ![Selected.emit] { perform Selected.emit(42); return; }",
    ));
    assert!(output.checked.is_none());
    assert!(output.diagnostics.iter().any(|diagnostic| diagnostic.code
        == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::TypeMismatch)));
}

#[test]
fn aliased_source_handler_matches_the_same_checked_action_as_perform() {
    let output = Frontend.check_project(input(
        "Selected",
        r#"
flow main() -> unit {
    handle { perform Selected.emit("hello"); } with {
        Selected.emit(value) => { resume; }
    };
    return;
}
"#,
    ));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let performed = checked
        .hir
        .exprs
        .iter()
        .find_map(|(_, expr)| match expr {
            HirExpr::Perform { action, .. } => Some(action.action_symbol.clone()),
            _ => None,
        })
        .unwrap();
    let (_, handled) = checked.hir.handler_arms.iter().next().unwrap();
    assert_eq!(handled.action.action_symbol, performed);
    assert!(matches!(performed, ResolveResult::Resolved(_)));
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert_eq!(summary.requested_actions.effects.iter().count(), 1);
    assert!(summary.escaping_effects.effects.is_empty());
    assert!(summary.escaping_effects.open.is_none());
}
