use std::sync::Arc;

use etas_core::{SourceFile, SourceId};
use etas_hir::{HirModuleId, HirProgram, HirRevision, HirTreeIndex, lower_program};

fn source(text: &str) -> HirProgram {
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(7), None, text));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    lower_program(&parsed.value)
}

#[test]
fn immutable_revisions_share_indexes_and_edit_without_changing_previous_views() {
    let before = HirTreeIndex::thread_build_count();
    let original =
        HirRevision::try_new(source("module main; flow main() -> i32 { return 1; }")).unwrap();
    let shared = original.clone();
    let view = original.view();
    assert!(Arc::ptr_eq(
        original.shared_program(),
        shared.shared_program()
    ));
    assert!(Arc::ptr_eq(original.shared_index(), shared.shared_index()));
    assert!(std::ptr::eq(view.index(), shared.view().index()));
    assert_eq!(HirTreeIndex::thread_build_count() - before, 1);
    let previous_items = original.items.len();

    let mut editor = shared.edit();
    *editor.program_mut() = source(
        "module main; flow helper() -> i32 { return 2; } flow main() -> i32 { return helper(); }",
    );
    let edited = editor.finish().unwrap();
    assert!(!Arc::ptr_eq(
        original.shared_program(),
        edited.shared_program()
    ));
    assert!(!Arc::ptr_eq(original.shared_index(), edited.shared_index()));
    assert_eq!(original.items.len(), previous_items);
    assert!(edited.items.len() > previous_items);
    assert_eq!(
        view.modules().flat_map(|module| module.items()).count(),
        previous_items
    );
    assert_eq!(
        edited
            .view()
            .modules()
            .flat_map(|module| module.items())
            .count(),
        edited.items.len()
    );
    assert_eq!(HirTreeIndex::thread_build_count() - before, 2);
}

#[test]
fn invalid_edits_cannot_publish_a_revision_or_modify_the_previous_one() {
    let original =
        HirRevision::try_new(source("module main; flow main() -> unit { return; }")).unwrap();
    let mut editor = original.clone().edit();
    editor.program_mut().modules.push(HirModuleId(u32::MAX));
    assert!(editor.finish().is_err());
    assert_eq!(original.modules.len(), 1);
    assert_eq!(original.view().modules().count(), 1);
}

#[test]
fn mutating_a_detached_program_handle_preserves_the_frozen_pair() {
    let original =
        HirRevision::try_new(source("module main; flow main() -> unit { return; }")).unwrap();
    let mut detached = original.shared_program().clone();
    Arc::make_mut(&mut detached).modules.clear();
    assert_eq!(original.modules.len(), 1);
    assert_eq!(original.view().modules().count(), 1);
    assert!(!Arc::ptr_eq(original.shared_program(), &detached));
}
