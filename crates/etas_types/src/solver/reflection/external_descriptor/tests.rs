use super::*;
use crate::{ExternalEffectArgInput, ExternalEffectRefInput};
use etas_code::{
    CodeFingerprint, DefinitionKey, DefinitionKind, PackageKey,
    descriptor::{
        BuiltinTypeInfo, EffectInfo, GenericOwner, GenericParameterInfo, ParameterInfo,
        PrimitiveInfo, SignatureInfo, StaticEffectArgInfo, TypeInfo,
    },
};

fn definition(package: &str, name: &str, kind: DefinitionKind) -> DefinitionKey {
    DefinitionKey::new(
        PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
            "etas.test.external-type",
            package.as_bytes(),
        )),
        vec!["module".into()],
        vec![name.into()],
        kind,
    )
    .unwrap()
}

#[test]
fn generic_descriptor_alias_substitutes_the_callable_type() {
    let alias = definition("first", "Draft", DefinitionKind::Type);
    let graph = DescriptorGraphInput {
        types: vec![
            TypeInfo::Primitive(PrimitiveInfo::I32),
            TypeInfo::Parameter(GenericParameterRef(0)),
            TypeInfo::Function(SignatureInfo {
                generics: vec![],
                parameters: vec![ParameterInfo {
                    name: None,
                    ty: TypeInfoRef(1),
                }],
                output: TypeInfoRef(1),
                declared_output: None,
                effects: None,
            }),
            TypeInfo::Builtin {
                constructor: BuiltinTypeInfo::CallableDefinition,
                arguments: vec![TypeInfoRef(2)],
            },
            TypeInfo::Alias {
                definition: alias.clone(),
                parameters: vec![GenericParameterRef(0)],
                target: TypeInfoRef(3),
            },
            TypeInfo::Applied {
                constructor: TypeInfoRef(4),
                arguments: vec![TypeInfoRef(0)],
            },
        ],
        binders: vec![GenericParameterInfo {
            owner: GenericOwner::Declaration(alias),
            position: 0,
            name: "T".into(),
            kind: GenericParameterKind::Type,
            bounds: vec![],
        }],
        ..Default::default()
    };
    graph.validate(Default::default()).unwrap();
    let registry = etas_std::standard_registry();
    let identities =
        ExternalDescriptorIdentities::new(&registry, &ReflectionSignatureInput::default()).unwrap();
    let mut budget = 100;
    let mut matcher =
        ExternalDescriptorMatcher::new(&graph, &identities, &[], &[], &mut budget).unwrap();
    let mut input = ExternalTypeInput::Applied {
        path: vec!["std".into(), "compiler".into(), "CallableDef".into()],
        args: vec![ExternalTypeInput::Function {
            input: vec![ExternalTypeInput::Primitive("i32".into())],
            output: Box::new(ExternalTypeInput::Primitive("i32".into())),
            effects: None,
        }],
    };
    assert!(matcher.matches(&input, TypeInfoRef(5)).unwrap());
    let ExternalTypeInput::Applied { args, .. } = &mut input else {
        unreachable!()
    };
    let ExternalTypeInput::Function {
        input: function_inputs,
        ..
    } = &mut args[0]
    else {
        unreachable!()
    };
    function_inputs[0] = ExternalTypeInput::Primitive("bool".into());
    assert!(!matcher.matches(&input, TypeInfoRef(5)).unwrap());
}

#[test]
fn external_applied_aliases_expand_with_checked_substitution_and_budget() {
    let graph = DescriptorGraphInput {
        types: vec![
            TypeInfo::Primitive(PrimitiveInfo::I32),
            TypeInfo::Function(SignatureInfo {
                generics: vec![],
                parameters: vec![ParameterInfo {
                    name: None,
                    ty: TypeInfoRef(0),
                }],
                output: TypeInfoRef(0),
                declared_output: None,
                effects: None,
            }),
            TypeInfo::Builtin {
                constructor: BuiltinTypeInfo::CallableDefinition,
                arguments: vec![TypeInfoRef(1)],
            },
        ],
        ..Default::default()
    };
    let f = vec!["library".into(), "Fn".into()];
    let draft = vec!["library".into(), "Draft".into()];
    let cycle = vec!["library".into(), "Cycle".into()];
    let identities = ReflectionSignatureInput {
        aliases: vec![
            (
                f.clone(),
                vec!["T".into()],
                ExternalTypeInput::Function {
                    input: vec![ExternalTypeInput::Var("T".into())],
                    output: Box::new(ExternalTypeInput::Var("T".into())),
                    effects: None,
                },
            ),
            (
                draft.clone(),
                vec!["U".into()],
                ExternalTypeInput::Applied {
                    path: vec!["std".into(), "compiler".into(), "CallableDef".into()],
                    args: vec![ExternalTypeInput::Applied {
                        path: f,
                        args: vec![ExternalTypeInput::Var("U".into())],
                    }],
                },
            ),
            (
                cycle.clone(),
                vec![],
                ExternalTypeInput::Named(cycle.clone()),
            ),
        ],
        ..Default::default()
    };
    let registry = etas_std::standard_registry();
    let identities = ExternalDescriptorIdentities::new(&registry, &identities).unwrap();
    let mut budget = 1000;
    let mut matcher =
        ExternalDescriptorMatcher::new(&graph, &identities, &[], &[], &mut budget).unwrap();
    let apply = |args| ExternalTypeInput::Applied {
        path: draft.clone(),
        args,
    };
    assert!(
        matcher
            .matches(
                &apply(vec![ExternalTypeInput::Primitive("i32".into())]),
                TypeInfoRef(2)
            )
            .unwrap()
    );
    assert!(
        !matcher
            .matches(
                &apply(vec![ExternalTypeInput::Primitive("bool".into())]),
                TypeInfoRef(2)
            )
            .unwrap()
    );
    assert!(matcher.matches(&apply(vec![]), TypeInfoRef(2)).is_err());
    assert!(
        matcher
            .matches(&ExternalTypeInput::Named(cycle), TypeInfoRef(2))
            .is_err()
    );
}

