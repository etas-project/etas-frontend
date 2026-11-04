use std::collections::HashSet;

use etas_hir::HirExprId;

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
pub struct AgentEffectLegalityFacts {
    pub(crate) movable_expressions: HashSet<HirExprId>,
}

impl AgentEffectLegalityFacts {
    pub fn is_movable(&self, expression: HirExprId) -> bool {
        self.movable_expressions.contains(&expression)
    }

    pub fn movable_expression_count(&self) -> usize {
        self.movable_expressions.len()
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
