use std::collections::HashMap;

use etas_hir::{HirExprId, HirItemId, HirPatId, HirStmtId, SymbolId};

use crate::{
    CheckedIndexKind, CheckedSliceKind, SolverReport, SpecObligation, SymbolTypeFact,
    TryExprTypeFact, TypeConstraint, TypeId, ValidationRequest,
};

pub struct BodyPipelineState {
    pub item: HirItemId,
    pub expected_return: Option<TypeId>,
    pub provisional: ProvisionalFacts,
    pub constraints: Vec<TypeConstraint>,
    pub spec_obligations: Vec<SpecObligation>,
    pub validations: Vec<ValidationRequest>,
    pub solver_report: SolverReport,
    pub handler_resume_stack: Vec<TypeId>,
    pub handler_finish_stack: Vec<TypeId>,
    pub handler_depth: usize,
}

impl BodyPipelineState {
    pub fn new(item: HirItemId) -> Self {
        Self {
            item,
            expected_return: None,
            provisional: ProvisionalFacts::default(),
            constraints: Vec::new(),
            spec_obligations: Vec::new(),
            validations: Vec::new(),
            solver_report: SolverReport::default(),
            handler_resume_stack: Vec::new(),
            handler_finish_stack: Vec::new(),
            handler_depth: 0,
        }
    }
}

#[derive(Default)]
pub struct ProvisionalFacts {
    pub expr_types: HashMap<HirExprId, TypeId>,
    pub expr_memory_places: HashMap<HirExprId, TypeId>,
    pub stmt_types: HashMap<HirStmtId, TypeId>,
    pub pat_types: HashMap<HirPatId, TypeId>,
    pub symbol_types: HashMap<SymbolId, SymbolTypeFact>,
    pub item_signatures: HashMap<HirItemId, crate::ItemSignature>,
    pub try_facts: HashMap<HirExprId, TryExprTypeFact>,
    pub index_facts: HashMap<HirExprId, CheckedIndexKind>,
    pub slice_facts: HashMap<HirExprId, CheckedSliceKind>,
}

impl ProvisionalFacts {
    pub fn record_try_expr(&mut self, expr: HirExprId, fact: TryExprTypeFact) {
        self.try_facts.insert(expr, fact);
    }
}
