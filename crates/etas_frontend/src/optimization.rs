use std::collections::{HashMap, HashSet};

use etas_hir::{HirExprId, HirItemId, SymbolId};

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct AgentExpressionLegalityFact {
    pub owner: HirItemId,
    pub reads: HashSet<SymbolId>,
    pub version_dependencies: HashSet<SymbolId>,
    pub effect_free: bool,
    pub deterministic: bool,
    pub order_stable: bool,
}

impl AgentExpressionLegalityFact {
    pub fn is_movable(&self) -> bool {
        self.effect_free && self.deterministic && self.order_stable
    }

    pub fn is_version_stable(&self) -> bool {
        self.version_dependencies.is_empty()
    }
}

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
pub struct AgentEffectLegalityFacts {
    pub(crate) expressions: HashMap<HirExprId, AgentExpressionLegalityFact>,
}

impl AgentEffectLegalityFacts {
    pub fn expression(&self, expression: HirExprId) -> Option<&AgentExpressionLegalityFact> {
        self.expressions.get(&expression)
    }

    pub fn is_movable(&self, expression: HirExprId) -> bool {
        self.expression(expression)
            .is_some_and(AgentExpressionLegalityFact::is_movable)
    }

    pub fn is_version_stable(&self, expression: HirExprId) -> bool {
        self.expression(expression).is_some_and(|fact| {
            fact.is_movable() && AgentExpressionLegalityFact::is_version_stable(fact)
        })
    }

    pub fn movable_expression_count(&self) -> usize {
        self.expressions
            .values()
            .filter(|fact| fact.is_movable())
            .count()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, etas_core::RetainedSize)]
pub struct HirOptimizationReport {
    pub passes: Vec<HirOptimizationPassReport>,
}

impl HirOptimizationReport {
    pub fn pass(&self, name: &str) -> Option<&HirOptimizationPassReport> {
        self.passes.iter().find(|pass| pass.name == name)
    }

    pub(crate) fn record(&mut self, report: HirOptimizationPassReport) {
        self.passes.push(report);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct HirOptimizationPassReport {
    pub name: String,
    pub matched: usize,
    pub rewritten: usize,
}

impl HirOptimizationPassReport {
    pub(crate) fn new(name: &str, matched: usize, rewritten: usize) -> Self {
        Self {
            name: name.to_owned(),
            matched,
            rewritten,
        }
    }
}
