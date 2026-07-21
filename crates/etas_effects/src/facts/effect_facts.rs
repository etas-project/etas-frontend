use std::collections::{BTreeMap, BTreeSet, HashMap};

use etas_core::{Diagnostic, Span};
use etas_hir::{HirBlockId, HirExprId, HirItemId, HirStmtId, SymbolId};

use crate::{
    EffectRow, EffectSummary, EffectUnit, InterpreterSupportFacts, RequirementFacts,
    ResidualCheckSet,
};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct EffectOutput {
    pub facts: EffectFacts,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct EffectAnalysisOutput {
    pub summaries: BTreeMap<EffectUnit, EffectSummary>,
    pub inputs: EffectMaterializationInputs,
    pub diagnostics: Vec<Diagnostic>,
    pub convergence: Vec<EffectComponentConvergence>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EffectComponentConvergence {
    pub units: Vec<EffectUnit>,
    pub status: EffectConvergenceStatus,
    pub iterations: usize,
    pub changes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EffectConvergenceStatus {
    Converged,
    IterationLimitReached,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct EffectMaterializationInputs {
    pub expr_effects: HashMap<HirExprId, EffectSummary>,
    pub stmt_effects: HashMap<HirStmtId, EffectSummary>,
    pub unit_effects: HashMap<EffectUnit, EffectSummary>,
    pub handler_values: HashMap<HirExprId, HandlerValueFact>,
    pub handle_applications: HashMap<HirExprId, HandleApplicationFact>,
    pub performed_actions: HashMap<HirExprId, PerformedActionFact>,
    pub try_captures: HashMap<HirExprId, TryCaptureFact>,
    pub latent_effects: HashMap<HirExprId, LatentEffectFact>,
    pub latent_realizations: HashMap<HirExprId, Vec<HirExprId>>,
    pub deferred_first_class_calls: HashMap<EffectUnit, Vec<HirExprId>>,
    pub solved_deferred_units: BTreeSet<EffectUnit>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct EffectFacts {
    pub expr_effects: HashMap<HirExprId, EffectSummary>,
    pub stmt_effects: HashMap<HirStmtId, EffectSummary>,
    pub item_effects: HashMap<HirItemId, EffectSummary>,
    pub unit_effects: HashMap<EffectUnit, EffectSummary>,
    pub symbol_effects: HashMap<SymbolId, EffectSummary>,
    pub handler_values: HashMap<HirExprId, HandlerValueFact>,
    pub handle_applications: HashMap<HirExprId, HandleApplicationFact>,
    pub performed_actions: HashMap<HirExprId, PerformedActionFact>,
    pub try_captures: HashMap<HirExprId, TryCaptureFact>,
    pub latent_effects: HashMap<HirExprId, LatentEffectFact>,
    pub public_contracts: Vec<PublicEffectContract>,
    pub trace_specs: TraceSpecFacts,
    pub requirements: RequirementFacts,
    pub interpreter_support: InterpreterSupportFacts,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TraceSpecFacts {
    pub items: HashMap<HirItemId, Vec<TraceSpecClauseFact>>,
    pub symbols: HashMap<SymbolId, Vec<TraceSpecClauseFact>>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum TraceSpecClauseFact {
    PendingTypedTraceSpecModel,
    TraceSpecReference {
        name: String,
    },
    Allow {
        pattern: EffectRow,
        label: String,
    },
    Deny {
        pattern: EffectRow,
        label: String,
    },
    RequireBefore {
        guard: EffectRow,
        guard_label: String,
        target: EffectRow,
        target_label: String,
    },
    RequireAfter {
        target: EffectRow,
        target_label: String,
        obligation: EffectRow,
        obligation_label: String,
    },
    Limit {
        count: usize,
    },
}

impl TraceSpecClauseFact {
    pub fn label(&self) -> String {
        match self {
            Self::PendingTypedTraceSpecModel => "pending typed trace spec model".to_owned(),
            Self::TraceSpecReference { name } => format!("trace spec {name}"),
            Self::Allow { label, .. } => format!("+{label}"),
            Self::Deny { label, .. } => format!("-{label}"),
            Self::RequireBefore {
                guard_label,
                target_label,
                ..
            } => format!("{guard_label} >> {target_label}"),
            Self::RequireAfter {
                target_label,
                obligation_label,
                ..
            } => format!("{obligation_label} << {target_label}"),
            Self::Limit { count } => format!("limit count {count}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PerformedActionFact {
    pub expr: HirExprId,
    pub effect_segments: Vec<String>,
    pub action: String,
    pub action_symbol: Option<SymbolId>,
    pub args: Vec<HirExprId>,
    pub span: Span,
    pub summary: EffectSummary,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LatentEffectFact {
    pub value: HirExprId,
    pub body: LatentEffectBodyRef,
    pub flow_type: etas_types::TypeId,
    pub inferred: EffectRow,
    pub declared_bound: Option<EffectRow>,
    pub summary: EffectSummary,
    pub realized_at: Vec<HirExprId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LatentEffectBodyRef {
    Expr(HirExprId),
    Block(HirBlockId),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PublicEffectContract {
    pub item: HirItemId,
    pub exported_name: QualifiedName,
    pub value: PublicEffectValue,
    pub declared: Option<EffectRow>,
    pub inferred: EffectRow,
    pub public_row: EffectRow,
    pub requested_actions: EffectRow,
    pub residual_checks: ResidualCheckSet,
    pub source: PublicEffectContractSource,
    pub latent_flows: Vec<LatentEffectContract>,
    pub requirements: RequirementFactsForContract,
    pub high_impact_ack: Option<EffectAcknowledgement>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QualifiedName {
    pub segments: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PublicEffectValue {
    ItemSignature(etas_types::ItemSignature),
    ValueType(etas_types::TypeId),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PublicEffectContractSource {
    ExplicitSourceAnnotation,
    GeneratedCheckedMetadata,
    DependencyMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LatentEffectContract {
    pub value: HirExprId,
    pub body: LatentEffectBodyRef,
    pub flow_type: etas_types::TypeId,
    pub declared_bound: Option<EffectRow>,
    pub inferred: EffectRow,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RequirementFactsForContract {
    pub requirements: crate::RequirementSet,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EffectAcknowledgement {
    pub effects: EffectRow,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HandlerValueFact {
    pub handled: EffectRow,
    pub produced: EffectRow,
    pub result: Option<etas_types::TypeId>,
    pub arms: Vec<HandlerArmFact>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HandlerArmFact {
    pub action: HandlerActionFact,
    pub resumable: bool,
    pub resumes: ResumeSummary,
    pub completion: HandlerCompletionSummary,
    pub arm_effects: EffectRow,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HandlerActionFact {
    pub effect_segments: Vec<String>,
    pub action: String,
    pub action_symbol: Option<SymbolId>,
    pub effect_type_args: Vec<etas_types::TypeId>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ResumeSummary {
    None,
    Once,
    Multiple,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HandlerCompletionSummary {
    pub resumes: ResumeSummary,
    pub finishes: FinishSummary,
    pub terminates_never: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FinishSummary {
    None,
    Once,
    Multiple,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HandleApplicationFact {
    pub handler: HandlerValueRef,
    pub handled: EffectRow,
    pub produced: EffectRow,
    pub remaining: EffectRow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HandlerValueRef {
    Expr { expr: HirExprId },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TryCaptureFact {
    pub operand: HirExprId,
    pub captured_error: etas_types::TypeId,
    pub result_type: etas_types::TypeId,
    pub conversions: Vec<ErrorConversionFact>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ErrorConversionFact {
    pub source_error: etas_types::TypeId,
    pub target_error: etas_types::TypeId,
}
