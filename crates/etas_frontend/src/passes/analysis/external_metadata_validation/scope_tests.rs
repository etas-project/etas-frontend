use super::*;

#[test]
fn empty_external_trace_choice_is_not_treated_as_empty_trace() {
    let error = validate_action_trace(
        &ProjectExternalActionTraceInput::Choice(Vec::new()),
        &GenericScope::default(),
        &[],
        0,
    )
    .unwrap_err();
    assert!(error.contains("choice requires an alternative"), "{error}");
    assert!(
        validate_action_trace(
            &ProjectExternalActionTraceInput::Empty,
            &GenericScope::default(),
            &[],
            0,
        )
        .is_ok()
    );
}

#[test]
fn trace_selector_declarations_obey_package_dependency_and_visibility() {
    let dependencies = BTreeSet::from([(ExternalPackageId(1), ExternalPackageId(2))]);
    assert!(
        validate_trace_selector_owner(
            &dependencies,
            ExternalPackageId(1),
            ExternalPackageId(1),
            "private",
            "spec"
        )
        .is_ok()
    );
    assert!(
        validate_trace_selector_owner(
            &dependencies,
            ExternalPackageId(1),
            ExternalPackageId(2),
            "public",
            "spec"
        )
        .is_ok()
    );
    assert!(
        validate_trace_selector_owner(
            &dependencies,
            ExternalPackageId(1),
            ExternalPackageId(2),
            "private",
            "spec"
        )
        .is_err()
    );
    assert!(
        validate_trace_selector_owner(
            &dependencies,
            ExternalPackageId(1),
            ExternalPackageId(3),
            "public",
            "action"
        )
        .is_err()
    );
}

#[test]
fn trace_selector_type_arguments_share_one_resource_budget() {
    let clause = crate::ProjectExternalTraceSpecClauseInput {
        kind: crate::ProjectExternalTraceSpecClauseKindInput::Allow,
        pattern: Some(ProjectExternalEffectRowInput {
            effects: vec![ProjectExternalEffectRefInput {
                path: vec!["dep".into(), "read".into()],
                args: vec![ProjectExternalEffectArgInput::Wildcard],
            }],
            tail: None,
        }),
        guard: None,
        target: None,
        obligation: None,
        selector_bounds: vec![crate::ProjectExternalTraceSelectorBoundInput {
            operand: crate::ProjectExternalTraceSelectorOperandInput::Pattern,
            effect_path: vec!["dep".into(), "read".into()],
            argument: 0,
            binding: "R".into(),
            spec_path: vec!["dep".into(), "Within".into()],
            spec_args: vec![ProjectExternalTypeInput::Primitive("i32".into()); 2],
        }],
    };
    let mut used = MAX_EXTERNAL_METADATA_GRAPH_NODES - 2;
    let error =
        validate_trace_selector_bounds(&clause, &GenericScope::default(), &mut used).unwrap_err();
    assert!(error.contains("maximum node count"), "{error}");
}

#[test]
fn nested_callable_type_variables_require_declared_enclosing_parameters() {
    let path = vec!["dep".into(), "Stage".into()];
    let scope = GenericScope {
        parameter_kinds: vec![etas_package_metadata::GenericParamKind::Type; 2],
        types: HashSet::from(["I".into(), "O".into()]),
        effects: HashSet::new(),
    };
    let output = ProjectExternalTypeInput::Var("O".into());
    assert!(
        validate_callable_in_scope(
            &path,
            &[],
            &[ProjectExternalTypeInput::Var("I".into())],
            &output,
            None,
            &scope
        )
        .is_ok()
    );
    let error = validate_callable_in_scope(
        &path,
        &[],
        &[ProjectExternalTypeInput::Var("Undeclared".into())],
        &output,
        None,
        &scope,
    )
    .unwrap_err();
    assert!(error.contains("not declared"), "{error}");
    assert!(
        validate_callable(
            &path,
            &[],
            &[ProjectExternalTypeInput::Var("I".into())],
            &output,
            None
        )
        .is_err()
    );
}

#[test]
fn nested_callable_cannot_reinterpret_an_effect_parameter_as_a_type() {
    let path = vec!["dep".into(), "Stage".into(), "run".into()];
    let scope = GenericScope {
        parameter_kinds: vec![etas_package_metadata::GenericParamKind::Effect],
        types: HashSet::new(),
        effects: HashSet::from(["E".into()]),
    };
    let params = vec![ProjectExternalCallableGenericParamInput {
        name: "E".into(),
        kind: ProjectExternalCallableGenericParamKindInput::Type,
        bounds: vec![],
    }];
    assert!(
        validate_callable_in_scope(
            &path,
            &params,
            &[],
            &ProjectExternalTypeInput::Primitive("unit".into()),
            None,
            &scope
        )
        .is_err()
    );
}

