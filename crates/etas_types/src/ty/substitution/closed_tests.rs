use super::*;
use crate::{NamedTypeRef, PrimitiveType, TypeVarId};

#[test]
fn closed_arguments_reject_unbound_types_through_the_complete_structural_graph() {
    let mut interner = TypeInterner::new();
    let parameter = interner.intern(Type::Named(NamedTypeRef {
        name: "Payload".into(),
        source_parameter: None,
    }));
    let variable = interner.intern(Type::Var(TypeVarId(0)));
    let array = interner.intern(Type::Array(parameter));
    let optional = interner.intern(Type::Option(array));
    let record = interner.intern(Type::Record(RecordType {
        fields: vec![FieldType {
            name: "items".into(),
            ty: optional,
        }],
    }));
    let function = interner.intern(Type::Function(FlowType {
        input: vec![record],
        output: variable,
        effects: Some(EffectRowRef {
            effects: vec![],
            tail: None,
        }),
    }));
    let store = interner.into_store();
    let names = HashMap::new();
    let known = |_| false;
    let mut engine =
        TypeSubstitutionEngine::existing(&store, &names).require_closed_parameters(&known);
    for ty in [parameter, array, optional, record, function, variable] {
        assert!(matches!(
            engine.substitute(ty),
            Err(TypeSubstitutionError::UnresolvedParameter(_))
        ));
    }
    // Declaration/template substitution is unchanged outside a runtime contract.
    assert_eq!(
        TypeSubstitutionEngine::existing(&store, &names).substitute(function),
        Ok(function)
    );
}

#[test]
fn closed_arguments_check_replacements_in_the_callers_namespace() {
    let mut interner = TypeInterner::new();
    let parameter = interner.intern(Type::Named(NamedTypeRef {
        name: "T".into(),
        source_parameter: None,
    }));
    let argument = interner.intern(Type::Array(parameter));
    let concrete = interner.primitive(PrimitiveType::I32);
    let concrete_array = interner.intern(Type::Array(concrete));
    let store = interner.into_store();
    let known = |_| false;
    let unresolved = HashMap::from([("T".into(), argument)]);
    assert_eq!(
        TypeSubstitutionEngine::existing(&store, &unresolved)
            .require_closed_parameters(&known)
            .substitute(parameter),
        Err(TypeSubstitutionError::UnresolvedParameter(parameter))
    );
    let closed = HashMap::from([("T".into(), concrete_array)]);
    assert_eq!(
        TypeSubstitutionEngine::existing(&store, &closed)
            .require_closed_parameters(&known)
            .substitute(parameter),
        Ok(concrete_array)
    );
}

#[test]
fn closed_arguments_admit_native_named_atoms_by_identity_not_spelling() {
    let mut interner = TypeInterner::new();
    let native = interner.intern(Type::Named(NamedTypeRef {
        name: "JsonValue".into(),
        source_parameter: None,
    }));
    let parameter = interner.intern(Type::Named(NamedTypeRef {
        name: "JsonValue".into(),
        source_parameter: Some(etas_hir::SymbolId(7)),
    }));
    let store = interner.into_store();
    let names = HashMap::new();
    let known = |ty| ty == native;
    let mut engine =
        TypeSubstitutionEngine::existing(&store, &names).require_closed_parameters(&known);
    assert_eq!(engine.substitute(native), Ok(native));
    assert_eq!(
        engine.substitute(parameter),
        Err(TypeSubstitutionError::UnresolvedParameter(parameter))
    );
}

#[test]
fn closed_arguments_require_bound_effect_rows_and_closed_argument_rows() {
    let interner = TypeInterner::new();
    let store = interner.into_store();
    let names = HashMap::new();
    let known = |_| false;
    let open = EffectRowRef {
        effects: vec![],
        tail: Some("E".into()),
    };
    let closed = EffectRowRef {
        effects: vec![],
        tail: None,
    };
    let bindings = HashMap::from([("E".into(), closed.clone())]);
    assert_eq!(
        TypeSubstitutionEngine::existing(&store, &names)
            .require_closed_parameters(&known)
            .substitute_effect_row(open.clone()),
        Err(TypeSubstitutionError::OpenEffectRow("E".into()))
    );
    assert_eq!(
        TypeSubstitutionEngine::existing(&store, &names)
            .with_effect_arguments(&bindings)
            .require_closed_parameters(&known)
            .substitute_effect_row(open.clone()),
        Ok(closed)
    );
    let unbound = HashMap::from([("E".into(), open.clone())]);
    assert_eq!(
        TypeSubstitutionEngine::existing(&store, &names)
            .with_effect_arguments(&unbound)
            .require_closed_parameters(&known)
            .substitute_effect_row(open),
        Err(TypeSubstitutionError::OpenEffectRow("E".into()))
    );
}
