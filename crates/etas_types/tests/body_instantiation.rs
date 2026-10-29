use std::sync::Arc;

use etas_core::{SourceFile, SourceId};
use etas_hir::{HirItem, HirItemId, HirProgram};
use etas_types::{
    BodyInstantiationError, CallableGenericArg, EffectArgRef, EffectRef, EffectRowRef,
    PrimitiveType, SignatureEnvironment, Type, TypeId, TypeOutput,
};

fn signatures(source: &str) -> (HirProgram, HirItemId, TypeOutput) {
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(91), None, source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    let item = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow) if hir.symbols.get(flow.symbol).unwrap().name == "target" => {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    let output = etas_types::build_signature_facts(&hir);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    (hir, item, output)
}

#[test]
fn instantiated_body_is_checked_under_its_bound_type_not_universal_t() {
    let (hir, item, output) = signatures(
        "flow boolean(value: bool) -> bool { return value; } flow target<T>(value: T) -> T ![] { let local: T = value; return !local; }",
    );
    let base = SignatureEnvironment::new(&hir, &output);
    let universal =
        etas_types::check_body_delta(&hir, &base, item, Arc::new(etas_std::standard_registry()));
    assert!(!universal.diagnostics.is_empty());
    let boolean = output
        .store
        .iter()
        .find_map(|(id, ty)| matches!(ty, Type::Primitive(PrimitiveType::Bool)).then_some(id))
        .unwrap();
    let instantiated = etas_types::check_instantiated_body_delta(
        &hir,
        &base,
        item,
        &[CallableGenericArg::Type(boolean)],
        Arc::new(etas_std::standard_registry()),
    )
    .unwrap();
    assert!(
        instantiated.diagnostics().is_empty(),
        "{:?}",
        instantiated.diagnostics()
    );
    assert_eq!(instantiated.signature().params, vec![boolean]);
    assert_eq!(instantiated.signature().output, boolean);
    let HirItem::Flow(flow) = &hir.items[item] else {
        panic!("flow owner");
    };
    assert!(
        matches!(instantiated.facts().symbol_types.get(&flow.params[0]), Some(etas_types::SymbolTypeFact::Param { ty }) if *ty == boolean)
    );
    assert!(
        matches!(output.facts.symbol_types.get(&flow.params[0]), Some(etas_types::SymbolTypeFact::Param { ty }) if matches!(output.store.get(*ty), Some(Type::Named(_))))
    );
    assert_eq!(instantiated.owner(), item);
    assert_eq!(instantiated.type_base(), base.type_base().identity());
}

fn boolean(output: &TypeOutput) -> TypeId {
    output
        .store
        .iter()
        .find_map(|(id, ty)| matches!(ty, Type::Primitive(PrimitiveType::Bool)).then_some(id))
        .unwrap()
}

struct Continue;
impl etas_utils::PipelineExecutionControl for Continue {
    fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
        Ok(())
    }
}

#[test]
fn extended_signature_base_accepts_bound_types_without_changing_the_template() {
    let (hir, item, output) = signatures("flow target<T>(value: T) -> T ![] { return value; }");
    let base = SignatureEnvironment::new(&hir, &output);
    let old_body =
        etas_types::check_body_delta(&hir, &base, item, Arc::new(etas_std::standard_registry()));
    let mut types = etas_types::TypeInterner::from_store(etas_types::TypeStore::with_base(
        base.type_base().clone(),
    ));
    let boolean = types.primitive(PrimitiveType::Bool);
    let pair = types.intern(Type::Tuple(vec![boolean, boolean]));
    assert!(pair.0 as usize >= base.type_base().len());
    let extended = base
        .extend_type_base_controlled(types.into_store(), usize::MAX, &Continue)
        .unwrap();
    assert_ne!(base.type_base().identity(), extended.type_base().identity());
    assert!(!extended.accepts(&old_body));
    assert!(base.accepts(&old_body));
    let result = etas_types::check_instantiated_body_delta(
        &hir,
        &extended,
        item,
        &[CallableGenericArg::Type(pair)],
        Arc::new(etas_std::standard_registry()),
    )
    .unwrap();
    assert!(
        result.diagnostics().is_empty(),
        "{:?}",
        result.diagnostics()
    );
    assert_eq!(result.type_base(), extended.type_base().identity());
    assert_eq!(result.signature().params, vec![pair]);
    assert_eq!(result.signature().output, pair);
    let Some(etas_types::ItemSignature::Flow(template)) = output.facts.item_signatures.get(&item)
    else {
        panic!("template");
    };
    assert_eq!(template.generic_params.len(), 1);
    assert_ne!(template.output, pair);
}