#[test]
fn returned_callable_generic_positions_preserve_declaration_order_at_the_environment_boundary() {
    use etas_package_metadata as metadata;
    let path = vec!["dep".into(), "factory".into()];
    let row = ProjectExternalEffectRowInput {
        effects: vec![],
        tail: None,
    };
    let node = metadata::CallableValueNode {
        residual_checks: Vec::new(),
        handler_requests: Some(Default::default()),
        behavior: metadata::CallableValueBehavior {
            determinism: metadata::CallableValueDeterminism::Deterministic,
            independent_determinism: Some(metadata::CallableValueDeterminism::Deterministic),
            host_requirements: vec![],
            interpreter_features: vec![],
        },
        parent_scope: metadata::CallableValueScope::Root,
        generic_params: vec![],
        generic_bindings: vec![
            metadata::CallableValueGenericBinding {
                label: "captured_E".into(),
                scope: metadata::CallableValueScope::Root,
                position: 0,
            },
            metadata::CallableValueGenericBinding {
                label: "captured_T".into(),
                scope: metadata::CallableValueScope::Root,
                position: 1,
            },
        ],
        param_names: vec![],
        params: vec![],
        output: metadata::Type {
            kind: metadata::TypeKind::Var,
            name: "captured_T".into(),
            ..Default::default()
        },
        public_effects: metadata::EffectRow {
            effects: vec![],
            tail: Some("captured_E".into()),
        },
        requested_actions: metadata::EffectRow {
            effects: vec![],
            tail: Some("captured_E".into()),
        },
        handled_requested_actions: metadata::EffectRow::default(),
        default_handler_effects: metadata::DefaultHandlerEffects::default(),
        parameter_bindings: vec![],
        action_trace: metadata::ActionTrace::Empty,
        returned_sources: None,
    };
    let mut input = ProjectExternalPublicMetadataInput {
        package: ExternalPackageId(0),
        types: vec![],
        values: vec![],
        enums: vec![],
        agents: vec![],
        tools: vec![],
        tool_schemas: vec![],
        effects: vec![],
        actions: vec![],
        trace_specs: vec![],
        spec_signatures: vec![],
        spec_impls: vec![],
        type_spec_satisfactions: vec![],
        callable_spec_satisfactions: vec![],
        trace_spec_conformances: vec![],
        action_summaries: vec![],
        trace_spec_summaries: vec![],
        re_exports: vec![],
        flows: vec![crate::ProjectExternalFlowSignatureInput {
            path: path.clone(),
            generic_params: vec![
                ProjectExternalCallableGenericParamInput {
                    name: "E".into(),
                    kind: ProjectExternalCallableGenericParamKindInput::Effect,
                    bounds: vec![],
                },
                ProjectExternalCallableGenericParamInput {
                    name: "T".into(),
                    kind: ProjectExternalCallableGenericParamKindInput::Type,
                    bounds: vec![],
                },
            ],
            param_names: vec![],
            params: vec![],
            output: ProjectExternalTypeInput::Function {
                input: vec![],
                output: Box::new(ProjectExternalTypeInput::Var("T".into())),
                effects: Some(ProjectExternalEffectRowInput {
                    effects: vec![],
                    tail: Some("E".into()),
                }),
            },
            effects: Some(row.clone()),
            visibility: "public".into(),
        }],
        effect_summaries: vec![crate::ProjectExternalEffectSummaryInput {
            residual_checks: vec![],
            behavior: None,
            handler_requests: None,
            item: path,
            public_effects: row.clone(),
            requested_actions: row.clone(),
            handled_requested_actions: row,
            default_handler_effects: None,
            latent_flows: vec![],
            action_trace: ProjectExternalActionTraceInput::Empty,
            returned_callables: Some(metadata::CallableValueGraph {
                sources: vec![metadata::CallableValueSource::Value(0)],
                values: vec![node],
            }),
        }],
    };
    validate_external_metadata(&input, &etas_std::standard_registry()).unwrap();
    let reference = etas_code::residual::identity::ResidualObligationRef::new(
        etas_code::residual::identity::ObligationId::from_canonical_fingerprint(
            etas_code::CodeFingerprint::from_bytes([44; 32]),
        ),
        Default::default(),
    )
    .unwrap();
    input.effect_summaries[0]
        .returned_callables
        .as_mut()
        .unwrap()
        .values[0]
        .residual_checks
        .push(reference.clone());
    let error = validate_external_metadata(&input, &etas_std::standard_registry()).unwrap_err();
    assert!(
        error.contains("require validated residual plans"),
        "{error}"
    );
    input.effect_summaries[0]
        .returned_callables
        .as_mut()
        .unwrap()
        .values[0]
        .residual_checks
        .clear();
    input.effect_summaries[0]
        .residual_checks
        .push(reference.clone());
    assert!(
        validate_external_metadata(&input, &etas_std::standard_registry())
            .unwrap_err()
            .contains("require validated residual plans")
    );
    input.effect_summaries[0].residual_checks.clear();
    input.effect_summaries[0]
        .latent_flows
        .push(crate::ProjectExternalLatentFlowSummaryInput {
            residual_checks: vec![reference],
            declared_bound: Default::default(),
            inferred_effects: Default::default(),
        });
    assert!(
        validate_external_metadata(&input, &etas_std::standard_registry())
            .unwrap_err()
            .contains("require validated residual plans")
    );
    input.effect_summaries[0].latent_flows.clear();
    input.flows[0].generic_params.swap(0, 1);
    let error = validate_external_metadata(&input, &etas_std::standard_registry()).unwrap_err();
    assert!(error.contains("wrong kind"), "{error}");
}
