use etas_effects::{ActionRef, CONSOLE_STDOUT_WRITE_ACTION, CONSOLE_TAG, Effect};
use etas_frontend::{Frontend, ProjectInput, SourceInput};

#[test]
fn nominal_agent_values_keep_the_checked_default_method_effects() {
    for body in [
        "let worker = Worker; worker();",
        "invoke(Worker);",
        "let worker = compile(^Worker); worker();",
    ] {
        let source = format!(
            r#"
module app;
import std.compiler.{{compile, CompileError}};
agent Worker() -> unit ![Console.stdout_write] {{
    perform Console.stdout_write("checked");
}}
flow invoke(worker: Worker) -> unit ![Console.stdout_write] {{ worker(); }}
flow main() -> unit ![Console.stdout_write, Error<CompileError>] {{ {body} }}
"#
        );
        let output =
            Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
        assert!(
            output.diagnostics.is_empty(),
            "{body}: {:?}",
            output.diagnostics
        );
        let checked = output.checked.unwrap();
        let stdout = Effect::Action(ActionRef {
            tag: CONSOLE_TAG,
            action: CONSOLE_STDOUT_WRITE_ACTION,
        });
        let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
        assert!(
            summary.escaping_effects.effects.contains(&stdout),
            "{body}: {summary:#?}"
        );
        assert!(
            summary.requested_actions.effects.contains(&stdout),
            "{body}: {summary:#?}"
        );
        let calls = checked
            .types
            .agent_method_calls
            .iter()
            .filter(|(call, _)| {
                matches!(
                    checked.hir.exprs.get(**call),
                    Some(etas_hir::HirExpr::Call { .. })
                )
            })
            .count();
        assert!(
            calls > 0,
            "{body}: nominal invocation lacks its checked method identity"
        );
    }
}

#[test]
fn compiled_agent_callbacks_keep_requested_actions_even_with_an_empty_escaping_row() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app;
import std.compiler.{compile, CompileError};
import std.io.println;
agent Worker(callback: () -> unit ![]) -> unit { callback(); }
flow main() -> unit ![Error<CompileError>] {
    let revised = compile(^Worker);
    revised(() => { println("handled")?; return; });
}

"#,
    )));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let stdout = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: CONSOLE_STDOUT_WRITE_ACTION,
    });
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert!(
        summary.requested_actions.effects.contains(&stdout),
        "{summary:#?}"
    );
    assert!(
        summary.default_actions.effects.contains(&stdout),
        "{summary:#?}"
    );
    assert!(
        !summary.escaping_effects.effects.contains(&stdout),
        "{summary:#?}"
    );
    assert!(
        summary.action_trace.action_set().contains(&stdout),
        "{summary:#?}"
    );
}

#[test]
fn generic_callable_aliases_keep_their_instantiated_error_summary() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app;
import std.compiler.{compile, CompileError};
type UserId = string;
flow fail<T>(value: T) -> unit ![Error<T>] { perform Error<T>.raise(value); }
agent Worker(callback: (UserId) -> unit ![Error<UserId>], value: UserId) -> unit ![Error<UserId>] {
    callback(value);
}
flow main() -> unit ![Error<CompileError>, Error<UserId>] {
    let revised = compile(^Worker);
    let callback: (UserId) -> unit ![Error<UserId>] = fail;
    revised(callback, UserId("checked"));
}
"#,
    )));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let subject = checked
        .types
        .symbol_types
        .iter()
        .find_map(|(symbol, fact)| {
            if checked.symbols.get(*symbol)?.name != "UserId" {
                return None;
            }
            match fact {
                etas_types::SymbolTypeFact::NominalType { constructor, .. } => {
                    Some(etas_types::TypeId(constructor.0))
                }
                _ => None,
            }
        })
        .unwrap();
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert!(
        summary
            .escaping_effects
            .effects
            .contains(&Effect::Error(subject)),
        "{summary:#?}"
    );
    for row in [&summary.escaping_effects, &summary.requested_actions] {
        for effect in row.effects.iter() {
            if let Effect::Error(ty) = effect {
                assert!(!matches!(
                    checked.type_store.get(*ty),
                    Some(etas_types::Type::Named(_)) | Some(etas_types::Type::Var(_))
                ));
            }
        }
    }
}

#[test]
fn handled_generic_callbacks_keep_requests_without_reintroducing_escaping_errors() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app;
import std.io.{println, IOError};
flow recover<T>(callback: (T) -> unit ![Error<IOError>], value: T) -> unit {
    handle { callback(value); } with { Error<IOError>.raise(_) => { finish (); } };
}
flow main() -> unit {
    recover((value: string) => { println(value); }, "handled");
}
"#,
    )));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    let stdout = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(summary.escaping_effects.effects.is_empty(), "{summary:#?}");
    assert!(
        summary.requested_actions.effects.contains(&stdout),
        "{summary:#?}"
    );
    assert!(
        summary.default_actions.effects.contains(&stdout),
        "{summary:#?}"
    );
    assert!(
        summary.action_trace.action_set().contains(&stdout),
        "{summary:#?}"
    );
}
