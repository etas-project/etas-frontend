use super::*;

fn with_semantics(
    test: impl FnOnce(
        &EffectSemantics<'_>,
        etas_hir::HirItemId,
        etas_hir::SymbolId,
        crate::EffectUnit,
        crate::EffectUnit,
        HirExprId,
        HirExprId,
    ),
) {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"module app;
flow pass(f: () -> unit ![]) -> unit ![] { let value = () => { return; }; f(); return; }
flow first() -> unit ![] { return; }
flow second() -> unit ![] { return; }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::HirRevision::try_new(etas_hir::lower_program(&parsed.value)).unwrap();
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let registry = etas_std::standard_registry();
    let effects = crate::EffectRegistry::with_standard_effects_from(&registry);
    let semantics = EffectSemantics::new(&hir, &types, &registry, &effects, &[], &[]);
    let flow = |name| {
        hir.items
            .iter()
            .find_map(|(item, data)| {
                let etas_hir::HirItem::Flow(flow) = data else {
                    return None;
                };
                (hir.symbols.get(flow.symbol)?.name == name)
                    .then_some((item, flow.params.first().copied()))
            })
            .unwrap()
    };
    let (owner, parameter) = flow("pass");
    let lambda = hir
        .exprs
        .iter()
        .find_map(|(expr, data)| matches!(data, etas_hir::HirExpr::Lambda { .. }).then_some(expr))
        .unwrap();
    let call = hir
        .exprs
        .iter()
        .find_map(|(expr, data)| matches!(data, etas_hir::HirExpr::Call { .. }).then_some(expr))
        .unwrap();
    test(
        &semantics,
        owner,
        parameter.unwrap(),
        crate::EffectUnit::Item(flow("first").0),
        crate::EffectUnit::Item(flow("second").0),
        lambda,
        call,
    );
}

#[test]
fn captured_environment_snapshots_are_copy_on_write_and_owner_checked() {
    with_semantics(|semantics, owner, parameter, first, second, lambda, _| {
        let site = semantics.callable_environment_site(lambda).unwrap();
        let id = CallableEnvironmentId {
            origin: site,
            application: site,
        };
        let snapshot = |owner, source| CallableEnvironment {
            owner,
            generics: Default::default(),
            parameters: [(
                ParameterCallTarget::Native(parameter),
                [LatentSource::Unit(source)].into(),
            )]
            .into(),
        };
        semantics
            .record_callable_environment(id, snapshot(owner, first))
            .unwrap();
        let retained = semantics.callable_environments.borrow().clone();
        semantics
            .record_callable_environment(id, snapshot(owner, second))
            .unwrap();
        assert_eq!(
            retained[&id].parameters[&ParameterCallTarget::Native(parameter)].len(),
            1
        );
        assert_eq!(
            semantics.callable_environment(id).unwrap().parameters
                [&ParameterCallTarget::Native(parameter)]
                .len(),
            2
        );
        assert!(
            semantics
                .record_callable_environment(id, snapshot(first.owner().unwrap(), first))
                .is_err()
        );
        assert_eq!(semantics.callable_environment(id).unwrap().owner, owner);
    });
}

#[test]
fn a_new_known_origin_does_not_erase_an_unproved_capture_branch() {
    with_semantics(|semantics, owner, parameter, first, _, lambda, call| {
        let site = semantics.callable_environment_site(lambda).unwrap();
        let id = CallableEnvironmentId {
            origin: site,
            application: site,
        };
        let snapshot = |origin| CallableEnvironment {
            owner,
            generics: BTreeMap::new(),
            parameters: [(ParameterCallTarget::Native(parameter), [origin].into())].into(),
        };
        let missing = LatentSource::Unavailable { expr: call };
        semantics
            .record_callable_environment(id, snapshot(missing))
            .unwrap();
        semantics
            .record_callable_environment(id, snapshot(LatentSource::Unit(first)))
            .unwrap();
        let environment = semantics.callable_environment(id).unwrap();
        let origins = &environment.parameters[&ParameterCallTarget::Native(parameter)];
        assert!(origins.contains(&missing));
        assert!(origins.contains(&LatentSource::Unit(first)));
    });
}

#[test]
fn conflicting_generic_capture_bindings_fail_without_mutating_the_saved_environment() {
    with_semantics(|semantics, owner, _, _, _, lambda, _| {
        use crate::facts::{CallableGenericParameter, CallableGenericValue};
        let site = semantics.callable_environment_site(lambda).unwrap();
        let id = CallableEnvironmentId {
            origin: site,
            application: site,
        };
        let key = CallableGenericParameter::Native { owner, position: 0 };
        let snapshot = |ty| CallableEnvironment {
            owner,
            parameters: Default::default(),
            generics: [(key, CallableGenericValue::Type(ty))].into(),
        };
        let primitive = |kind| {
            semantics
                .types
                .store
                .iter()
                .find_map(|(ty, value)| (value == &etas_types::Type::Primitive(kind)).then_some(ty))
                .unwrap()
        };
        let original = snapshot(primitive(etas_types::PrimitiveType::I32));
        semantics
            .record_callable_environment(id, original.clone())
            .unwrap();
        assert!(
            semantics
                .record_callable_environment(
                    id,
                    snapshot(primitive(etas_types::PrimitiveType::String))
                )
                .is_err()
        );
        assert_eq!(semantics.callable_environment(id).unwrap(), original);
    });
}

#[test]
fn cyclic_capture_references_are_relocated_by_a_finite_worklist() {
    with_semantics(|semantics, owner, parameter, _, _, lambda, call| {
        let site = semantics.callable_environment_site(lambda).unwrap();
        let id = CallableEnvironmentId {
            origin: site,
            application: site,
        };
        let unit = semantics
            .anonymous_flow_unit_for_expr(owner, lambda)
            .unwrap();
        semantics
            .record_callable_environment(
                id,
                CallableEnvironment {
                    owner,
                    generics: Default::default(),
                    parameters: [(
                        ParameterCallTarget::Native(parameter),
                        [LatentSource::Captured {
                            unit,
                            environment: id,
                        }]
                        .into(),
                    )]
                    .into(),
                },
            )
            .unwrap();
        let mapped = semantics
            .instantiate_callable_environment(
                id,
                call,
                &Default::default(),
                &EffectState::for_unit(crate::EffectUnit::Item(owner)),
            )
            .unwrap();
        assert_ne!(id, mapped);
        let snapshot = semantics.callable_environment(mapped).unwrap();
        assert!(
            snapshot.parameters[&ParameterCallTarget::Native(parameter)].contains(
                &LatentSource::Captured {
                    unit,
                    environment: mapped
                }
            )
        );
        assert_eq!(semantics.callable_environments.borrow().len(), 2);
    });
}
