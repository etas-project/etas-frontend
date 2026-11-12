use etas_frontend::{Frontend, ProjectInput, SchedulingDecision, SourceInput, TaskBranchShape};

fn check(body: &str) -> etas_frontend::ProjectOutput {
    Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(body)))
}

#[test]
fn independent_task_branches_are_planned_concurrently() {
    let output = check(
        r#"
import std.task.{run, start, await, Scheduler};
import std.runtime.checkpoint;
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        let left = start(() => { checkpoint("left"); return 20; });
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
    assert_eq!(
        checked
            .cpu_parallel
            .closures
            .values()
            .filter(|fact| fact.eligible)
            .count(),
        1,
        "pure sibling qualifies while checkpoint stays on the coordinator"
    );
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
fn generic_cpu_callable_carries_a_materialized_freeze_contract() {
    let output = check(
        r#"
flow identity<T>(value: T) -> T { return value; }
flow main() -> i32 { return identity(7); }
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let fact = checked
        .cpu_parallel
        .callables
        .values()
        .find(|fact| {
            checked.hir.items[fact.owner]
                .symbol()
                .is_some_and(|symbol| checked.symbols.get(symbol).unwrap().name == "identity")
        })
        .unwrap();
    assert!(fact.eligible, "{fact:?}");
    let contract = fact.freeze_contract.as_ref().unwrap();
    let instance = checked
        .types
        .generic_instantiations
        .values()
        .find(|instance| !instance.type_bindings.is_empty())
        .unwrap();
    let bindings = instance.type_bindings.iter().cloned().collect();
    let mut substitution =
        etas_types::TypeSubstitutionEngine::existing(&checked.type_store, &bindings);
    assert!(contract.is_materialized(&checked.type_store, &mut substitution));
    assert_eq!(contract.parameters, vec![contract.output]);
}

#[test]
fn generic_closure_has_its_own_frozen_capture_and_work_contract() {
    let output = check(
        r#"
type Config<T> = { fallback: T, valid: bool };
flow make<T>(fallback: T) -> ((T) -> T ![]) {
    let config = Config<T> { fallback = fallback, valid = true };
    let values = [fallback];
    return (value: T) => {
        let snapshot = values;
        var step = 0;
        while step < 20000 limit Iterations(20000) { step = step + 1; }
        if config.valid { return value; }
        return config.fallback;
    };
}
flow main() -> i32 { let mapper = make(99); return mapper(7); }
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let fact = checked
        .cpu_parallel
        .closures
        .values()
        .find(|fact| {
            checked.hir.items[fact.owner]
                .symbol()
                .is_some_and(|symbol| checked.symbols.get(symbol).unwrap().name == "make")
        })
        .unwrap();
    assert!(fact.eligible, "{fact:?}");
    assert!(fact.estimated_work.unwrap() >= 10000);
    let contract = fact.freeze_contract.as_ref().unwrap();
    assert_eq!(contract.captures.len(), 2);
    assert!(
        contract
            .captures
            .windows(2)
            .all(|pair| pair[0].symbol < pair[1].symbol)
    );
    let bindings = checked
        .types
        .generic_instantiations
        .values()
        .find(|instance| !instance.type_bindings.is_empty())
        .unwrap()
        .type_bindings
        .iter()
        .cloned()
        .collect();
    let mut substitution =
        etas_types::TypeSubstitutionEngine::existing(&checked.type_store, &bindings);
    assert!(contract.is_materialized(&checked.type_store, &mut substitution));
    assert!(!checked.cpu_parallel.callables[&fact.owner].eligible);
}

#[test]
fn package_cpu_callable_keeps_its_definition_owner() {
    use etas_frontend::{
        CpuCallableOrigin, ExternalPackageId, ProjectDependencySourcePackage,
        ProjectExternalPackageInput, SourceKind,
    };
    let package = etas_code::PackageKey::from_canonical_fingerprint(
        etas_code::CodeFingerprint::of_bytes("etas.test.cpu-package", b"workers"),
    );
    let dependency = ExternalPackageId(7);
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        "module app.main; import dependency.jobs.burn; flow main() -> i32 { return burn(7); }",
    ));
    input.sources.push(SourceInput { id: etas_core::SourceId(1), path: None,
        text: "module dependency.jobs; public flow burn(value: i32) -> i32 { var step = 0; while step < 20000 limit Iterations(20000) { step = step + 1; } return value; }".into(),
        kind: SourceKind::DependencySourceOverlay { package: dependency, import_root: "dependency".into() } });
    input
        .environment
        .external_packages
        .push(ProjectExternalPackageInput {
            id: dependency,
            name: "workers".into(),
            version: "1.0.0".into(),
            edition: "2026".into(),
            import_root: "dependency".into(),
            std_compatibility: etas_std::standard_registry()
                .version()
                .compatibility_identity(),
        });
    input
        .environment
        .dependency_source_packages
        .push(ProjectDependencySourcePackage {
            package: dependency,
            import_root: "dependency".into(),
            definition_package: package,
        });
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let fact = checked
        .cpu_parallel
        .callables
        .values()
        .find(|fact| matches!(&fact.origin, Some(CpuCallableOrigin::Package(_))))
        .unwrap();
    assert!(fact.eligible, "{fact:?}");
    assert!(fact.estimated_work.unwrap() > 10000);
    let Some(CpuCallableOrigin::Package(identity)) = &fact.origin else {
        unreachable!();
    };
    let identity = identity.as_declaration().unwrap();
    assert_eq!(identity.package(), package);
    assert_eq!(identity.module(), &["dependency", "jobs"]);
    assert_eq!(identity.declaration(), &["burn"]);
}

#[test]
fn memory_tasks_publish_runtime_key_selectors() {
    let output = check(
        r#"
import std.task.{map_concurrent, run, Scheduler};
alias Schema = MemoryRegion<{ Papers: Store<string, string> }>;
let Memory = std.memory.region<Schema>(stable_id = "resources", store = "test");
flow write(key: string) -> string {
    Memory.Papers.put(key, key);
    return key;
}
flow main() -> unit {
    run((scheduler: Scheduler) => { return map_concurrent(["a", "b"], write); });
    return;
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.unwrap();
    assert!(
        checked
            .concurrency
            .sites
            .values()
            .any(|site| matches!(site.decision, SchedulingDecision::RuntimeGuard(_))),
        "{:#?}",
        checked.concurrency.sites
    );
    let plan = checked
        .concurrency
        .resource_callables
        .values()
        .find(|plan| !plan.resources.is_empty())
        .unwrap();
    assert!(plan.complete, "{plan:#?}");
    assert_eq!(plan.resources.len(), 1);
    assert!(plan.resources[0].key.is_some());
}

#[test]
fn task_owner_resource_plan_excludes_children_and_non_memory_host_calls() {
    let output = check(
        r#"
import std.io.read_line;
import std.task.{run, start, Scheduler};
alias Schema = MemoryRegion<{ Values: Store<string, i32> }>;
let Owner = std.memory.region<Schema>(stable_id = "owner-records", store = "owner");
let Peer = std.memory.region<Schema>(stable_id = "peer-records", store = "peer");
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        Owner.Values.put("owner", 1);
        let child = start(() => {
            Peer.Values.put("peer", 1);
            return 0;
        });
        read_line();
        return 0;
    });
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let owner = checked
        .concurrency
        .resource_closures
        .values()
        .find(|plan| plan.parameters.len() == 1)
        .expect("run owner plan");
    assert!(owner.complete, "{owner:#?}");
    assert!(!owner.opaque_memory, "{owner:#?}");
    assert_eq!(owner.resources.len(), 1);

    let child = checked
        .concurrency
        .resource_closures
        .values()
        .find(|plan| plan.parameters.is_empty())
        .expect("started child plan");
    assert!(child.complete, "{child:#?}");
    assert!(!child.opaque_memory, "{child:#?}");
    assert_eq!(child.resources.len(), 1);
}

#[test]
fn dynamic_memory_keys_publish_a_store_wide_resource_guard() {
    let output = check(
        r#"
import std.io.read_line;
import std.task.{run, start, Scheduler};
import std.text.from_i32;
alias Schema = MemoryRegion<{ Values: Store<string, i32> }>;
let Owner = std.memory.region<Schema>(stable_id = "owner-records", store = "owner");
let Peer = std.memory.region<Schema>(stable_id = "peer-records", store = "peer");
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        Owner.Values.put("owner", 1);
        let child = start(() => {
            var index: i32 = 0;
            while index < 2 limit Iterations(2) {
                Peer.Values.put("part-" + from_i32(index), index);
                index = index + 1;
            }
            return 0;
        });
        read_line();
        return 0;
    });
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let child = checked
        .concurrency
        .resource_closures
        .values()
        .find(|plan| plan.parameters.is_empty())
        .expect("started child plan");
    assert!(child.complete, "{child:#?}");
    assert!(!child.opaque_memory, "{child:#?}");
    assert_eq!(child.resources.len(), 1);
    assert_eq!(
        child.resources[0].key, None,
        "computed keys must conservatively guard the whole Store"
    );
}

#[test]
fn collector_resource_plans_cover_insert_nested_calls_and_non_memory_perform() {
    let output = check(
        r#"
import std.io.read_line;
import std.task.{run, start, Scheduler};
alias Schema = MemoryRegion<{ Values: Store<string, string> }>;
let Memory = std.memory.region<Schema>(stable_id = "collector", store = "test");
flow save(key: string, value: string) -> unit {
    Memory.Values.insert(key, value);
    return;
}
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        let child = start(() => {
            save("prefix-" + read_line(), read_line());
            perform Console.stdout_write("saved");
            return 0;
        });
        return 0;
    });
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let child = checked
        .concurrency
        .resource_closures
        .values()
        .find(|plan| plan.parameters.is_empty())
        .unwrap();
    assert!(child.complete && !child.opaque_memory, "{child:#?}");
    assert_eq!(child.calls.len(), 1);
    assert_eq!(child.calls[0].arguments, vec![None, None]);
    let save = &checked.concurrency.resource_callables[&child.calls[0].callable];
    assert!(save.complete && !save.opaque_memory, "{save:#?}");
    assert_eq!(save.resources.len(), 1);
    assert!(save.resources[0].key.is_some());
}

#[test]
fn resource_scanner_visits_match_arms_and_handler_owned_task_bodies() {
    let output = check(
        r#"
import std.task.{run, start, Scheduler};
effect Register { action job() -> unit; }
alias Schema = MemoryRegion<{ Values: Store<string, string> }>;
let Memory = std.memory.region<Schema>(stable_id = "collector", store = "test");
flow append() -> i32 {
    match Some("value") {
        Some(value) => { Memory.Values.insert("key", value); }
        None() => {}
    }
    return 0;
}
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        handle { perform Register.job(); } with {
            Register.job() => {
                let child = start(() => append());
                resume ();
            }
        };
        return 0;
    });
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let child = checked
        .concurrency
        .resource_closures
        .values()
        .find(|plan| plan.parameters.is_empty())
        .expect("handler-owned child plan");
    assert!(child.complete && !child.opaque_memory, "{child:#?}");
    assert_eq!(child.calls.len(), 1);
    let append = &checked.concurrency.resource_callables[&child.calls[0].callable];
    assert!(append.complete && !append.opaque_memory, "{append:#?}");
    assert_eq!(
        append.resources.len(),
        1,
        "match-arm insert must be guarded"
    );
}

