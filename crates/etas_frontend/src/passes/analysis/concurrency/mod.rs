mod conflict;
mod ownership;
mod plan;
mod resource;

use std::collections::HashMap;
use std::sync::Arc;

use etas_core::{SourceId, Span};
use etas_effects::{Effect, EffectRow, EffectSet, EffectSummary, TaskCallKind};
use etas_hir::{HirExprId, HirItemId, ScopeId, SymbolId};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::ProjectContext;
use crate::passes::artifacts::{
    CONCURRENCY_FACTS, EFFECT_OUTPUT, HIR_OUTPUT, PREPARED_EFFECT_REGISTRY,
};

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    etas_core::RetainedSize,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct ConcurrencyFacts {
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub sites: HashMap<HirExprId, ConcurrencySiteFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub owners: HashMap<ScopeId, TaskScopeFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub starts: HashMap<HirExprId, TaskStartFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub awaits: HashMap<HirExprId, TaskAwaitFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub continuation_transfers: HashMap<HirExprId, ContinuationTransferFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub plans: HashMap<ScopeId, ConcurrencyPlanFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub resource_callables: HashMap<HirItemId, ResourceGuardCallablePlan>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub resource_closures: HashMap<HirExprId, ResourceGuardCallablePlan>,
}

/// Read-only selectors evaluated by the coordinator before task admission.
#[derive(
    Clone, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub enum ResourceValueSelector {
    Expr(HirExprId),
    SymbolPath {
        symbol: SymbolId,
        fields: Vec<String>,
    },
}

#[derive(
    Clone, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub struct RuntimeResourceSelector {
    pub store: ResourceValueSelector,
    /// None acquires the entire Store (scan/query/clear and similar operations).
    pub key: Option<HirExprId>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub struct ResourceGuardCall {
    pub callable: HirItemId,
    pub arguments: Vec<HirExprId>,
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    etas_core::RetainedSize,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct ResourceGuardCallablePlan {
    pub parameters: Vec<SymbolId>,
    pub resources: Vec<RuntimeResourceSelector>,
    pub calls: Vec<ResourceGuardCall>,
    pub complete: bool,
    /// Memory effects whose resource footprint is not represented above.
    pub opaque_memory: bool,
}

#[derive(
    Clone, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub struct ConcurrencySiteFact {
    pub kind: TaskCallKind,
    pub owner_scope: ScopeId,
    pub decision: SchedulingDecision,
    pub branches: Vec<TaskBranchFact>,
    pub concurrency_limit: Option<u32>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub struct TaskScopeFact {
    pub owner_scope: ScopeId,
    pub sites: Vec<HirExprId>,
    pub branches: Vec<TaskBranchFact>,
    pub decision: SchedulingDecision,
    pub concurrency_limit: Option<u32>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub struct TaskBranchFact {
    pub id: TaskSiteId,
    pub source_index: usize,
    pub task_site: HirExprId,
    pub effects: EffectRow,
    pub behavior: EffectSummary,
    pub actions: EffectSet,
    pub shape: TaskBranchShape,
}

pub type BranchFact = TaskBranchFact;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub enum TaskBranchShape {
    ExplicitStart,
    TupleElement,
    ArrayElement,
    MapperInvocation,
    Dynamic,
}

#[derive(
    Clone, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub enum SchedulingDecision {
    Concurrent,
    Serialize,
    RuntimeGuard(ResourceGuardPlan),
    Reject,
}

#[derive(
    Clone, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub struct ResourceGuardPlan {
    pub conflicts: Vec<ResourceConflict>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub struct ResourceConflict {
    pub left_task: HirExprId,
    pub right_task: HirExprId,
    pub left: Effect,
    pub right: Effect,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    etas_core::RetainedSize,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct TaskSiteId {
    pub source: SourceId,
    pub start: u32,
    pub end: u32,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    etas_core::RetainedSize,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct AwaitSiteId {
    pub source: SourceId,
    pub start: u32,
    pub end: u32,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    etas_core::RetainedSize,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct ContinuationTransferId {
    pub source: SourceId,
    pub start: u32,
    pub end: u32,
}

#[derive(
    Clone, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub struct TaskStartFact {
    pub id: TaskSiteId,
    pub call: HirExprId,
    pub owner_scope: ScopeId,
    pub span: Span,
    pub output: etas_types::TypeId,
    pub effects: EffectRow,
    pub behavior: EffectSummary,
    pub decision: SchedulingDecision,
    pub concurrency_limit: Option<u32>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub struct TaskAwaitFact {
    pub id: AwaitSiteId,
    pub call: HirExprId,
    pub owner_scope: ScopeId,
    pub span: Span,
    pub output: etas_types::TypeId,
    pub task: Option<TaskSiteId>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub struct ContinuationTransferFact {
    pub id: ContinuationTransferId,
    pub call: HirExprId,
    pub binder: SymbolId,
    pub span: Span,
    pub target: etas_effects::TransferTarget,
}

#[derive(
    Clone, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub struct ConcurrencyPlanFact {
    pub owner_scope: ScopeId,
    pub branches: Vec<TaskSiteId>,
    pub decision: SchedulingDecision,
    pub concurrency_limit: Option<u32>,
}

pub struct AnalyzeConcurrencyPass;

impl Pass<ProjectContext> for AnalyzeConcurrencyPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("AnalyzeConcurrencyPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                EFFECT_OUTPUT,
                HIR_OUTPUT,
                PREPARED_EFFECT_REGISTRY,
                crate::passes::artifacts::TYPE_OUTPUT,
            ]))
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
        let Some(hir) = context.hir.as_ref() else {
            return PassResult::failed("concurrency analysis requires checked HIR");
        };
        context.concurrency = Some(Arc::new(plan::analyze(
            &hir.view(),
            &effects.facts,
            &artifacts.registry,
            &context.types.as_ref().expect("checked types").facts,
            &context.std_registry,
        )));
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(CONCURRENCY_FACTS))
    }
}
