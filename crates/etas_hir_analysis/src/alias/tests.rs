use std::collections::BTreeMap;

use etas_core::{SourceId, Span, TextSize};
use etas_hir::{
    HirArg, HirBlock, HirBlockId, HirExpr, HirExprId, HirFieldInit, HirFlowBody, HirFlowDecl,
    HirItem, HirItemId, HirLambdaBody, HirLiteral, HirModule, HirPat, HirPatId, HirStmt, HirStmtId,
    ResolveResult, ScopeId, ScopeOwner, SymbolId, unresolved_path_from_segments,
};

use crate::{
    alias::{
        AliasAnalysisInput, AliasCallTarget, AliasConstraint, AliasConstraintFrame,
        AliasConstraintModel, AliasContext, AliasOracle, AliasPrecisionConfig, AliasSummary,
        AliasTarget, AliasValue, AliasValueExpr, AliasVar, AllocationKind, ContextSensitivity,
        FieldSensitivity, FlowSensitivity, HeapModel, IndexSensitivity, Place, analyze_aliases,
    },
    unit::{HirSemanticUnit, HirSemanticUnitCollector},
};

#[derive(Default)]
struct TestOracle {
    calls: BTreeMap<HirExprId, AliasCallTarget>,
}

impl AliasOracle for TestOracle {
    fn symbol_target(&self, symbol: SymbolId) -> Option<AliasTarget> {
        Some(AliasTarget::Symbol(symbol))
    }

    fn allocation_site(&self, _expr: HirExprId, _kind: AllocationKind) -> Option<AliasTarget> {
        None
    }

    fn intrinsic_call_summary(
        &self,
        _call: HirExprId,
        _callee: HirExprId,
    ) -> Option<crate::alias::AliasIntrinsicSummary> {
        None
    }

    fn resolved_call_target(&self, call: HirExprId, _callee: HirExprId) -> AliasCallTarget {
        self.calls
            .get(&call)
            .copied()
            .unwrap_or(AliasCallTarget::External)
    }

    fn resource_constructor(&self, _call: HirExprId) -> Option<AliasTarget> {
        None
    }
}

#[derive(Default)]
struct HirBuilder {
    program: etas_hir::HirProgram,
    next_symbol: u32,
}

impl HirBuilder {
    fn symbol(&mut self) -> SymbolId {
        let symbol = SymbolId(self.next_symbol);
        self.next_symbol += 1;
        symbol
    }

    fn path(&mut self, symbol: SymbolId) -> HirExprId {
        let mut path = unresolved_path_from_segments(&["x"], span());
        path.resolution = ResolveResult::Resolved(symbol);
        self.expr(HirExpr::Path(path))
    }

    fn bool_lit(&mut self) -> HirExprId {
        self.expr(HirExpr::Literal(HirLiteral::Bool {
            value: true,
            span: span(),
        }))
    }

    fn string_lit(&mut self, value: &str) -> HirExprId {
        self.expr(HirExpr::Literal(HirLiteral::String {
            value: value.to_owned(),
            span: span(),
        }))
    }

    fn record(&mut self, fields: Vec<(&str, HirExprId)>) -> HirExprId {
        self.expr(HirExpr::Record(etas_hir::HirRecordExpr {
            path: None,
            generic_args: Vec::new(),
            fields: fields
                .into_iter()
                .map(|(name, value)| HirFieldInit::Named {
                    name: name.to_owned(),
                    value,
                    span: span(),
                })
                .collect(),
            span: span(),
        }))
    }

    fn map(&mut self, entries: Vec<(HirExprId, HirExprId)>) -> HirExprId {
        self.expr(HirExpr::Map {
            entries: entries
                .into_iter()
                .map(|(key, value)| etas_hir::HirMapEntry {
                    key,
                    value,
                    span: span(),
                })
                .collect(),
            span: span(),
        })
    }

    fn field(&mut self, base: HirExprId, field: &str) -> HirExprId {
        self.expr(HirExpr::Field {
            base,
            field: field.to_owned(),
            span: span(),
        })
    }

