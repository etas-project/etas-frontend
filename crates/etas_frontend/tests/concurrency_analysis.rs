use etas_frontend::{Frontend, ProjectInput, SchedulingDecision, SourceInput, TaskBranchShape};

fn check(body: &str) -> etas_frontend::ProjectOutput {
    Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(body)))
}

#[test]
fn independent_task_branches_are_planned_concurrently() {
    let output = check(
        r#"
import std.task.{run, start, await, Scheduler};
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        let left = start(() => { return 20; });
        let right = start(() => { return 22; });
        return await(left) + await(right);
    });
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.expect("checked concurrent task project");
    let scope = checked
        .concurrency
        .owners
        .values()
        .next()
        .expect("task scope fact");
    assert_eq!(scope.branches.len(), 2);
    assert_eq!(scope.decision, SchedulingDecision::Concurrent);
    assert_eq!(checked.concurrency.starts.len(), 2);
    assert_eq!(checked.concurrency.awaits.len(), 2);
    assert!(
        checked
            .concurrency
            .awaits
            .values()
            .all(|fact| fact.task.is_some())
    );
    assert_eq!(checked.concurrency.plans.len(), 1);
}

#[test]
fn conflicting_task_actions_are_serialized() {
    let output = check(
        r#"
import std.io.println;
import std.task.{run, start, await, Scheduler};
flow main() -> unit {
    run((scheduler: Scheduler) => {
        let left = start(() => { println("left")?; });
        let right = start(() => { println("right")?; });
        await(left);
        await(right);
    });
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.expect("checked serialized task project");
    let scope = checked
        .concurrency
        .owners
        .values()
        .next()
        .expect("task scope fact");
    assert_eq!(scope.branches.len(), 2);
    assert_eq!(scope.decision, SchedulingDecision::Serialize);
}

#[test]
fn join_tuple_is_analyzed_per_branch() {
    let output = check(
        r#"
import std.io.println;
import std.task.{join, run, Scheduler};
flow main() -> unit {
    run((scheduler: Scheduler) => {
        join((
            () => { println("left")?; },
            () => { println("right")?; }
        ));
    });
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.expect("checked join project");
    let scope = checked
        .concurrency
        .owners
        .values()
        .next()
        .expect("task scope fact");
    assert_eq!(scope.branches.len(), 2);
    assert!(
        scope
            .branches
            .iter()
            .all(|branch| branch.shape == TaskBranchShape::TupleElement)
    );
    assert_eq!(scope.decision, SchedulingDecision::Serialize);
}

#[test]
fn pure_callable_is_cpu_parallel_eligible() {
    let output = check(
        r#"
flow add_one(value: i32) -> i32 { return value + 1; }
flow main() -> i32 { return add_one(41); }
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.expect("checked pure project");
    assert!(
        checked
            .cpu_parallel
            .callables
            .values()
            .any(|fact| fact.eligible)
    );
}

#[test]
fn cpu_work_estimates_distinguish_leaf_bounded_and_recursive_calls() {
    let output = check(
        r#"
flow tiny(value: i32) -> i32 { return value + 1; }
flow bounded(value: i32) -> i32 {
    var index = 0;
    while index < 2000 limit Iterations(2000) { index = tiny(index); }
    return value;
}
flow wrapper(value: i32) -> i32 { return bounded(value); }
flow recursive(value: i32) -> i32 {
    if value == 0 { return 0; }
    return recursive(value - 1);
}
flow main() -> i32 { return wrapper(1); }
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let cost = |name: &str| {
        let fact = checked
            .cpu_parallel
            .callables
            .values()
            .find(|fact| {
                checked.hir.items[fact.callable]
                    .symbol()
                    .is_some_and(|symbol| checked.hir.symbols.get(symbol).unwrap().name == name)
            })
            .unwrap();
        fact.estimated_work
    };
    assert!(cost("tiny").unwrap() < 32);
    assert!(cost("bounded").unwrap() >= 20000);
    assert!(cost("wrapper").unwrap() > cost("bounded").unwrap());
    assert_eq!(cost("recursive"), None);
}

#[test]
fn standard_array_combinators_keep_each_source_branch() {
    for combinator in ["join", "try_join", "collect", "race"] {
        let output_type = match combinator {
            "try_join" => "Result<Array<i32>, i32>",
            "collect" => "Array<Result<i32, i32>>",
            "race" => "i32",
            _ => "Array<i32>",
        };
        let invocation = if combinator == "race" {
            "race<i32, i32>"
        } else {
            combinator
        };
        let output = check(&format!(
            r#"
import std.task.{{{combinator}, run, Scheduler}};
flow one() -> i32 ![Error<i32>] {{
    if 1 == 0 {{ return perform Error<i32>.raise(1); }}
    return 1;
}}
flow other() -> i32 ![Error<i32>] {{ return perform Error<i32>.raise(2); }}
flow main() -> {output_type} ![Error<i32>] {{
    return run((scheduler: Scheduler) => {{ return {invocation}([one, other]); }});
}}
"#
        ));
        assert!(
            output.diagnostics.is_empty(),
            "{combinator}: {:#?}",
            output.diagnostics
        );
        let checked = output.checked.expect("checked combinator");
        let scope = checked.concurrency.owners.values().next().unwrap();
        assert_eq!(scope.branches.len(), 2, "{combinator}");
        assert!(
            scope
                .branches
                .iter()
                .all(|branch| branch.shape == TaskBranchShape::ArrayElement)
        );
    }
}

#[test]
fn repeated_effectful_mappers_are_serialized() {
    for combinator in ["map_concurrent", "map_concurrent_collect"] {
        let invocation = if combinator == "map_concurrent_collect" {
            "map_concurrent_collect<i32, i32, i32>"
        } else {
            "map_concurrent"
        };
        let output = check(&format!(
            r#"
import std.task.{{{combinator}, run, Scheduler}};
flow report(value: i32) -> i32 ![Console.stdout_write, Error<i32>] {{
    perform Console.stdout_write("task");
    if value < 0 {{ return perform Error<i32>.raise(value); }}
    return value;
}}
flow main() -> unit {{
    run((scheduler: Scheduler) => {{ {invocation}([1, 2], report); }});
}}
"#
        ));
        assert!(
            output.diagnostics.is_empty(),
            "{combinator}: {:#?}",
            output.diagnostics
        );
        let checked = output.checked.expect("checked mapper");
        let scope = checked.concurrency.owners.values().next().unwrap();
        assert_eq!(scope.branches[0].shape, TaskBranchShape::MapperInvocation);
        assert_eq!(scope.decision, SchedulingDecision::Serialize);
    }
}
