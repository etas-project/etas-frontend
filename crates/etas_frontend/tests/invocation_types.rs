use etas_frontend::{Frontend, ProjectInput, SourceInput};

#[test]
fn solved_implicit_rows_are_present_in_published_checked_generic_facts() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(r#"
flow twice<effect E>(f: () -> unit ![E]) -> unit ![E] { f(); f(); }
flow main() -> unit ![Console.stdout_write] { twice(() => { perform Console.stdout_write("checked callback"); return; }); }
"#)));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let fact = checked
        .types
        .generic_instantiations
        .values()
        .find(|fact| fact.effect_row_bindings.iter().any(|(name, _)| name == "E"))
        .unwrap();
    assert!(fact.deferred_effect_row_obligations.is_empty());
    let row = &fact
        .effect_row_bindings
        .iter()
        .find(|(name, _)| name == "E")
        .unwrap()
        .1;
    assert_eq!(row.tail, None);
    assert_eq!(
        row.effects
            .iter()
            .map(|effect| effect.name.as_str())
            .collect::<Vec<_>>(),
        ["Console.stdout_write"]
    );
    let main = checked.entry.unwrap();
    let summary = &checked.effects.item_effects[&main];
    assert_eq!(summary.escaping_effects.effects.iter().count(), 1);
    assert_eq!(summary.requested_actions.effects.iter().count(), 1);
}

#[test]
fn internal_generic_body_types_are_materialized_for_checked_callers() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app;
type UserId = string;
flow boxed<T>(value: T) -> bool ![] {
    let box: Option<Array<T>> = Some([value]);
    return match box { Some(_) => true, None => false, };
}
flow main() -> bool ![] { return boxed<UserId>(UserId("checked")); }
"#,
    )));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let argument = checked
        .types
        .generic_instantiations
        .values()
        .flat_map(|fact| &fact.type_bindings)
        .find_map(|(_, ty)| {
            matches!(
                checked.type_store.get(*ty),
                Some(etas_types::Type::Nominal(_))
            )
            .then_some(*ty)
        })
        .unwrap();
    assert!(checked.type_store.iter().any(|(_, ty)| match ty {
        etas_types::Type::Option(inner) =>
            checked.type_store.get(*inner) == Some(&etas_types::Type::Array(argument)),
        _ => false,
    }));
}

#[test]
fn an_unbounded_polymorphic_recursion_is_rejected_without_native_stack_exhaustion() {
    let mut source = SourceInput::anonymous(
        r#"
flow recur<T>(value: T) -> unit ![] { recur<Array<T>>([value]); }
flow main() -> unit ![] { recur<i32>(0); }
"#,
    );
    source.id = etas_core::SourceId(912);
    let output = Frontend.check_project(ProjectInput::single_source(source));
    assert!(output.checked.is_none());
    assert!(
        output
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.severity == etas_core::Severity::Error
                    && diagnostic.message.contains("expansion depth")
                    && diagnostic.code
                        == etas_core::DiagnosticCode::Type(
                            etas_core::TypeDiagnosticCode::CompilerResourceLimit
                        )
                    && diagnostic.primary.span.source == etas_core::SourceId(912)
            ),
        "{:?}",
        output.diagnostics
    );
}
