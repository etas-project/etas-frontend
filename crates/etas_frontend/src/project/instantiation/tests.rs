use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn recursion(
    growing: bool,
) -> (
    etas_hir::HirRevision,
    etas_types::TypeOutput,
    etas_effects::EffectFacts,
) {
    let call = if growing {
        "recur<Array<T>>([value]);"
    } else {
        "recur<T>(value);"
    };
    let source = format!(
        "flow recur<T>(value: T) -> unit ![] {{ {call} }} flow main() -> unit ![] {{ recur<i32>(0); }}"
    );
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let hir = etas_hir::HirRevision::try_new(hir).unwrap();
    let mut effects = etas_effects::EffectFacts::default();
    for (call, expr) in hir.exprs.iter() {
        let etas_hir::HirExpr::Call { callee, .. } = expr else {
            continue;
        };
        let etas_hir::HirExpr::Path(path) = &hir.exprs[*callee] else {
            panic!("fixture call must use a resolved source path");
        };
        let etas_hir::ResolveResult::Resolved(symbol) = path.resolution else {
            panic!("fixture call must be resolved");
        };
        let etas_hir::SymbolDef::Item { item } = hir.symbols.get(symbol).unwrap().def else {
            panic!("fixture target must be a source item");
        };
        effects
            .native_call_targets
            .insert(call, [etas_effects::EffectUnit::Item(item)].into());
    }
    assert_eq!(effects.native_call_targets.len(), 2);
    (hir, types, effects)
}

#[test]
fn a_recursive_instantiation_reaches_a_finite_fixed_point() {
    let (hir, types, effects) = recursion(false);
    let result = prepare(
        PreparationInput {
            hir: &hir,
            types: &types,
            effects: &effects,
            limits: etas_code::descriptor::DescriptorLimits {
                max_nodes: 8,
                ..Default::default()
            },
            control: None,
        },
        &[],
        None,
    );
    let prepared = match result {
        Ok(result) => result,
        Err(_) => panic!("regular recursion must converge"),
    };
    assert_eq!(prepared.plan.instances.len(), 1);
    assert!(
        matches!(&prepared.plan.instances[0].arguments[..], [NativeGenericArgument::Type(ty)] if matches!(prepared.store.get(*ty), Some(etas_types::Type::Primitive(etas_types::PrimitiveType::I32))))
    );
}

#[test]
fn an_expanding_instantiation_graph_reports_a_resource_error() {
    let (hir, types, effects) = recursion(true);
    let result = prepare(
        PreparationInput {
            hir: &hir,
            types: &types,
            effects: &effects,
            limits: etas_code::descriptor::DescriptorLimits {
                max_nodes: 4,
                ..Default::default()
            },
            control: None,
        },
        &[],
        None,
    );
    assert!(matches!(result, Err(InstantiationError::Resource { .. })));
}

#[test]
fn instantiation_work_remains_cooperatively_cancellable() {
    struct Stop(AtomicUsize);
    impl etas_utils::PipelineExecutionControl for Stop {
        fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 1 {
                return Err(etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    reason: "test stop".into(),
                });
            }
            Ok(())
        }
    }
    let (hir, types, effects) = recursion(true);
    let control = Stop(AtomicUsize::new(0));
    let result = prepare(
        PreparationInput {
            hir: &hir,
            types: &types,
            effects: &effects,
            limits: etas_code::descriptor::DescriptorLimits {
                max_nodes: 64,
                ..Default::default()
            },
            control: Some(&control),
        },
        &[],
        None,
    );
    assert!(matches!(
        result,
        Err(InstantiationError::Interrupted(
            etas_utils::PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::Cancelled,
                ..
            }
        ))
    ));
    assert_eq!(control.0.load(Ordering::SeqCst), 2);
}