    fn index(&mut self, base: HirExprId, index: HirExprId) -> HirExprId {
        self.expr(HirExpr::Index {
            base,
            index,
            span: span(),
        })
    }

    fn call(&mut self, arg: HirExprId) -> HirExprId {
        let callee = self.bool_lit();
        self.expr(HirExpr::Call {
            callee,
            generic_args: Vec::new(),
            args: vec![HirArg::Positional(arg)],
            span: span(),
        })
    }

    fn call_with_callee(&mut self, callee: HirExprId, args: Vec<HirExprId>) -> HirExprId {
        self.expr(HirExpr::Call {
            callee,
            generic_args: Vec::new(),
            args: args.into_iter().map(HirArg::Positional).collect(),
            span: span(),
        })
    }

    fn lambda_expr(&mut self, params: Vec<SymbolId>, body: HirExprId) -> HirExprId {
        self.expr(HirExpr::Lambda {
            params,
            body: HirLambdaBody::Expr(body),
            scope: ScopeId(0),
            span: span(),
        })
    }

    fn if_expr(
        &mut self,
        cond: HirExprId,
        then_block: HirBlockId,
        else_block: HirBlockId,
    ) -> HirExprId {
        self.expr(HirExpr::If {
            cond,
            then_block,
            else_branch: Some(etas_hir::HirElseBranch::Block(else_block)),
            span: span(),
        })
    }

    fn expr(&mut self, expr: HirExpr) -> HirExprId {
        self.program.exprs.alloc(expr)
    }

    fn pat(&mut self, symbol: SymbolId) -> HirPatId {
        self.program.pats.alloc(HirPat::Binding {
            symbol,
            span: span(),
        })
    }

    fn let_stmt(&mut self, symbol: SymbolId, value: HirExprId) -> HirStmtId {
        let pat = self.pat(symbol);
        self.program.stmts.alloc(HirStmt::Let {
            pat,
            type_annotation: None,
            value,
            span: span(),
        })
    }

    fn var_stmt(&mut self, symbol: SymbolId, value: HirExprId) -> HirStmtId {
        let pat = self.pat(symbol);
        self.program.stmts.alloc(HirStmt::Var {
            pat,
            type_annotation: None,
            value,
            span: span(),
        })
    }

    fn assign_stmt(&mut self, target: HirExprId, value: HirExprId) -> HirStmtId {
        self.program.stmts.alloc(HirStmt::Assign {
            target,
            value,
            span: span(),
        })
    }

    fn expr_stmt(&mut self, expr: HirExprId) -> HirStmtId {
        self.program
            .stmts
            .alloc(HirStmt::Expr { expr, span: span() })
    }

    fn return_stmt(&mut self, value: HirExprId) -> HirStmtId {
        self.program.stmts.alloc(HirStmt::Return {
            value: Some(value),
            span: span(),
        })
    }

    fn block(&mut self, stmts: Vec<HirStmtId>, final_expr: Option<HirExprId>) -> HirBlockId {
        self.program.blocks.alloc_with_id(|id| HirBlock {
            id,
            stmts,
            final_expr,
            scope: ScopeId(0),
            span: span(),
        })
    }

    fn flow(&mut self, params: Vec<SymbolId>, body: HirBlockId) -> HirItemId {
        let symbol = self.symbol();
        self.program.items.alloc(HirItem::Flow(HirFlowDecl {
            symbol,
            type_params: Vec::new(),
            params,
            return_type: None,
            effects: None,
            conformances: Vec::new(),
            body: HirFlowBody::Block(body),
            scope: ScopeId(0),
            span: span(),
        }))
    }

