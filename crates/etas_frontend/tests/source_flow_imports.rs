use etas_core::SourceId;
use etas_frontend::{EntryPolicy, Frontend, ProjectInput, SourceInput};

#[test]
fn qualified_source_flow_overloads_keep_the_complete_candidate_group() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app;
import library.helpers;
flow main() -> i32 {
    let text: string = helpers.pick("answer");
    return library.helpers.pick(41);
}
"#,
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let mut dependency = SourceInput::anonymous(
        r#"
module library.helpers;
public flow pick(value: i32) -> i32 ![] { return value + 1; }
public flow pick(value: string) -> string ![] { return value + "!"; }
"#,
    );
    dependency.id = SourceId(1);
    input.sources.push(dependency);
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.checked.is_some());
}