#[test]
fn extending_a_signature_base_rejects_an_equivalent_but_foreign_universe() {
    let (hir, _, output) = signatures("flow target<T>(value: T) -> T ![] { return value; }");
    let first = SignatureEnvironment::new(&hir, &output);
    let foreign = SignatureEnvironment::new(&hir, &output);
    assert_eq!(first.type_base().identity(), foreign.type_base().identity());
    assert!(!first.type_base().ptr_eq(foreign.type_base()));
    let mut types = etas_types::TypeInterner::from_store(etas_types::TypeStore::with_base(
        foreign.type_base().clone(),
    ));
    let boolean = types.primitive(PrimitiveType::Bool);
    types.intern(Type::Tuple(vec![boolean, boolean]));
    assert!(
        first
            .extend_type_base_controlled(types.into_store(), usize::MAX, &Continue)
            .is_err()
    );
    assert!(
        first
            .extend_type_base_controlled(etas_types::TypeStore::new(), usize::MAX, &Continue)
            .is_err()
    );
}

#[test]
fn extending_without_new_types_preserves_the_original_base() {
    let (hir, _, output) = signatures("flow target<T>(value: T) -> T ![] { return value; }");
    let first = SignatureEnvironment::new(&hir, &output);
    let same = first
        .extend_type_base_controlled(
            etas_types::TypeStore::with_base(first.type_base().clone()),
            usize::MAX,
            &Continue,
        )
        .unwrap();
    assert!(first.type_base().ptr_eq(same.type_base()));
}

#[test]
fn controlled_extension_preserves_the_existing_base_fingerprint() {
    let (hir, _, output) = signatures("flow target<T>(value: T) -> T ![] { return value; }");
    let base = SignatureEnvironment::new(&hir, &output);
    let mut types = etas_types::TypeInterner::from_store(etas_types::TypeStore::with_base(
        base.type_base().clone(),
    ));
    let boolean = types.primitive(PrimitiveType::Bool);
    types.intern(Type::Tuple(vec![boolean, boolean]));
    let store = types.into_store();
    let ordinary = etas_types::ty::SharedTypeBase::new(store.clone());
    let controlled = base
        .extend_type_base_controlled(store, usize::MAX, &Continue)
        .unwrap();
    assert_eq!(controlled.type_base().identity(), ordinary.identity());
    assert_eq!(controlled.type_base().len(), ordinary.len());
    let same = base
        .extend_type_base_controlled(
            etas_types::TypeStore::with_base(base.type_base().clone()),
            usize::MAX,
            &Continue,
        )
        .unwrap();
    assert!(same.type_base().ptr_eq(base.type_base()));
}

#[test]
fn controlled_extension_is_bounded_and_cancellable_without_changing_the_base() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct StopAfter {
        checks: AtomicUsize,
        allowed: usize,
    }
    impl etas_utils::PipelineExecutionControl for StopAfter {
        fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
            if self.checks.fetch_add(1, Ordering::Relaxed) >= self.allowed {
                return Err(etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    reason: "test cancellation".into(),
                });
            }
            Ok(())
        }
    }
    let (hir, _, output) = signatures("flow target<T>(value: T) -> T ![] { return value; }");
    let base = SignatureEnvironment::new(&hir, &output);
    let mut types = etas_types::TypeInterner::from_store(etas_types::TypeStore::with_base(
        base.type_base().clone(),
    ));
    let boolean = types.primitive(PrimitiveType::Bool);
    types.intern(Type::Tuple(vec![boolean, boolean]));
    let store = types.into_store();
    let original = base.type_base().identity();
    let allowed = StopAfter {
        checks: AtomicUsize::new(0),
        allowed: usize::MAX,
    };
    assert!(matches!(
        base.extend_type_base_controlled(store.clone(), base.type_base().len(), &allowed),
        Err(etas_types::SignatureBaseExtensionError::TypeLimit { .. })
    ));
    let count = store.len();
    for checks in [0, 3, 2 + count + 3, 2 + 2 * count + 3] {
        let stop = StopAfter {
            checks: AtomicUsize::new(0),
            allowed: checks,
        };
        assert!(
            matches!(base.extend_type_base_controlled(store.clone(), usize::MAX, &stop),
            Err(etas_types::SignatureBaseExtensionError::Interrupted(error)) if error.kind == etas_utils::PipelineInterruptionKind::Cancelled),
            "checkpoint {checks}"
        );
    }
    assert_eq!(base.type_base().identity(), original);
    let immediate = StopAfter {
        checks: AtomicUsize::new(0),
        allowed: 0,
    };
    assert!(matches!(
        base.extend_type_base_controlled(
            etas_types::TypeStore::with_base(base.type_base().clone()),
            usize::MAX,
            &immediate
        ),
        Err(etas_types::SignatureBaseExtensionError::Interrupted(_))
    ));
}