    fn finish(self) -> etas_hir::HirProgram {
        let mut program = self.program;
        let items = program
            .items
            .iter()
            .map(|(item, _)| item)
            .collect::<Vec<_>>();
        let module = program.modules_arena.alloc_with_id(|id| HirModule {
            id,
            name: None,
            imports: Vec::new(),
            items,
            scope: ScopeId(0),
            span: span(),
        });
        let scope = program
            .scopes
            .alloc(None, ScopeOwner::Module(module), span());
        program
            .modules_arena
            .get_mut(module)
            .expect("new test module should exist")
            .scope = scope;
        program.modules.push(module);
        program
    }
}

fn span() -> Span {
    Span::empty(SourceId(0), TextSize::ZERO)
}

fn run(
    program: &etas_hir::HirProgram,
    config: AliasPrecisionConfig,
    oracle: TestOracle,
) -> crate::alias::AliasAnalysisOutput {
    let context = crate::HirAnalysisContext::new(program);
    let units = HirSemanticUnitCollector::collect_with_context(program, &context);
    analyze_aliases(AliasAnalysisInput {
        hir: program,
        units,
        config,
        oracle,
    })
}

fn item_summary(output: &crate::alias::AliasAnalysisOutput, item: HirItemId) -> &AliasSummary {
    output
        .summaries
        .get(HirSemanticUnit::Item(item))
        .expect("item summary")
}

fn assert_contains_place(value: &AliasValue, place: &Place) {
    assert!(
        value
            .may
            .places()
            .is_some_and(|places| places.contains(place)),
        "expected {value:?} to contain {place:?}"
    );
}

#[test]
fn balanced_config_uses_hybrid_constraint_model() {
    match AliasPrecisionConfig::balanced().constraint_model {
        AliasConstraintModel::Hybrid {
            pre_unify_locals,
            inclusion_for_resources,
            inclusion_for_public_api,
        } => {
            assert!(pre_unify_locals);
            assert!(inclusion_for_resources);
            assert!(inclusion_for_public_api);
        }
        other => panic!("balanced config should use hybrid model, got {other:?}"),
    }
}

#[test]
fn inclusion_constraint_solver_propagates_subset_copy() {
    let src = AliasVar::Synthetic(0);
    let dst = AliasVar::Synthetic(1);
    let place = Place::new(AliasTarget::MemoryPlace(vec![
        "ProjectMemory".to_owned(),
        "Papers".to_owned(),
    ]));
    let mut frame = AliasConstraintFrame::new(AliasPrecisionConfig::precise());
    frame.seed(src.clone(), AliasValue::from_place(place.clone()));
    frame.add_constraint(AliasConstraint::Copy {
        dst: dst.clone(),
        src,
    });

    let solution = frame.solve();

    assert!(!solution.incomplete);
    assert_contains_place(&solution.value(&dst), &place);
}

#[test]
fn unification_constraint_solver_merges_equivalent_vars() {
    let left = AliasVar::Synthetic(0);
    let right = AliasVar::Synthetic(1);
    let left_place = Place::new(AliasTarget::Symbol(SymbolId(10)));
    let right_place = Place::new(AliasTarget::Symbol(SymbolId(11)));
    let mut frame = AliasConstraintFrame::new(AliasPrecisionConfig::fast());
    frame.seed(left.clone(), AliasValue::from_place(left_place.clone()));
    frame.seed(right.clone(), AliasValue::from_place(right_place.clone()));
    frame.add_constraint(AliasConstraint::Copy {
        dst: left.clone(),
        src: right.clone(),
    });

    let solution = frame.solve();

    assert!(!solution.incomplete);
    assert_contains_place(&solution.value(&left), &left_place);
    assert_contains_place(&solution.value(&left), &right_place);
    assert_contains_place(&solution.value(&right), &left_place);
    assert_contains_place(&solution.value(&right), &right_place);
}

