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
fn named_dependency_providers_keep_names_separate_from_type_fallbacks() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
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
    ));
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
