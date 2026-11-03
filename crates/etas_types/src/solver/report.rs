use std::collections::HashMap;

use etas_core::{Span, TypeDiagnosticCode};
use etas_hir::HirExprId;

use crate::{CheckedIndexKind, CheckedSliceKind, TypeId, solver::unification::Substitution};

#[derive(Clone, Debug, Default)]
pub struct SolverReport {
    pub substitutions: Substitution,
    pub named_substitutions: HashMap<String, TypeId>,
    pub inferred_expr_types: HashMap<HirExprId, TypeId>,
    pub reflections: HashMap<HirExprId, crate::CheckedReflectionFact>,
    pub agent_inferences: HashMap<HirExprId, crate::CheckedAgentInferenceFact>,
    pub agent_method_calls: HashMap<HirExprId, etas_hir::SymbolId>,
    pub std_intrinsic_calls: HashMap<HirExprId, crate::facts::CheckedStdCallFact>,
    pub action_calls: HashMap<HirExprId, crate::facts::CheckedActionCallFact>,
    pub generic_instantiations: HashMap<HirExprId, crate::GenericInstantiationFact>,
    pub deferred_handler_instantiations:
        HashMap<etas_hir::HirHandlerArmId, crate::DeferredHandlerInstantiation>,
    pub(crate) invocation_effects: HashMap<HirExprId, crate::EffectRowRef>,
    pub index_facts: HashMap<HirExprId, CheckedIndexKind>,
    pub slice_facts: HashMap<HirExprId, CheckedSliceKind>,
    pub checked_index_errors: HashMap<HirExprId, TypeId>,
    pub(crate) pending_spec_obligations: Vec<crate::SpecObligation>,
    pub failures: Vec<SolverFailure>,
}

impl SolverReport {
    pub fn push(&mut self, failure: SolverFailure) {
        self.failures.push(failure);
    }

    pub fn append(&mut self, other: SolverReport) {
        self.substitutions.extend(&other.substitutions);
        self.named_substitutions.extend(other.named_substitutions);
        for (expr, ty) in other.inferred_expr_types {
            self.inferred_expr_types.insert(expr, ty);
        }
        self.generic_instantiations
            .extend(other.generic_instantiations);
        self.deferred_handler_instantiations
            .extend(other.deferred_handler_instantiations);
        self.invocation_effects.extend(other.invocation_effects);
        self.reflections.extend(other.reflections);
        self.agent_inferences.extend(other.agent_inferences);
        self.agent_method_calls.extend(other.agent_method_calls);
        self.std_intrinsic_calls.extend(other.std_intrinsic_calls);
        self.action_calls.extend(other.action_calls);
        self.index_facts.extend(other.index_facts);
        self.slice_facts.extend(other.slice_facts);
        self.checked_index_errors.extend(other.checked_index_errors);
        self.pending_spec_obligations
            .extend(other.pending_spec_obligations);
        self.failures.extend(other.failures);
    }

    pub fn record_index_kind(&mut self, expr: HirExprId, kind: CheckedIndexKind) {
        let output = &mut self.index_facts;
        output.insert(expr, kind);
    }

    pub fn record_slice_kind(&mut self, expr: HirExprId, kind: CheckedSliceKind) {
        let output = &mut self.slice_facts;
        output.insert(expr, kind);
    }

    pub fn record_checked_index_error(&mut self, expr: HirExprId, error: TypeId) {
        self.checked_index_errors.insert(expr, error);
    }

    pub fn is_solved(&self) -> bool {
        self.failures.is_empty()
    }

    pub fn failures(&self) -> impl Iterator<Item = &SolverFailure> {
        self.failures.iter()
    }
}

#[derive(Clone, Debug)]
pub struct SolverFailure {
    pub code: TypeDiagnosticCode,
    pub span: Span,
    pub message: String,
}
