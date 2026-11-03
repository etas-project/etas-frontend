//! Residual monitor plan lowering and verification for checked trace specs.
//!
//! Plans are produced from the original analysis and dispositions of the same
//! effect pipeline run and the checked canonical projection supplied by the
//! project layer. A verified plan proves that its obligations, source
//! identities and monitor match that analysis. It does not prove backend
//! support, final revision binding or runtime installation; those are later
//! gates and the plans here are never execution authority.

use std::collections::{BTreeMap, BTreeSet};

use etas_code::residual::{
    binding::{BindingLayout, BindingLayoutLimits},
    identity::{
        BehaviorContractRef, MonitorTemplateId, ObligationId, ObligationIdentityLimits,
        ResidualObligationRef, ResidualRuleKind,
    },
    model::{ResidualKind, ResidualObligation, ResidualObligationSet, ResidualSetLimits},
    monitor::{
        AlternativeLimits, ConcreteEvidenceLimits, MAX_TRACE_TEMPLATE_NODES, TraceComponentLimits,
        TraceComponentMonitor,
    },
    pattern::ActionPatternLimits,
    plan::{SemanticPlanComponent, SemanticPlanLimits, SemanticPlanMapping, SemanticResidualPlan},
};
use etas_hir::HirItemId;

use crate::trace_spec::{
    AnalyzedTraceLoweringInput, TraceCallableScopeInput, TraceObligationIdentityInput,
    TraceObligationSourceLimits, TracePlanSourceBinding, TracePlanSourceInput,
    TracePlanSourceLimits, TraceResidualClause, TraceResidualEligibility,
    TraceResidualIneligibility, TraceResidualRequirement, TraceSpecAnalysisOutput,
    TraceStaticDispositions, classify_trace_residual,
};
use crate::{
    EffectFacts, EffectPipelineError, EffectRegistry, EffectSourceSnapshot, ResidualIdentityError,
    TraceMonitorProjectionRequest, TraceMonitorProjector, TraceMonitorSourceLimits,
};

/// Deterministic bounds for residual lowering. The values participate in
/// semantic plan hashes, so they are fixed constants of this plan schema.
#[derive(Clone, Copy, Debug)]
pub struct ResidualPlanLimits {
    pub monitor: AlternativeLimits,
    pub identity: ObligationIdentityLimits,
    pub layout: BindingLayoutLimits,
    pub source: TraceMonitorSourceLimits,
    pub max_snapshots: usize,
    pub max_conformances: usize,
    pub max_obligations_per_plan: usize,
    pub max_total_obligations: usize,
    pub max_identity_work: usize,
}

impl Default for ResidualPlanLimits {
    fn default() -> Self {
        let identity = ObligationIdentityLimits {
            max_generic_arguments: 64,
            max_structural_site_depth: etas_package_metadata::MAX_METADATA_GRAPH_DEPTH,
        };
        Self {
            monitor: AlternativeLimits {
                evidence: ConcreteEvidenceLimits {
                    pattern: ActionPatternLimits {
                        max_selectors: 16,
                        max_identity_segments: 64,
                        max_identity_bytes: 4096,
                        max_literal_bytes: 4096,
                    },
                    max_bindings: 64,
                    max_value_bytes: 64 * 1024,
                },
                max_alternatives: 64,
                max_clauses: 256,
                max_patterns: 512,
                max_checks_per_event: 4096,
                max_product_work_per_event: 16 * 1024,
            },
            identity,
            layout: BindingLayoutLimits {
                max_bindings: 64,
                max_projections_per_binding: 16,
                max_total_projections: 256,
            },
            source: TraceMonitorSourceLimits {
                max_sources: 4096,
                max_sites: etas_package_metadata::MAX_METADATA_GRAPH_NODES,
                max_clauses: 256,
                max_snapshot_bytes: 64 * 1024 * 1024,
                identity,
            },
            max_snapshots: 4096,
            max_conformances: etas_package_metadata::MAX_METADATA_GRAPH_NODES,
            max_obligations_per_plan: 256,
            max_total_obligations: MAX_TRACE_TEMPLATE_NODES,
            max_identity_work: 1 << 20,
        }
    }
}

impl ResidualPlanLimits {
    fn component(&self) -> TraceComponentLimits {
        TraceComponentLimits {
            identity: self.identity,
            layout: self.layout,
            monitor: self.monitor,
        }
    }

