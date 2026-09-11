use etas_frontend::{Frontend, ProjectInput, SourceInput};

#[test]
fn source_enum_members_resolve_through_imports_and_visibility() {
    for public in [true, false] {
        let mut input = ProjectInput::single_source(SourceInput::anonymous(
            r#"
module app;
import data.Tree;
flow main() -> Tree<i32> { return Tree.Leaf(7); }
"#,
        ));
        let mut dependency = SourceInput::anonymous(format!(
            "module data; {} enum Tree<T> {{ Leaf(T), Branch(Tree<T>, Tree<T>), }}",
            if public { "public" } else { "" }
        ));
        dependency.id = etas_core::SourceId(1);
        input.sources.push(dependency);
        let output = Frontend.check_project(input);
        assert_eq!(output.checked.is_some(), public, "{:?}", output.diagnostics);
    }
}

#[test]
fn imported_named_constructors_patterns_and_aliases() {
    for (import, name) in [
        ("import data.Response;", "Response"),
        ("import data.Response as Reply;", "Reply"),
        ("import data.*;", "Response"),
    ] {
        let mut input = ProjectInput::single_source(SourceInput::anonymous(format!(
            r#"
module app;
{import}
flow main() -> string {{
    let value = {name}.Failure {{ code = 503, message = "unavailable" }};
    return match value {{ {name}.Success(text) => text, {name}.Failure {{ code: _, message }} => message }};
}}
"#
        )));
        let mut dependency = SourceInput::anonymous(
            "module data; public enum Response { Success(string), Failure { code: i32, message: string } }",
        );
        dependency.id = etas_core::SourceId(1);
        input.sources.push(dependency);
        let output = Frontend.check_project(input);
        assert!(
            output.checked.is_some(),
            "{import}: {:?}",
            output.diagnostics
        );
    }
}
