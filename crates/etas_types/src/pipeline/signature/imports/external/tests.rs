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
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(73),
        None,
        "import dep.A;",
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
            spec_signatures: Vec::new(),
            spec_impls: Vec::new(),
            type_spec_satisfactions: Vec::new(),
            callable_spec_satisfactions: Vec::new(),
            trace_spec_conformances: Vec::new(),
            actions: Vec::new(),
        }],
        symbol_bindings: vec![crate::ExternalSymbolBindingInput {
            symbol,
            package,
            path: path("A"),
            span,
        }],
        ..Default::default()
    };
    let mut ctx = TypePipelineContext::new(&hir);
    apply_external_signature_input(&mut ctx, &input);
    assert(&ctx, symbol);
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