#[test]
fn relocated_callable_argument_and_effect_row_can_enter_the_signature_base() {
    use std::collections::HashMap;

    let (hir, item, output) =
        signatures("flow target<T, effect E>(value: T) -> T ![E] { return value; }");
    let base = SignatureEnvironment::new(&hir, &output);
    let mut source = etas_types::TypeInterner::new();
    let boolean = source.primitive(PrimitiveType::Bool);
    let function = source.intern(Type::Function(etas_types::FlowType {
        input: vec![boolean],
        output: boolean,
        effects: Some(console_row()),
    }));
    let source = source.into_store();
    assert!(
        base.extend_type_base_controlled(source.clone(), usize::MAX, &Continue)
            .is_err()
    );

    let mut target = etas_types::TypeInterner::from_store(etas_types::TypeStore::with_base(
        base.type_base().clone(),
    ));
    let declarations = HashMap::new();
    let effects = HashMap::from([(
        "Console.stdout_write".to_string(),
        "Console.stdout_write".to_string(),
    )]);
    let mut relocation = etas_types::TypeSubstitutionEngine::relocating_materialized(
        etas_types::ty::TypeRelocationMaterializationInput {
            source: &source,
            target: &mut target,
            declaration_types: &declarations,
            effect_names: &effects,
        },
    );
    let relocated_function = relocation.substitute(function).unwrap();
    let relocated_row = relocation.substitute_effect_row(console_row()).unwrap();
    let extended = base
        .extend_type_base_controlled(target.into_store(), usize::MAX, &Continue)
        .unwrap();
    let result = etas_types::check_instantiated_body_delta(
        &hir,
        &extended,
        item,
        &[
            CallableGenericArg::Type(relocated_function),
            CallableGenericArg::EffectRow(relocated_row.clone()),
        ],
        Arc::new(etas_std::standard_registry()),
    )
    .unwrap();
    assert!(
        result.diagnostics().is_empty(),
        "{:?}",
        result.diagnostics()
    );
    assert_eq!(result.signature().params, vec![relocated_function]);
    assert_eq!(result.signature().effects, Some(relocated_row));
}

fn console_row() -> EffectRowRef {
    EffectRowRef {
        effects: vec![EffectRef {
            name: "Console.stdout_write".into(),
            args: vec![],
        }],
        tail: None,
    }
}

#[test]
fn bound_types_and_rows_reach_nested_local_callable_annotations() {
    let (hir, item, output) = signatures(
        "flow boolean(value: bool) -> bool { return value; }
         flow target<T, effect E>(value: T, callback: () -> T ![E]) -> T ![E] {
             let selected: () -> T ![E] = callback;
             return selected();
         }",
    );
    let boolean = boolean(&output);
    let base = SignatureEnvironment::new(&hir, &output);
    let result = etas_types::check_instantiated_body_delta(
        &hir,
        &base,
        item,
        &[
            CallableGenericArg::Type(boolean),
            CallableGenericArg::EffectRow(console_row()),
        ],
        Arc::new(etas_std::standard_registry()),
    )
    .unwrap();
    assert!(
        result.diagnostics().is_empty(),
        "{:?}",
        result.diagnostics()
    );
    assert_eq!(result.signature().effects, Some(console_row()));
    assert!(result.facts().expr_types.values().any(|ty| {
        let node = output.store.get(*ty).or_else(|| {
            let index = (ty.0 as usize).checked_sub(result.base_len())?;
            result.local_types().get(index)
        });
        matches!(node, Some(Type::Function(flow)) if flow.output == boolean && flow.effects == Some(console_row()))
    }));
    let Some(etas_types::ItemSignature::Flow(template)) = output.facts.item_signatures.get(&item)
    else {
        panic!("flow signature");
    };
    assert_eq!(template.generic_params.len(), 2);
    assert_eq!(
        template.effects.as_ref().unwrap().tail.as_deref(),
        Some("E")
    );
}

