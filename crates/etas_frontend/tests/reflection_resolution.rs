use etas_core::{DiagnosticPhase, SourceId};
use etas_frontend::{
    Frontend, HirPathResolution, ImportTarget, ProjectInput, ProjectOutput, SourceInput, SourceKind,
};
use etas_hir::{HirExpr, ResolveResult};

fn check(sources: &[&str]) -> ProjectOutput {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
    input.sources = sources
        .iter()
        .enumerate()
        .map(|(index, text)| SourceInput {
            id: SourceId(index as u32 + 1),
            path: None,
            text: (*text).into(),
            kind: SourceKind::SourceProjectFile,
        })
        .collect();
    Frontend.check_project(input)
}

#[test]
fn inferred_compiled_agent_methods_preserve_module_visibility() {
    for (visibility, allowed) in [("public ", true), ("", false)] {
        let library = format!(
            "module app.lib; public agent Worker; impl agent Worker {{ {visibility}flow run(value: i32) -> i32 {{ return value; }} }}"
        );
        let output = check(&[
            &library,
            "module app.main; import app.lib.Worker; import std.compiler.compile; import std.compiler.CompileError; flow main() -> i32 ![Error<CompileError>] { let current = compile(^Worker); return current.run(7); }",
        ]);
        assert_eq!(
            output.checked.is_some(),
            allowed,
            "{:?}",
            output.diagnostics
        );
        if allowed {
            assert_eq!(
                output
                    .checked
                    .as_ref()
                    .unwrap()
                    .types
                    .agent_method_calls
                    .len(),
                1
            );
        } else {
            assert!(
                output.diagnostics.iter().any(|diagnostic| diagnostic
                    .message
                    .contains("agent method is private to its defining module")),
                "{:?}",
                output.diagnostics
            );
        }
    }
}

#[test]
fn reflection_paths_use_resolved_explicit_alias_and_reexport_targets() {
    let output = check(&[
        "module app.lib; public flow Normalize() -> i32 { return 42; }",
        "module app.bridge; public import app.lib.Normalize as Forwarded;",
        "module app.main; import app.bridge.Forwarded as Selected; flow main() -> unit { let definition = ^Selected; return; }",
    ]);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.types.as_ref().unwrap().facts.reflections.len(), 1);
    assert!(
        !output.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.phase,
            DiagnosticPhase::Lex | DiagnosticPhase::Parse | DiagnosticPhase::NameResolution
        )),
        "{:?}",
        output.diagnostics
    );
    let hir = &output.hir.as_ref().unwrap().hir;
    hir.view();
    let (expr, reflection) = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            HirExpr::Reflect(reflection) => Some((id, reflection)),
            _ => None,
        })
        .unwrap();
    assert!(matches!(
        reflection.target.root_path().resolution,
        ResolveResult::Resolved(_)
    ));
    let paths = output.resolved_paths.as_ref().unwrap();
    let fact = paths
        .expr_paths
        .iter()
        .find(|path| path.expr == expr)
        .unwrap();
    let HirPathResolution::ExplicitImport {
        target: ImportTarget::SourceItem {
            module, item, name, ..
        },
    } = &fact.result
    else {
        panic!("{:?}", fact.result)
    };
    assert_eq!(name, "Forwarded");
    assert_eq!(item.source, SourceId(1));
    assert_eq!(item.index, 0);
    assert_eq!(
        output.modules.as_ref().unwrap().modules[*module]
            .path
            .segments,
        ["app", "lib"]
    );
    assert_eq!(fact.path.span.source, SourceId(3));
}

