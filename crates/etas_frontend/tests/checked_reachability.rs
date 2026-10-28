use std::{collections::BTreeSet, sync::Arc};

use etas_core::SourceId;
use etas_frontend::{
    CheckedCallableRoot, CheckedProgram, CheckedReachabilityError, EntryPolicy, Frontend,
    ModulePath, ProjectEntry, ProjectInput, SourceInput, SourceKind,
};
use etas_hir::{HirExpr, HirItem, HirItemId};

fn checked(sources: &[(&str, &str)]) -> CheckedProgram {
    let mut input = ProjectInput {
        project_root: "/workspace/checked-reachability".into(),
        source_root: None,
        sources: sources
            .iter()
            .enumerate()
            .map(|(id, (path, text))| SourceInput {
                id: SourceId(id as u32),
                path: Some((*path).into()),
                text: (*text).into(),
                kind: SourceKind::SourceProjectFile,
            })
            .collect(),
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".into(), "main".into()],
            }),
            flow: "main".into(),
        },
        options: Default::default(),
        environment: Default::default(),
    };
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.unwrap()
}

fn flow(program: &CheckedProgram, name: &str) -> HirItemId {
    program
        .hir
        .items
        .iter()
        .find_map(|(item, declaration)| {
            let HirItem::Flow(declaration) = declaration else {
                return None;
            };
            (program.symbols.get(declaration.symbol)?.name == name).then_some(item)
        })
        .unwrap()
}

#[test]
fn selected_callable_follows_grouped_alias_imports_not_all_checked_bodies() {
    let program = checked(&[
        (
            "src/app/main.es",
            "module app.main; import app.dep.{helper as selected}; flow main() -> i32 { return selected(); } flow unused() -> i32 { return 99; }",
        ),
        (
            "src/app/dep.es",
            "module app.dep; public flow helper() -> i32 { return 7; } public flow other() -> i32 { return 8; }",
        ),
    ]);
    let main = flow(&program, "main");
    let entry = program.entry;
    assert_eq!(
        program.checked_call_reachability(main).unwrap(),
        BTreeSet::from([main, flow(&program, "helper")])
    );
    assert_eq!(
        program.entry, entry,
        "native entry selection must not mutate the checked project"
    );
}

#[test]
fn selected_agent_method_does_not_make_its_sibling_reachable() {
    let program = checked(&[(
        "src/app/main.es",
        "module app.main; agent Worker() -> i32 { return 1; } impl agent Worker { flow loud() -> i32 { return 7; } } flow main(worker: Worker) -> i32 { return worker.loud(); }",
    )]);
    let main = flow(&program, "main");
    let loud = flow(&program, "loud");
    assert_eq!(
        program.checked_call_reachability(main).unwrap(),
        BTreeSet::from([main, loud])
    );
    assert_eq!(
        program.checked_call_reachability(loud).unwrap(),
        BTreeSet::from([loud])
    );
}

#[test]
fn selected_callable_rejects_missing_solved_analysis() {
    let mut program = checked(&[(
        "src/app/main.es",
        "module app.main; flow main() -> i32 { return helper(); } flow helper() -> i32 { return 7; }",
    )]);
    let main = flow(&program, "main");
    let helper = flow(&program, "helper");
    Arc::make_mut(&mut Arc::make_mut(&mut program).effects)
        .item_effects
        .remove(&helper);
    assert_eq!(
        program.checked_call_reachability(main),
        Err(CheckedReachabilityError::MissingAnalysis(helper))
    );
    assert_eq!(
        program.checked_call_reachability(HirItemId(u32::MAX)),
        Err(CheckedReachabilityError::InvalidEntry(HirItemId(u32::MAX)))
    );
}

#[test]
fn first_class_source_references_have_latent_analysis_without_executing_their_effects() {
    use etas_frontend::{CheckRequest, CheckScope, FrontendSession};
    let source = r#"
module app;
import std.io.println;
flow write() -> unit ![] { println("only on invocation")?; return; }
flow factory() -> (() -> unit ![]) { return write; }
flow unrelated() -> i32 { return 8; }
flow main() -> unit { let callback = factory(); return; }
"#;
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    let output = session
        .check(
            project,
            CheckRequest::full_project().with_scope(CheckScope::EntryReachable),
        )
        .unwrap()
        .output;
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.as_ref().unwrap();
    let write = flow(checked, "write");
    let main = flow(checked, "main");
    assert!(checked.reachability.analysis_items.contains(&write));
    assert!(!checked.reachability.reachable_items.contains(&write));
    assert!(
        !checked
            .reachability
            .analysis_items
            .contains(&flow(checked, "unrelated"))
    );
    assert!(
        !checked.effects.item_effects[&write]
            .requested_actions
            .is_empty()
    );
    assert!(
        checked.effects.item_effects[&main]
            .requested_actions
            .is_empty()
    );
    assert!(
        checked.effects.item_effects[&flow(checked, "factory")]
            .requested_actions
            .is_empty()
    );
    assert!(
        checked
            .checked_callable_reachability(CheckedCallableRoot::Item(write))
            .is_ok()
    );
}

