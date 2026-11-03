use super::*;

#[test]
fn mutually_exclusive_resume_branches_are_single_use() {
    for control in [
        "if cached { resume 1; } else { resume 2; }",
        "if cached { if other { resume 1; } else { resume 2; } } else { resume 3; }",
        "match cached { true => { resume 1; }, false => { resume 2; } }",
        "if cached { resume 1; } else { finish 2; }",
        "let nested = handler { Solver.ask() => { resume 9; } }; resume 1;",
        "if cached { resume 1; } resume 2;",
        "if cached { finish 1; } resume 2;",
        "while cached limit Iterations(2) { resume 1; } resume 2;",
        "match cached { true => { resume 1; }, false => {} } resume 2;",
        "if cached { resume 1; resume 2; } else { resume 3; }",
    ] {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(format!(
            r#"
module app;
effect Solver {{ action ask() -> i32; }}
flow main(cached: bool, other: bool) -> i32 {{
    return perform Solver.ask() with {{ Solver.ask() => {{ {control} }} }};
}}
"#
        )));
        assert!(
            parsed.diagnostics.is_empty(),
            "{control}: {:?}",
            parsed.diagnostics
        );
        let hir = lower_program(&parsed.value);
        let types = etas_types::check_program(&hir);
        assert!(
            types.diagnostics.is_empty(),
            "{control}: {:?}",
            types.diagnostics
        );
        let output = check_program(&hir, &types);
        assert!(
            output.diagnostics.is_empty(),
            "{control}: {:?}",
            output.diagnostics
        );
        assert!(
            output
                .facts
                .handler_values
                .values()
                .flat_map(|value| &value.arms)
                .all(|arm| arm.resumes == ResumeSummary::Once)
        );
    }
}

#[test]
fn handler_control_rejects_missing_completion() {
    for control in [
        "if cached { resume 1; }",
        "if cached { resume 1; } else { let value = 2; }",
    ] {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(format!(
            r#"
module app;
effect Solver {{ action ask() -> i32; }}
flow main(cached: bool) -> i32 {{
    return perform Solver.ask() with {{ Solver.ask() => {{ {control} }} }};
}}
"#
        )));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let types = etas_types::check_program(&hir);
        assert!(
            types.diagnostics.iter().any(|diagnostic| diagnostic.code
                == DiagnosticCode::Type(etas_core::TypeDiagnosticCode::HandlerCompletionRequired)),
            "{control}: {:?}",
            types.diagnostics
        );
    }
}
