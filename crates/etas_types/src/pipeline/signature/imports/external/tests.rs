use super::*;
use crate::{ExternalNamedSignatureInput as Decl, ExternalTypeInput as Ty};

fn path(name: &str) -> Vec<String> {
    vec!["dep".into(), name.into()]
}

fn declaration(name: &str, ty: Ty) -> Decl {
    Decl {
        path: path(name),
        generic_params: Vec::new(),
        ty: Some(ty),
    }
}

fn with_declarations(
    declarations: Vec<Decl>,
    assert: impl FnOnce(&TypePipelineContext<'_>, etas_hir::SymbolId),
) {
    with_declarations_and_specs(declarations, Vec::new(), assert);
}

fn with_declarations_and_specs(
    declarations: Vec<Decl>,
    specs: Vec<crate::ExternalSpecSignatureInput>,
    assert: impl FnOnce(&TypePipelineContext<'_>, etas_hir::SymbolId),
) {
    with_binding(
        declarations,
        specs,
        "import dep.A;",
        path("A"),
        |ctx, symbol| assert(ctx, symbol),
    );
}

fn with_binding(
    declarations: Vec<Decl>,
    specs: Vec<crate::ExternalSpecSignatureInput>,
    source: &str,
    binding_path: Vec<String>,
    assert: impl FnOnce(&mut TypePipelineContext<'_>, etas_hir::SymbolId),
) {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(73),
        None,
        source,
    ));
    assert!(parsed.diagnostics.is_empty());
    let hir = etas_hir::lower_program(&parsed.value);
    let symbol = hir
        .symbols
        .iter()
        .find(|symbol| symbol.name == "A")
        .unwrap()
        .id;
    let span = hir.symbols.get(symbol).unwrap().definition_span;
    let package = ExternalPackageKey(7);
    let input = ExternalSignatureInput {
        metadata: vec![crate::ExternalPublicMetadataInput {
            behavior_types: vec![],
            package,
            types: declarations,
            values: Vec::new(),
            enums: Vec::new(),
            flows: Vec::new(),
            agents: Vec::new(),
            tools: Vec::new(),
            effects: Vec::new(),
            trace_specs: Vec::new(),
            spec_signatures: specs,
            spec_impls: Vec::new(),
            methods: Vec::new(),
            type_spec_satisfactions: Vec::new(),
            callable_spec_satisfactions: Vec::new(),
            trace_spec_conformances: Vec::new(),
            actions: Vec::new(),
        }],
        symbol_bindings: vec![crate::ExternalSymbolBindingInput {
            symbol,
            package,
            path: binding_path,
            span,
        }],
        ..Default::default()
    };
    let mut ctx = TypePipelineContext::new(&hir);
    apply_external_signature_input(&mut ctx, &input);
    assert(&mut ctx, symbol);
}

#[test]
fn imported_spec_alias_reuses_checked_external_method_signature() {
    let method_path = vec!["dep".into(), "Show".into(), "render".into()];
    let signature = crate::ExternalSpecSignatureInput {
        path: path("Show"),
        kind: crate::ExternalSpecKindInput::Type,
        param_names: vec![],
        generic_params: vec![],
        callable: None,
        methods: vec![crate::ExternalSpecMethodInput {
            receiver_param: None,
            name: "render".into(),
            path: method_path.clone(),
            signature: Some(crate::ExternalFlowSignatureInput {
                path: method_path,
                generic_params: vec![],
                param_names: vec!["self".into()],
                params: vec![Ty::Primitive("i32".into())],
                output: Ty::Primitive("string".into()),
                effects: None,
            }),
        }],
        super_specs: vec![],
    };
    with_binding(
        vec![],
        vec![signature],
        "import dep.Show as A;",
        path("Show"),
        |ctx, symbol| {
            assert!(ctx.diagnostics.is_empty(), "{:?}", ctx.diagnostics);
            let identity = crate::facts::specs::ExternalSpecIdentity {
                package: 7,
                path: path("Show"),
            };
            let external = &ctx.signature_facts.external_specs.signatures[&identity].methods[0];
            let alias = &ctx.signature_facts.spec_signatures[&symbol].methods[0];
            assert_eq!(alias.name, external.name);
            assert_eq!(alias.signature, external.signature);
            assert!(matches!((&alias.identity, &external.identity),
            (SpecMethodIdentity::External { package: left, path: left_path }, SpecMethodIdentity::External { package: right, path: right_path })
            if left == right && left_path == right_path));
        },
    );
}

#[test]
fn imported_spec_alias_rejects_missing_checked_method_facts() {
    let signature = crate::ExternalSpecSignatureInput {
        path: path("Show"),
        kind: crate::ExternalSpecKindInput::Type,
        param_names: vec![],
        generic_params: vec![],
        callable: None,
        methods: vec![crate::ExternalSpecMethodInput {
            receiver_param: None,
            name: "render".into(),
            path: vec!["dep".into(), "Show".into(), "render".into()],
            signature: None,
        }],
        super_specs: vec![],
    };
    with_binding(
        vec![],
        vec![signature.clone()],
        "import dep.Show as A;",
        path("Show"),
        |ctx, symbol| {
            assert!(ctx.diagnostics.is_empty(), "{:?}", ctx.diagnostics);
            let identity = crate::facts::specs::ExternalSpecIdentity {
                package: 7,
                path: path("Show"),
            };
            ctx.signature_facts
                .external_specs
                .signatures
                .get_mut(&identity)
                .unwrap()
                .methods
                .clear();
            let span = ctx.hir.symbols.get(symbol).unwrap().definition_span;
            assert!(
                lower_external_spec_signature(ctx, ExternalPackageKey(7), symbol, span, &signature)
                    .is_none()
            );
            assert!(!ctx.diagnostics.is_empty());
        },
    );
}

#[test]
fn external_spec_methods_are_checked_without_importing_the_spec_name() {
    let method_path = vec!["dep".into(), "Show".into(), "render".into()];
    let signature = crate::ExternalSpecSignatureInput {
        path: path("Show"),
        kind: crate::ExternalSpecKindInput::Type,
        param_names: vec![],
        generic_params: vec![],
        callable: None,
        methods: vec![crate::ExternalSpecMethodInput {
            receiver_param: None,
            name: "render".into(),
            path: method_path.clone(),
            signature: Some(crate::ExternalFlowSignatureInput {
                path: method_path.clone(),
                param_names: vec!["self".into(), "value".into()],
                generic_params: vec![
                    crate::ExternalCallableGenericParamInput {
                        name: "T".into(),
                        kind: crate::ExternalCallableGenericParamKindInput::Type,
                        bounds: vec![crate::ExternalSpecBoundInput {
                            spec: path("Show"),
                            args: vec![],
                        }],
                    },
                    crate::ExternalCallableGenericParamInput {
                        name: "E".into(),
                        kind: crate::ExternalCallableGenericParamKindInput::Effect,
                        bounds: vec![],
                    },
                ],
                params: vec![Ty::Primitive("i32".into()), Ty::Var("T".into())],
                output: Ty::Primitive("string".into()),
                effects: Some(crate::ExternalEffectRowInput {
                    effects: vec![crate::ExternalEffectRefInput {
                        path: vec!["Error".into()],
                        args: vec![crate::ExternalEffectArgInput::Type(Ty::Var("T".into()))],
                    }],
                    tail: Some("E".into()),
                }),
            }),
        }],
        super_specs: vec![],
    };
    with_declarations_and_specs(
        vec![declaration("A", Ty::Primitive("i32".into()))],
        vec![signature],
        |ctx, _| {
            assert!(ctx.diagnostics.is_empty(), "{:?}", ctx.diagnostics);
            let identity = crate::facts::specs::ExternalSpecIdentity {
                package: 7,
                path: path("Show"),
            };
            let signature = &ctx.signature_facts.external_specs.signatures[&identity];
            assert_eq!(signature.methods.len(), 1);
            let method = &signature.methods[0];
            assert_eq!(method.name, "render");
            assert!(
                matches!(&method.identity, SpecMethodIdentity::External { package: 7, path } if *path == method_path)
            );
            let callable = method.signature.as_ref().unwrap();
            assert_eq!(callable.params.len(), 2);
            assert_eq!(callable.generic_params.len(), 2);
            assert_eq!(callable.params[1], callable.generic_params[0].subject);
            assert_eq!(callable.generic_params[0].bounds.len(), 1);
            assert_eq!(
                callable.effects.as_ref().unwrap().tail.as_deref(),
                Some("E")
            );
            assert_eq!(
                callable.effects.as_ref().unwrap().effects[0].args,
                vec![crate::EffectArgRef::Type(callable.params[1])]
            );
            assert!(matches!(
                ctx.interner.store().get(callable.params[0]),
                Some(Type::Primitive(crate::PrimitiveType::I32))
            ));
            assert!(matches!(
                ctx.interner.store().get(callable.output),
                Some(Type::Primitive(crate::PrimitiveType::String))
            ));
        },
    );
}

#[test]
fn external_aliases_resolve_forward_generic_nominals_in_every_declaration_order() {
    let mut generic = declaration(
        "Box",
        Ty::Record {
            fields: vec![crate::ExternalRecordFieldInput {
                name: "value".into(),
                ty: Ty::Var("T".into()),
            }],
        },
    );
    generic
        .generic_params
        .push(crate::ExternalCallableGenericParamInput {
            name: "T".into(),
            kind: crate::ExternalCallableGenericParamKindInput::Type,
            bounds: Vec::new(),
        });
    let declarations = [
        declaration(
            "A",
            Ty::Alias {
                path: path("A"),
                target: Box::new(Ty::Named(path("B"))),
            },
        ),
        declaration(
            "B",
            Ty::Alias {
                path: path("B"),
                target: Box::new(Ty::Applied {
                    path: path("Box"),
                    args: vec![Ty::Primitive("string".into())],
                }),
            },
        ),
        generic,
    ];
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        with_declarations(
            order.map(|index| declarations[index].clone()).to_vec(),
            |ctx, symbol| {
                assert!(ctx.diagnostics.is_empty(), "{:?}", ctx.diagnostics);
                let SymbolTypeFact::TypeAlias { target, .. } =
                    &ctx.signature_facts.symbol_types[&symbol]
                else {
                    panic!("alias");
                };
                let Some(Type::Applied { constructor, args }) = ctx.interner.store().get(*target)
                else {
                    panic!("applied nominal");
                };
                assert_eq!(args.len(), 1);
                assert!(matches!(
                    ctx.interner.store().get(args[0]),
                    Some(Type::Primitive(crate::PrimitiveType::String))
                ));
                let Some(Type::Nominal(nominal)) = ctx.interner.store().get(TypeId(constructor.0))
                else {
                    panic!("nominal identity");
                };
                assert_eq!(nominal.params, ["T"]);
                assert!(matches!(
                    nominal
                        .representation
                        .and_then(|id| ctx.interner.store().get(id)),
                    Some(Type::Record(_))
                ));
            },
        );
    }
}

