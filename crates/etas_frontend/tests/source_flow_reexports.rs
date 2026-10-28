use etas_core::{DiagnosticCode, EffectDiagnosticCode, SourceId};
use etas_effects::{ActionTraceDomain, Effect, EffectPipelineInput, EffectUnit, RunEffectPipeline};
use etas_frontend::{Frontend, ProjectInput, SourceInput};
use etas_hir::{HirExpr, HirItem, HirRevision, ResolveResult, SymbolId};

fn project(facade: &str, imports: &str, call: &str) -> ProjectInput {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(format!(
        "module app; {imports} flow main() -> i32 {{ {call} }}"
    )));
    for (id, source) in [
        (1, "module lib.leaf; public effect Probe { action emit(text: string) -> unit; } public flow emit() -> i32 { perform Probe.emit(\"leaf\"); return 7; }".to_owned()),
        (2, format!("module lib.facade; {facade}")),
        (3, "module lib.api; public import lib.facade.*;".to_owned()),
        (4, "module lib.unrelated; public flow emit() -> i32 { return 99; }".to_owned()),
    ] {
        let mut source = SourceInput::anonymous(source);
        source.id = SourceId(id);
        input.sources.push(source);
    }
    input
}

fn leaf_item(checked: &etas_frontend::CheckedProject) -> etas_hir::HirItemId {
    checked
        .hir
        .items
        .iter()
        .find_map(|(item, data)| {
            let HirItem::Flow(flow) = data else {
                return None;
            };
            let symbol = checked.symbols.get(flow.symbol)?;
            let path = checked
                .modules_arena
                .get(symbol.defining_module)?
                .name
                .as_ref()?;
            (symbol.name == "emit"
                && path
                    .segments
                    .iter()
                    .map(|part| part.name.as_str())
                    .eq(["lib", "leaf"]))
            .then_some(item)
        })
        .unwrap()
}

#[test]
fn source_flow_reexports_preserve_inferred_effects_and_canonical_call_targets() {
    for (facade, imports, call) in [
        (
            "public import lib.leaf.emit as exposed;",
            "import lib.facade.exposed;",
            "return exposed();",
        ),
        (
            "public import lib.leaf.emit as exposed;",
            "import lib.facade.exposed as renamed;",
            "return renamed();",
        ),
        (
            "public import lib.leaf.emit as exposed;",
            "import lib.api.exposed as invoke;",
            "return invoke();",
        ),
        (
            "public import lib.leaf.*;",
            "import lib.facade.{emit as renamed};",
            "return renamed();",
        ),
        (
            "public import lib.leaf.emit as exposed;",
            "import lib.facade.*;",
            "let value = exposed; return value();",
        ),
    ] {
        let output = Frontend.check_project(project(facade, imports, call));
        assert!(
            output.diagnostics.is_empty(),
            "{imports}: {:?}",
            output.diagnostics
        );
        let checked = output.checked.unwrap();
        let leaf = leaf_item(&checked);
        let source = &checked.effects.item_effects[&leaf];
        let main = &checked.effects.item_effects[&checked.entry.unwrap()];
        assert_eq!(source.requested_actions.effects.iter().count(), 1);
        assert_eq!(source.escaping_effects.effects.iter().count(), 1);
        assert_eq!(main.requested_actions, source.requested_actions);
        assert_eq!(main.escaping_effects, source.escaping_effects);
        assert_eq!(main.default_actions, source.default_actions);
        assert_eq!(main.action_trace, source.action_trace);
        assert!(matches!(main.action_trace, ActionTraceDomain::Event(_)));
        let calls = checked
            .hir
            .exprs
            .iter()
            .filter_map(|(expr, data)| matches!(data, HirExpr::Call { .. }).then_some(expr))
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            checked.effects.native_call_targets[&calls[0]],
            [EffectUnit::Item(leaf)].into_iter().collect()
        );
    }
}

