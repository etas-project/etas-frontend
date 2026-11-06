use etas_frontend::{Frontend, ProjectInput, SchedulingDecision, SourceInput};

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
