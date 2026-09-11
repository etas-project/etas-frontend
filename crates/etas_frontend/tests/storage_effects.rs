use etas_effects::{Effect, MEMORY_READ_ACTION, MEMORY_TAG};
use etas_frontend::{CheckRequest, FrontendSession, ProjectInput, SourceInput};

#[test]
fn paged_memory_intrinsics_use_the_checked_store_selector_through_alias_imports() {
    for (name, args) in [
        ("get_entry", "Memory.Papers, \"key\""),
        ("page", "Memory.Papers, None(), 1"),
    ] {
        let mut session = FrontendSession::new();
        let source = format!(
            r#"
module app.main;
import std.memory.{name} as read_store;
alias Schema = MemoryRegion<{{ Papers: Store<string, string> }}>;
let Memory = std.memory.region<Schema>(stable_id = "paging-effects", store = "test");
flow main() -> unit {{
    read_store({args});
}}
"#
        );
        let project =
            session.open_project(ProjectInput::single_source(SourceInput::anonymous(source)));
        let response = session
            .check(project, CheckRequest::full_project())
            .unwrap();
        let checked = response
            .output
            .checked
            .as_ref()
            .unwrap_or_else(|| panic!("{name}: {:?}", response.diagnostics));
        let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
        let actions = summary
            .requested_actions
            .effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::AppliedAction(action) if action.action.tag == MEMORY_TAG => Some(action),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(actions.len(), 1, "{name}: {summary:?}");
        assert_eq!(actions[0].action.action, MEMORY_READ_ACTION);
        assert_eq!(
            actions[0].args,
            vec![etas_types::EffectArgRef::Path(
                ["app", "main", "Memory", "Papers"]
                    .map(str::to_owned)
                    .to_vec()
            )],
            "{name}: no placeholder or broad selector is permitted"
        );
    }
}

#[test]
fn memory_selector_survives_local_alias_and_source_wrappers() {
    for expression in [
        "let selected = Memory.Papers; read_store(selected, \"key\");",
        "let selected = identity(Memory.Papers); read_store(selected, \"key\");",
        "read_store(identity(Memory.Papers), \"key\");",
        "read_wrapper(Memory.Papers);",
    ] {
        let mut session = FrontendSession::new();
        let source = format!(
            r#"
module app.main;
import std.memory.get_entry as read_store;
alias Schema = MemoryRegion<{{ Papers: Store<string, string> }}>;
let Memory = std.memory.region<Schema>(stable_id = "provenance", store = "test");
flow identity(store: Store<string, string>) -> Store<string, string> {{ return store; }}
flow read_wrapper(store: Store<string, string>) -> unit {{ read_store(store, "key"); }}
flow main() -> unit {{ {expression} }}
"#
        );
        let project =
            session.open_project(ProjectInput::single_source(SourceInput::anonymous(source)));
        let response = session
            .check(project, CheckRequest::full_project())
            .unwrap();
        let checked = response
            .output
            .checked
            .as_ref()
            .unwrap_or_else(|| panic!("{expression}: {:?}", response.diagnostics));
        let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
        assert_eq!(
            summary.requested_actions.effects.iter().count(),
            1,
            "{expression}: {summary:?}"
        );
        let effect = summary.requested_actions.effects.iter().next().unwrap();
        let Effect::AppliedAction(action) = effect else {
            panic!("unscoped action: {effect:?}")
        };
        assert_eq!(action.action.tag, MEMORY_TAG);
        assert_eq!(
            action.args,
            vec![etas_types::EffectArgRef::Path(
                ["app", "main", "Memory", "Papers"]
                    .map(str::to_owned)
                    .to_vec()
            )]
        );
    }
}

#[test]
fn selected_store_preserves_alternative_actions_without_inventing_a_sequence() {
    let mut session = FrontendSession::new();
    let source = r#"
module app.main;
import std.memory.get_entry;
alias Schema = MemoryRegion<{ Papers: Store<string, string>, Notes: Store<string, string> }>;
let Memory = std.memory.region<Schema>(stable_id = "choice", store = "test");
flow choose(first: Store<string, string>, second: Store<string, string>, flag: bool) -> Store<string, string> {
    if flag { return first; }
    return second;
}
flow main() -> unit {
    let selected = choose(Memory.Papers, Memory.Notes, true);
    get_entry(selected, "key");
}
"#;
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    let response = session
        .check(project, CheckRequest::full_project())
        .unwrap();
    let checked = response
        .output
        .checked
        .as_ref()
        .unwrap_or_else(|| panic!("{:?}", response.diagnostics));
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert_eq!(
        summary.requested_actions.effects.iter().count(),
        2,
        "{summary:?}"
    );
    let etas_effects::ActionTraceDomain::Choice(branches) = &summary.action_trace else {
        panic!("alternative stores must not produce a sequential/wildcard trace: {summary:?}")
    };
    assert_eq!(branches.len(), 2);
    let mut stores = Vec::new();
    for branch in branches {
        let etas_effects::ActionTraceDomain::Event(event) = branch else {
            panic!("{branch:?}")
        };
        let Effect::AppliedAction(action) = &event.action else {
            panic!("{event:?}")
        };
        assert_eq!(action.action.tag, MEMORY_TAG);
        assert_eq!(action.action.action, MEMORY_READ_ACTION);
        let [etas_types::EffectArgRef::Path(path)] = action.args.as_slice() else {
            panic!("{action:?}")
        };
        assert_eq!(&path[..3], ["app", "main", "Memory"]);
        stores.push(path[3].as_str());
    }
    stores.sort();
    assert_eq!(stores, ["Notes", "Papers"]);
}