#[test]
fn top_level_callable_values_are_analyzed_without_becoming_executed_actions() {
    use etas_frontend::{CheckRequest, CheckScope, FrontendSession};
    let source = r#"
module app;
import std.io.println;
flow write(value: i32) -> i32 ![] { println("only on invocation")?; return value; }
let callback = write;
type Callbacks = { next: (i32) -> i32 ![] };
let callbacks = Callbacks { next = callback };
flow factory() -> Callbacks { return callbacks; }
flow unrelated() -> i32 { return 8; }
flow main() -> unit { let selected = factory(); return; }
"#;
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    let output = session
        .check(
            project,
            CheckRequest::full_project().with_scope(CheckScope::EntryReachable),
        )
        .unwrap()
        .output;
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    for (item, declaration) in checked.hir.items.iter() {
        if matches!(declaration, HirItem::TopLevelLet(_)) {
            assert!(checked.reachability.analysis_items.contains(&item));
            assert!(!checked.reachability.reachable_items.contains(&item));
            assert!(
                checked.effects.item_effects[&item]
                    .requested_actions
                    .is_empty()
            );
        }
    }
    assert!(
        checked
            .reachability
            .analysis_items
            .contains(&flow(&checked, "write"))
    );
    assert!(
        !checked
            .reachability
            .reachable_items
            .contains(&flow(&checked, "write"))
    );
    assert!(
        !checked
            .reachability
            .analysis_items
            .contains(&flow(&checked, "unrelated"))
    );
    assert!(
        checked.effects.item_effects[&checked.entry.unwrap()]
            .requested_actions
            .is_empty()
    );
    assert!(
        checked
            .checked_callable_reachability(CheckedCallableRoot::Item(flow(&checked, "factory")))
            .is_ok()
    );
}

#[test]
fn selected_agent_method_rejects_a_missing_checked_selection() {
    let mut program = checked(&[(
        "src/app/main.es",
        "module app.main; agent Worker() -> i32 { return 1; } impl agent Worker { flow loud() -> i32 { return 7; } } flow main(worker: Worker) -> i32 { return worker.loud(); }",
    )]);
    let main = flow(&program, "main");
    let selected = *program.types.agent_method_calls.values().next().unwrap();
    Arc::make_mut(&mut Arc::make_mut(&mut program).types)
        .agent_methods
        .remove(&selected);
    assert_eq!(
        program.checked_call_reachability(main),
        Err(CheckedReachabilityError::MissingMethod(selected))
    );
}

#[test]
fn selected_lambda_does_not_include_its_containing_body_or_sibling() {
    let program = checked(&[(
        "src/app/main.es",
        r#"
module app.main;
flow helper() -> i32 { return 7; }
flow unrelated() -> i32 { return 99; }
flow factory() -> () -> i32 ![] {
    unrelated();
    return () => { return helper(); };
}
flow main() -> i32 { return 0; }
"#,
    )]);
    let lambda = program
        .hir
        .exprs
        .iter()
        .find_map(|(expr, data)| matches!(data, HirExpr::Lambda { .. }).then_some(expr))
        .unwrap();
    let root = CheckedCallableRoot::Lambda(lambda);
    assert_eq!(
        program.checked_callable_owner(root).unwrap(),
        flow(&program, "factory")
    );
    assert_eq!(
        program.checked_callable_reachability(root).unwrap(),
        BTreeSet::from([flow(&program, "helper")])
    );
    assert_eq!(program.entry, Some(flow(&program, "main")));
}

#[test]
fn selected_lambda_requires_its_own_consistent_checked_facts() {
    let mut program = checked(&[(
        "src/app/main.es",
        r#"
module app.main;
flow main() -> i32 { let action = () => 7; return action(); }
"#,
    )]);
    let lambda = program
        .hir
        .exprs
        .iter()
        .find_map(|(expr, data)| matches!(data, HirExpr::Lambda { .. }).then_some(expr))
        .unwrap();
    let root = CheckedCallableRoot::Lambda(lambda);
    let mut invalid = program.clone();
    Arc::make_mut(&mut Arc::make_mut(&mut invalid).effects)
        .latent_effects
        .get_mut(&lambda)
        .unwrap()
        .value = etas_hir::HirExprId(u32::MAX);
    assert_eq!(
        invalid.checked_callable_reachability(root),
        Err(CheckedReachabilityError::InconsistentLambdaFacts(lambda))
    );
    Arc::make_mut(&mut Arc::make_mut(&mut program).effects)
        .latent_effects
        .remove(&lambda);
    assert_eq!(
        program.checked_callable_reachability(root),
        Err(CheckedReachabilityError::MissingLambdaAnalysis(lambda))
    );
    assert_eq!(
        program.checked_callable_owner(CheckedCallableRoot::Lambda(etas_hir::HirExprId(u32::MAX))),
        Err(CheckedReachabilityError::InvalidLambda(
            etas_hir::HirExprId(u32::MAX)
        ))
    );
}

#[test]
fn static_resources_are_checked_data_not_runtime_body_dependencies() {
    use etas_frontend::{CheckRequest, CheckScope, FrontendSession};
    let source = r#"
module app.main;
alias Schema = MemoryRegion<{ Entries: Store<string, string> }>;
let Data = std.memory.region<Schema>(stable_id = "checked-region", store = "checked-store");
flow main() -> unit { let value = Data.Entries.get("key"); return; }
"#;
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    let result = session
        .check(
            project,
            CheckRequest::full_project().with_scope(CheckScope::EntryReachable),
        )
        .unwrap();
    let mut program = result
        .output
        .checked
        .clone()
        .expect("checked resource program");
    let root = program.entry.unwrap();
    let resource = *program.types.resource_handles.keys().next().unwrap();
    let etas_hir::SymbolDef::TopLevelLet { item, .. } = program.symbols.get(resource).unwrap().def
    else {
        panic!("resource binding");
    };
    assert!(!program.effects.item_effects.contains_key(&item));
    assert_eq!(
        program.checked_call_reachability(root).unwrap(),
        BTreeSet::from([root])
    );
    Arc::make_mut(&mut Arc::make_mut(&mut program).types)
        .resource_handles
        .remove(&resource);
    assert_eq!(
        program.checked_call_reachability(root),
        Err(CheckedReachabilityError::MissingResourceFact(resource))
    );
}