#[test]
fn a_bound_effect_row_cannot_be_erased_by_an_explicit_generic_argument() {
    let (hir, item, output) = signatures(
        "flow boolean(value: bool) -> bool { return value; }
         flow pure_callback<T, effect F>(callback: () -> T ![F]) -> unit { return; }
         flow target<T, effect E>(value: T, callback: () -> T ![E]) -> T ![E] {
             pure_callback<T, ![]>(callback);
             return value;
         }",
    );
    let base = SignatureEnvironment::new(&hir, &output);
    for (row, valid) in [
        (console_row(), false),
        (
            EffectRowRef {
                effects: vec![],
                tail: None,
            },
            true,
        ),
    ] {
        let result = etas_types::check_instantiated_body_delta(
            &hir,
            &base,
            item,
            &[
                CallableGenericArg::Type(boolean(&output)),
                CallableGenericArg::EffectRow(row),
            ],
            Arc::new(etas_std::standard_registry()),
        )
        .unwrap();
        assert_eq!(
            result.diagnostics().is_empty(),
            valid,
            "{:?}",
            result.diagnostics()
        );
    }
}

#[test]
fn instantiated_body_checks_the_actual_type_against_declared_spec_bounds() {
    let (hir, item, output) = signatures(
        "spec Allowed;
         type Good;
         impl Good ~ Allowed;
         flow boolean(value: bool) -> bool { return value; }
         flow target<T ~ Allowed>(value: T) -> T ![] { return value; }",
    );
    let good = output
        .store
        .iter()
        .find_map(|(id, ty)| {
            matches!(ty, Type::Nominal(nominal) if nominal.name == "Good").then_some(id)
        })
        .unwrap();
    let base = SignatureEnvironment::new(&hir, &output);
    for (ty, valid) in [(good, true), (boolean(&output), false)] {
        let result = etas_types::check_instantiated_body_delta(
            &hir,
            &base,
            item,
            &[CallableGenericArg::Type(ty)],
            Arc::new(etas_std::standard_registry()),
        )
        .unwrap();
        assert_eq!(
            result.diagnostics().is_empty(),
            valid,
            "{:?}",
            result.diagnostics()
        );
        if !valid {
            assert!(
                result
                    .diagnostics()
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains("spec bound"))
            );
        }
    }
}

#[test]
fn bound_static_action_selectors_use_the_actual_type_identity() {
    let (hir, item, output) = signatures(
        "effect Probe { action read<R>(value: R) -> R; }
         flow boolean(value: bool) -> bool { return value; }
         flow target<R>(value: R) -> R ![Probe.read<R>] {
             return perform Probe.read<R>(value);
         }",
    );
    let boolean = boolean(&output);
    let base = SignatureEnvironment::new(&hir, &output);
    let result = etas_types::check_instantiated_body_delta(
        &hir,
        &base,
        item,
        &[CallableGenericArg::Type(boolean)],
        Arc::new(etas_std::standard_registry()),
    )
    .unwrap();
    assert!(
        result.diagnostics().is_empty(),
        "{:?}",
        result.diagnostics()
    );
    assert_eq!(
        result.signature().effects.as_ref().unwrap().effects[0].args,
        vec![EffectArgRef::Type(boolean)]
    );
    let perform = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| matches!(expr, etas_hir::HirExpr::Perform { .. }).then_some(id))
        .unwrap();
    assert_eq!(result.facts().expr_types[&perform], boolean);
}

#[test]
fn recursive_calls_and_other_generic_declarations_keep_their_own_binders() {
    for recursive in ["target(false)", "target<T>(false)"] {
        let (hir, item, output) = signatures(&format!(
            "flow boolean(value: bool) -> bool {{ return value; }}
             flow helper<T>(value: T) -> T ![] {{ return value; }}
             flow target<T>(value: T) -> T ![] {{
                 let number: i32 = helper(1);
                 if value {{ return {recursive}; }}
                 return !value;
             }}",
        ));
        let base = SignatureEnvironment::new(&hir, &output);
        let result = etas_types::check_instantiated_body_delta(
            &hir,
            &base,
            item,
            &[CallableGenericArg::Type(boolean(&output))],
            Arc::new(etas_std::standard_registry()),
        )
        .unwrap();
        assert!(
            result.diagnostics().is_empty(),
            "{recursive}: {:?}",
            result.diagnostics()
        );
    }
}