#[test]
fn external_nominal_member_references_keep_the_predeclared_recursive_identity() {
    let record = |other| Ty::Record {
        fields: vec![crate::ExternalRecordFieldInput {
            name: "next".into(),
            ty: Ty::Option(Box::new(Ty::Named(path(other)))),
        }],
    };
    with_declarations(
        vec![declaration("A", record("B")), declaration("B", record("A"))],
        |ctx, _| {
            assert!(ctx.diagnostics.is_empty(), "{:?}", ctx.diagnostics);
            for (owner, other) in [("A", "B"), ("B", "A")] {
                let identity = ctx.external_type_paths[&path(owner)];
                let Type::Nominal(nominal) = ctx.interner.store().get(identity).unwrap() else {
                    panic!("nominal");
                };
                let Type::Record(record) = ctx
                    .interner
                    .store()
                    .get(nominal.representation.unwrap())
                    .unwrap()
                else {
                    panic!("record");
                };
                assert_eq!(
                    ctx.interner.store().get(record.fields[0].ty),
                    Some(&Type::Option(ctx.external_type_paths[&path(other)]))
                );
            }
        },
    );
}

#[test]
fn an_inline_nominal_alias_target_uses_the_declared_identity() {
    with_declarations(
        vec![
            declaration(
                "A",
                Ty::Alias {
                    path: path("A"),
                    target: Box::new(Ty::Nominal {
                        path: path("B"),
                        representation: Some(Box::new(Ty::Primitive("bool".into()))),
                    }),
                },
            ),
            declaration("B", Ty::Primitive("bool".into())),
        ],
        |ctx, symbol| {
            assert!(ctx.diagnostics.is_empty(), "{:?}", ctx.diagnostics);
            let SymbolTypeFact::TypeAlias { target, .. } =
                &ctx.signature_facts.symbol_types[&symbol]
            else {
                panic!("alias");
            };
            assert_eq!(*target, ctx.external_type_paths[&path("B")]);
        },
    );
}