#[test]
fn source_flow_reexport_specializes_inferred_generic_action_selectors() {
    let mut input = project(
        "public import lib.leaf.emit as exposed;",
        "import lib.api.exposed as invoke;",
        "return invoke<i32>(7);",
    );
    input.sources[1].text = r#"
module lib.leaf;
public effect Probe { action emit<T>(value: T) -> unit; }
public flow emit<T>(value: T) -> T {
    perform Probe.emit<T>(value);
    return value;
}
"#
    .into();
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let main = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert_eq!(main.requested_actions, main.escaping_effects);
    let actions = main.requested_actions.effects.iter().collect::<Vec<_>>();
    let [Effect::AppliedAction(action)] = actions.as_slice() else {
        panic!("expected one specialized action: {actions:?}");
    };
    let [etas_types::EffectArgRef::Type(ty)] = action.args.as_slice() else {
        panic!("expected a checked type selector: {action:?}");
    };
    assert_eq!(
        checked.type_store.get(*ty),
        Some(&etas_types::Type::Primitive(etas_types::PrimitiveType::I32))
    );
    assert!(
        matches!(&main.action_trace, ActionTraceDomain::Event(event) if event.action == *actions[0])
    );
}

#[test]
fn source_flow_reexports_cannot_bypass_reexported_trace_spec() {
    let mut input = project(
        "public import lib.leaf.{emit as exposed, Probe, Safe};",
        "",
        "return invoke();",
    );
    input.sources[0].text = r#"
module app;
import lib.api.{exposed as invoke, Safe};
flow main() -> i32 ~ Safe { return invoke(); }
"#
    .into();
    input.sources[1]
        .text
        .push_str(" public spec Safe: trace = -Probe.emit;");
    let spec_source = input.sources[1].text.clone();
    let output = Frontend.check_project(input);
    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecDenied)
                && diagnostic.primary.span.source == SourceId(1)
                && &spec_source[diagnostic.primary.span.range.start.to_usize()
                    ..diagnostic.primary.span.range.end.to_usize()]
                    == "-Probe.emit"
        }),
        "{:?}",
        output.diagnostics
    );
    assert!(output.diagnostics.iter().all(|diagnostic| {
        diagnostic.code != DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
    }));
}

#[test]
fn source_import_effect_analysis_rejects_missing_cyclic_and_dangling_identity_facts() {
    let output = Frontend.check_project(project(
        "public import lib.leaf.emit as exposed;",
        "import lib.leaf.emit as invoke;",
        "return invoke();",
    ));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let (call, callee) = checked
        .hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            HirExpr::Call { callee, .. } => Some((id, *callee)),
            _ => None,
        })
        .unwrap();
    let HirExpr::Path(path) = checked.hir.exprs.get(callee).unwrap() else {
        unreachable!();
    };
    let ResolveResult::Resolved(alias) = path.resolution else {
        unreachable!();
    };
    let span = checked
        .hir
        .exprs
        .get(call)
        .unwrap()
        .span(&checked.hir.blocks);
    let types = etas_types::TypeOutput {
        facts: checked.types.clone(),
        store: checked.type_store.clone(),
        diagnostics: vec![],
    };
    for (label, target) in [
        ("missing", None),
        ("cycle", Some(alias)),
        ("dangling", Some(SymbolId(u32::MAX))),
    ] {
        let mut hir = checked.hir.program().clone();
        hir.source_import_targets.remove(&alias);
        if let Some(target) = target {
            hir.source_import_targets.insert(alias, target);
        }
        let hir = HirRevision::try_new(hir).unwrap();
        let result = RunEffectPipeline::run(EffectPipelineInput {
            frozen_calls: None,
            hir: &hir,
            types: &types,
            std_registry: &checked.std_registry,
            dependency_metadata: None,
            tool_bindings: &[],
            external_summaries: &[],
            external_trace_specs: &[],
            external_artifact_anchors: &[],
            reachable_items: None,
        })
        .unwrap();
        assert!(
            result.effects.diagnostics.iter().any(|diagnostic| {
                diagnostic.code
                    == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                    && diagnostic.primary.span == span
            }),
            "{label}: {:?}",
            result.effects.diagnostics
        );
        assert!(
            result
                .effects
                .facts
                .native_call_targets
                .get(&call)
                .is_none_or(|targets| targets.is_empty()),
            "{label} must not recover a source body from matching path text"
        );
    }
}