#[test]
fn invalid_generic_application_is_rejected_before_body_collection() {
    let (hir, item, output) = signatures("flow target<T>(value: T) -> T ![] { return value; }");
    let base = SignatureEnvironment::new(&hir, &output);
    for (arguments, expected) in [
        (
            vec![],
            BodyInstantiationError::ArgumentCount {
                expected: 1,
                actual: 0,
            },
        ),
        (
            vec![CallableGenericArg::EffectRow(console_row())],
            BodyInstantiationError::ArgumentKind { position: 0 },
        ),
        (
            vec![CallableGenericArg::Type(TypeId(u32::MAX))],
            BodyInstantiationError::MissingType(TypeId(u32::MAX)),
        ),
    ] {
        let error = etas_types::check_instantiated_body_delta(
            &hir,
            &base,
            item,
            &arguments,
            Arc::new(etas_std::standard_registry()),
        )
        .unwrap_err();
        assert_eq!(error, expected);
    }
}

#[test]
fn bound_body_keeps_nominal_arguments_opaque() {
    let (hir, item, output) =
        signatures("type Id = string; flow target<T>(value: T) -> T ![] { return \"not-an-id\"; }");
    let nominal = output
        .store
        .iter()
        .find_map(|(id, ty)| {
            matches!(ty, Type::Nominal(nominal) if nominal.name == "Id").then_some(id)
        })
        .unwrap();
    let base = SignatureEnvironment::new(&hir, &output);
    let result = etas_types::check_instantiated_body_delta(
        &hir,
        &base,
        item,
        &[CallableGenericArg::Type(nominal)],
        Arc::new(etas_std::standard_registry()),
    )
    .unwrap();
    assert_eq!(result.signature().output, nominal);
    assert!(
        result
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.message.contains("not assignable"))
    );
    assert!(
        result
            .diagnostics()
            .iter()
            .all(|diagnostic| diagnostic.primary.span.source == SourceId(91))
    );
}

#[test]
fn instantiated_body_retains_cancellation_without_publishing_a_partial_delta() {
    struct Cancel {
        checks: std::sync::atomic::AtomicUsize,
        after: usize,
    }
    impl etas_utils::PipelineExecutionControl for Cancel {
        fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
            if self
                .checks
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                < self.after
            {
                return Ok(());
            }
            Err(etas_utils::PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::Cancelled,
                reason: "cancelled instantiated body".into(),
            })
        }
    }
    let (hir, item, output) = signatures(&format!(
        "flow boolean(value: bool) -> bool {{ return value; }}
         flow target<T>(value: T) -> T ![] {{ {} return !value; }}",
        (0..128)
            .map(|index| format!("let local{index}: T = value;"))
            .collect::<String>(),
    ));
    let base = SignatureEnvironment::new(&hir, &output);
    for after in [0, 32] {
        let control = Cancel {
            checks: std::sync::atomic::AtomicUsize::new(0),
            after,
        };
        assert!(
            matches!(etas_types::check_instantiated_body_delta_controlled(
        &hir, &base, item, &[CallableGenericArg::Type(boolean(&output))], Arc::new(etas_std::standard_registry()), &control,
    ), Err(BodyInstantiationError::Interrupted(interruption)) if interruption.kind == etas_utils::PipelineInterruptionKind::Cancelled)
        );
        assert_eq!(
            control.checks.load(std::sync::atomic::Ordering::Relaxed),
            after + 1
        );
    }
    assert!(output.facts.expr_types.is_empty());
}

#[test]
fn phantom_parameters_do_not_hide_missing_types_in_argument_graphs() {
    let (hir, item, mut output) = signatures("flow target<T>() -> unit ![] { return; }");
    let mut interner = etas_types::TypeInterner::from_store((*output.store).clone());
    let invalid = interner.intern(Type::Array(TypeId(u32::MAX)));
    output.store = Arc::new(interner.into_store());
    let base = SignatureEnvironment::new(&hir, &output);
    assert!(matches!(
        etas_types::check_instantiated_body_delta(
            &hir,
            &base,
            item,
            &[CallableGenericArg::Type(invalid)],
            Arc::new(etas_std::standard_registry()),
        ),
        Err(BodyInstantiationError::Substitution(
            etas_types::TypeSubstitutionError::MissingType(TypeId(u32::MAX))
        ))
    ));
}