#[test]
fn a_single_instantiation_can_be_cancelled_during_type_substitution() {
    struct Stop(AtomicUsize);
    impl etas_utils::PipelineExecutionControl for Stop {
        fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 1 {
                return Err(etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    reason: "cancel inside substitution".into(),
                });
            }
            Ok(())
        }
    }
    let (hir, types, mut effects) = recursion(false);
    effects.native_call_targets.clear();
    let owner = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "recur") =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    let integer = types
        .store
        .iter()
        .find_map(|(id, ty)| {
            matches!(
                ty,
                etas_types::Type::Primitive(etas_types::PrimitiveType::I32)
            )
            .then_some(id)
        })
        .unwrap();
    let seed = NativeInstantiation {
        owner,
        arguments: vec![NativeGenericArgument::Type(integer)],
    };
    let control = Stop(AtomicUsize::new(0));
    let result = prepare(
        PreparationInput {
            hir: &hir,
            types: &types,
            effects: &effects,
            limits: Default::default(),
            control: Some(&control),
        },
        &[seed],
        None,
    );
    assert!(matches!(
        result,
        Err(InstantiationError::Interrupted(
            etas_utils::PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::Cancelled,
                ..
            }
        ))
    ));
    assert_eq!(control.0.load(Ordering::SeqCst), 2);
}

#[test]
fn expanding_type_depth_is_a_resource_failure_before_native_stack_exhaustion() {
    let (hir, types, effects) = recursion(true);
    let result = prepare(
        PreparationInput {
            hir: &hir,
            types: &types,
            effects: &effects,
            limits: etas_code::descriptor::DescriptorLimits {
                max_nodes: 64,
                max_expansion_depth: 4,
                ..Default::default()
            },
            control: None,
        },
        &[],
        None,
    );
    assert!(
        matches!(result, Err(InstantiationError::Resource { reason, .. }) if reason.contains("expansion depth"))
    );
}

#[test]
fn a_wrong_kind_argument_is_not_an_open_template() {
    let signature = etas_types::CallableSignature {
        generic_params: vec![etas_types::CallableGenericParam {
            kind: etas_types::CallableGenericParamKind::Effect,
            name: "E".into(),
            subject: etas_types::TypeId(0),
            bounds: vec![],
        }],
        params: vec![],
        output: etas_types::TypeId(0),
        effects: None,
        requested_actions: None,
    };
    let instance = NativeInstantiation {
        owner: etas_hir::HirItemId(0),
        arguments: vec![NativeGenericArgument::Type(etas_types::TypeId(0))],
    };
    assert!(
        instance
            .bindings(Some(&signature))
            .unwrap_err()
            .contains("kind")
    );
}

#[test]
fn invocation_preparation_rejects_unmaterialized_deferred_rows() {
    let output = crate::Frontend.check_project(crate::ProjectInput::single_source(crate::SourceInput::anonymous("flow twice<effect E>(f: () -> unit ![E]) -> unit ![E] { f(); f(); } flow main() -> unit ![] { twice(() => { return; }); }")));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let effects = &checked.effects;
    let call = *effects.generic_effect_rows.keys().next().unwrap();
    let mut types = output.types.unwrap();
    let fact = Arc::make_mut(&mut types.facts)
        .generic_instantiations
        .get_mut(&call)
        .unwrap();
    assert!(fact.deferred_effect_row_obligations.is_empty());
    assert_eq!(
        fact.effect_row_bindings,
        vec![(
            "E".into(),
            etas_types::EffectRowRef {
                effects: vec![],
                tail: None
            }
        )]
    );
    fact.effect_row_bindings.clear();
    fact.deferred_effect_row_obligations
        .push(etas_types::DeferredEffectRowObligation {
            param: "E".into(),
            source: call,
        });
    let result = prepare(
        PreparationInput {
            hir: &checked.hir,
            types: &types,
            effects,
            limits: Default::default(),
            control: None,
        },
        &[],
        None,
    );
    assert!(
        matches!(result, Err(InstantiationError::Invalid(reason)) if reason.contains("unmaterialized deferred"))
    );
}
