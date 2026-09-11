fn check(source: &str) -> etas_types::TypeOutput {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(9),
        None,
        source,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    etas_types::check_program(&hir)
}

#[test]
fn storage_outcome_constructors_preserve_their_enum_and_generic_identity() {
    let output = check(
        r#"
flow classify(value: WriteOutcome<string, i32>) -> string {
    return match value {
        WriteOutcome.Committed(receipt) => receipt,
        WriteOutcome.NotCommitted(_, _) => "rejected",
        WriteOutcome.Unknown(_) => "unknown",
    };
}
flow confirmed(value: ConfirmedOutcome<string, i32>) -> string {
    return match value {
        ConfirmedOutcome.Committed(receipt) => receipt,
        ConfirmedOutcome.NotCommitted(_, _) => "rejected",
    };
}
flow found(value: ReconcileResult<string, i32>) -> string {
    return match value {
        ReconcileResult.Found(receipt) => confirmed(receipt),
        ReconcileResult.Unresolved() => "unresolved",
        ReconcileResult.Expired() => "expired",
    };
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn storage_confirmation_rejects_uncertain_or_wrong_generic_outcomes() {
    for source in [
        "flow bad(x: WriteOutcome<string, i32>) -> ConfirmedOutcome<string, i32> { return x; }",
        "flow bad(x: WriteOutcome<string, i32>) -> ReconcileResult<string, i32> { return ReconcileResult.Found(x); }",
        "flow bad(x: ConfirmedOutcome<string, i32>) -> string { return match x { WriteOutcome.Committed(v) => v, _ => \"no\", }; }",
        "flow bad(x: ConfirmedOutcome<string, i32>) -> string { return match x { ConfirmedOutcome.Unknown(_) => \"unknown\", _ => \"no\", }; }",
        "flow bad(x: ConfirmedOutcome<string, i32>) -> ReconcileResult<i32, string> { return ReconcileResult.Found(x); }",
        "flow bad(v: MemoryVersion) -> WriteCondition { return WriteOutcome.Match(v); }",
        "flow bad(v: MemoryTombstone) -> MemoryVersion { return v; }",
        "flow bad(v: MemoryTombstone) -> WriteCondition { return Match(v); }",
        "flow bad(v: MemoryTombstone) -> MemoryWriteChange { return MemoryWriteChange.Written(v); }",
    ] {
        let output = check(source);
        assert!(
            !output.diagnostics.is_empty(),
            "invalid outcome accepted: {source}"
        );
    }
}
