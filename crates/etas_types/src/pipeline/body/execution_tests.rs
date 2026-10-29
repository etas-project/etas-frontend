use std::cell::Cell;

use etas_hir::{HirExpr, HirItem, HirItemId, HirProgram};
use etas_utils::{PipelineInterruption, PipelineInterruptionKind};

use super::{collect::entry::collect_body_item, state::BodyPipelineState};
use crate::pipeline::context::{BodyCollectContext, TypePipelineContext};

fn program(source: &str) -> (HirProgram, HirItemId) {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(17),
        None,
        source,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    let item = hir
        .items
        .iter()
        .find_map(|(item, node)| match node {
            HirItem::Flow(flow) if hir.symbols.get(flow.symbol).unwrap().name == "main" => {
                Some(item)
            }
            _ => None,
        })
        .unwrap();
    (hir, item)
}

fn context(hir: &HirProgram) -> TypePipelineContext<'_> {
    let mut ctx = TypePipelineContext::new(hir);
    crate::pipeline::signature::run_for_hir(&mut ctx);
    assert!(ctx.diagnostics.is_empty(), "{:?}", ctx.diagnostics);
    ctx
}

fn interrupt_at(
    checks: &Cell<usize>,
    stop: usize,
    kind: PipelineInterruptionKind,
) -> impl Fn() -> Result<(), PipelineInterruption> + '_ {
    move || {
        checks.set(checks.get() + 1);
        if checks.get() == stop {
            Err(PipelineInterruption {
                kind,
                reason: "body collection interrupted".into(),
            })
        } else {
            Ok(())
        }
    }
}

fn provisional_nodes(state: &BodyPipelineState) -> usize {
    state.provisional.expr_types.len()
        + state.provisional.stmt_types.len()
        + state.provisional.pat_types.len()
}

fn type_names<K: Copy + Ord>(
    types: &std::collections::HashMap<K, crate::TypeId>,
    store: &crate::TypeStore,
) -> std::collections::BTreeMap<K, String> {
    types
        .iter()
        .map(|(id, ty)| (*id, crate::ty::display_type(store, *ty)))
        .collect()
}

#[test]
fn body_collection_interrupts_inside_wide_literals_calls_and_patterns() {
    let values = (0..128).map(|i| i.to_string()).collect::<Vec<_>>();
    let fields = (0..128).map(|i| format!("f{i} = {i}")).collect::<Vec<_>>();
    let params = (0..128).map(|i| format!("p{i}: i32")).collect::<Vec<_>>();
    let bindings = (0..128).map(|i| format!("p{i}")).collect::<Vec<_>>();
    for source in [
        format!(
            "flow main() -> Array<i32> {{ return [{}]; }}",
            values.join(";")
        ),
        format!("flow main() {{ return ({}); }}", values.join(",")),
        format!("flow main() {{ return {{ {} }}; }}", fields.join(",")),
        format!(
            "flow consume({}) -> i32 {{ return 0; }} flow main() -> i32 {{ return consume({}); }}",
            params.join(","),
            values.join(","),
        ),
        format!(
            "flow main(value: ({})) -> unit {{ let ({}) = value; return; }}",
            vec!["i32"; 128].join(","),
            bindings.join(","),
        ),
    ] {
        let (hir, item) = program(&source);
        let mut complete = context(&hir);
        let mut full_state = BodyPipelineState::new(item);
        let checks = Cell::new(0);
        collect_body_item(&mut complete, &mut full_state, &|| {
            checks.set(checks.get() + 1);
            Ok::<_, std::convert::Infallible>(())
        })
        .unwrap();
        assert!(provisional_nodes(&full_state) >= 128);
        let stop = checks.get() / 2;
        assert!(stop > 16);
        for kind in [
            PipelineInterruptionKind::Cancelled,
            PipelineInterruptionKind::ResourceLimit,
        ] {
            let mut ctx = context(&hir);
            let mut state = BodyPipelineState::new(item);
            let checks = Cell::new(0);
            let error = collect_body_item(&mut ctx, &mut state, &interrupt_at(&checks, stop, kind))
                .unwrap_err();
            assert_eq!(error.kind, kind);
            assert_eq!(checks.get(), stop);
            assert!(provisional_nodes(&state) > 0);
            assert!(provisional_nodes(&state) < provisional_nodes(&full_state));
            assert!(state.solver_report.inferred_expr_types.is_empty());
            assert!(state.solver_report.failures.is_empty());
            assert!(ctx.signature_facts.expr_types.is_empty());
            assert!(ctx.signature_facts.stmt_types.is_empty());
            assert!(ctx.signature_facts.pattern_types.is_empty());
            assert!(ctx.diagnostics.is_empty());
        }
    }
}