    fn plan(&self) -> SemanticPlanLimits {
        let entries = self.max_obligations_per_plan;
        SemanticPlanLimits {
            max_obligations: entries,
            max_components: entries,
            max_layouts: 1,
            max_mappings: entries,
            max_plan_nodes: entries
                .saturating_mul(4)
                .saturating_add(self.layout.max_total_projections)
                .saturating_add(self.layout.max_bindings)
                .saturating_add(1),
            identity: self.identity,
            layout: self.layout,
        }
    }

    fn obligation_set(&self) -> ResidualSetLimits {
        ResidualSetLimits {
            identity: self.identity,
            max_obligations: self.max_obligations_per_plan,
            max_generic_arguments: self.identity.max_generic_arguments,
        }
    }

    fn source_obligations(&self) -> TraceObligationSourceLimits {
        TraceObligationSourceLimits {
            max_snapshots: self.max_snapshots,
            max_snapshot_bytes: self.source.max_snapshot_bytes,
            identity: self.identity,
        }
    }
}

/// Why an eligible conformance has no plan. A missing plan always leaves the
/// conformance with its static rejection; it is never treated as accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResidualPlanFailure {
    /// No checked canonical projection was supplied to this pipeline run.
    MissingProjector,
    Projection(String),
    Identity(String),
    Plan(String),
    Verification(String),
}

/// A lowered but not yet verified plan for one checked conformance.
pub struct LoweredTracePlan {
    requirement: TraceResidualRequirement,
    contract: BehaviorContractRef,
    monitor: MonitorTemplateId,
    plan: SemanticResidualPlan,
    references: Vec<(ResidualObligationRef, TraceResidualClause)>,
}

/// Output of `LowerResidualMonitorPlansPass`: the eligibility of every
/// classified conformance plus lowering results for eligible ones.
#[derive(Default)]
pub struct ResidualMonitorPlans {
    eligibility: BTreeMap<usize, TraceResidualEligibility>,
    lowered: BTreeMap<usize, Result<LoweredTracePlan, ResidualPlanFailure>>,
}

impl ResidualMonitorPlans {
    pub fn eligibility(&self, type_fact_index: usize) -> Option<&TraceResidualEligibility> {
        self.eligibility.get(&type_fact_index)
    }
}

/// A plan whose obligations, source identities, contract reference and
/// monitor were independently re-derived from the same checked analysis.
/// Backend support and final revision binding remain separate gates.
pub struct VerifiedTracePlan {
    requirement: TraceResidualRequirement,
    contract: BehaviorContractRef,
    monitor: MonitorTemplateId,
    plan: std::sync::Arc<SemanticResidualPlan>,
    references: BTreeSet<ResidualObligationRef>,
}

impl VerifiedTracePlan {
    pub fn item(&self) -> HirItemId {
        self.requirement.item()
    }
    pub fn type_fact_index(&self) -> usize {
        self.requirement.type_fact_index()
    }
    pub fn requirement(&self) -> &TraceResidualRequirement {
        &self.requirement
    }
    pub fn contract(&self) -> BehaviorContractRef {
        self.contract
    }
    pub fn monitor(&self) -> MonitorTemplateId {
        self.monitor
    }
    pub fn plan(&self) -> &SemanticResidualPlan {
        &self.plan
    }
    /// The immutable plan shared with later binding and engine preparation.
    pub fn shared_plan(&self) -> &std::sync::Arc<SemanticResidualPlan> {
        &self.plan
    }
    pub fn references(&self) -> &BTreeSet<ResidualObligationRef> {
        &self.references
    }
}

/// Output of `VerifyResidualPlansPass`. Session-local; not portable evidence.
#[derive(Default)]
pub struct VerifiedResidualPlans {
    plans: BTreeMap<usize, VerifiedTracePlan>,
    failures: BTreeMap<usize, ResidualPlanFailure>,
    eligibility: BTreeMap<usize, TraceResidualEligibility>,
}

impl std::fmt::Debug for VerifiedResidualPlans {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedResidualPlans")
            .field("plans", &self.plans.len())
            .field("failures", &self.failures)
            .finish_non_exhaustive()
    }
}

