use super::*;

#[test]
fn an_inference_variable_cannot_prove_its_own_row_binding() {
    check_scopes(&[], false);
}

#[test]
fn a_checked_lexical_row_is_a_distinct_argument_with_the_same_spelling() {
    check_scopes(&["E".into()], true);
}

#[test]
fn an_unrelated_lexical_binder_does_not_resolve_a_self_binding() {
    check_scopes(&["Other".into()], false);
}

fn check_scopes(lexical: &[String], bound: bool) {
    let mut types = crate::TypeInterner::new();
    let unit = types.primitive(crate::PrimitiveType::Unit);
    let row = EffectRowRef {
        effects: vec![],
        tail: Some("E".into()),
    };
    let function = types.intern(Type::Function(crate::FlowType {
        input: vec![],
        output: unit,
        effects: Some(row.clone()),
    }));
    let mut bindings = HashMap::new();
    let inferred = infer_scoped_bindings_from_types(
        types.store(),
        function,
        function,
        &HashMap::new(),
        &Substitution::default(),
        EffectRowParameterScopes {
            inferred: &["E".into()],
            lexical,
        },
        &mut bindings,
    );
    assert!(inferred.matches);
    if bound {
        assert!(inferred.deferred.is_empty());
        assert_eq!(bindings.get("E"), Some(&row));
    } else {
        assert_eq!(inferred.deferred, ["E".into()].into());
        assert!(bindings.is_empty());
    }
}