#[test]
fn external_alias_cycles_are_rejected_at_the_real_import_span() {
    with_declarations(
        vec![
            declaration(
                "A",
                Ty::Alias {
                    path: path("A"),
                    target: Box::new(Ty::Named(path("B"))),
                },
            ),
            declaration(
                "B",
                Ty::Alias {
                    path: path("B"),
                    target: Box::new(Ty::Named(path("A"))),
                },
            ),
        ],
        |ctx, _| {
            assert!(
                ctx.diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains("recursive external alias")),
                "{:?}",
                ctx.diagnostics
            );
            assert!(
                ctx.diagnostics
                    .iter()
                    .all(|diagnostic| diagnostic.primary.span.source == etas_core::SourceId(73))
            );
        },
    );
}

#[test]
fn long_external_alias_chains_use_the_worklist_instead_of_the_rust_stack() {
    let mut declarations = vec![declaration(
        "A",
        Ty::Alias {
            path: path("A"),
            target: Box::new(Ty::Named(path("Link0"))),
        },
    )];
    for index in 0..4096 {
        let name = format!("Link{index}");
        let target = if index == 4095 {
            Ty::Primitive("i32".into())
        } else {
            Ty::Named(path(&format!("Link{}", index + 1)))
        };
        declarations.push(declaration(
            &name,
            Ty::Alias {
                path: path(&name),
                target: Box::new(target),
            },
        ));
    }
    with_declarations(declarations, |ctx, symbol| {
        assert!(ctx.diagnostics.is_empty(), "{:?}", ctx.diagnostics);
        let SymbolTypeFact::TypeAlias { target, .. } = &ctx.signature_facts.symbol_types[&symbol]
        else {
            panic!("alias");
        };
        assert_eq!(
            ctx.interner.store().get(*target),
            Some(&Type::Primitive(crate::PrimitiveType::I32))
        );
    });
}
