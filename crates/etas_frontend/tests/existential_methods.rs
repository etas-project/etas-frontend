use etas_frontend::{Frontend, ProjectInput, SourceInput};

const SOURCE: &str = r#"
spec Named { flow name(self) -> string ![Console.stdout_write]; }
type Label = string;
impl Label ~ Named {
    flow name(self) -> string ![Console.stdout_write] {
        perform Console.stdout_write("method\n");
        return "label";
    }
}
flow read(value: ? ~ Named) -> string ![Console.stdout_write] { return value.name(); }
flow main() -> usize ![Console.stdout_write] {
    let value: ? ~ Named = Label("example");
    return read(value).len();
}
"#;

#[test]
fn heterogeneous_objects_retain_both_method_bodies_through_iteration() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        include_str!("fixtures/method_resolution/existential_sequence.es"),
    )));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert!(summary.escaping_effects.effects.is_empty());
    assert_eq!(summary.requested_actions.effects.len(), 1);
    assert_eq!(checked.types.existential_packs.len(), 2);
    let targets = checked
        .types
        .existential_packs
        .values()
        .flat_map(|pack| &pack.witnesses)
        .map(|witness| match witness.target {
            etas_types::method::MethodTarget::Source { callable, .. } => callable,
            _ => panic!("source witness"),
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(targets.len(), 2);
    for target in targets {
        assert!(
            checked
                .effects
                .native_call_targets
                .values()
                .any(|targets| targets.contains(&etas_effects::EffectUnit::Item(target)))
        );
    }
}

#[test]
fn existential_method_invocation_keeps_the_implementation_actions_and_trace() {
    let generic = SOURCE
        .replace(
            "flow main()",
            "flow pack<T ~ Named>(value: T) -> ? ~ Named ![] { return value; }\nflow main()",
        )
        .replace(
            "let value: ? ~ Named = Label(\"example\");",
            "let value = pack(Label(\"example\"));",
        );
    for source in [SOURCE.to_owned(), generic] {
        let output =
            Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let checked = output.checked.unwrap();
        let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
        assert_eq!(summary.requested_actions.effects.len(), 1);
        assert_eq!(summary.escaping_effects.effects.len(), 1);
        assert!(
            matches!(
                summary.action_trace,
                etas_effects::ActionTraceDomain::Event(_)
            ),
            "{:?}",
            summary.action_trace
        );
        let callable = checked
            .types
            .methods
            .entries()
            .find_map(|entry| match entry.target {
                etas_types::method::MethodTarget::Source { callable, .. }
                    if matches!(
                        entry.evidence,
                        etas_types::method::MethodEvidence::Implementation { .. }
                    ) =>
                {
                    Some(callable)
                }
                _ => None,
            })
            .unwrap();
        assert!(
            checked
                .effects
                .native_call_targets
                .values()
                .any(|targets| targets.contains(&etas_effects::EffectUnit::Item(callable)))
        );
    }
}
