use etas_frontend::{EntryPolicy, Frontend, ProjectInput, SourceInput};

fn check(source: &str) -> etas_frontend::CheckedProgram {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.expect("checked current syntax")
}

#[test]
fn empty_nominal_declarations_construct_and_implement_methods() {
    check(
        r#"module app;
type Empty;
type Marker<T>;
impl Empty { flow number(self) -> i32 ![] { return 42; } }
flow main() -> i32 ![] {
    let marker: Marker<string> = Marker<string> {};
    return (Empty {}).number();
}"#,
    );
}

#[test]
fn record_updates_preserve_nominal_arguments_and_structural_shapes() {
    check(
        r#"module app;
type Paper<T> = { title: string, value: T, year: u32 };
flow revise<T>(paper: Paper<T>, title: string, year: u32) -> Paper<T> ![] {
    return paper { title, year };
}
flow rename(row: { title: string, year: u32 }, title: string) -> { title: string, year: u32 } ![] {
    return row { title };
}
flow main() -> Paper<i32> ![] {
    let paper = Paper<i32> { title = "draft", value = 42, year = 2025 };
    return revise(paper, "published", 2026);
}"#,
    );
}

#[test]
fn declared_effect_instance_covers_an_action_selected_by_its_owner() {
    check(
        r#"module app;
effect Registered<T> { action put(value: T) -> unit; }
flow publish<T>(value: T) -> unit ![Registered<T>] {
    perform Registered<T>.put(value);
}
flow main() -> unit ![] {
    publish<i32>(7) with { Registered<i32>.put(value) => (); };
}"#,
    );
}

#[test]
fn named_dependency_providers_keep_names_separate_from_type_fallbacks() {
    check_injection(
        r#"module std.effects;
public effect Inject<T> { action get(name: string) -> T; }
type Config = { @inject title: string, @inject author: string };
flow main() -> string ![] {
    let author = "Ada";
    let config = handle Config {} with {
        string => "fallback";
        title: string => { "Named" };
        author,
    };
    let revised = config { title = "Updated" };
    return revised.title + config.author;
}"#,
    );
}

#[test]
fn injected_constructor_result_retains_the_provided_callback() {
    check_injection(
        r#"module std.effects;
public effect Inject<T> { action get(name: string) -> T; }
spec Service { flow read(self) -> i32 ![]; }
type Reader = { run: () -> i32 ![] };
impl Reader ~ Service { flow read(self) -> i32 ![] { return (self.run)(); } }
type App<S ~ Service> = { @inject service: S };
flow make<S ~ Service>() -> App<S> { return App<S> {}; }
flow execute(a: App<Reader>) -> i32 ![] { return a.service.read(); }
flow main() -> i32 ![] {
  let a = make<Reader>() with { service: Reader => Reader { run = () => 42 }; };
  return execute(a);
}
"#,
    );
}

fn check_injection(source: &str) {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    for (index, source) in [
        "module std.runtime.budget; public type Money = { private amount: f64, private currency: string };",
        "module std.runtime.time; public type Duration = { millis: u128 };",
    ].into_iter().enumerate() {
        let mut source = SourceInput::anonymous(source);
        source.id = etas_core::SourceId(index as u32 + 1);
        input.sources.push(source);
    }
    let output = Frontend.check_standard_library_source(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.checked.is_some());
}

#[test]
fn handler_tail_values_resume_with_independent_action_and_answer_types() {
    check(
        r#"module app;
effect Choice { action choose(value: i32) -> i32; }
flow selected(value: i32) -> i32 ![] {
    let local = handler { Choice.choose(value) => value + 1; };
    return perform Choice.choose(value) with local;
}
flow main() -> string ![] {
    let result = handle { let sum = selected(20) + perform Choice.choose(20); "chosen" } with {
        Choice.choose(value) as k => {
            if value == 0 { finish "empty"; }
            if value < 0 { resume 1; }
            let twice = number => { return number * 2; };
            twice(value) + 1
        }
    };
    return "ok";
}"#,
    );
}

#[test]
fn handler_tails_keep_never_paths_and_nested_blocks() {
    check(
        r#"module app;
effect Choice { action choose() -> i32; }
effect Failure { action stop() -> never; }
flow stop() -> never ![Failure] { return perform Failure.stop(); }
flow main() -> i32 ![] {
    return handle { handle { perform Choice.choose() } with {
        Choice.choose() => {
            { let ignored = 2; ignored; };
            if false { stop() } else { 42 }
        }
    } } with {
        Failure.stop() => finish 0;
    };
}"#,
    );
}