impl VerifiedResidualPlans {
    pub fn plan(&self, type_fact_index: usize) -> Option<&VerifiedTracePlan> {
        self.plans.get(&type_fact_index)
    }
    pub fn plans(&self) -> impl Iterator<Item = &VerifiedTracePlan> {
        self.plans.values()
    }
    pub fn failure(&self, type_fact_index: usize) -> Option<&ResidualPlanFailure> {
        self.failures.get(&type_fact_index)
    }
    pub fn eligibility(&self, type_fact_index: usize) -> Option<&TraceResidualEligibility> {
        self.eligibility.get(&type_fact_index)
    }
    pub fn is_empty(&self) -> bool {
        self.plans.is_empty()
    }
    /// All verified obligation references owned by `item`.
    pub fn item_references(&self, item: HirItemId) -> BTreeSet<&ResidualObligationRef> {
        self.plans
            .values()
            .filter(|plan| plan.item() == item)
            .flat_map(|plan| plan.references.iter())
            .collect()
    }
}

/// Inputs shared by lowering and verification of one pipeline run.
#[derive(Clone, Copy)]
pub struct ResidualPlanInput<'a> {
    pub analysis: &'a TraceSpecAnalysisOutput,
    pub dispositions: &'a TraceStaticDispositions,
    pub hir: &'a etas_hir::HirRevision,
    pub registry: &'a EffectRegistry,
    pub facts: &'a EffectFacts,
    pub snapshots: &'a [EffectSourceSnapshot<'a>],
    pub projector: Option<&'a dyn TraceMonitorProjector>,
    pub limits: ResidualPlanLimits,
    pub control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
}

impl<'a> ResidualPlanInput<'a> {
    fn checkpoint(&self) -> Result<(), EffectPipelineError> {
        if let Some(control) = self.control {
            control
                .checkpoint()
                .map_err(EffectPipelineError::Interrupted)?;
        }
        Ok(())
    }

    fn contract(&self) -> BehaviorContractRef {
        self.projector
            .map_or(BehaviorContractRef::SourceConstraint, |projector| {
                projector.residual_contract()
            })
    }

    fn scope(&self, type_fact_index: usize, item: HirItemId) -> TraceCallableScopeInput<'a> {
        TraceCallableScopeInput {
            analysis: self.analysis,
            dispositions: self.dispositions,
            type_fact_index,
            item,
            hir: self.hir,
            snapshots: self.snapshots,
            limits: self.limits.source_obligations(),
            control: self.control,
        }
    }

    fn project(
        &self,
        type_fact_index: usize,
        item: HirItemId,
    ) -> Result<Result<TraceComponentMonitor, ResidualPlanFailure>, EffectPipelineError> {
        let Some(projector) = self.projector else {
            return Ok(Err(ResidualPlanFailure::MissingProjector));
        };
        let request = TraceMonitorProjectionRequest {
            registry: self.registry,
            facts: self.facts,
            lowering: AnalyzedTraceLoweringInput {
                analysis: self.analysis,
                dispositions: self.dispositions,
                type_fact_index,
                item,
                limits: self.limits.monitor,
                control: self.control,
            },
            source_limits: self.limits.source,
        };
        match projector.project_trace_monitor(request) {
            Ok(monitor) => Ok(Ok(monitor)),
            Err(ResidualIdentityError::Interrupted(interruption)) => {
                Err(EffectPipelineError::Interrupted(interruption))
            }
            Err(ResidualIdentityError::ResourceLimit { resource, limit }) => {
                Err(EffectPipelineError::AnalysisLimitExceeded { resource, limit })
            }
            Err(ResidualIdentityError::Unavailable(reason)) => {
                Ok(Err(ResidualPlanFailure::Projection(reason)))
            }
        }
    }

    /// Conformances to consider, in checked type-fact order.
    fn conformances(&self) -> Result<Vec<(usize, HirItemId)>, EffectPipelineError> {
        let indices = self.dispositions.indices().collect::<Vec<_>>();
        if indices.len() > self.limits.max_conformances {
            return Err(EffectPipelineError::AnalysisLimitExceeded {
                resource: "residual trace conformances",
                limit: self.limits.max_conformances,
            });
        }
        indices
            .into_iter()
            .map(|index| {
                self.analysis
                    .source_conformance(index)
                    .map(|source| (index, source.item))
                    .ok_or_else(|| EffectPipelineError::InvalidTraceSpecSourceSites {
                        reason: "residual lowering has no checked source conformance".into(),
                    })
            })
            .collect()
    }
}