#[test]
fn wildcard_reflection_keeps_the_imported_module_identity() {
    let output = check(&[
        "module app.lib; public flow Normalize() -> i32 { return 42; }",
        "module app.main; import app.lib.*; flow main() -> unit { let definition = ^Normalize; return; }",
    ]);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.types.as_ref().unwrap().facts.reflections.len(), 1);
    let hir = &output.hir.as_ref().unwrap().hir;
    let (expr, reflection) = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            HirExpr::Reflect(reflection) => Some((id, reflection)),
            _ => None,
        })
        .unwrap();
    assert!(
        matches!(
            reflection.target.root_path().resolution,
            ResolveResult::Resolved(_)
        ),
        "{:?}",
        output.diagnostics
    );
    let fact = output
        .resolved_paths
        .as_ref()
        .unwrap()
        .expr_paths
        .iter()
        .find(|path| path.expr == expr)
        .unwrap();
    let HirPathResolution::WildcardImport { target, .. } = &fact.result else {
        panic!("{:?}", fact.result)
    };
    let etas_frontend::ResolvedModuleTarget::Source { module, .. } = target else {
        panic!("source module")
    };
    assert_eq!(
        output.modules.as_ref().unwrap().modules[*module]
            .path
            .segments,
        ["app", "lib"]
    );
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unresolved name")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn reflection_does_not_bypass_private_imports() {
    let output = check(&[
        "module app.lib; flow Hidden() -> i32 { return 42; }",
        "module app.main; import app.lib.Hidden; flow main() -> unit { let definition = ^Hidden; return; }",
    ]);
    let private = output
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("is private"))
        .expect("private import rejected");
    assert_eq!(private.primary.span.source, SourceId(2));
    assert!(output.checked.is_none());
    assert!(output.resolved_imports.as_ref().is_none_or(|imports| {
        imports
            .imports
            .iter()
            .all(|import| import.local_name != "Hidden")
    }));
}

#[test]
fn reflection_module_alias_resolves_public_reexport_identity() {
    let output = check(&[
        "module app.lib; public flow Normalize() -> i32 { return 42; }",
        "module app.bridge; public import app.lib.Normalize as Forwarded;",
        "module app.main; import app.bridge as selected; flow main() -> unit { let definition = ^selected.Forwarded; return; }",
    ]);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let types = &output.types.as_ref().unwrap().facts;
    assert_eq!(types.reflections.len(), 1);
    let etas_types::CheckedReflectionTarget::Callable { declaration } =
        types.reflections.values().next().unwrap().target
    else {
        panic!("expected checked callable identity")
    };
    let hir = &output.hir.as_ref().unwrap().hir;
    assert_eq!(hir.symbols.get(declaration).unwrap().name, "Normalize");
    assert_eq!(
        hir.symbols.get(declaration).unwrap().definition_span.source,
        SourceId(1)
    );
}

#[test]
fn reflection_module_alias_cannot_access_private_definition() {
    let output = check(&[
        "module app.lib; flow Hidden() -> i32 { return 42; }",
        "module app.main; import app.lib as selected; flow main() -> unit { let definition = ^selected.Hidden; return; }",
    ]);
    assert!(output.checked.is_none());
    assert!(
        output
            .types
            .as_ref()
            .is_none_or(|types| types.facts.reflections.is_empty()),
        "private definition must never produce a checked reflection"
    );
}

#[test]
fn reflection_module_alias_preserves_agent_default_entry_prefix() {
    let output = check(&[
        "module app.lib; public agent Worker() -> Prompt { return Prompt.new(); }",
        "module app.main; import app.lib as selected; flow main() -> unit { let definition = ^selected.Worker.run; return; }",
    ]);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let types = &output.types.as_ref().unwrap().facts;
    let etas_types::CheckedReflectionTarget::AgentDefaultEntry { declaration, .. } =
        types.reflections.values().next().unwrap().target
    else {
        panic!("expected checked default-entry identity")
    };
    let hir = &output.hir.as_ref().unwrap().hir;
    assert_eq!(hir.symbols.get(declaration).unwrap().name, "Worker");
    assert_eq!(
        hir.symbols.get(declaration).unwrap().definition_span.source,
        SourceId(1)
    );
}