#[test]
fn comparison_budget_is_shared_across_matchers() {
    let graph = DescriptorGraphInput {
        types: vec![TypeInfo::Primitive(PrimitiveInfo::I32)],
        ..Default::default()
    };
    let registry = etas_std::standard_registry();
    let identities =
        ExternalDescriptorIdentities::new(&registry, &ReflectionSignatureInput::default()).unwrap();
    let mut budget = 1;
    {
        let mut matcher =
            ExternalDescriptorMatcher::new(&graph, &identities, &[], &[], &mut budget).unwrap();
        assert!(
            matcher
                .matches(&ExternalTypeInput::Primitive("i32".into()), TypeInfoRef(0))
                .unwrap()
        );
    }
    let mut matcher =
        ExternalDescriptorMatcher::new(&graph, &identities, &[], &[], &mut budget).unwrap();
    assert!(
        matcher
            .matches(&ExternalTypeInput::Primitive("i32".into()), TypeInfoRef(0))
            .is_err()
    );
}

#[test]
fn equal_non_callable_definition_arguments_are_rejected() {
    let graph = DescriptorGraphInput {
        types: vec![
            TypeInfo::Primitive(PrimitiveInfo::Bool),
            TypeInfo::Builtin {
                constructor: BuiltinTypeInfo::CallableDefinition,
                arguments: vec![TypeInfoRef(0)],
            },
        ],
        ..Default::default()
    };
    graph.validate(Default::default()).unwrap();
    let registry = etas_std::standard_registry();
    let identities =
        ExternalDescriptorIdentities::new(&registry, &ReflectionSignatureInput::default()).unwrap();
    let mut budget = 100;
    let mut matcher =
        ExternalDescriptorMatcher::new(&graph, &identities, &[], &[], &mut budget).unwrap();
    let input = ExternalTypeInput::Applied {
        path: vec!["std".into(), "compiler".into(), "CallableDef".into()],
        args: vec![ExternalTypeInput::Primitive("bool".into())],
    };
    assert_eq!(
        matcher.matches(&input, TypeInfoRef(1)),
        Err("callable definition requires a callable type argument")
    );
}

#[test]
fn nominal_matching_uses_package_identity_through_aliases() {
    let original = definition("first", "Item", DefinitionKind::Type);
    let unrelated = definition("second", "Item", DefinitionKind::Type);
    let alias = definition("first", "Alias", DefinitionKind::Type);
    let graph = DescriptorGraphInput {
        types: vec![
            TypeInfo::Nominal {
                definition: original.clone(),
                parameters: vec![],
                representation: None,
            },
            TypeInfo::Alias {
                definition: alias,
                parameters: vec![],
                target: TypeInfoRef(0),
            },
        ],
        ..Default::default()
    };
    let registry = etas_std::standard_registry();
    let identities = ReflectionSignatureInput {
        declarations: vec![
            (vec!["first".into(), "Item".into()], original),
            (vec!["second".into(), "Item".into()], unrelated),
        ],
        ..Default::default()
    };
    let identities = ExternalDescriptorIdentities::new(&registry, &identities).unwrap();
    let mut budget = 100;
    let mut matcher =
        ExternalDescriptorMatcher::new(&graph, &identities, &[], &[], &mut budget).unwrap();
    assert!(
        matcher
            .matches(
                &ExternalTypeInput::Named(vec!["first".into(), "Item".into()]),
                TypeInfoRef(1)
            )
            .unwrap()
    );
    assert!(
        !matcher
            .matches(
                &ExternalTypeInput::Named(vec!["second".into(), "Item".into()]),
                TypeInfoRef(1)
            )
            .unwrap()
    );
    assert!(
        matcher
            .matches(
                &ExternalTypeInput::Named(vec!["missing".into(), "Item".into()]),
                TypeInfoRef(1)
            )
            .is_err()
    );
}

