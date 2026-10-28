use etas_frontend::{
    CheckRequest, CheckScope, EntryPolicy, FrontendSession, ProjectInput, SourceInput,
};

#[test]
fn retained_definition_behavior_is_checked_without_becoming_runtime_reachable() {
    for retained in [false, true] {
        let mut input = ProjectInput::single_source(SourceInput::anonymous(
            r#"
module app;
import std.io.println;
flow helper() -> i32 { println("not executed")?; return 7; }
agent Worker() -> i32 { return helper(); }
flow main(args: Array<string>) -> i32 { let definition = ^Worker; return 0; }
"#,
        ));
        input.options.entry_policy = EntryPolicy::Runnable;
        if retained {
            input.options.reflection.retain_bodies = vec!["app.Worker".into()];
        }
        let mut session = FrontendSession::new();
        let project = session.open_project(input);
        let output = session
            .check(
                project,
                CheckRequest::full_project().with_scope(CheckScope::EntryReachable),
            )
            .unwrap()
            .output;
        assert!(
            !output
                .diagnostics
                .iter()
                .any(|d| d.severity == etas_core::Severity::Error),
            "{:?}",
            output.diagnostics
        );
        let checked = output.checked.unwrap();
        let item = |name: &str| {
            checked
                .hir
                .items
                .iter()
                .find_map(|(item, node)| {
                    node.symbol()
                        .and_then(|symbol| checked.symbols.get(symbol))
                        .and_then(|symbol| (symbol.name == name).then_some(item))
                })
                .unwrap()
        };
        let worker = item("Worker");
        let helper = item("helper");
        for target in [worker, helper] {
            assert!(!checked.reachability.reachable_items.contains(&target));
            assert_eq!(
                checked.reachability.analysis_items.contains(&target),
                retained
            );
            assert_eq!(checked.effects.item_effects.contains_key(&target), retained);
        }
        assert!(
            checked
                .reachability
                .runtime_source_requirements
                .dependencies
                .is_empty()
        );
        let main = checked.entry.unwrap();
        let entry = &checked.effects.item_effects[&main];
        assert!(entry.requested_actions.is_empty());
        assert_eq!(
            checked.checked_call_reachability(main).unwrap(),
            std::collections::BTreeSet::from([main])
        );
        if retained {
            assert!(
                !checked.effects.item_effects[&worker]
                    .requested_actions
                    .is_empty()
            );
        }
    }
}

#[test]
fn retained_definition_analysis_checks_the_body_instead_of_fabricating_a_baseline() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        "module app; agent Worker() -> i32 { return false; } flow main(args: Array<string>) -> i32 { let definition = ^Worker; return 0; }",
    ));
    input.options.entry_policy = EntryPolicy::Runnable;
    input.options.reflection.retain_bodies = vec!["app.Worker".into()];
    let mut session = FrontendSession::new();
    let project = session.open_project(input);
    let output = session
        .check(
            project,
            CheckRequest::full_project().with_scope(CheckScope::EntryReachable),
        )
        .unwrap()
        .output;
    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|d| d.code
            == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn typed_agent_invocations_complete_the_runtime_closure_without_enabling_pure_reflections() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app;
import std.compiler.{compile, CompileError};
agent Helper() -> i32 { return 7; }
agent Worker() -> i32 ![Error<CompileError>] {
    let next = compile(^Helper);
    return next();
}
agent Unused() -> i32 { return false; }
flow main(args: Array<string>) -> i32 ![Error<CompileError>] {
    let unused = ^Unused;
    let revised = compile(^Worker);
    return revised();
}
"#,
    ));
    input.options.entry_policy = EntryPolicy::Runnable;
    let mut session = FrontendSession::new();
    let project = session.open_project(input);
    let result = session
        .check(
            project,
            CheckRequest::full_project().with_scope(CheckScope::EntryReachable),
        )
        .unwrap();
    assert!(
        !result
            .output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        result.output.diagnostics
    );
    let checked = result.output.checked.unwrap();
    for (name, analyzed) in [("Worker", true), ("Helper", true), ("Unused", false)] {
        let item = checked
            .hir
            .items
            .iter()
            .find_map(|(item, node)| {
                node.symbol()
                    .and_then(|symbol| checked.symbols.get(symbol))
                    .is_some_and(|symbol| symbol.name == name)
                    .then_some(item)
            })
            .unwrap();
        assert_eq!(
            checked.reachability.analysis_items.contains(&item),
            analyzed,
            "{name}"
        );
        assert_eq!(
            checked.effects.item_effects.contains_key(&item),
            analyzed,
            "{name}"
        );
        assert_eq!(
            checked.reachability.reachable_items.contains(&item),
            analyzed,
            "{name}"
        );
    }
    let bodies = result
        .pipeline_records
        .iter()
        .filter(|record| record.pass == "TypeCheckBodyPass" && record.changed)
        .map(|record| record.unit.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(bodies.len(), 3, "{bodies:?}");
    assert_eq!(
        bodies
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        bodies.len()
    );
    assert_eq!(
        checked
            .checked_call_reachability(checked.entry.unwrap())
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn typed_agent_invocation_completion_checks_the_actual_body() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app;
import std.compiler.{compile, CompileError};
agent Worker() -> i32 { return false; }
flow main(args: Array<string>) -> i32 ![Error<CompileError>] {
    let revised = compile(^Worker);
    return revised();
}
"#,
    ));
    input.options.entry_policy = EntryPolicy::Runnable;
    let mut session = FrontendSession::new();
    let project = session.open_project(input);
    let output = session
        .check(
            project,
            CheckRequest::full_project().with_scope(CheckScope::EntryReachable),
        )
        .unwrap()
        .output;
    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        output.diagnostics
    );
}