#[test]
fn hybrid_resource_constraints_do_not_pollute_resource_source() {
    let resource = AliasVar::Synthetic(0);
    let normal = AliasVar::Synthetic(1);
    let local = AliasVar::Synthetic(2);
    let resource_place = Place::new(AliasTarget::MemoryPlace(vec![
        "ProjectMemory".to_owned(),
        "Papers".to_owned(),
    ]));
    let normal_place = Place::new(AliasTarget::Symbol(SymbolId(99)));
    let mut frame = AliasConstraintFrame::new(AliasPrecisionConfig::balanced());
    frame.seed(
        resource.clone(),
        AliasValue::from_place(resource_place.clone()),
    );
    frame.seed(normal.clone(), AliasValue::from_place(normal_place.clone()));
    frame.add_constraint(AliasConstraint::Copy {
        dst: local.clone(),
        src: resource.clone(),
    });
    frame.add_constraint(AliasConstraint::Copy {
        dst: local,
        src: normal,
    });

    let solution = frame.solve();

    assert_contains_place(&solution.value(&resource), &resource_place);
    assert!(
        !solution
            .value(&resource)
            .may
            .places()
            .is_some_and(|places| places.contains(&normal_place)),
        "resource source must not be unified with ordinary locals"
    );
}

#[test]
fn propagates_let_alias_to_return() {
    let mut builder = HirBuilder::default();
    let p = builder.symbol();
    let x = builder.symbol();
    let p_path = builder.path(p);
    let let_x = builder.let_stmt(x, p_path);
    let x_path = builder.path(x);
    let body = builder.block(vec![let_x], Some(x_path));
    let item = builder.flow(vec![p], body);
    let program = builder.finish();

    let output = run(
        &program,
        AliasPrecisionConfig::balanced(),
        TestOracle::default(),
    );

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(
        item_summary(&output, item).return_alias,
        AliasValueExpr::FormalParam(0)
    );
}

#[test]
fn strong_update_replaces_var_alias() {
    let mut builder = HirBuilder::default();
    let p = builder.symbol();
    let q = builder.symbol();
    let x = builder.symbol();
    let p_path = builder.path(p);
    let var_x = builder.var_stmt(x, p_path);
    let x_target = builder.path(x);
    let q_path = builder.path(q);
    let assign = builder.assign_stmt(x_target, q_path);
    let x_path = builder.path(x);
    let body = builder.block(vec![var_x, assign], Some(x_path));
    let item = builder.flow(vec![p, q], body);
    let program = builder.finish();

    let output = run(
        &program,
        AliasPrecisionConfig::balanced(),
        TestOracle::default(),
    );

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(
        item_summary(&output, item).return_alias,
        AliasValueExpr::FormalParam(1)
    );
}

#[test]
fn branch_join_weakens_alias_to_union() {
    let mut builder = HirBuilder::default();
    let p = builder.symbol();
    let q = builder.symbol();
    let x = builder.symbol();
    let init = {
        let p_path = builder.path(p);
        builder.var_stmt(x, p_path)
    };
    let then_block = {
        let x_target = builder.path(x);
        let p_path = builder.path(p);
        let stmt = builder.assign_stmt(x_target, p_path);
        builder.block(vec![stmt], None)
    };
    let else_block = {
        let x_target = builder.path(x);
        let q_path = builder.path(q);
        let stmt = builder.assign_stmt(x_target, q_path);
        builder.block(vec![stmt], None)
    };
    let cond = builder.bool_lit();
    let if_expr = builder.if_expr(cond, then_block, else_block);
    let if_stmt = builder.expr_stmt(if_expr);
    let x_path = builder.path(x);
    let body = builder.block(vec![init, if_stmt], Some(x_path));
    let item = builder.flow(vec![p, q], body);
    let program = builder.finish();

    let output = run(
        &program,
        AliasPrecisionConfig::balanced(),
        TestOracle::default(),
    );

    let expected = AliasValueExpr::Union(vec![
        AliasValueExpr::FormalParam(0),
        AliasValueExpr::FormalParam(1),
    ]);
    assert_eq!(item_summary(&output, item).return_alias, expected);
}

