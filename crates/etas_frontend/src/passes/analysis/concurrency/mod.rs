mod conflict;
mod ownership;
mod plan;

use std::collections::HashMap;
use std::sync::Arc;

use etas_effects::{Effect, EffectRow, EffectSet, TaskCallKind};
use etas_hir::{HirExprId, ScopeId};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::ProjectContext;
use crate::passes::artifacts::{CONCURRENCY_FACTS, EFFECT_OUTPUT, PREPARED_EFFECT_REGISTRY};

#[derive(Clone, Debug, Default, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ConcurrencyFacts {
    pub sites: HashMap<HirExprId, ConcurrencySiteFact>,
    pub owners: HashMap<ScopeId, TaskScopeFact>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ConcurrencySiteFact {
    pub kind: TaskCallKind,
    pub decision: SchedulingDecision,
    pub branches: Vec<BranchFact>,
    pub concurrency_limit: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct TaskScopeFact {
    pub owner_scope: ScopeId,
    pub sites: Vec<HirExprId>,
    pub branches: Vec<BranchFact>,
    pub decision: SchedulingDecision,
    pub concurrency_limit: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct BranchFact {
    pub task_site: HirExprId,
    pub effects: EffectRow,
    pub actions: EffectSet,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub enum SchedulingDecision {
    Concurrent,
    Serialize,
    RuntimeGuard(ResourceGuardPlan),
    Reject,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ResourceGuardPlan {
    pub conflicts: Vec<ResourceConflict>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ResourceConflict {
    pub left_task: HirExprId,
    pub right_task: HirExprId,
    pub left: Effect,
    pub right: Effect,
}

pub struct AnalyzeConcurrencyPass;

impl Pass<ProjectContext> for AnalyzeConcurrencyPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("AnalyzeConcurrencyPass", PassKind::Analysis)
            .requires(ArtifactSet::from([EFFECT_OUTPUT, PREPARED_EFFECT_REGISTRY]))
            .produces(ArtifactSet::one(CONCURRENCY_FACTS))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let Some(effects) = context.effects.as_ref() else {
            return PassResult::failed("concurrency analysis requires checked effect facts");
        };
        let Some(artifacts) = context.effect_pipeline_artifacts.as_ref() else {
            return PassResult::failed("concurrency analysis requires the checked effect registry");
        };
        context.concurrency = Some(Arc::new(plan::analyze(&effects.facts, &artifacts.registry)));
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(CONCURRENCY_FACTS))
    }
}