#[test]
fn reflection_of_imported_agent_auxiliary_method_preserves_body_and_owner() {
    for import in [
        "import app.lib as selected;",
        "import app.lib.Worker as Selected;",
    ] {
        let target = if import.contains("as selected") {
            "selected.Worker.explain"
        } else {
            "Selected.explain"
        };
        let consumer = format!(
            "module app.main; {import} flow main() -> unit {{ let definition = ^{target}; return; }}"
        );
        let output = check(&[
            "module app.lib; public agent Worker; impl agent Worker { public flow explain() -> i32 { return 42; } }",
            &consumer,
        ]);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let facts = &output.types.as_ref().unwrap().facts;
        let etas_types::CheckedReflectionTarget::Callable { declaration } =
            facts.reflections.values().next().unwrap().target
        else {
            panic!("auxiliary method identity");
        };
        let method = &facts.agent_methods[&declaration];
        assert_eq!(method.name, "explain");
        let hir = &output.hir.as_ref().unwrap().hir;
        let etas_hir::HirItem::Agent(agent) = &hir.items[method.agent] else {
            panic!("agent owner");
        };
        assert_eq!(hir.symbols.get(agent.symbol).unwrap().name, "Worker");
        assert!(matches!(hir.items[method.body], etas_hir::HirItem::Flow(_)));
    }
}