#[test]
fn reads_field_sensitive_record_alias() {
    let mut builder = HirBuilder::default();
    let p = builder.symbol();
    let r = builder.symbol();
    let p_path = builder.path(p);
    let record = builder.record(vec![("a", p_path)]);
    let let_r = builder.let_stmt(r, record);
    let r_path = builder.path(r);
    let field = builder.field(r_path, "a");
    let body = builder.block(vec![let_r], Some(field));
    let item = builder.flow(vec![p], body);
    let program = builder.finish();

    let output = run(
        &program,
        AliasPrecisionConfig::balanced(),
        TestOracle::default(),
    );

    assert_eq!(
        item_summary(&output, item).return_alias,
        AliasValueExpr::FormalParam(0)
    );
}

#[test]
fn key_sensitive_map_alias_uses_configured_projection() {
    let mut builder = HirBuilder::default();
    let p = builder.symbol();
    let m = builder.symbol();
    let key = builder.string_lit("k");
    let p_path = builder.path(p);
    let map = builder.map(vec![(key, p_path)]);
    let let_m = builder.let_stmt(m, map);
    let m_path = builder.path(m);
    let index_key = builder.string_lit("k");
    let index = builder.index(m_path, index_key);
    let body = builder.block(vec![let_m], Some(index));
    let item = builder.flow(vec![p], body);
    let program = builder.finish();
    let mut config = AliasPrecisionConfig::balanced();
    config.index_sensitivity = IndexSensitivity::KeySensitive;

    let output = run(&program, config, TestOracle::default());

    assert_eq!(
        item_summary(&output, item).return_alias,
        AliasValueExpr::FormalParam(0)
    );
}

#[test]
fn applies_direct_interprocedural_return_summary() {
    let mut builder = HirBuilder::default();
    let callee_param = builder.symbol();
    let callee_return = builder.path(callee_param);
    let callee_return_stmt = builder.return_stmt(callee_return);
    let callee_body = builder.block(vec![callee_return_stmt], None);
    let callee_item = builder.flow(vec![callee_param], callee_body);

    let caller_param = builder.symbol();
    let caller_arg = builder.path(caller_param);
    let call = builder.call(caller_arg);
    let caller_body = builder.block(Vec::new(), Some(call));
    let caller_item = builder.flow(vec![caller_param], caller_body);
    let program = builder.finish();

    let mut oracle = TestOracle::default();
    oracle.calls.insert(
        call,
        AliasCallTarget::Direct(HirSemanticUnit::Item(callee_item)),
    );

    let output = run(&program, AliasPrecisionConfig::balanced(), oracle);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(
        item_summary(&output, caller_item).return_alias,
        AliasValueExpr::FormalParam(0)
    );
}

#[test]
fn unknown_external_call_fails_closed() {
    let mut builder = HirBuilder::default();
    let p = builder.symbol();
    let arg = builder.path(p);
    let call = builder.call(arg);
    let body = builder.block(Vec::new(), Some(call));
    let item = builder.flow(vec![p], body);
    let program = builder.finish();

    let output = run(
        &program,
        AliasPrecisionConfig::balanced(),
        TestOracle::default(),
    );

    assert!(!output.diagnostics.is_empty());
    assert!(output.facts.unknown_exprs.contains(&call));
    assert_eq!(
        item_summary(&output, item).return_alias,
        AliasValueExpr::Unknown
    );
}

#[test]
fn precision_budget_widens_known_alias_set_to_unknown() {
    let mut builder = HirBuilder::default();
    let p = builder.symbol();
    let q = builder.symbol();
    let x = builder.symbol();
    let init = {
        let p_path = builder.path(p);
        builder.var_stmt(x, p_path)
    };
    let then_block = {
        let x_target = builder.path(x);
        let p_path = builder.path(p);
        let stmt = builder.assign_stmt(x_target, p_path);
        builder.block(vec![stmt], None)
    };
    let else_block = {
        let x_target = builder.path(x);
        let q_path = builder.path(q);
        let stmt = builder.assign_stmt(x_target, q_path);
        builder.block(vec![stmt], None)
    };
    let cond = builder.bool_lit();
    let if_expr = builder.if_expr(cond, then_block, else_block);
    let if_stmt = builder.expr_stmt(if_expr);
    let x_path = builder.path(x);
    let body = builder.block(vec![init, if_stmt], Some(x_path));
    let item = builder.flow(vec![p, q], body);
    let program = builder.finish();
    let mut config = AliasPrecisionConfig::balanced();
    config.max_alias_set_size = 1;

    let output = run(&program, config, TestOracle::default());

    assert_eq!(
        item_summary(&output, item).return_alias,
        AliasValueExpr::Unknown
    );
}