#[test]
fn bound_spec_arguments_use_all_of_the_declared_type_bindings() {
    let (hir, item, output) = signatures(
        "spec Tagged<U>;
         type Good;
         impl Good ~ Tagged<bool>;
         flow seeds(value: bool, other: i32) -> bool { return value; }
         flow target<T ~ Tagged<U>, U>(value: T, other: U) -> T ![] { return value; }",
    );
    let good = output
        .store
        .iter()
        .find_map(|(id, ty)| {
            matches!(ty, Type::Nominal(nominal) if nominal.name == "Good").then_some(id)
        })
        .unwrap();
    let integer = output
        .store
        .iter()
        .find_map(|(id, ty)| matches!(ty, Type::Primitive(PrimitiveType::I32)).then_some(id))
        .unwrap();
    let base = SignatureEnvironment::new(&hir, &output);
    for (argument, valid) in [(boolean(&output), true), (integer, false)] {
        let result = etas_types::check_instantiated_body_delta(
            &hir,
            &base,
            item,
            &[
                CallableGenericArg::Type(good),
                CallableGenericArg::Type(argument),
            ],
            Arc::new(etas_std::standard_registry()),
        )
        .unwrap();
        assert_eq!(
            result.diagnostics().is_empty(),
            valid,
            "{:?}",
            result.diagnostics()
        );
    }
}

#[test]
fn bound_row_is_merged_without_duplicating_explicit_members() {
    let (hir, item, output) = signatures(
        "flow target<effect E>(callback: () -> unit ![E]) -> unit ![Console.stdout_write, E] {
             let selected: () -> unit ![Console.stdout_write, E] = callback;
             selected();
         }",
    );
    let base = SignatureEnvironment::new(&hir, &output);
    let result = etas_types::check_instantiated_body_delta(
        &hir,
        &base,
        item,
        &[CallableGenericArg::EffectRow(console_row())],
        Arc::new(etas_std::standard_registry()),
    )
    .unwrap();
    assert!(
        result.diagnostics().is_empty(),
        "{:?}",
        result.diagnostics()
    );
    assert_eq!(result.signature().effects, Some(console_row()));
    assert!(result.facts().expr_types.values().any(|ty| {
        let node = output.store.get(*ty).or_else(|| {
            let index = (ty.0 as usize).checked_sub(result.base_len())?;
            result.local_types().get(index)
        });
        matches!(node, Some(Type::Function(flow)) if flow.effects == Some(console_row()))
    }));
}

#[test]
fn bound_arguments_cannot_be_inferred_from_the_replacement_body() {
    let (hir, item, mut output) =
        signatures("flow target<T>(value: T) -> T ![] { return !value; }");
    let mut interner = etas_types::TypeInterner::from_store((*output.store).clone());
    let variable = interner.intern(Type::Var(etas_types::TypeVarId(713)));
    let nested = interner.intern(Type::Array(variable));
    let unit = interner.primitive(PrimitiveType::Unit);
    let deferred_flow = interner.intern(Type::Function(etas_types::FlowType {
        input: vec![],
        output: unit,
        effects: None,
    }));
    output.store = Arc::new(interner.into_store());
    let base = SignatureEnvironment::new(&hir, &output);
    for ty in [variable, nested] {
        assert!(matches!(etas_types::check_instantiated_body_delta(
            &hir, &base, item, &[CallableGenericArg::Type(ty)], Arc::new(etas_std::standard_registry()),
        ), Err(BodyInstantiationError::Substitution(etas_types::TypeSubstitutionError::UnresolvedType(found))) if found == variable));
    }
    assert!(matches!(etas_types::check_instantiated_body_delta(
        &hir, &base, item, &[CallableGenericArg::Type(deferred_flow)], Arc::new(etas_std::standard_registry()),
    ), Err(BodyInstantiationError::Substitution(etas_types::TypeSubstitutionError::UnresolvedType(found))) if found == deferred_flow));
}

#[test]
fn bound_effect_arguments_must_already_be_closed() {
    let (hir, item, output) = signatures("flow target<effect E>() -> unit ![E] { return; }");
    let base = SignatureEnvironment::new(&hir, &output);
    assert!(matches!(etas_types::check_instantiated_body_delta(
        &hir, &base, item, &[CallableGenericArg::EffectRow(EffectRowRef { effects: vec![], tail: Some("outer".into()) })],
        Arc::new(etas_std::standard_registry()),
    ), Err(BodyInstantiationError::Substitution(etas_types::TypeSubstitutionError::OpenEffectRow(tail))) if tail == "outer"));
}
