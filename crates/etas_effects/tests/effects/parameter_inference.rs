use super::*;

fn analyze(source: &str) -> (etas_hir::HirProgram, etas_effects::EffectOutput) {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        source,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    (hir, output)
}

#[test]
fn inferred_callback_behavior_is_interpreted_inside_the_callee_handler() {
    let (hir, output) = analyze(
        r#"
effect Probe extends Network { action read() -> i32; }
flow invoke(callback: unit -> i32) -> i32 {
    return handle { callback() } with { Probe.read() => { resume 7; } };
}
flow main() -> i32 ![] {
    return invoke(() => perform Probe.read());
}
"#,
    );
    let invoke = flow_item(&hir, "invoke");
    let main = flow_item(&hir, "main");
    let HirItem::Flow(flow) = &hir.items[invoke] else {
        panic!("invoke must be a flow")
    };
    let inferred = &output.facts.inferred_parameter_effects[&flow.params[0]];
    assert_eq!(inferred.requested_actions.effects.iter().count(), 1);
    assert!(
        inferred
            .escaping_effects
            .effects
            .iter()
            .any(is_action_effect)
    );
    for item in [invoke, main] {
        let summary = &output.facts.item_effects[&item];
        assert!(summary.escaping_effects.effects.is_empty(), "{summary:?}");
        assert_eq!(summary.requested_actions, inferred.requested_actions);
        assert_eq!(summary.handled_actions, inferred.requested_actions);
    }
}

#[test]
fn inferred_callback_trace_retains_each_invocation() {
    let (hir, output) = analyze(
        r#"
effect Probe extends Network { action read() -> i32; }
flow twice(callback: unit -> i32) -> i32 {
    let first = callback();
    return first + callback();
}
flow main() -> i32 ![Network] { return twice(() => perform Probe.read()); }
"#,
    );
    for name in ["twice", "main"] {
        let summary = &output.facts.item_effects[&flow_item(&hir, name)];
        let ActionTraceDomain::Seq(events) = &summary.action_trace else {
            panic!(
                "two calls must preserve an ordered trace: {:?}",
                summary.action_trace
            );
        };
        assert_eq!(events.len(), 2);
        assert!(
            events
                .iter()
                .all(|event| matches!(event, ActionTraceDomain::Event(_)))
        );
        assert_eq!(summary.requested_actions.effects.iter().count(), 1);
        assert!(
            summary
                .escaping_effects
                .effects
                .iter()
                .any(is_action_effect)
        );
    }
}

#[test]
fn callback_inference_propagates_through_more_than_sixteen_parameter_bindings() {
    let mut source = String::from("effect Probe extends Network { action ping() -> unit; }\n");
    for index in 0..24 {
        let call = if index == 0 {
            "callback()".into()
        } else {
            format!("layer{}(callback)", index - 1)
        };
        source.push_str(&format!(
            "flow layer{index}(callback: unit -> unit) -> unit {{ {call}; return; }}\n"
        ));
    }
    source.push_str("flow main() -> unit ![Network] { layer23(() => { perform Probe.ping(); return; }); return; }");
    let (hir, output) = analyze(&source);
    assert_eq!(output.facts.inferred_parameter_effects.len(), 24);
    for index in 0..24 {
        let summary = &output.facts.item_effects[&flow_item(&hir, &format!("layer{index}"))];
        assert_eq!(summary.requested_actions.effects.iter().count(), 1);
        assert!(
            summary
                .escaping_effects
                .effects
                .iter()
                .any(is_action_effect)
        );
    }
}
