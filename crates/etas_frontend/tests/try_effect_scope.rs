use etas_effects::Effect;
use etas_frontend::{Frontend, ProjectInput, SourceInput};

#[test]
fn try_does_not_capture_the_same_error_from_a_preceding_statement() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app;
import std.io.{println, IOError};
flow main() -> unit ![Error<IOError>] {
    println("not captured");
    println("captured")?;
    return;
}
"#,
    )));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    assert_eq!(checked.effects.try_captures.len(), 1);
    let error = checked
        .effects
        .try_captures
        .values()
        .next()
        .unwrap()
        .captured_error;
    let etas_types::ItemSignature::Flow(signature) =
        &checked.types.item_signatures[&checked.entry.unwrap()]
    else {
        panic!("checked entry signature")
    };
    let declared = signature
        .effects
        .as_ref()
        .unwrap()
        .effects
        .iter()
        .find(|effect| effect.name == "Error")
        .unwrap();
    assert_eq!(declared.args, [etas_types::EffectArgRef::Type(error)]);
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert!(
        summary
            .escaping_effects
            .effects
            .contains(&Effect::Error(error)),
        "{summary:#?}"
    );
}

#[test]
fn try_selects_only_the_operand_error_after_a_different_fallible_call() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app;
import std.compiler.{compile, CompileError};
agent Worker() -> unit { return; }
flow main() -> unit ![Error<CompileError>] {
    let revised = compile(^Worker);
    let values = [1];
    let captured = values[0]?;
    return;
}
"#,
    )));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    let captures = checked.effects.try_captures.values().collect::<Vec<_>>();
    assert_eq!(captures.len(), 1);
    assert_eq!(
        Some(captures[0].captured_error),
        checked.types.known_std_types.index_error
    );
    assert!(
        !summary
            .escaping_effects
            .effects
            .contains(&Effect::Error(captures[0].captured_error)),
        "{summary:#?}"
    );
    let errors = summary
        .escaping_effects
        .effects
        .iter()
        .filter(|effect| matches!(effect, Effect::Error(_)))
        .count();
    assert_eq!(
        errors, 1,
        "the preceding CompileError must remain: {summary:#?}"
    );
}