#[test]
fn flow_insensitive_config_weakens_sequential_strong_update() {
    let mut builder = HirBuilder::default();
    let p = builder.symbol();
    let q = builder.symbol();
    let x = builder.symbol();
    let p_path = builder.path(p);
    let var_x = builder.var_stmt(x, p_path);
    let x_target = builder.path(x);
    let q_path = builder.path(q);
    let assign = builder.assign_stmt(x_target, q_path);
    let x_path = builder.path(x);
    let body = builder.block(vec![var_x, assign], Some(x_path));
    let item = builder.flow(vec![p, q], body);
    let program = builder.finish();
    let mut config = AliasPrecisionConfig::balanced();
    config.flow_sensitivity = FlowSensitivity::FlowInsensitive;

    let output = run(&program, config, TestOracle::default());

    assert_eq!(
        item_summary(&output, item).return_alias,
        AliasValueExpr::Union(vec![
            AliasValueExpr::FormalParam(0),
            AliasValueExpr::FormalParam(1),
        ])
    );
}

#[test]
fn collapsed_field_sensitivity_uses_single_field_bucket() {
    let mut builder = HirBuilder::default();
    let p = builder.symbol();
    let q = builder.symbol();
    let r = builder.symbol();
    let p_path = builder.path(p);
    let q_path = builder.path(q);
    let record = builder.record(vec![("a", p_path), ("b", q_path)]);
    let let_r = builder.let_stmt(r, record);
    let r_path = builder.path(r);
    let field = builder.field(r_path, "a");
    let body = builder.block(vec![let_r], Some(field));
    let item = builder.flow(vec![p, q], body);
    let program = builder.finish();
    let mut config = AliasPrecisionConfig::balanced();
    config.field_sensitivity = FieldSensitivity::Collapsed;

    let output = run(&program, config, TestOracle::default());

    assert_eq!(
        item_summary(&output, item).return_alias,
        AliasValueExpr::Union(vec![
            AliasValueExpr::FormalParam(0),
            AliasValueExpr::FormalParam(1),
        ])
    );
}

#[test]
fn no_heap_model_marks_allocation_projection_unknown() {
    let mut builder = HirBuilder::default();
    let p = builder.symbol();
    let r = builder.symbol();
    let p_path = builder.path(p);
    let record = builder.record(vec![("a", p_path)]);
    let let_r = builder.let_stmt(r, record);
    let r_path = builder.path(r);
    let field = builder.field(r_path, "a");
    let body = builder.block(vec![let_r], Some(field));
    let item = builder.flow(vec![p], body);
    let program = builder.finish();
    let mut config = AliasPrecisionConfig::balanced();
    config.heap_model = HeapModel::NoHeap;

    let output = run(&program, config, TestOracle::default());

    assert!(!output.diagnostics.is_empty());
    assert_eq!(
        item_summary(&output, item).return_alias,
        AliasValueExpr::Unknown
    );
}

#[test]
fn allocation_summary_carries_real_unit_identity() {
    let mut builder = HirBuilder::default();
    let record = builder.record(Vec::new());
    let body = builder.block(Vec::new(), Some(record));
    let item = builder.flow(Vec::new(), body);
    let program = builder.finish();

    let output = run(
        &program,
        AliasPrecisionConfig::balanced(),
        TestOracle::default(),
    );

    assert_eq!(
        item_summary(&output, item).return_alias,
        AliasValueExpr::Allocation {
            unit: Some(HirSemanticUnit::Item(item)),
            expr: record,
        }
    );
}