fn classify(
    input: &ResidualPlanInput<'_>,
    index: usize,
    item: HirItemId,
) -> Result<TraceResidualEligibility, EffectPipelineError> {
    match classify_trace_residual(
        input.analysis,
        input.dispositions,
        index,
        item,
        input.control,
    ) {
        // Invalid per-item analysis is reported by trace validation at the
        // source anchor; it never becomes a residual obligation.
        Err(EffectPipelineError::InvalidTraceSpecSourceSites { .. }) => Ok(
            TraceResidualEligibility::Unsupported(TraceResidualIneligibility::MissingClauseFacts),
        ),
        other => other,
    }
}

/// Lowers every eligible conformance to an immutable semantic plan.
pub fn lower_residual_monitor_plans(
    input: ResidualPlanInput<'_>,
) -> Result<ResidualMonitorPlans, EffectPipelineError> {
    let mut output = ResidualMonitorPlans::default();
    let mut remaining = input.limits.max_total_obligations;
    for (index, item) in input.conformances()? {
        input.checkpoint()?;
        let eligibility = classify(&input, index, item)?;
        if let TraceResidualEligibility::NeedsConcrete(requirement) = &eligibility {
            remaining = remaining.checked_sub(requirement.clauses().len()).ok_or(
                EffectPipelineError::AnalysisLimitExceeded {
                    resource: "residual obligations",
                    limit: input.limits.max_total_obligations,
                },
            )?;
            let lowered = lower_requirement(&input, requirement.clone())?;
            output.lowered.insert(index, lowered);
        }
        output.eligibility.insert(index, eligibility);
    }
    Ok(output)
}

fn lower_requirement(
    input: &ResidualPlanInput<'_>,
    requirement: TraceResidualRequirement,
) -> Result<Result<LoweredTracePlan, ResidualPlanFailure>, EffectPipelineError> {
    let index = requirement.type_fact_index();
    let item = requirement.item();
    if requirement.clauses().len() > input.limits.max_obligations_per_plan {
        return Err(EffectPipelineError::AnalysisLimitExceeded {
            resource: "residual obligations per plan",
            limit: input.limits.max_obligations_per_plan,
        });
    }
    let monitor = match input.project(index, item)? {
        Ok(monitor) => monitor,
        Err(failure) => return Ok(Err(failure)),
    };
    let contract = input.contract();
    let layout = match BindingLayout::new(BTreeMap::new(), input.limits.layout) {
        Ok(layout) => layout,
        Err(error) => return Ok(Err(ResidualPlanFailure::Plan(format!("{error:?}")))),
    };
    let mut obligations = ResidualObligationSet::new(input.limits.obligation_set());
    let mut components = Vec::with_capacity(requirement.clauses().len());
    let mut mappings = Vec::with_capacity(requirement.clauses().len());
    let mut references = Vec::with_capacity(requirement.clauses().len());
    for clause in requirement.clauses() {
        input.checkpoint()?;
        let identity = match identity(input, &requirement, clause, contract)? {
            Ok(identity) => identity,
            Err(failure) => return Ok(Err(failure)),
        };
        let reference = match identity
            .reference(input.limits.identity.max_generic_arguments, input.control)
        {
            Ok(reference) => reference,
            Err(crate::trace_spec::TraceObligationReferenceError::Interrupted(interruption)) => {
                return Err(EffectPipelineError::Interrupted(interruption));
            }
            Err(error) => return Ok(Err(ResidualPlanFailure::Identity(format!("{error:?}")))),
        };
        let obligation = match ResidualObligation::new(
            identity.into_inputs(),
            ResidualKind::TraceContract {
                monitor: monitor.template_id(),
            },
            layout.id(),
            input.limits.identity,
        ) {
            Ok(obligation) => obligation,
            Err(error) => return Ok(Err(ResidualPlanFailure::Identity(format!("{error:?}")))),
        };
        let component = match monitor.compile_component(
            obligation.clone(),
            &layout,
            input.limits.component(),
        ) {
            Ok(component) => component,
            Err(error) => return Ok(Err(ResidualPlanFailure::Plan(format!("{error:?}")))),
        };
        mappings.push(SemanticPlanMapping {
            obligation: obligation.id(),
            component: component.id(),
        });
        match obligations.insert(obligation) {
            Ok(true) => {}
            Ok(false) => {
                return Ok(Err(ResidualPlanFailure::Plan(
                    "duplicate residual obligation identity".into(),
                )));
            }
            Err(error) => return Ok(Err(ResidualPlanFailure::Plan(format!("{error:?}")))),
        }
        components.push(SemanticPlanComponent::Trace(Box::new(component)));
        references.push((reference, *clause));
    }
    let plan = match SemanticResidualPlan::build(
        obligations,
        components,
        vec![layout],
        mappings,
        input.limits.plan(),
    ) {
        Ok(plan) => plan,
        Err(error) => return Ok(Err(ResidualPlanFailure::Plan(format!("{error:?}")))),
    };
    Ok(Ok(LoweredTracePlan {
        requirement,
        contract,
        monitor: monitor.template_id(),
        plan,
        references,
    }))
}

