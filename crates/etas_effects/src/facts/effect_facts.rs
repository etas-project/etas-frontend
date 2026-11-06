use std::collections::{BTreeMap, BTreeSet, HashMap};

use etas_core::{Diagnostic, Span};
use etas_hir::{HirBlockId, HirExprId, HirHandlerArmId, HirItemId, HirStmtId, ScopeId, SymbolId};

use crate::{
    EffectRow, EffectSummary, EffectUnit, InterpreterSupportFacts, RequirementFacts,
    ResidualCheckSet,
};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct EffectOutput {
    pub facts: EffectFacts,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct EffectAnalysisOutput {
    pub summaries: BTreeMap<EffectUnit, EffectSummary>,
    pub inputs: EffectMaterializationInputs,
    pub diagnostics: Vec<Diagnostic>,
    pub convergence: Vec<EffectComponentConvergence>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct EffectComponentConvergence {
    pub units: Vec<EffectUnit>,
    pub status: EffectConvergenceStatus,
    pub iterations: usize,
    pub changes: usize,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum EffectConvergenceStatus {
    Converged,
    IterationLimitReached,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct EffectMaterializationInputs {
    #[serde(default)]
    pub external_descriptor_graphs: BTreeMap<
        etas_code::CodeFingerprint,
        std::sync::Arc<etas_code::descriptor::DescriptorGraphInput>,
    >,
    pub method_evidence:
        BTreeMap<etas_code::CodeFingerprint, std::sync::Arc<super::MetadataSpecMethodDefinition>>,
    pub handled_body_requests: HashMap<HirExprId, crate::RequestInstantiationSummary>,
    pub frozen_callable_graphs:
        BTreeMap<etas_code::CodeFingerprint, crate::FrozenCallableGraphIdentity>,
    pub callable_references:
        BTreeMap<crate::CallableReturnReference, super::CallableReferenceAnalysis>,
    pub metadata_callables:
        BTreeMap<crate::MetadataCallableSource, super::MetadataCallableAnalysis>,
    pub metadata_callable_graphs:
        BTreeMap<etas_code::CodeFingerprint, std::sync::Arc<super::MetadataCallableGraphFact>>,
    pub captured_callables: BTreeMap<super::CapturedCallableId, super::CapturedCallableAnalysis>,
    pub native_call_targets: HashMap<HirExprId, BTreeSet<EffectUnit>>,
    pub metadata_call_targets: HashMap<HirExprId, BTreeSet<crate::MetadataCallableSource>>,
    pub generic_effect_rows: HashMap<HirExprId, Vec<(String, EffectRow)>>,
    pub callable_environments: BTreeMap<super::CallableEnvironmentId, super::CallableEnvironment>,
    pub handler_action_effects: HashMap<HirHandlerArmId, crate::Effect>,
    pub expr_effects: HashMap<HirExprId, EffectSummary>,
    pub stmt_effects: HashMap<HirStmtId, EffectSummary>,
    pub unit_effects: HashMap<EffectUnit, EffectSummary>,
    pub handler_values: HashMap<HirExprId, HandlerValueFact>,
    pub handle_applications: HashMap<HirExprId, HandleApplicationFact>,
    pub performed_actions: HashMap<HirExprId, PerformedActionFact>,
    pub try_captures: HashMap<HirExprId, TryCaptureFact>,
    pub latent_effects: HashMap<HirExprId, LatentEffectFact>,
    pub std_callable_values: HashMap<HirExprId, StdCallableEffectFact>,
    pub std_action_calls: HashMap<HirExprId, StdActionCallFact>,
    pub latent_realizations: HashMap<HirExprId, Vec<HirExprId>>,
    pub deferred_first_class_calls: HashMap<EffectUnit, Vec<HirExprId>>,
    pub inferred_parameter_effects: HashMap<SymbolId, EffectSummary>,
    pub unresolved_parameter_calls: BTreeMap<EffectUnit, BTreeSet<HirExprId>>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct EffectFacts {
    #[serde(default)]
    pub external_descriptor_graphs: BTreeMap<
        etas_code::CodeFingerprint,
        std::sync::Arc<etas_code::descriptor::DescriptorGraphInput>,
    >,
    pub method_evidence:
        BTreeMap<etas_code::CodeFingerprint, std::sync::Arc<super::MetadataSpecMethodDefinition>>,
    pub handled_body_requests: HashMap<HirExprId, crate::RequestInstantiationSummary>,
    pub frozen_callable_graphs:
        BTreeMap<etas_code::CodeFingerprint, crate::FrozenCallableGraphIdentity>,
    pub callable_references:
        BTreeMap<crate::CallableReturnReference, super::CallableReferenceAnalysis>,
    pub metadata_callables:
        BTreeMap<crate::MetadataCallableSource, super::MetadataCallableAnalysis>,
    pub metadata_callable_graphs:
        BTreeMap<etas_code::CodeFingerprint, std::sync::Arc<super::MetadataCallableGraphFact>>,
    pub captured_callables: BTreeMap<super::CapturedCallableId, super::CapturedCallableAnalysis>,
    pub native_call_targets: HashMap<HirExprId, BTreeSet<EffectUnit>>,
    pub metadata_call_targets: HashMap<HirExprId, BTreeSet<crate::MetadataCallableSource>>,
    pub generic_effect_rows: HashMap<HirExprId, Vec<(String, EffectRow)>>,
    pub callable_environments: BTreeMap<super::CallableEnvironmentId, super::CallableEnvironment>,
    pub inferred_parameter_effects: HashMap<SymbolId, EffectSummary>,
    pub handler_action_effects: HashMap<HirHandlerArmId, crate::Effect>,
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
    pub std_callable_values: HashMap<HirExprId, StdCallableEffectFact>,
    pub std_action_calls: HashMap<HirExprId, StdActionCallFact>,
    #[serde(default)]
    pub task_calls: HashMap<HirExprId, TaskCallFact>,
    #[serde(default)]
    pub continuation_transfers: HashMap<HirExprId, ContinuationTransferFact>,
    pub public_contracts: Vec<PublicEffectContract>,
    pub trace_specs: TraceSpecFacts,
    pub requirements: RequirementFacts,
    pub interpreter_support: InterpreterSupportFacts,
}

impl EffectFacts {
    /// Resolve only a scope in a graph carried by the checked effect output.
    pub fn external_handler_scope(
        &self,
        reference: crate::ParameterHandlerScopeRef,
    ) -> Option<(
        &etas_code::descriptor::DescriptorGraphInput,
        &etas_code::descriptor::ParameterHandlerScopeInfo,
    )> {
        use etas_code::descriptor::{CallableOutcomeInfo, TraceInfo};
        let crate::ParameterHandlerScopeRef::External {
            graph,
            parameter_call,
            call_index,
            scope,
        } = reference
        else {
            return None;
        };
        let graph = self.external_descriptor_graphs.get(&graph)?;
        let trace = graph.traces.get(parameter_call as usize)?;
        let call = match trace {
            TraceInfo::ParameterCall(call)
            | TraceInfo::Outcome(CallableOutcomeInfo::ParameterNonReturn(call))
            | TraceInfo::Outcome(CallableOutcomeInfo::ParameterNonReturnTemplate(call))
                if call_index == 0 =>
            {
                call
            }
            TraceInfo::Widened {
                parameter_calls, ..
            } => parameter_calls.get(call_index as usize)?,
            _ => return None,
        };
        Some((graph.as_ref(), call.handlers.as_ref()?.get(scope as usize)?))
    }
}

#[cfg(test)]
mod external_scope_tests {
    use super::*;
    use etas_code::descriptor::{
        DescriptorGraphInput, EffectRowInfo, EffectRowInfoRef, ParameterCallInfo,
        ParameterHandlerScopeInfo, ParameterInfo, PrimitiveInfo, SignatureInfo, TraceInfo,
        TypeInfo, TypeInfoRef,
    };
    use etas_code::{CallableId, CodeFingerprint, DefinitionKey, DefinitionKind, PackageKey};

    #[test]
    fn widened_scope_lookup_distinguishes_parameter_calls_and_graphs() {
        let key = CodeFingerprint::from_bytes([3; 32]);
        let callable = CallableId::declaration(
            DefinitionKey::new(
                PackageKey::from_canonical_fingerprint(key),
                vec!["lib".into()],
                vec!["f".into()],
                DefinitionKind::Flow,
            )
            .unwrap(),
        )
        .unwrap();
        let call = |handlers| ParameterCallInfo {
            callable: callable.clone(),
            signature: TypeInfoRef(1),
            position: 0,
            arguments: None,
            handlers,
        };
        let graph = DescriptorGraphInput {
            types: vec![
                TypeInfo::Primitive(PrimitiveInfo::Unit),
                TypeInfo::Function(SignatureInfo {
                    generics: vec![],
                    parameters: vec![ParameterInfo {
                        name: Some("callback".into()),
                        ty: TypeInfoRef(2),
                    }],
                    output: TypeInfoRef(0),
                    declared_output: None,
                    effects: None,
                }),
                TypeInfo::Function(SignatureInfo {
                    generics: vec![],
                    parameters: vec![],
                    output: TypeInfoRef(0),
                    declared_output: None,
                    effects: None,
                }),
            ],
            rows: vec![EffectRowInfo {
                effects: vec![],
                tail: None,
            }],
            binders: vec![],
            traces: vec![TraceInfo::Widened {
                actions: EffectRowInfoRef(0),
                parameter_calls: vec![
                    call(None),
                    call(Some(vec![ParameterHandlerScopeInfo::Handle {
                        arms: vec![],
                    }])),
                ],
            }],
        };
        graph.validate(Default::default()).unwrap();
        let mut facts = EffectFacts::default();
        facts
            .external_descriptor_graphs
            .insert(key, std::sync::Arc::new(graph));
        let reference = |graph, call_index, scope| crate::ParameterHandlerScopeRef::External {
            graph,
            parameter_call: 0,
            call_index,
            scope,
        };
        assert!(facts.external_handler_scope(reference(key, 1, 0)).is_some());
        assert!(facts.external_handler_scope(reference(key, 0, 0)).is_none());
        assert!(facts.external_handler_scope(reference(key, 1, 1)).is_none());
        assert!(
            facts
                .external_handler_scope(reference(CodeFingerprint::from_bytes([4; 32]), 1, 0))
                .is_none()
        );
    }
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct TraceSpecFacts {
    /// Source sites validated during this effect analysis. Consumers must
    /// revalidate them against the retained source snapshot before use.
    #[serde(
        serialize_with = "serialize_clause_sites",
        deserialize_with = "deserialize_clause_sites"
    )]
    pub source_clause_sites: BTreeMap<
        etas_code::residual::identity::ClauseId,
        etas_code::residual::identity::SourceClauseSite,
    >,
    /// Declaration-level OR alternatives; each inner vector is one conjunction.
    pub declaration_alternatives:
        HashMap<HirItemId, crate::trace_spec::TraceSpecClauseAlternatives>,
    pub items: HashMap<HirItemId, Vec<TraceSpecClauseFact>>,
    pub symbols: HashMap<SymbolId, Vec<TraceSpecClauseFact>>,
    pub conformances: HashMap<HirItemId, Vec<TraceSpecConformanceModelFact>>,
}

fn serialize_clause_sites<S>(
    sites: &BTreeMap<
        etas_code::residual::identity::ClauseId,
        etas_code::residual::identity::SourceClauseSite,
    >,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::{Error, SerializeSeq};
    if sites.len() > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
        return Err(S::Error::custom("trace clause site limit exceeded"));
    }
    let mut sequence = serializer.serialize_seq(Some(sites.len()))?;
    for (id, site) in sites {
        if *id != site.id() {
            return Err(S::Error::custom(
                "trace clause site key does not match its identity",
            ));
        }
        sequence.serialize_element(site)?;
    }
    sequence.end()
}

fn deserialize_clause_sites<'de, D>(
    deserializer: D,
) -> Result<
    BTreeMap<
        etas_code::residual::identity::ClauseId,
        etas_code::residual::identity::SourceClauseSite,
    >,
    D::Error,
>
where
    D: serde::Deserializer<'de>,
{
    struct Sites;
    impl<'de> serde::de::Visitor<'de> for Sites {
        type Value = BTreeMap<
            etas_code::residual::identity::ClauseId,
            etas_code::residual::identity::SourceClauseSite,
        >;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a bounded sequence of unique trace clause sites")
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            use serde::de::Error;
            let limit = etas_package_metadata::MAX_METADATA_GRAPH_NODES;
            if sequence.size_hint().is_some_and(|size| size > limit) {
                return Err(A::Error::custom("trace clause site limit exceeded"));
            }
            let mut sites = BTreeMap::new();
            while let Some(site) =
                sequence.next_element::<etas_code::residual::identity::SourceClauseSite>()?
            {
                if sites.len() >= limit {
                    return Err(A::Error::custom("trace clause site limit exceeded"));
                }
                if sites.insert(site.id(), site).is_some() {
                    return Err(A::Error::custom("duplicate trace clause site identity"));
                }
            }
            Ok(sites)
        }
    }
    deserializer.deserialize_seq(Sites)
}

/// A checked conformance's alternatives, before the legacy display projection
/// flattens them. The span joins this model to its checked type conformance.
#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct TraceSpecConformanceModelFact {
    pub span: Span,
    /// Present only when the declaration and all static source bindings have
    /// stable identities. This is not a verified residual plan.
    #[serde(default)]
    pub source_spec_instance: Option<etas_code::residual::identity::SourceSpecInstance>,
    pub alternatives: crate::trace_spec::TraceSpecClauseAlternatives,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
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

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct PerformedActionFact {
    pub expr: HirExprId,
    pub invocation: crate::Effect,
    pub effect_segments: Vec<String>,
    pub action: String,
    pub action_symbol: Option<SymbolId>,
    pub args: Vec<HirExprId>,
    pub span: Span,
    pub summary: EffectSummary,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct StdCallableEffectFact {
    pub value: HirExprId,
    pub symbol: SymbolId,
    pub intrinsic: u32,
    pub flow_type: etas_types::TypeId,
    pub summary: EffectSummary,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct StdActionCallFact {
    pub intrinsic: u32,
    pub parameters: Vec<etas_types::TypeId>,
    pub output: etas_types::TypeId,
    pub invocation: crate::Effect,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum TaskCallKind {
    Run,
    Start,
    Await,
    Requeue,
    Park,
    Join,
    TryJoin,
    Collect,
    Race,
    MapConcurrent,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct TaskCallFact {
    pub kind: TaskCallKind,
    pub call: HirExprId,
    pub owner_scope: ScopeId,
    pub job: Option<HirExprId>,
    pub output: etas_types::TypeId,
    pub effects: EffectRow,
    pub behavior: EffectSummary,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ContinuationTransferFact {
    pub call: HirExprId,
    pub binder: SymbolId,
    pub target: TransferTarget,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum TransferTarget {
    Requeue,
    Park { task: HirExprId },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct LatentEffectFact {
    pub value: HirExprId,
    pub body: LatentEffectBodyRef,
    pub flow_type: etas_types::TypeId,
    pub inferred: EffectRow,
    pub declared_bound: Option<EffectRow>,
    pub summary: EffectSummary,
    pub realized_at: Vec<HirExprId>,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum LatentEffectBodyRef {
    Expr(HirExprId),
    Block(HirBlockId),
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
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

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct QualifiedName {
    pub segments: Vec<String>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum PublicEffectValue {
    ItemSignature(etas_types::ItemSignature),
    ValueType(etas_types::TypeId),
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum PublicEffectContractSource {
    ExplicitSourceAnnotation,
    GeneratedCheckedMetadata,
    DependencyMetadata,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct LatentEffectContract {
    pub value: HirExprId,
    pub body: LatentEffectBodyRef,
    pub flow_type: etas_types::TypeId,
    pub declared_bound: Option<EffectRow>,
    pub inferred: EffectRow,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct RequirementFactsForContract {
    pub requirements: crate::RequirementSet,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct EffectAcknowledgement {
    pub effects: EffectRow,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct HandlerValueFact {
    pub handled: EffectRow,
    pub produced: EffectRow,
    pub result: Option<etas_types::TypeId>,
    pub arms: Vec<HandlerArmFact>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct HandlerArmFact {
    pub unit: EffectUnit,
    pub action: HandlerActionFact,
    pub resumable: bool,
    pub resumes: ResumeSummary,
    pub completion: HandlerCompletionSummary,
    pub arm_effects: EffectRow,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct HandlerActionFact {
    pub effect_segments: Vec<String>,
    pub action: String,
    pub action_symbol: Option<SymbolId>,
    pub effect_type_args: Vec<etas_types::TypeId>,
    pub span: Span,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum ResumeSummary {
    None,
    Once,
    Multiple,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct HandlerCompletionSummary {
    pub resumes: ResumeSummary,
    pub finishes: FinishSummary,
    pub terminates_never: bool,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum FinishSummary {
    None,
    Once,
    Multiple,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct HandleApplicationFact {
    pub handler: HandlerValueRef,
    pub handled: EffectRow,
    pub produced: EffectRow,
    pub remaining: EffectRow,
    /// Applied arm traces at their request positions, or an explicit widened
    /// approximation. None means the handler's latent trace is unavailable.
    pub action_trace: Option<crate::ActionTraceDomain>,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum HandlerValueRef {
    Expr { expr: HirExprId },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct TryCaptureFact {
    pub operand: HirExprId,
    pub captured_error: etas_types::TypeId,
    pub result_type: etas_types::TypeId,
    pub conversions: Vec<ErrorConversionFact>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ErrorConversionFact {
    pub source_error: etas_types::TypeId,
    pub target_error: etas_types::TypeId,
}