#[test]
fn lambda_summary_records_captured_symbol_aliases() {
    let mut builder = HirBuilder::default();
    let p = builder.symbol();
    let f = builder.symbol();
    let p_path = builder.path(p);
    let lambda = builder.lambda_expr(Vec::new(), p_path);
    let let_f = builder.let_stmt(f, lambda);
    let body = builder.block(vec![let_f], None);
    let owner = builder.flow(vec![p], body);
    let program = builder.finish();

    let output = run(
        &program,
        AliasPrecisionConfig::balanced(),
        TestOracle::default(),
    );
    let lambda_unit = HirSemanticUnit::AnonymousFlow {
        owner,
        expr: lambda,
        body: crate::unit::HirAnonymousFlowBody::Expr(p_path),
    };

    assert_eq!(
        output
            .summaries
            .get(lambda_unit)
            .expect("lambda summary")
            .captures
            .get(&p),
        Some(&AliasValueExpr::FormalParam(0))
    );
}

#[test]
fn captured_lambda_summary_instantiates_captured_alias_at_call_site() {
    let mut builder = HirBuilder::default();
    let p = builder.symbol();
    let f = builder.symbol();
    let p_path = builder.path(p);
    let lambda = builder.lambda_expr(Vec::new(), p_path);
    let let_f = builder.let_stmt(f, lambda);
    let f_path = builder.path(f);
    let call = builder.call_with_callee(f_path, Vec::new());
    let body = builder.block(vec![let_f], Some(call));
    let owner = builder.flow(vec![p], body);
    let program = builder.finish();
    let lambda_unit = HirSemanticUnit::AnonymousFlow {
        owner,
        expr: lambda,
        body: crate::unit::HirAnonymousFlowBody::Expr(p_path),
    };
    let mut oracle = TestOracle::default();
    oracle
        .calls
        .insert(call, AliasCallTarget::Direct(lambda_unit));

    let output = run(&program, AliasPrecisionConfig::balanced(), oracle);

    assert_eq!(
        item_summary(&output, owner).return_alias,
        AliasValueExpr::FormalParam(0)
    );
}

#[test]
fn call_string_context_sensitivity_creates_contextual_summary_keys() {
    let mut builder = HirBuilder::default();
    let callee_body = builder.block(Vec::new(), None);
    let callee = builder.flow(Vec::new(), callee_body);
    let arg1 = builder.bool_lit();
    let call1 = builder.call(arg1);
    let arg2 = builder.bool_lit();
    let call2 = builder.call(arg2);
    let stmt1 = builder.expr_stmt(call1);
    let body = builder.block(vec![stmt1], Some(call2));
    builder.flow(Vec::new(), body);
    let program = builder.finish();
    let mut config = AliasPrecisionConfig::balanced();
    config.context_sensitivity = ContextSensitivity::CallString { k: 1 };
    let mut oracle = TestOracle::default();
    oracle.calls.insert(
        call1,
        AliasCallTarget::Direct(HirSemanticUnit::Item(callee)),
    );
    oracle.calls.insert(
        call2,
        AliasCallTarget::Direct(HirSemanticUnit::Item(callee)),
    );

    let output = run(&program, config, oracle);

    assert!(
        output.diagnostics.iter().all(|diagnostic| !diagnostic
            .message
            .contains("call-string context sensitivity")),
        "{:?}",
        output.diagnostics
    );
    let callee_contexts = output
        .contextual_summaries
        .iter()
        .filter(|(unit, _)| unit.semantic == HirSemanticUnit::Item(callee))
        .map(|(unit, _)| unit.context)
        .collect::<Vec<_>>();
    assert!(callee_contexts.contains(&AliasContext::from_calls(&[call1])));
    assert!(callee_contexts.contains(&AliasContext::from_calls(&[call2])));
}