fn identity(
    input: &ResidualPlanInput<'_>,
    requirement: &TraceResidualRequirement,
    clause: &TraceResidualClause,
    contract: BehaviorContractRef,
) -> Result<
    Result<crate::trace_spec::TraceObligationIdentity, ResidualPlanFailure>,
    EffectPipelineError,
> {
    use crate::trace_spec::TraceObligationIdentityError as Error;
    match crate::trace_spec::trace_obligation_identity(TraceObligationIdentityInput {
        scope: input.scope(requirement.type_fact_index(), requirement.item()),
        alternative_index: clause.alternative_index,
        clause_index: clause.clause_index,
        clause_sites: &input.facts.trace_specs.source_clause_sites,
        contract,
        rule_kind: ResidualRuleKind::TraceContract,
        rule_suffix: &[],
        max_identity_work: input.limits.max_identity_work,
    }) {
        Ok(identity) => Ok(Ok(identity)),
        Err(Error::Interrupted(interruption)) => {
            Err(EffectPipelineError::Interrupted(interruption))
        }
        Err(Error::ResourceLimit) => Err(EffectPipelineError::AnalysisLimitExceeded {
            resource: "residual obligation identity work",
            limit: input.limits.max_identity_work,
        }),
        Err(error) => Ok(Err(ResidualPlanFailure::Identity(format!("{error:?}")))),
    }
}

/// Re-derives eligibility, obligation identities, contract reference and the
/// canonical monitor for every lowered plan, then validates source linkage
/// of the exact plan obligations. Only fully matching plans are verified.
pub fn verify_residual_plans(
    input: ResidualPlanInput<'_>,
    lowered: ResidualMonitorPlans,
) -> Result<VerifiedResidualPlans, EffectPipelineError> {
    let mut output = VerifiedResidualPlans::default();
    let expected = input.conformances()?;
    if expected.len() != lowered.eligibility.len()
        || expected
            .iter()
            .any(|(index, _)| !lowered.eligibility.contains_key(index))
        || lowered
            .lowered
            .keys()
            .any(|index| !lowered.eligibility.contains_key(index))
    {
        return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
            reason: "residual monitor plans do not cover the classified conformances".into(),
        });
    }
    let ResidualMonitorPlans {
        eligibility,
        mut lowered,
    } = lowered;
    for (index, item) in expected {
        input.checkpoint()?;
        let recorded = eligibility.get(&index).ok_or_else(|| {
            EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "residual eligibility is missing".into(),
            }
        })?;
        let actual = classify(&input, index, item)?;
        if *recorded != actual {
            return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "residual eligibility disagrees with its checked analysis".into(),
            });
        }
        let entry = lowered.remove(&index);
        match (&actual, entry) {
            (TraceResidualEligibility::NeedsConcrete(requirement), Some(Ok(plan))) => {
                match verify_plan(&input, requirement, plan)? {
                    Ok(verified) => {
                        output.plans.insert(index, verified);
                    }
                    Err(failure) => {
                        output.failures.insert(index, failure);
                    }
                }
            }
            (TraceResidualEligibility::NeedsConcrete(_), Some(Err(failure))) => {
                output.failures.insert(index, failure);
            }
            (TraceResidualEligibility::NeedsConcrete(_), None) => {
                return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "eligible residual conformance has no lowering result".into(),
                });
            }
            (_, Some(_)) => {
                return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "residual plan was lowered for an ineligible conformance".into(),
                });
            }
            (_, None) => {}
        }
        output.eligibility.insert(index, actual);
    }
    Ok(output)
}

