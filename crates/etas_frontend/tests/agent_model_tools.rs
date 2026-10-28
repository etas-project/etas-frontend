use etas_frontend::{Frontend, ProjectInput, SourceInput};

#[test]
fn explicit_inference_uses_solved_tool_summaries_but_pure_agent_methods_do_not() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app.main;
import std.io.println;
@tools([Emit])
agent Pure() -> string { return "no inference"; }
@tools([Emit])
agent Worker() -> string { return perform infer<string>(Prompt.new()); }
tool Emit() -> unit ![Console] { println("handled tool output")?; return; }
flow main() -> string { return Worker(); }
"#,
    )));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let item = |name: &str| {
        checked
            .hir
            .items
            .iter()
            .find_map(|(id, declaration)| {
                let symbol = match declaration {
                    etas_hir::HirItem::Agent(agent) => agent.symbol,
                    etas_hir::HirItem::Tool(tool) => tool.symbol,
                    _ => return None,
                };
                (checked.symbols.get(symbol)?.name == name).then_some(id)
            })
            .unwrap()
    };
    let pure = &checked.effects.item_effects[&item("Pure")];
    assert!(pure.requested_actions.is_empty(), "{pure:?}");
    assert_eq!(pure.support, etas_effects::InterpreterSupport::LocalOnly);
    let worker = &checked.effects.item_effects[&item("Worker")];
    let console = etas_effects::Effect::Action(etas_effects::ActionRef {
        tag: etas_effects::CONSOLE_TAG,
        action: etas_effects::CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(worker.escaping_effects.is_empty(), "{worker:?}");
    assert!(
        worker.requested_actions.effects.contains(&console),
        "{worker:?}"
    );
    assert!(
        worker.default_actions.effects.contains(&console),
        "{worker:?}"
    );
    assert!(
        worker.action_trace.action_set().contains(&console),
        "{worker:?}"
    );
    assert!(
        checked.effects.item_effects[&checked.entry.unwrap()]
            .requested_actions
            .effects
            .contains(&console)
    );
}

#[test]
fn trace_spec_checks_the_possible_tool_actions_of_explicit_inference() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app.main;
import std.io.println;
tool Emit() -> unit ![Console] { println("possible model tool request")?; return; }
@tools([Emit])
agent Worker() -> string { return perform infer<string>(Prompt.new()); }
spec Quiet: trace = -Console.stdout_write;
flow main() -> string ~ Quiet { return Worker(); }
"#,
    )));
    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Effect(etas_core::EffectDiagnosticCode::TraceSpecDenied)),
        "{:?}",
        output.diagnostics
    );
}
