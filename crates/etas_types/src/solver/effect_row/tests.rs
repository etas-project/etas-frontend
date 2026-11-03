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

#[test]
fn row_equivalence_compares_both_sets_without_counting_duplicates() {
    let types = crate::TypeInterner::new();
    let names = HashMap::new();
    let substitutions = Substitution::default();
    assert!(rows_equivalent(
        types.store(),
        &row(&["a", "a"], None),
        &row(&["a"], None),
        &names,
        &substitutions
    ));
    assert!(!rows_equivalent(
        types.store(),
        &row(&["a", "a"], None),
        &row(&["a", "b"], None),
        &names,
        &substitutions
    ));
}

#[test]
fn row_equivalence_distinguishes_canonical_nominal_arguments() {
    use etas_code::{DefinitionKey, DefinitionKind, PackageKey};
    let identity = |name: &str| {
        DefinitionKey::new(
            PackageKey::from_name_and_version(name, "1.0.0").unwrap(),
            vec!["app".into()],
            vec!["Region".into()],
            DefinitionKind::Type,
        )
        .unwrap()
    };
    let mut types = crate::TypeInterner::new();
    let region = Type::Nominal(crate::NominalTypeRef {
        name: "Region".into(),
        params: Vec::new(),
        representation: None,
        read_only_erased_view: false,
    });
    let first = types
        .intern_canonical_nominal(identity("first"), region.clone())
        .unwrap();
    let second = types
        .intern_canonical_nominal(identity("second"), region.clone())
        .unwrap();
    let ordinary = types.intern(region);
    let make_row = |ty| EffectRowRef {
        effects: vec![EffectRef {
            name: "Fs.read".into(),
            args: vec![EffectArgRef::Type(ty)],
        }],
        tail: None,
    };
    let equivalent = |lhs, rhs| {
        rows_equivalent(
            types.store(),
            &make_row(lhs),
            &make_row(rhs),
            &HashMap::new(),
            &Substitution::default(),
        )
    };
    assert!(equivalent(first, first));
    assert!(!equivalent(first, second));
    assert!(!equivalent(first, ordinary));
}

#[test]
fn row_type_substitution_does_not_capture_another_declarations_binder() {
    use etas_code::{DefinitionKey, DefinitionKind, PackageKey};
    let owner = DefinitionKey::new(
        PackageKey::from_name_and_version("library", "1.0.0").unwrap(),
        vec!["lib".into()],
        vec!["Box".into()],
        DefinitionKind::Type,
    )
    .unwrap();
    let mut types = crate::TypeInterner::new();
    let param = types
        .intern_canonical_parameter(
            crate::ty::CanonicalTypeParameterIdentity::new(owner, 0).unwrap(),
            "T".into(),
        )
        .unwrap();
    let boolean = types.primitive(crate::PrimitiveType::Bool);
    assert_eq!(
        resolve_type(
            types.store(),
            param,
            &HashMap::from([("T".into(), boolean)]),
            &Substitution::default()
        ),
        param
    );
}

#[test]
fn overlapping_fixed_effects_solve_all_equations_independently_of_order() {
    let first = equation(&["a"], &["a", "b"], None, 0);
    let second = equation(&["b"], &["a", "b"], None, 1);
    for equations in [vec![first(), second()], vec![second(), first()]] {
        let (results, bindings) = solve_equations(equations, HashMap::new());
        assert!(results.iter().all(|result| result.matches));
        let names = bindings["E"]
            .effects
            .iter()
            .map(|effect| effect.name.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(names, BTreeSet::from(["a", "b"]));
    }
}

#[test]
fn incompatible_row_equations_and_explicit_bindings_are_rejected() {
    let equations = vec![
        equation(&["a"], &["a", "b"], None, 0)(),
        equation(&[], &["a"], None, 1)(),
    ];
    let (results, _) = solve_equations(equations, HashMap::new());
    assert!(results.iter().any(|result| !result.matches));

    let explicit = HashMap::from([("E".into(), row(&["b", "c"], None))]);
    let (results, _) = solve_equations(vec![equation(&["a"], &["a", "b"], None, 0)()], explicit);
    assert!(!results[0].matches);
}

#[test]
fn a_concrete_binding_does_not_discard_a_deferred_source() {
    let equations = vec![
        equation(&[], &["a"], None, 0)(),
        equation(&[], &[], Some("E"), 1)(),
    ];
    let (results, bindings) = solve_equations(equations, HashMap::new());
    assert!(results.iter().all(|result| result.matches));
    assert_eq!(bindings["E"], row(&["a"], None));
    assert_eq!(results[1].deferred, BTreeSet::from(["E".into()]));
}

fn row(names: &[&str], tail: Option<&str>) -> EffectRowRef {
    EffectRowRef {
        effects: names
            .iter()
            .map(|name| EffectRef {
                name: (*name).into(),
                args: Vec::new(),
            })
            .collect(),
        tail: tail.map(str::to_owned),
    }
}

fn equation<'a>(
    fixed: &'a [&str],
    actual: &'a [&str],
    tail: Option<&'a str>,
    origin: usize,
) -> impl Fn() -> RowEquation + 'a {
    move || RowEquation {
        parameter: "E".into(),
        fixed: row(fixed, None).effects,
        actual: row(actual, tail),
        origin,
    }
}

fn solve_equations(
    equations: Vec<RowEquation>,
    mut bindings: HashMap<String, EffectRowRef>,
) -> (Vec<EffectRowInference>, HashMap<String, EffectRowRef>) {
    let store = crate::TypeInterner::new();
    let types = HashMap::new();
    let substitutions = Substitution::default();
    let params = vec!["E".into()];
    let inputs = InferenceInputs {
        store: store.store(),
        type_bindings: &types,
        substitutions: &substitutions,
        params: &params,
        lexical_params: &[],
    };
    let count = equations
        .iter()
        .map(|equation| equation.origin + 1)
        .max()
        .unwrap_or(0);
    let results;
    {
        let mut state = InferenceState {
            bindings: &mut bindings,
            results: vec![
                EffectRowInference {
                    matches: true,
                    deferred: BTreeSet::new(),
                    deferred_fixed: Vec::new(),
                    concrete: Vec::new(),
                };
                count
            ],
            current: 0,
            equations,
            visited: HashSet::new(),
        };
        solve_row_equations(&inputs, &mut state);
        results = state.results;
    }
    (results, bindings)
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