#[test]
fn inherited_memory_handlers_keep_the_opaque_fallback() {
    let output = check(
        r#"
import std.task.{run, start, Scheduler};
effect Publish { action value() -> unit; }
alias Schema = MemoryRegion<{ Values: Store<string, string> }>;
let Memory = std.memory.region<Schema>(stable_id = "handler-memory", store = "test");
flow send() -> i32 { perform Publish.value(); return 0; }
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        handle { let child = start(send); } with {
            Publish.value() => { Memory.Values.put("key", "value"); resume (); }
        };
        return 0;
    });
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.unwrap();
    assert!(
        checked
            .concurrency
            .resource_callables
            .values()
            .any(|plan| !plan.complete && plan.opaque_memory)
    );
}

#[test]
fn source_tools_and_memory_free_inference_have_resource_plans() {
    let output = check(
        r#"
alias Schema = MemoryRegion<{ Values: Store<string, string> }>;
let Memory = std.memory.region<Schema>(stable_id = "tool-memory", store = "test");
tool save(value: string) -> unit ![Memory, Error<StorageError>, Error<MemoryConflict>] {
    Memory.Values.insert(value, value);
    return;
}
agent Ask(input: string) -> string { return perform infer<string>(Prompt.new()); }
@tools([save])
agent MemoryAsk(input: string) -> string ![Memory, Error<StorageError>, Error<MemoryConflict>] {
    return perform infer<string>(Prompt.new());
}
flow main() -> unit { save("value"); return; }
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.unwrap();
    for name in ["save", "Ask", "MemoryAsk"] {
        let (item, _) = checked
            .hir
            .items
            .iter()
            .find(|(_, item)| {
                item.symbol()
                    .is_some_and(|symbol| checked.hir.symbols.get(symbol).unwrap().name == name)
            })
            .unwrap();
        let plan = &checked.concurrency.resource_callables[&item];
        if name == "MemoryAsk" {
            assert!(!plan.complete && plan.opaque_memory, "{plan:#?}");
            continue;
        }
        assert!(plan.complete && !plan.opaque_memory, "{name}: {plan:#?}");
        assert_eq!(plan.resources.len(), usize::from(name == "save"));
    }
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
                checked.hir.items[fact.owner]
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