fn verify_plan(
    input: &ResidualPlanInput<'_>,
    requirement: &TraceResidualRequirement,
    lowered: LoweredTracePlan,
) -> Result<Result<VerifiedTracePlan, ResidualPlanFailure>, EffectPipelineError> {
    let failure = |reason: &str| Ok(Err(ResidualPlanFailure::Verification(reason.into())));
    let contract = input.contract();
    if lowered.requirement != *requirement || lowered.contract != contract {
        return failure("lowered plan requirement or contract changed");
    }
    let monitor = match input.project(requirement.type_fact_index(), requirement.item())? {
        Ok(monitor) => monitor,
        Err(failure) => return Ok(Err(failure)),
    };
    if monitor.template_id() != lowered.monitor {
        return failure("plan monitor differs from the checked canonical projection");
    }
    let clauses = requirement.clauses();
    if lowered.references.len() != clauses.len()
        || lowered.plan.obligations().len() != clauses.len()
        || lowered
            .references
            .iter()
            .zip(clauses)
            .any(|((_, recorded), expected)| recorded != expected)
    {
        return failure("plan obligations differ from the residual requirement");
    }
    let mut references = BTreeSet::new();
    for ((reference, clause), _) in lowered.references.iter().zip(clauses) {
        input.checkpoint()?;
        let identity = match identity(input, requirement, clause, contract)? {
            Ok(identity) => identity,
            Err(failure) => return Ok(Err(failure)),
        };
        let id: ObligationId = identity.id();
        if reference.obligation() != id {
            return failure("plan obligation identity differs from its source");
        }
        match identity.validate_reference(
            reference,
            input.limits.identity.max_generic_arguments,
            input.control,
        ) {
            Ok(()) => {}
            Err(crate::trace_spec::TraceObligationReferenceError::Interrupted(interruption)) => {
                return Err(EffectPipelineError::Interrupted(interruption));
            }
            Err(_) => return failure("plan obligation bindings differ from its source"),
        }
        let Some(SemanticPlanComponent::Trace(component)) = lowered.plan.component_for(id) else {
            return failure("plan obligation has no trace component");
        };
        if component.obligation().identity() != identity.inputs()
            || !matches!(
                component.obligation().kind(),
                ResidualKind::TraceContract { monitor } if *monitor == lowered.monitor
            )
        {
            return failure("plan component does not implement the checked monitor");
        }
        if !references.insert(reference.clone()) {
            return failure("duplicate residual obligation reference");
        }
    }
    let bindings = lowered
        .references
        .iter()
        .map(|(reference, clause)| TracePlanSourceBinding {
            reference,
            type_fact_index: requirement.type_fact_index(),
            alternative_index: clause.alternative_index,
            clause_index: clause.clause_index,
            rule_suffix: &[],
        })
        .collect::<Vec<_>>();
    let limits = TracePlanSourceLimits {
        max_obligations: input.limits.max_obligations_per_plan,
        max_bindings: input.limits.max_obligations_per_plan,
        max_clause_sites: input.limits.source.max_sites,
        max_total_identity_work: input.limits.max_identity_work,
        source: input.limits.source_obligations(),
    };
    match crate::trace_spec::validate_trace_plan_sources(TracePlanSourceInput {
        plan: &lowered.plan,
        analysis: input.analysis,
        dispositions: input.dispositions,
        hir: input.hir,
        expected_contract: contract,
        clause_sites: &input.facts.trace_specs.source_clause_sites,
        snapshots: input.snapshots,
        expected: &references,
        bindings: &bindings,
        limits,
        control: input.control,
    }) {
        Ok(()) => {}
        Err(crate::trace_spec::TracePlanSourceError::Interrupted(interruption)) => {
            return Err(EffectPipelineError::Interrupted(interruption));
        }
        Err(error) => {
            return Ok(Err(ResidualPlanFailure::Verification(format!(
                "plan source linkage: {error:?}"
            ))));
        }
    }
    Ok(Ok(VerifiedTracePlan {
        requirement: lowered.requirement,
        contract,
        monitor: lowered.monitor,
        plan: std::sync::Arc::new(lowered.plan),
        references,
    }))
}

#[cfg(test)]
mod tests;