#[test]
fn private_agent_methods_cannot_be_called_or_reflected_through_imports_or_agent_values() {
    for operation in [
        "let definition = ^selected.Worker.explain;",
        "selected.Worker.explain();",
        "selected.Worker();",
        "let worker = selected.Worker; let definition = ^worker.explain;",
        "let worker = selected.Worker; worker.explain();",
    ] {
        let consumer = format!(
            "module app.main; import app.lib as selected; flow main() -> unit {{ {operation} return; }}"
        );
        let output = check(&[
            "module app.lib; public agent Worker; impl agent Worker { private flow run() -> i32 { return 42; } private flow explain() -> i32 { return 42; } }",
            &consumer,
        ]);
        assert!(
            output.checked.is_none(),
            "private method must not be accessible: {operation}"
        );
        assert!(
            output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("private")),
            "{operation}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn spec_selected_reflection_distinguishes_same_named_agent_methods() {
    let output = check(&[r#"
module app.main;
spec Left<I, O>: callable I => O ![];
spec Right<I, O>: callable I => O ![];
agent Worker;
impl agent Worker ~ Left<i32, i32> {
    flow run(value: i32) -> i32 { return value + 1; }
}
impl agent Worker ~ Right<string, string> {
    flow run(value: string) -> string { return value; }
}
flow main() -> unit {
    let left = ^Worker::Left<i32, i32>.run;
    let right = ^Worker::Right<string, string>.run;
    let worker = Worker;
    let dynamic_left = ^worker::Left<i32, i32>.run;
    return;
}
"#]);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.as_ref().unwrap();
    assert_eq!(checked.types.reflections.len(), 3);
    let mut methods = std::collections::HashSet::new();
    let mut selected_specs = std::collections::HashSet::new();
    for fact in checked.types.reflections.values() {
        let bound = fact.selected_spec.as_ref().expect("checked selected spec");
        let etas_types::CheckedSpecRef::Source(spec) = bound.spec else {
            panic!("source spec identity");
        };
        assert_eq!(bound.args.len(), 2);
        selected_specs.insert(spec);
        match fact.target {
            etas_types::CheckedReflectionTarget::Callable { declaration } => {
                methods.insert(declaration);
            }
            etas_types::CheckedReflectionTarget::AgentValue {
                method: Some(method),
                ..
            } => {
                methods.insert(method);
            }
            _ => panic!("spec selection must preserve the selected method identity"),
        }
    }
    assert_eq!(methods.len(), 2);
    assert_eq!(selected_specs.len(), 2);
}

#[test]
fn spec_selected_reflection_uses_checked_type_spec_implementation_evidence() {
    let output = check(&[r#"
module app.main;
spec Explain { flow explain(value: i32) -> i32; }
agent Worker;
impl agent Worker ~ Explain {
    flow explain(value: i32) -> i32 { return value; }
}
flow main() -> unit { let definition = ^Worker::Explain.explain; return; }
"#]);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.as_ref().unwrap();
    let fact = checked.types.reflections.values().next().unwrap();
    assert!(fact.selected_spec.is_some());
    let etas_types::CheckedReflectionTarget::Callable { declaration } = fact.target else {
        panic!("selected implementation method");
    };
    assert_eq!(checked.types.agent_methods[&declaration].name, "explain");
}

#[test]
fn reflecting_a_spec_selected_method_does_not_request_its_actions() {
    let output = check(&[r#"
module app.main;
spec Emit<I, O>: callable I => O;
agent Worker;
impl agent Worker ~ Emit<unit, unit> {
    flow run() -> unit { perform Console.stdout_write("not executed by reflection"); return; }
}
flow main() -> unit { let definition = ^Worker::Emit.run; return; }
"#]);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.as_ref().unwrap();
    let (main, _) = checked
        .items
        .iter()
        .find(|(_, item)| {
            matches!(item, etas_hir::HirItem::Flow(flow)
                if checked.symbols.get(flow.symbol).unwrap().name == "main")
        })
        .unwrap();
    let summary = &checked.effects.item_effects[&main];
    assert!(summary.requested_actions.is_empty());
    assert!(summary.escaping_effects.is_empty());
    let method = checked.types.agent_methods.values().next().unwrap();
    assert!(
        !checked.effects.item_effects[&method.body]
            .requested_actions
            .is_empty()
    );
}

#[test]
fn spec_selected_reflection_uses_resolved_import_aliases() {
    let output = check(&[
        r#"
module app.lib;
public spec Stage<I, O>: callable I => O ![];
public agent Worker;
impl agent Worker ~ Stage<i32, i32> {
    public flow run(value: i32) -> i32 { return value; }
}
"#,
        r#"
module app.main;
import app.lib.Worker as Selected;
import app.lib.Stage as Contract;
flow main() -> unit { let definition = ^Selected::Contract<i32, i32>.run; return; }
"#,
    ]);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.as_ref().unwrap();
    let fact = checked.types.reflections.values().next().unwrap();
    let etas_types::CheckedSpecRef::Source(spec) = fact.selected_spec.as_ref().unwrap().spec else {
        panic!("canonical source spec");
    };
    assert_eq!(checked.symbols.get(spec).unwrap().name, "Stage");
    assert_eq!(
        checked.symbols.get(spec).unwrap().definition_span.source,
        SourceId(1)
    );
}

#[test]
fn spec_selected_reflection_distinguishes_instantiations_without_nominal_projection() {
    let declarations = r#"
module app.main;
spec Echo<I, O>: callable I => O ![];
type Name = string;
type OtherName = string;
alias Count = i32;
agent Worker;
impl agent Worker ~ Echo<i32, i32> {
    flow run(value: i32) -> i32 { return value; }
}
impl agent Worker ~ Echo<Name, Name> {
    flow run(value: Name) -> Name { return value; }
}
"#;
    for (selection, valid) in [
        ("^Worker::Echo<Count, i32>.run", true),
        ("^Worker::Echo<Name, Name>.run", true),
        ("^Worker::Echo<OtherName, OtherName>.run", false),
        ("^Worker::Echo<string, string>.run", false),
        ("^Worker::Echo.run", false),
    ] {
        let source = format!(
            "{declarations} flow main() -> unit {{ let definition = {selection}; return; }}"
        );
        let output = check(&[&source]);
        assert_eq!(
            output.checked.is_some(),
            valid,
            "{selection}: {:?}",
            output.diagnostics
        );
        if !valid {
            assert!(output.types.as_ref().unwrap().facts.reflections.is_empty());
        }
    }
}

#[test]
fn spec_selected_reflection_rejects_wrong_conformance_arguments_and_private_methods() {
    for selection in [
        "^selected.Worker::selected.Stage<string, string>.run",
        "^selected.Worker::selected.Other<i32, i32>.run",
        "^selected.Worker::selected.Stage<i32, i32>.missing",
        "^selected.Worker::selected.Stage<i32, i32>.run",
    ] {
        let consumer = format!(
            "module app.main; import app.lib as selected; flow main() -> unit {{ let definition = {selection}; return; }}"
        );
        let output = check(&[
            r#"
module app.lib;
public spec Stage<I, O>: callable I => O ![];
public spec Other<I, O>: callable I => O ![];
public agent Worker;
impl agent Worker ~ Stage<i32, i32> {
    private flow run(value: i32) -> i32 { return value; }
}
"#,
            &consumer,
        ]);
        assert!(output.checked.is_none(), "accepted {selection}");
        assert!(output.types.as_ref().unwrap().facts.reflections.is_empty());
    }
}
