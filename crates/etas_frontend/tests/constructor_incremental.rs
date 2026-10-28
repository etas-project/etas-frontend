use etas_core::SourceId;
use etas_frontend::{
    CheckRequest, FrontendSession, FrontendSessionOptions, ProjectChangeSet, ProjectInput,
    ProjectRevision, SourceChange, SourceInput, SourceVersion,
};

#[test]
fn changing_only_a_nominal_constructor_bound_cannot_reuse_a_successful_body() {
    for constructor in [
        "Bounded<Value>({ value = Value(42) })",
        "Bounded<Value> { value = Value(42) }",
    ] {
        let source = format!(
            r#"
module app;
spec Allowed;
spec Blocked;
type Value = i32;
impl Value ~ Allowed;
type Bounded<T ~ Allowed> = {{ value: T }};
flow main() -> Bounded<Value> {{ return {constructor}; }}
"#
        );
        assert_edit_rejected(
            &source,
            "Bounded<T ~ Allowed>",
            "Bounded<T ~ Blocked>",
            "spec",
        );
        assert_edit_rejected(
            &source,
            "impl Value ~ Allowed;",
            "impl Value ~ Blocked;",
            "spec",
        );
    }
}

#[test]
fn changing_enum_bound_evidence_invalidates_a_cached_nullary_constructor() {
    let source = r#"
module app;
spec Allowed;
spec Blocked;
type Value = i32;
impl Value ~ Allowed;
enum Bounded<T ~ Allowed> { Empty, Item(T), }
flow main() -> Bounded<Value> { return Bounded.Empty; }
"#;
    assert_edit_rejected(
        source,
        "Bounded<T ~ Allowed>",
        "Bounded<T ~ Blocked>",
        "spec",
    );
    assert_edit_rejected(
        source,
        "impl Value ~ Allowed;",
        "impl Value ~ Blocked;",
        "spec",
    );
}

#[test]
fn changing_only_an_alias_own_bound_invalidates_cached_call_and_record_constructors() {
    for constructor in [
        "Restricted<Value>({ value = Value(42) })",
        "Restricted { value = Value(42) }",
    ] {
        let source = format!(
            r#"
module app;
spec Allowed;
spec Blocked;
type Value = i32;
impl Value ~ Allowed;
type Wrapper<T> = {{ value: T }};
alias Restricted<T ~ Allowed> = Wrapper<T>;
flow main() -> Wrapper<Value> {{ return {constructor}; }}
"#
        );
        assert_edit_rejected(
            &source,
            "Restricted<T ~ Allowed>",
            "Restricted<T ~ Blocked>",
            "spec",
        );
        assert_edit_rejected(
            &source,
            "impl Value ~ Allowed;",
            "impl Value ~ Blocked;",
            "spec",
        );
    }
}

#[test]
fn renaming_only_an_enum_payload_field_invalidates_its_cached_constructor() {
    assert_edit_rejected(
        r#"
module app;
enum Reply { Named { status: i32 }, }
flow main() -> Reply { return Reply.Named { status = 42 }; }
"#,
        "Named { status: i32 }",
        "Named { result: i32 }",
        "field",
    );
}

fn assert_edit_rejected(source: &str, from: &str, to: &str, expected: &str) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    for disk in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "etas-constructor-cache-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let input = ProjectInput::single_source(SourceInput::anonymous(source));
        let options = if disk {
            FrontendSessionOptions::disk_cache(&root)
        } else {
            FrontendSessionOptions::memory_only()
        };
        let mut session = FrontendSession::with_options(options).unwrap();
        let mut project = session.open_project(input.clone());
        let first = session
            .check(project, CheckRequest::full_project())
            .unwrap();
        assert!(
            first.output.diagnostics.is_empty(),
            "{:?}",
            first.output.diagnostics
        );
        assert!(first.output.checked.is_some());
        if disk {
            let index = etas_frontend::FrontendArtifactKey::project(
                etas_frontend::FrontendArtifactKind::BodyArtifactReuseIndex,
            )
            .to_cache_key();
            assert!(first.cache.stored_artifacts.contains(&index));
            drop(session);
            session =
                FrontendSession::with_options(FrontendSessionOptions::disk_cache(&root)).unwrap();
            project = session.try_open_project(input).unwrap();
        }
        let modified = source.replace(from, to);
        assert_eq!(source.len(), modified.len());
        session
            .apply_changes(
                project,
                ProjectChangeSet {
                    revision: ProjectRevision(1),
                    source_changes: vec![SourceChange::Replace {
                        source: SourceId(0),
                        version: SourceVersion(1),
                        text: modified,
                    }],
                    dependency_overlay_changes: vec![],
                    environment_change: None,
                    option_changes: vec![],
                },
            )
            .unwrap();
        let next = session.check(project, CheckRequest::incremental()).unwrap();
        assert!(
            next.output.checked.is_none(),
            "cached body bypassed the edit {from} -> {to}"
        );
        assert!(
            next.output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(expected)),
            "{:?}",
            next.output.diagnostics
        );
        drop(session);
        if disk {
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
