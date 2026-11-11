use super::*;

#[test]
fn overriding_infer_keeps_an_independent_step_of_the_same_method() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.io.println;
tool write() -> unit ![] { println("tool")?; }
@tools([write])
agent Writer(schema: Schema<string>) -> string {
    let history = std.agent.conversation.from_prompt(Prompt.new());
    let first = std.agent.step(history, schema);
    return perform infer<string>(Prompt.new());
}
flow main(schema: Schema<string>) -> string {
    return Writer.run(schema) with {
        Agentic.infer<_, string>(_, _) => resume "mock";
    };
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    let summary = &output.facts.item_effects[&main];
    assert_eq!(summary.requested_actions.effects.len(), 2, "{summary:#?}");
    assert_eq!(summary.default_actions.effects.len(), 1, "{summary:#?}");
    assert_eq!(summary.determinism, Determinism::NonDeterministic);
    assert_eq!(
        summary.action_trace.action_set(),
        summary.requested_actions.effects
    );
    assert!(!summary.action_trace.has_parameter_calls());
    assert!(summary.escaping_effects.is_empty());
}

#[test]
fn single_step_is_nondeterministic_without_executing_exposed_tools() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.io.println;
tool write() -> unit ![] { println("tool")?; }
@tools([write])
agent StepOnly(schema: Schema<string>) -> unit {
    let history = std.agent.conversation.from_prompt(Prompt.new());
    let outcome = std.agent.step(history, schema);
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let item = hir
        .items
        .iter()
        .find_map(|(id, item)| matches!(item, HirItem::Agent(_)).then_some(id))
        .unwrap();
    let summary = &output.facts.item_effects[&item];
    assert_eq!(summary.determinism, Determinism::NonDeterministic);
    assert!(summary.escaping_effects.is_empty());
    let actions = summary.requested_actions.effects.iter().collect::<Vec<_>>();
    assert_eq!(actions.len(), 1, "{summary:#?}");
    let descriptor = test_std_registry()
        .intrinsic(etas_std::StdIntrinsicId(
            etas_std::intrinsic::runtime::AGENT_STEP,
        ))
        .unwrap();
    let etas_std::LoweringHint::DefaultAction { stable_id, .. } = descriptor.lowering else {
        panic!("step must have its checked default action descriptor");
    };
    assert!(matches!(actions[0], Effect::AppliedAction(instance)
        if instance.action.action == etas_effects::EffectActionId(stable_id)));
    assert_eq!(summary.default_actions, summary.requested_actions);
    assert_eq!(
        summary.action_trace.action_set(),
        summary.requested_actions.effects
    );
}
