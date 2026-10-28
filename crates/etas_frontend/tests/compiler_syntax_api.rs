use etas_core::{DiagnosticCode, TypeDiagnosticCode};
use etas_frontend::{Frontend, ProjectInput, SourceInput};

#[test]
fn syntax_objects_cannot_be_forged_through_constructors_or_aliases() {
    for (imports, declarations, body) in [
        (
            "import std.compiler.CallableDef;",
            "",
            "let value = CallableDef({});",
        ),
        (
            "import std.compiler.AgentDef;",
            "agent Worker() -> i32 { return 1; }",
            "let value = AgentDef<Worker>({});",
        ),
        (
            "import std.compiler.AgentDef;",
            "agent Worker() -> i32 { return 1; } alias Definition<T> = AgentDef<T>;",
            "let value = Definition<Worker>({});",
        ),
        (
            "import std.compiler.SyntaxBlock;",
            "",
            "let value = SyntaxBlock { text = \"return;\" };",
        ),
        (
            "import std.compiler.SyntaxBlock;",
            "",
            "let value = SyntaxBlock({ text = \"return;\" });",
        ),
        (
            "import std.compiler.SyntaxBlock as Block;",
            "",
            "let value = Block { text = \"return;\" };",
        ),
        (
            "import std.compiler.SyntaxBlock;",
            "alias Block = SyntaxBlock;",
            "let value = Block({ text = \"return;\" });",
        ),
    ] {
        let source = format!(
            "module main; {imports} {declarations} flow main() -> unit {{ {body} return; }}"
        );
        let output =
            Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
        assert!(
            output.diagnostics.iter().any(|diagnostic| diagnostic.code
                == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
                && diagnostic.message.contains("registered backend operations")),
            "{body}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn opaque_standard_type_names_are_not_first_class_constructor_values() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        "module main; import std.compiler.SyntaxBlock; flow main() -> unit { let constructor = SyntaxBlock; constructor({ text = \"return;\" }); return; }")));
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Type(TypeDiagnosticCode::NonCallableCallee)),
        "{:?}",
        output.diagnostics
    );
    assert!(
        output
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.phase == etas_core::DiagnosticPhase::TypeCheck)
    );
}

#[test]
fn qualified_module_paths_do_not_allow_opaque_record_construction() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        "module main; import std.compiler; flow main() -> unit { let value = compiler.SyntaxBlock { text = \"return;\" }; return; }")));
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code
                == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        output.diagnostics
    );
    assert!(
        output
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.phase == etas_core::DiagnosticPhase::TypeCheck)
    );
}

#[test]
fn syntax_text_is_a_read_only_projection_not_mutable_storage() {
    for mutation in [
        "syntax.text = \"return 2;\";",
        "holder.body.text = \"return 2;\";",
    ] {
        let source = format!(
            r#"
module main;
import std.compiler.parse_body;
flow main() -> unit {{
    match parse_body("return 1;") {{
        Ok(body) => {{ var syntax = body; var holder = {{ body }}; {mutation} }},
        Err(_) => {{}},
    }}
    return;
}}
"#
        );
        let output =
            Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
        assert!(
            output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("is read-only")),
            "{:?}",
            output.diagnostics
        );
    }
}

#[test]
fn pure_compiler_syntax_api_has_empty_effect_and_action_summaries() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(r#"
module main;
import std.compiler.parse_body;
import std.compiler.format;
flow main() -> string ![] {
    match parse_body("return unresolved_name();") {
        Ok(body) => { return format(body); },
        Err(error) => { for diagnostic in error.diagnostics { return diagnostic.message; } return "no diagnostics"; },
    }
}
"#)));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let effects = checked
        .effects
        .item_effects
        .get(&checked.entry.unwrap())
        .unwrap();
    assert!(effects.escaping_effects.is_empty());
    assert!(effects.requested_actions.is_empty());
}
