use std::collections::{HashMap, HashSet};

use etas_hir::{HirBlockId, HirExprId, HirItemId, SymbolId};
use etas_utils::PassContext;

use crate::{
    BODY_UNIT_KIND, OptimizationLevel, ProjectCompileOptions, ProjectContext, UnitId, UnitTarget,
};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub(crate) struct OptimizationPlan {
    pub level: OptimizationLevel,
    pub quality_context_token_budget: Option<usize>,
}

impl OptimizationPlan {
    pub(crate) fn from_options(options: &ProjectCompileOptions) -> Self {
        Self {
            level: options.optimization_level,
            quality_context_token_budget: options.quality_context_token_budget,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub enum HirOptimizationGroup {
    O1,
    O2,
    O3,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct HirOptimizationReport {
    pub requested_level: OptimizationLevel,
    pub effective_level: OptimizationLevel,
    pub passes: Vec<HirOptimizationPassReport>,
    pub policy_transforms: Vec<HirPolicyTransformReport>,
}

impl Default for HirOptimizationReport {
    fn default() -> Self {
        Self::new(OptimizationLevel::default())
    }
}

impl HirOptimizationReport {
    pub(crate) fn new(level: OptimizationLevel) -> Self {
        Self {
            requested_level: level,
            effective_level: level,
            passes: Vec::new(),
            policy_transforms: Vec::new(),
        }
    }

    pub fn pass(&self, name: &str) -> Option<&HirOptimizationPassReport> {
        self.passes.iter().find(|pass| pass.name == name)
    }

    pub(crate) fn record(&mut self, report: HirOptimizationPassReport) {
        if let Some(existing) = self
            .passes
            .iter_mut()
            .find(|existing| existing.name == report.name)
        {
            existing.matched += report.matched;
            existing.rewritten += report.rewritten;
            existing.affected_units += report.affected_units;
            existing.changed |= report.changed;
        } else {
            self.passes.push(report);
        }
    }

    pub fn policy_transform(&self, name: &str) -> Option<&HirPolicyTransformReport> {
        self.policy_transforms
            .iter()
            .find(|transform| transform.name == name)
    }

    pub(crate) fn record_policy(&mut self, report: HirPolicyTransformReport) {
        if let Some(existing) = self
            .policy_transforms
            .iter_mut()
            .find(|existing| existing.name == report.name)
        {
            existing.matched += report.matched;
            existing.rewritten += report.rewritten;
            existing.affected_units += report.affected_units;
            existing.changed |= report.changed;
        } else {
            self.policy_transforms.push(report);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct HirOptimizationPassReport {
    pub name: String,
    pub group: HirOptimizationGroup,
    pub matched: usize,
    pub rewritten: usize,
    pub affected_units: usize,
    pub changed: bool,
}

impl HirOptimizationPassReport {
    pub(crate) fn new(
        name: &str,
        group: HirOptimizationGroup,
        matched: usize,
        rewritten: usize,
        affected_units: usize,
        changed: bool,
    ) -> Self {
        Self {
            name: name.to_owned(),
            group,
            matched,
            rewritten,
            affected_units,
            changed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct HirPolicyTransformReport {
    pub name: String,
    pub matched: usize,
    pub rewritten: usize,
    pub affected_units: usize,
    pub changed: bool,
}

impl HirPolicyTransformReport {
    pub(crate) fn new(name: &str, matched: usize, rewritten: usize, changed: bool) -> Self {
        Self {
            name: name.to_owned(),
            matched,
            rewritten,
            affected_units: usize::from(changed),
            changed,
        }
    }
}

pub(crate) struct OptimizationBody {
    pub blocks: Vec<HirBlockId>,
    pub expressions: Vec<HirExprId>,
}

pub(crate) fn current_optimization_body(
    context: &ProjectContext,
    pass_context: &PassContext<ProjectContext>,
) -> OptimizationBody {
    let unit = pass_context
        .current_unit
        .expect("body optimization pass requires a current unit");
    assert_eq!(unit.kind, BODY_UNIT_KIND);
    let unit_id = UnitId(unit.id as u32);
    let units = context
        .units
        .as_ref()
        .expect("body optimization requires unit tree");
    let node = units
        .nodes
        .get(unit_id)
        .expect("body optimization unit must exist");
    let UnitTarget::AstBody(body) = &node.target else {
        panic!("body optimization unit must target an AST body")
    };
    let bindings = context
        .hir_body_bindings
        .as_ref()
        .expect("body optimization requires HIR body bindings");
    OptimizationBody {
        blocks: bindings.body_blocks.get(body).cloned().unwrap_or_default(),
        expressions: bindings.body_exprs.get(body).cloned().unwrap_or_default(),
    }
}