#[test]
fn nested_function_effects_use_canonical_action_and_argument_types() {
    let effect = definition("first", "Fs", DefinitionKind::Effect);
    let target = EffectTargetInfo::Action {
        effect,
        name: "read".into(),
    };
    let graph = DescriptorGraphInput {
        types: vec![
            TypeInfo::Primitive(PrimitiveInfo::I32),
            TypeInfo::Function(SignatureInfo {
                generics: vec![],
                parameters: vec![ParameterInfo {
                    name: None,
                    ty: TypeInfoRef(0),
                }],
                output: TypeInfoRef(0),
                declared_output: None,
                effects: Some(EffectRowInfoRef(0)),
            }),
        ],
        rows: vec![etas_code::descriptor::EffectRowInfo {
            effects: vec![EffectInfo {
                target: target.clone(),
                arguments: vec![StaticEffectArgInfo::Type(TypeInfoRef(0))],
            }],
            tail: None,
        }],
        ..Default::default()
    };
    let registry = etas_std::standard_registry();
    let identities = ReflectionSignatureInput {
        effect_targets: vec![("first.Fs.read".into(), target)],
        ..Default::default()
    };
    let identities = ExternalDescriptorIdentities::new(&registry, &identities).unwrap();
    let mut budget = 100;
    let mut matcher =
        ExternalDescriptorMatcher::new(&graph, &identities, &[], &[], &mut budget).unwrap();
    let mut actual = ExternalTypeInput::Function {
        input: vec![ExternalTypeInput::Primitive("i32".into())],
        output: Box::new(ExternalTypeInput::Primitive("i32".into())),
        effects: Some(ExternalEffectRowInput {
            effects: vec![ExternalEffectRefInput {
                path: vec!["first".into(), "Fs".into(), "read".into()],
                args: vec![ExternalEffectArgInput::Type(ExternalTypeInput::Primitive(
                    "i32".into(),
                ))],
            }],
            tail: None,
        }),
    };
    assert!(matcher.matches(&actual, TypeInfoRef(1)).unwrap());
    let ExternalTypeInput::Function {
        effects: Some(row), ..
    } = &mut actual
    else {
        unreachable!()
    };
    row.effects[0].args[0] =
        ExternalEffectArgInput::Type(ExternalTypeInput::Primitive("bool".into()));
    assert!(!matcher.matches(&actual, TypeInfoRef(1)).unwrap());
}

#[test]
fn callable_selector_requires_the_checked_method_identity() {
    use etas_code::{CallableId, ImplementationKey, MethodId};
    let agent = definition("first", "Worker", DefinitionKind::Agent);
    let method = |name: &str| {
        CallableId::method(
            MethodId::new(
                ImplementationKey::new(
                    agent.package(),
                    agent.module().to_vec(),
                    agent.clone(),
                    None,
                    CodeFingerprint::of_bytes("etas.test.method", name.as_bytes()),
                )
                .unwrap(),
                "run".into(),
            )
            .unwrap(),
        )
    };
    let expected_method = method("expected");
    let other_method = method("other");
    let target = EffectTargetInfo::Action {
        effect: definition("first", "Agentic", DefinitionKind::Effect),
        name: "infer".into(),
    };
    let graph = DescriptorGraphInput {
        rows: vec![etas_code::descriptor::EffectRowInfo {
            effects: vec![EffectInfo {
                target: target.clone(),
                arguments: vec![StaticEffectArgInfo::Callable(expected_method.clone())],
            }],
            tail: None,
        }],
        ..Default::default()
    };
    let registry = etas_std::standard_registry();
    let path = vec!["first".into(), "Worker".into(), "run".into()];
    let identities = ReflectionSignatureInput {
        effect_targets: vec![("first.Agentic.infer".into(), target)],
        methods: vec![(path.clone(), expected_method)],
        ..Default::default()
    };
    let actual = ExternalEffectRowInput {
        effects: vec![ExternalEffectRefInput {
            path: vec!["first".into(), "Agentic".into(), "infer".into()],
            args: vec![ExternalEffectArgInput::Path(path.clone())],
        }],
        tail: None,
    };
    let mut budget = 100;
    let index = ExternalDescriptorIdentities::new(&registry, &identities).unwrap();
    let mut matcher =
        ExternalDescriptorMatcher::new(&graph, &index, &[], &[], &mut budget).unwrap();
    assert!(
        matcher
            .matches_optional_effect_row(Some(&actual), Some(EffectRowInfoRef(0)))
            .unwrap()
    );

    let mut identities = identities;
    identities.methods[0].1 = other_method;
    let index = ExternalDescriptorIdentities::new(&registry, &identities).unwrap();
    let mut matcher =
        ExternalDescriptorMatcher::new(&graph, &index, &[], &[], &mut budget).unwrap();
    assert!(
        !matcher
            .matches_optional_effect_row(Some(&actual), Some(EffectRowInfoRef(0)))
            .unwrap()
    );
    identities.methods.clear();
    let index = ExternalDescriptorIdentities::new(&registry, &identities).unwrap();
    let mut matcher =
        ExternalDescriptorMatcher::new(&graph, &index, &[], &[], &mut budget).unwrap();
    assert_eq!(
        matcher.matches_optional_effect_row(Some(&actual), Some(EffectRowInfoRef(0))),
        Err("external callable selector has no checked method identity")
    );
}