#[test]
fn body_pipeline_does_not_validate_or_materialize_an_interrupted_collection() {
    let (hir, item) = program(&format!(
        "flow main() -> unit {{ {} let bad: bool = 123; return; }}",
        "let value = 1;".repeat(128),
    ));
    for kind in [
        PipelineInterruptionKind::Cancelled,
        PipelineInterruptionKind::ResourceLimit,
    ] {
        let mut ctx = context(&hir);
        let symbols = ctx.signature_facts.symbol_types.len();
        let checks = Cell::new(0);
        let error = super::run_with_checkpoint(&mut ctx, item, &interrupt_at(&checks, 64, kind))
            .unwrap_err();
        assert_eq!(error.kind, kind);
        assert_eq!(checks.get(), 64);
        assert!(ctx.diagnostics.is_empty());
        assert!(ctx.signature_facts.expr_types.is_empty());
        assert!(ctx.signature_facts.stmt_types.is_empty());
        assert!(ctx.signature_facts.pattern_types.is_empty());
        assert_eq!(ctx.signature_facts.symbol_types.len(), symbols);

        super::run(&mut ctx, item);
        assert!(ctx.diagnostics.iter().any(|d| d.code
            == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::TypeMismatch)));
        assert!(!ctx.signature_facts.expr_types.is_empty());
    }
}

#[test]
fn interrupted_lambda_and_handler_restore_their_local_collection_scopes() {
    for source in [
        format!(
            "flow main() -> unit {{ let callback = () => {{ {} return; }}; return; }}",
            "let value = 1;".repeat(64),
        ),
        format!(
            "effect Probe {{ action ask() -> unit; }} flow main() -> unit {{ let h = handler {{ Probe.ask() => {{ {} resume; }} }}; return; }}",
            "let value = 1;".repeat(64),
        ),
    ] {
        let (hir, item) = program(&source);
        let expr = hir
            .exprs
            .iter()
            .find_map(|(id, expr)| {
                matches!(expr, HirExpr::Lambda { .. } | HirExpr::Handler { .. }).then_some(id)
            })
            .unwrap();
        let mut ctx = context(&hir);
        let unit = ctx.interner.primitive(crate::PrimitiveType::Unit);
        let mut state = BodyPipelineState::new(item);
        state.expected_return = Some(unit);
        state.inferred_returns = Some(Vec::new());
        state.handler_depth = 2;
        state.handler_resume_stack = vec![unit];
        state.handler_finish_stack = vec![Some(unit)];
        state.handler_finish_candidates = vec![Vec::new()];
        let expected = match hir.exprs[expr] {
            HirExpr::Handler { .. } => {
                ctx.interner
                    .intern(crate::Type::Handler(crate::HandlerType {
                        handled: crate::EffectRowRef {
                            effects: Vec::new(),
                            tail: None,
                        },
                        produced: crate::HandlerProducedEffects::Infer,
                        result: Some(unit),
                    }))
            }
            _ => ctx.interner.intern(crate::Type::Function(crate::FlowType {
                input: Vec::new(),
                output: unit,
                effects: None,
            })),
        };
        let checks = Cell::new(0);
        let execution = interrupt_at(&checks, 24, PipelineInterruptionKind::Cancelled);
        let error = super::collect::expr::collect_expr(
            &mut BodyCollectContext {
                ctx: &mut ctx,
                state: &mut state,
                execution: &execution,
            },
            expr,
            Some(expected),
        )
        .unwrap_err();
        assert_eq!(error.kind, PipelineInterruptionKind::Cancelled);
        assert_eq!(checks.get(), 24);
        assert_eq!(state.expected_return, Some(unit));
        assert_eq!(state.inferred_returns, Some(Vec::new()));
        assert_eq!(state.handler_depth, 2);
        assert_eq!(state.handler_resume_stack, vec![unit]);
        assert_eq!(state.handler_finish_stack, vec![Some(unit)]);
        assert_eq!(state.handler_finish_candidates, vec![Vec::new()]);
        assert!(!state.provisional.expr_types.is_empty());
        assert!(!state.provisional.expr_types.contains_key(&expr));
    }
}

#[test]
fn successful_controlled_collection_matches_the_existing_body_pipeline() {
    let (hir, item) = program(
        "flow identity<T>(value: T) -> T { return value; } flow main() -> i32 { let (a, b) = (1, 2); let record = { value = a }; let xs = [record.value; b]; if true { return identity(xs[0]); } return b; }",
    );
    let mut ordinary = context(&hir);
    super::run(&mut ordinary, item);
    assert!(
        ordinary.diagnostics.is_empty(),
        "{:?}",
        ordinary.diagnostics
    );
    let mut controlled = context(&hir);
    let checks = Cell::new(0);
    super::run_with_checkpoint(&mut controlled, item, &|| {
        checks.set(checks.get() + 1);
        Ok::<_, std::convert::Infallible>(())
    })
    .unwrap();
    assert!(checks.get() > 30);
    assert!(
        controlled.diagnostics.is_empty(),
        "{:?}",
        controlled.diagnostics
    );
    assert_eq!(
        type_names(
            &ordinary.signature_facts.expr_types,
            ordinary.interner.store()
        ),
        type_names(
            &controlled.signature_facts.expr_types,
            controlled.interner.store()
        )
    );
    assert_eq!(
        type_names(
            &ordinary.signature_facts.stmt_types,
            ordinary.interner.store()
        ),
        type_names(
            &controlled.signature_facts.stmt_types,
            controlled.interner.store()
        )
    );
    assert_eq!(
        type_names(
            &ordinary.signature_facts.pattern_types,
            ordinary.interner.store()
        ),
        type_names(
            &controlled.signature_facts.pattern_types,
            controlled.interner.store()
        )
    );
}
