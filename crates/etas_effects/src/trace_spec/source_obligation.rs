//! Source provenance checks for an analyzed trace obligation. These do not
//! prove scope/activation bindings, plan coverage, or runtime installation.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use etas_code::residual::{
    identity::{
        CheckedSourceSnapshot, ClauseId, ObligationId, ObligationIdentityError,
        ObligationIdentityLimits, ResidualObligationRef, ResidualOwnerId, SourceClauseSite,
        StaticBindingRecipe,
    },
    model::ResidualObligation,
    plan::SemanticResidualPlan,
};

use super::TraceSpecAnalysisOutput;
use crate::EffectSourceSnapshot;

#[derive(Clone, Copy, Debug)]
pub struct TraceObligationSourceLimits {
    pub max_snapshots: usize,
    pub max_snapshot_bytes: usize,
    pub identity: ObligationIdentityLimits,
}

pub struct TraceObligationSourceInput<'a> {
    pub analysis: &'a TraceSpecAnalysisOutput,
    pub type_fact_index: usize,
    pub alternative_index: usize,
    pub clause_index: usize,
    pub obligation: &'a ResidualObligation,
    pub clause_sites: &'a BTreeMap<ClauseId, SourceClauseSite>,
    pub snapshots: &'a [EffectSourceSnapshot<'a>],
    pub rule_suffix: &'a [u32],
    pub limits: TraceObligationSourceLimits,
    pub control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
}

/// A source callable's lexical activation site, not a dynamic invocation or
/// proof of plan coverage. The identities are derived from the exact source.
pub struct CheckedTraceCallableScope {
    pub owner: ResidualOwnerId,
    pub scope: etas_code::residual::identity::SemanticScopeId,
    pub activation: etas_code::residual::identity::ActivationSiteId,
    pub declaration: etas_code::residual::identity::SourceRevisionLocation,
}

#[derive(Clone, Copy)]
pub struct TraceCallableScopeInput<'a> {
    pub analysis: &'a TraceSpecAnalysisOutput,
    pub dispositions: &'a super::TraceStaticDispositions,
    pub type_fact_index: usize,
    pub item: etas_hir::HirItemId,
    pub hir: &'a etas_hir::HirRevision,
    pub snapshots: &'a [EffectSourceSnapshot<'a>],
    pub limits: TraceObligationSourceLimits,
    pub control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
}

/// The pre-analysis-source identity for one trace-contract rule. The semantic
/// monitor, eligibility, plan coverage and runtime installation are separate.
pub struct TraceObligationIdentity {
    id: ObligationId,
    inputs: etas_code::residual::identity::ObligationIdentityInputs,
}

impl TraceObligationIdentity {
    pub fn id(&self) -> ObligationId {
        self.id
    }
    pub fn inputs(&self) -> &etas_code::residual::identity::ObligationIdentityInputs {
        &self.inputs
    }
    pub fn into_inputs(self) -> etas_code::residual::identity::ObligationIdentityInputs {
        self.inputs
    }

    /// Produces a symbolic summary reference, not a residual plan or permit.
    pub fn reference(
        &self,
        max_bindings: usize,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<ResidualObligationRef, TraceObligationReferenceError> {
        use etas_code::residual::identity::GenericArgumentIdentity;
        if self.inputs.static_bindings.len() > max_bindings {
            return Err(TraceObligationReferenceError::BindingLimit);
        }
        let mut substitutions = BTreeMap::new();
        for (parameter, recipe) in &self.inputs.static_bindings {
            if let Some(control) = control {
                control
                    .checkpoint()
                    .map_err(TraceObligationReferenceError::Interrupted)?;
            }
            let value = match recipe {
                StaticBindingRecipe::TypeParameter(id) if id == parameter => {
                    GenericArgumentIdentity::TypeParameter(*id)
                }
                StaticBindingRecipe::EffectRowParameter(id) if id == parameter => {
                    GenericArgumentIdentity::EffectRowParameter(*id)
                }
                _ => {
                    return Err(TraceObligationReferenceError::InvalidSourceBinding(
                        *parameter,
                    ));
                }
            };
            substitutions.insert(*parameter, value);
        }
        let reference = ResidualObligationRef::new(self.id, substitutions)
            .map_err(TraceObligationReferenceError::InvalidIdentity)?;
        self.validate_reference(&reference, max_bindings, control)?;
        Ok(reference)
    }

    /// Checks that a specialized reference retains exactly this source
    /// obligation's binder identities and kinds. This is not plan evidence.
    pub fn validate_reference(
        &self,
        reference: &ResidualObligationRef,
        max_bindings: usize,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<(), TraceObligationReferenceError> {
        validate_reference_bindings(self.id, &self.inputs, reference, max_bindings, control)
    }
}

fn validate_reference_bindings(
    id: ObligationId,
    inputs: &etas_code::residual::identity::ObligationIdentityInputs,
    reference: &ResidualObligationRef,
    max_bindings: usize,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<(), TraceObligationReferenceError> {
    use etas_code::residual::identity::GenericArgumentIdentity;
    if inputs.static_bindings.len() > max_bindings || reference.substitutions().len() > max_bindings
    {
        return Err(TraceObligationReferenceError::BindingLimit);
    }
    if reference.obligation() != id {
        return Err(TraceObligationReferenceError::ObligationMismatch);
    }
    if reference.substitutions().len() != inputs.static_bindings.len() {
        return Err(TraceObligationReferenceError::BindingSetMismatch);
    }
    for (parameter, recipe) in &inputs.static_bindings {
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(TraceObligationReferenceError::Interrupted)?;
        }
        let value = reference
            .substitutions()
            .get(parameter)
            .ok_or(TraceObligationReferenceError::BindingSetMismatch)?;
        match (recipe, value) {
            (
                StaticBindingRecipe::TypeParameter(id),
                GenericArgumentIdentity::Type(_) | GenericArgumentIdentity::TypeParameter(_),
            ) if id == parameter => {}
            (
                StaticBindingRecipe::EffectRowParameter(id),
                GenericArgumentIdentity::EffectRow(_)
                | GenericArgumentIdentity::EffectRowParameter(_),
            ) if id == parameter => {}
            (StaticBindingRecipe::TypeParameter(id), _)
            | (StaticBindingRecipe::EffectRowParameter(id), _)
                if id == parameter =>
            {
                return Err(TraceObligationReferenceError::BindingKindMismatch(
                    *parameter,
                ));
            }
            _ => {
                return Err(TraceObligationReferenceError::InvalidSourceBinding(
                    *parameter,
                ));
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceObligationReferenceError {
    BindingLimit,
    ObligationMismatch,
    BindingSetMismatch,
    BindingKindMismatch(etas_code::residual::identity::GenericParameterId),
    InvalidSourceBinding(etas_code::residual::identity::GenericParameterId),
    InvalidIdentity(ObligationIdentityError),
    Interrupted(etas_utils::PipelineInterruption),
}

pub struct TraceObligationIdentityInput<'a> {
    pub scope: TraceCallableScopeInput<'a>,
    pub alternative_index: usize,
    pub clause_index: usize,
    pub clause_sites: &'a BTreeMap<ClauseId, SourceClauseSite>,
    /// Source constraint, or the already frozen input contract for a candidate.
    pub contract: etas_code::residual::identity::BehaviorContractRef,
    pub rule_kind: etas_code::residual::identity::ResidualRuleKind,
    pub rule_suffix: &'a [u32],
    pub max_identity_work: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceObligationIdentityError {
    Scope(TraceCallableScopeError),
    Analysis(crate::EffectPipelineError),
    Source(TraceObligationSourceError),
    MissingSpecInstance,
    MissingClause,
    MissingClauseSite,
    MissingSnapshot,
    InvalidBinder,
    ResourceLimit,
    InvalidIdentity(ObligationIdentityError),
    Interrupted(etas_utils::PipelineInterruption),
}

/// Derives an obligation identity directly from checked source provenance.
/// It does not classify an unknown as runtime-checkable or construct a plan.
pub fn trace_obligation_identity(
    input: TraceObligationIdentityInput<'_>,
) -> Result<TraceObligationIdentity, TraceObligationIdentityError> {
    let snapshots = checked_snapshots(
        input.scope.snapshots,
        input.scope.limits,
        input.scope.control,
    )
    .map_err(TraceObligationIdentityError::Source)?;
    let mut remaining = input.max_identity_work;
    trace_obligation_identity_with_snapshots(input, &snapshots, &mut remaining)
}

fn trace_obligation_identity_with_snapshots(
    input: TraceObligationIdentityInput<'_>,
    snapshots: &HashMap<etas_code::StableSourceKey, CheckedSourceSnapshot<'_>>,
    remaining: &mut usize,
) -> Result<TraceObligationIdentity, TraceObligationIdentityError> {
    use TraceObligationIdentityError as Error;
    use etas_code::{
        descriptor::GenericParameterKind,
        residual::{
            identity::{GenericParameterId, ObligationIdentityInputs},
            type_template::StaticTypeNode,
        },
    };
    let source = input
        .scope
        .analysis
        .source_conformance(input.scope.type_fact_index)
        .ok_or(Error::MissingSpecInstance)?;
    charge_work(remaining, input.scope.snapshots.len()).map_err(|_| Error::ResourceLimit)?;
    for (index, clauses) in source.clause_ids.iter().enumerate() {
        if let Some(control) = input.scope.control {
            control.checkpoint().map_err(Error::Interrupted)?;
        }
        for _ in 0..3 {
            charge_work(remaining, clauses.len().saturating_add(1))
                .map_err(|_| Error::ResourceLimit)?;
        }
        let permission = input
            .scope
            .analysis
            .checked_permission_fact(input.scope.type_fact_index, index)
            .map_err(Error::Analysis)?;
        for _ in 0..2 {
            charge_work(remaining, permission.uncovered().len())
                .map_err(|_| Error::ResourceLimit)?;
            charge_work(remaining, permission.requested.effects.len())
                .map_err(|_| Error::ResourceLimit)?;
        }
    }
    let scope = checked_trace_callable_scope_with_snapshots(input.scope, snapshots)
        .map_err(Error::Scope)?;
    let spec = source
        .source_instance
        .as_ref()
        .ok_or(Error::MissingSpecInstance)?;
    let clause = input
        .scope
        .analysis
        .checked_clause_fact(
            input.scope.type_fact_index,
            input.alternative_index,
            input.clause_index,
        )
        .map_err(Error::Analysis)?
        .source_clause
        .ok_or(Error::MissingClause)?;
    let site = input
        .clause_sites
        .get(&clause)
        .ok_or(Error::MissingClauseSite)?;
    if site.id() != clause {
        return Err(Error::MissingClauseSite);
    }
    charge_work(remaining, spec.structural_path().len()).map_err(|_| Error::ResourceLimit)?;
    charge_work(remaining, input.rule_suffix.len()).map_err(|_| Error::ResourceLimit)?;
    for _ in 0..2 {
        charge_work(remaining, site.structural_path().len()).map_err(|_| Error::ResourceLimit)?;
    }
    for recipe in spec.arguments() {
        if let Some(control) = input.scope.control {
            control.checkpoint().map_err(Error::Interrupted)?;
        }
        charge_recipe_work(remaining, recipe).map_err(|_| Error::ResourceLimit)?;
    }
    let spec_snapshot = snapshots
        .get(&spec.declaration().source)
        .ok_or(Error::MissingSnapshot)?;
    spec.validate_snapshot(spec_snapshot, input.scope.limits.identity)
        .map_err(Error::InvalidIdentity)?;
    let clause_snapshot = snapshots
        .get(&site.location().source)
        .ok_or(Error::MissingSnapshot)?;
    site.validate_snapshot(clause_snapshot, input.scope.limits.identity)
        .map_err(Error::InvalidIdentity)?;
    let rule = site
        .rule(
            clause_snapshot,
            input.rule_kind,
            input.rule_suffix,
            input.scope.limits.identity,
        )
        .map_err(Error::InvalidIdentity)?;
    let declaration = input
        .scope
        .hir
        .items
        .get(input.scope.item)
        .ok_or(Error::InvalidBinder)?;
    let params = match declaration {
        etas_hir::HirItem::Flow(flow) => flow.type_params.as_slice(),
        etas_hir::HirItem::Tool(tool) => tool.type_params.as_slice(),
        etas_hir::HirItem::Agent(_) => &[],
        _ => return Err(Error::InvalidBinder),
    };
    if params.len() > input.scope.limits.identity.max_generic_arguments {
        return Err(Error::ResourceLimit);
    }
    let mut available = BTreeMap::new();
    for (position, symbol) in params.iter().enumerate() {
        *remaining = remaining.checked_sub(1).ok_or(Error::ResourceLimit)?;
        if let Some(control) = input.scope.control {
            control.checkpoint().map_err(Error::Interrupted)?;
        }
        let data = input
            .scope
            .hir
            .symbols
            .get(*symbol)
            .ok_or(Error::InvalidBinder)?;
        let position = u32::try_from(position).map_err(|_| Error::ResourceLimit)?;
        let kind = match &data.def {
            etas_hir::SymbolDef::TypeParam {
                owner, param_index, ..
            } if *owner == input.scope.item && *param_index == position => {
                GenericParameterKind::Type
            }
            etas_hir::SymbolDef::EffectParam { owner, param_index }
                if *owner == input.scope.item && *param_index == position =>
            {
                GenericParameterKind::EffectRow
            }
            _ => return Err(Error::InvalidBinder),
        };
        let id = GenericParameterId::from_source_owner(scope.owner, position, kind);
        if available.insert(id, kind).is_some() {
            return Err(Error::InvalidBinder);
        }
    }
    let mut static_bindings = BTreeMap::new();
    let mut record = |id, kind| -> Result<(), TraceObligationIdentityError> {
        if available.get(&id) != Some(&kind) {
            return Err(Error::InvalidBinder);
        }
        let recipe = match kind {
            GenericParameterKind::Type => StaticBindingRecipe::TypeParameter(id),
            GenericParameterKind::EffectRow => StaticBindingRecipe::EffectRowParameter(id),
        };
        static_bindings.insert(id, recipe);
        Ok(())
    };
    for recipe in spec.arguments() {
        *remaining = remaining.checked_sub(1).ok_or(Error::ResourceLimit)?;
        if let Some(control) = input.scope.control {
            control.checkpoint().map_err(Error::Interrupted)?;
        }
        match recipe {
            StaticBindingRecipe::TypeParameter(id) => record(*id, GenericParameterKind::Type)?,
            StaticBindingRecipe::EffectRowParameter(id) => {
                record(*id, GenericParameterKind::EffectRow)?
            }
            StaticBindingRecipe::TypeTemplate(template) => {
                for node in template.nodes() {
                    *remaining = remaining.checked_sub(1).ok_or(Error::ResourceLimit)?;
                    if let Some(control) = input.scope.control {
                        control.checkpoint().map_err(Error::Interrupted)?;
                    }
                    if let StaticTypeNode::Parameter(id) = node {
                        record(*id, GenericParameterKind::Type)?;
                    }
                }
            }
            StaticBindingRecipe::ExplicitType(_) | StaticBindingRecipe::ExplicitEffectRow(_) => {}
        }
    }
    charge_work(remaining, static_bindings.len()).map_err(|_| Error::ResourceLimit)?;
    let inputs = ObligationIdentityInputs {
        origin: site.location().clone(),
        owner: scope.owner,
        contract: input.contract,
        spec: spec.id(),
        clause,
        rule,
        scope: scope.scope,
        activation: scope.activation,
        static_bindings,
    };
    let id = ObligationId::from_inputs(&inputs, input.scope.limits.identity)
        .map_err(Error::InvalidIdentity)?;
    Ok(TraceObligationIdentity { id, inputs })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceCallableScopeError {
    Analysis(crate::EffectPipelineError),
    Source(TraceObligationSourceError),
    MissingItem,
    MissingOwner,
    MissingSnapshot,
    OwnerMismatch,
    InvalidIdentity(ObligationIdentityError),
}

pub fn checked_trace_callable_scope(
    input: TraceCallableScopeInput<'_>,
) -> Result<CheckedTraceCallableScope, TraceCallableScopeError> {
    let snapshots = checked_snapshots(input.snapshots, input.limits, input.control)
        .map_err(TraceCallableScopeError::Source)?;
    checked_trace_callable_scope_with_snapshots(input, &snapshots)
}

fn checked_trace_callable_scope_with_snapshots(
    input: TraceCallableScopeInput<'_>,
    snapshots: &HashMap<etas_code::StableSourceKey, CheckedSourceSnapshot<'_>>,
) -> Result<CheckedTraceCallableScope, TraceCallableScopeError> {
    use TraceCallableScopeError as Error;
    use etas_code::residual::identity::{
        ActivationSiteId, CheckedSourceStructuralSite, SemanticScopeId,
    };
    let conformance = input
        .dispositions
        .verified_conformance(
            input.type_fact_index,
            input.item,
            input.analysis,
            input.control,
        )
        .map_err(Error::Analysis)?;
    let owner = conformance
        .source()
        .source_owner
        .as_ref()
        .ok_or(Error::MissingOwner)?;
    let declaration = input.hir.items.get(input.item).ok_or(Error::MissingItem)?;
    let kind = super::materialize::checked_callable_owner_kind(input.hir, input.item)
        .map_err(Error::Analysis)?;
    let span = declaration.span();
    let file = input
        .snapshots
        .iter()
        .find(|file| file.source == span.source)
        .ok_or(Error::MissingSnapshot)?;
    if file.key != owner.declaration().source {
        return Err(Error::OwnerMismatch);
    }
    let snapshot = snapshots.get(&file.key).ok_or(Error::MissingSnapshot)?;
    let location = snapshot
        .location(u64::from(span.range.start.0), u64::from(span.range.end.0))
        .map_err(Error::InvalidIdentity)?;
    let id = ResidualOwnerId::from_checked_source_declaration(snapshot, &location, kind)
        .map_err(Error::InvalidIdentity)?;
    if &location != owner.declaration() || kind != owner.kind() || id != owner.id() {
        return Err(Error::OwnerMismatch);
    }
    let site = CheckedSourceStructuralSite {
        location: &location,
        snapshot,
        structural_path: &[],
    };
    let scope = SemanticScopeId::from_checked_source_site(id, &site, input.limits.identity)
        .map_err(Error::InvalidIdentity)?;
    let activation = ActivationSiteId::from_checked_source_site(id, &site, input.limits.identity)
        .map_err(Error::InvalidIdentity)?;
    Ok(CheckedTraceCallableScope {
        owner: id,
        scope,
        activation,
        declaration: location,
    })
}

#[derive(Clone, Copy, Debug)]
pub struct TracePlanSourceLimits {
    pub max_obligations: usize,
    pub max_bindings: usize,
    pub max_clause_sites: usize,
    pub max_total_identity_work: usize,
    pub source: TraceObligationSourceLimits,
}

pub struct TracePlanSourceBinding<'a> {
    pub reference: &'a ResidualObligationRef,
    pub type_fact_index: usize,
    pub alternative_index: usize,
    pub clause_index: usize,
    pub rule_suffix: &'a [u32],
}

pub struct TracePlanSourceInput<'a> {
    pub plan: &'a SemanticResidualPlan,
    pub analysis: &'a TraceSpecAnalysisOutput,
    pub dispositions: &'a super::TraceStaticDispositions,
    pub hir: &'a etas_hir::HirRevision,
    /// Supplied by the checked compilation context, never read from the plan.
    pub expected_contract: etas_code::residual::identity::BehaviorContractRef,
    pub clause_sites: &'a BTreeMap<ClauseId, SourceClauseSite>,
    pub snapshots: &'a [EffectSourceSnapshot<'a>],
    pub expected: &'a BTreeSet<ResidualObligationRef>,
    pub bindings: &'a [TracePlanSourceBinding<'a>],
    pub limits: TracePlanSourceLimits,
    pub control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TracePlanSourceError {
    ObligationLimit,
    BindingLimit,
    ClauseSiteLimit,
    IdentityWorkLimit,
    DuplicateBinding(etas_code::CodeFingerprint),
    UnexpectedBinding(etas_code::CodeFingerprint),
    MissingBinding(etas_code::CodeFingerprint),
    UnexpectedObligation(ObligationId),
    MissingObligation(ObligationId),
    SourceIdentityMismatch(ObligationId),
    SourceIdentity {
        id: ObligationId,
        reference: etas_code::CodeFingerprint,
        reason: Box<TraceObligationIdentityError>,
    },
    Snapshots(TraceObligationSourceError),
    Interrupted(etas_utils::PipelineInterruption),
    Obligation {
        id: ObligationId,
        reference: etas_code::CodeFingerprint,
        reason: TraceObligationSourceError,
    },
    Reference {
        id: ObligationId,
        reference: etas_code::CodeFingerprint,
        reason: TraceObligationReferenceError,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceObligationSourceError {
    SnapshotLimit,
    SnapshotBytesLimit,
    DuplicateSnapshot,
    MissingAnalysis,
    MissingOwner,
    MissingSpecInstance,
    MissingClause,
    MissingClauseSite,
    MissingSnapshot,
    OwnerMismatch,
    SpecInstanceMismatch,
    ClauseMismatch,
    OriginMismatch,
    RuleMismatch,
    InvalidIdentity(ObligationIdentityError),
    Interrupted(etas_utils::PipelineInterruption),
}

pub fn validate_trace_obligation_source(
    input: TraceObligationSourceInput<'_>,
) -> Result<(), TraceObligationSourceError> {
    let snapshots = checked_snapshots(input.snapshots, input.limits, input.control)?;
    validate_source_link(&input, &snapshots)
}

/// Validates source linkage for the exact supplied references under one shared
/// budget. Binding values' provenance, monitor coverage and installation
/// require independent validation.
pub fn validate_trace_plan_sources(
    input: TracePlanSourceInput<'_>,
) -> Result<(), TracePlanSourceError> {
    use TracePlanSourceError as Error;
    if input.plan.obligations().len() > input.limits.max_obligations {
        return Err(Error::ObligationLimit);
    }
    if input.bindings.len() > input.limits.max_bindings
        || input.expected.len() > input.limits.max_bindings
    {
        return Err(Error::BindingLimit);
    }
    if input.clause_sites.len() > input.limits.max_clause_sites {
        return Err(Error::ClauseSiteLimit);
    }
    let snapshots = checked_snapshots(input.snapshots, input.limits.source, input.control)
        .map_err(|error| match error {
            TraceObligationSourceError::Interrupted(interruption) => {
                Error::Interrupted(interruption)
            }
            error => Error::Snapshots(error),
        })?;
    let mut seen = BTreeSet::new();
    let mut seen_ids = BTreeSet::new();
    let mut identities = BTreeMap::new();
    let mut remaining_work = input.limits.max_total_identity_work;
    for binding in input.bindings {
        if let Some(control) = input.control {
            control.checkpoint().map_err(Error::Interrupted)?;
        }
        charge_work(&mut remaining_work, 1)?;
        charge_work(&mut remaining_work, binding.rule_suffix.len())?;
        charge_work(&mut remaining_work, binding.reference.substitutions().len())?;
        let reference = binding.reference.fingerprint();
        if !input.expected.contains(binding.reference) {
            return Err(Error::UnexpectedBinding(reference));
        }
        if !seen.insert(binding.reference) {
            return Err(Error::DuplicateBinding(reference));
        }
        let id = binding.reference.obligation();
        seen_ids.insert(id);
        let obligation = input
            .plan
            .obligations()
            .get(id)
            .ok_or(Error::UnexpectedObligation(id))?;
        let source = input
            .analysis
            .source_conformance(binding.type_fact_index)
            .ok_or(Error::Obligation {
                id,
                reference,
                reason: TraceObligationSourceError::MissingAnalysis,
            })?;
        let spec = source.source_instance.as_ref().ok_or(Error::Obligation {
            id,
            reference,
            reason: TraceObligationSourceError::MissingSpecInstance,
        })?;
        let site = input
            .clause_sites
            .get(&obligation.identity().clause)
            .ok_or(Error::Obligation {
                id,
                reference,
                reason: TraceObligationSourceError::MissingClauseSite,
            })?;
        charge_work(&mut remaining_work, spec.structural_path().len())?;
        // Clause validation and rule reconstruction both revisit the site.
        for _ in 0..3 {
            charge_work(&mut remaining_work, site.structural_path().len())?;
        }
        for recipe in spec
            .arguments()
            .iter()
            .chain(obligation.identity().static_bindings.values())
        {
            if let Some(control) = input.control {
                control.checkpoint().map_err(Error::Interrupted)?;
            }
            charge_recipe_work(&mut remaining_work, recipe)?;
        }
        validate_source_link(
            &TraceObligationSourceInput {
                analysis: input.analysis,
                type_fact_index: binding.type_fact_index,
                alternative_index: binding.alternative_index,
                clause_index: binding.clause_index,
                obligation,
                clause_sites: input.clause_sites,
                snapshots: input.snapshots,
                rule_suffix: binding.rule_suffix,
                limits: input.limits.source,
                control: input.control,
            },
            &snapshots,
        )
        .map_err(|reason| match reason {
            TraceObligationSourceError::Interrupted(interruption) => {
                Error::Interrupted(interruption)
            }
            reason => Error::Obligation {
                id,
                reference,
                reason,
            },
        })?;
        let key = (
            binding.type_fact_index,
            binding.alternative_index,
            binding.clause_index,
            obligation.identity().rule.fingerprint(),
        );
        if let std::collections::btree_map::Entry::Vacant(entry) = identities.entry(key) {
            let identity = trace_obligation_identity_with_snapshots(
                TraceObligationIdentityInput {
                    scope: TraceCallableScopeInput {
                        analysis: input.analysis,
                        dispositions: input.dispositions,
                        type_fact_index: binding.type_fact_index,
                        item: source.item,
                        hir: input.hir,
                        snapshots: input.snapshots,
                        limits: input.limits.source,
                        control: input.control,
                    },
                    alternative_index: binding.alternative_index,
                    clause_index: binding.clause_index,
                    clause_sites: input.clause_sites,
                    contract: input.expected_contract,
                    rule_kind: obligation.identity().rule.kind(),
                    rule_suffix: binding.rule_suffix,
                    max_identity_work: remaining_work,
                },
                &snapshots,
                &mut remaining_work,
            )
            .map_err(|reason| match reason {
                TraceObligationIdentityError::ResourceLimit => Error::IdentityWorkLimit,
                TraceObligationIdentityError::Interrupted(interruption)
                | TraceObligationIdentityError::Source(TraceObligationSourceError::Interrupted(
                    interruption,
                ))
                | TraceObligationIdentityError::Analysis(
                    crate::EffectPipelineError::Interrupted(interruption),
                )
                | TraceObligationIdentityError::Scope(TraceCallableScopeError::Source(
                    TraceObligationSourceError::Interrupted(interruption),
                ))
                | TraceObligationIdentityError::Scope(TraceCallableScopeError::Analysis(
                    crate::EffectPipelineError::Interrupted(interruption),
                )) => Error::Interrupted(interruption),
                reason => Error::SourceIdentity {
                    id,
                    reference,
                    reason: Box::new(reason),
                },
            })?;
            entry.insert(identity);
        }
        let identity = identities
            .get(&key)
            .ok_or(Error::SourceIdentityMismatch(id))?;
        if identity.id() != id || identity.inputs() != obligation.identity() {
            return Err(Error::SourceIdentityMismatch(id));
        }
        charge_work(
            &mut remaining_work,
            obligation.identity().static_bindings.len(),
        )?;
        validate_reference_bindings(
            id,
            obligation.identity(),
            binding.reference,
            input.limits.source.identity.max_generic_arguments,
            input.control,
        )
        .map_err(|reason| match reason {
            TraceObligationReferenceError::Interrupted(interruption) => {
                Error::Interrupted(interruption)
            }
            reason => Error::Reference {
                id,
                reference,
                reason,
            },
        })?;
    }
    for reference in input.expected {
        if let Some(control) = input.control {
            control.checkpoint().map_err(Error::Interrupted)?;
        }
        charge_work(&mut remaining_work, 1)?;
        charge_work(&mut remaining_work, reference.substitutions().len())?;
        if !seen.contains(reference) {
            return Err(Error::MissingBinding(reference.fingerprint()));
        }
    }
    for obligation in input.plan.obligations().iter() {
        if let Some(control) = input.control {
            control.checkpoint().map_err(Error::Interrupted)?;
        }
        charge_work(&mut remaining_work, 1)?;
        if !seen_ids.contains(&obligation.id()) {
            return Err(Error::MissingObligation(obligation.id()));
        }
    }
    Ok(())
}

fn charge_work(remaining: &mut usize, amount: usize) -> Result<(), TracePlanSourceError> {
    *remaining = remaining
        .checked_sub(amount)
        .ok_or(TracePlanSourceError::IdentityWorkLimit)?;
    Ok(())
}

fn charge_recipe_work(
    remaining: &mut usize,
    recipe: &StaticBindingRecipe,
) -> Result<(), TracePlanSourceError> {
    charge_work(remaining, 1)?;
    if let StaticBindingRecipe::TypeTemplate(template) = recipe {
        // Usage accounting, identity validation and fingerprinting each walk
        // the template; charge before performing the first traversal.
        charge_work(remaining, template.nodes().len())?;
        let (_, edges, bytes) = template.resource_usage();
        for _ in 0..2 {
            charge_work(remaining, template.nodes().len())?;
        }
        for _ in 0..3 {
            charge_work(remaining, edges)?;
            charge_work(remaining, bytes)?;
        }
    }
    Ok(())
}

fn checked_snapshots<'a>(
    input: &[EffectSourceSnapshot<'a>],
    limits: TraceObligationSourceLimits,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<
    HashMap<etas_code::StableSourceKey, CheckedSourceSnapshot<'a>>,
    TraceObligationSourceError,
> {
    use TraceObligationSourceError as Error;
    if let Some(control) = control {
        control.checkpoint().map_err(Error::Interrupted)?;
    }
    if input.len() > limits.max_snapshots {
        return Err(Error::SnapshotLimit);
    }
    let mut total_bytes = 0usize;
    let mut source_ids = HashSet::new();
    let mut snapshots = HashMap::new();
    for source in input {
        if let Some(control) = control {
            control.checkpoint().map_err(Error::Interrupted)?;
        }
        total_bytes = total_bytes
            .checked_add(source.text.len())
            .ok_or(Error::SnapshotBytesLimit)?;
        if total_bytes > limits.max_snapshot_bytes {
            return Err(Error::SnapshotBytesLimit);
        }
        if !source_ids.insert(source.source) || snapshots.contains_key(&source.key) {
            return Err(Error::DuplicateSnapshot);
        }
        snapshots.insert(
            source.key,
            CheckedSourceSnapshot::new(source.key, source.text.as_bytes()),
        );
    }
    Ok(snapshots)
}

fn validate_source_link(
    input: &TraceObligationSourceInput<'_>,
    snapshots: &HashMap<etas_code::StableSourceKey, CheckedSourceSnapshot<'_>>,
) -> Result<(), TraceObligationSourceError> {
    use TraceObligationSourceError as Error;
    if let Some(control) = input.control {
        control.checkpoint().map_err(Error::Interrupted)?;
    }
    let source = input
        .analysis
        .source_conformance(input.type_fact_index)
        .ok_or(Error::MissingAnalysis)?;
    let clause = input
        .analysis
        .checked_clause_fact(
            input.type_fact_index,
            input.alternative_index,
            input.clause_index,
        )
        .map_err(|_| Error::MissingAnalysis)?;
    let clause = clause.source_clause.ok_or(Error::MissingClause)?;
    let owner = source.source_owner.as_ref().ok_or(Error::MissingOwner)?;
    let spec = source
        .source_instance
        .as_ref()
        .ok_or(Error::MissingSpecInstance)?;
    let site = input
        .clause_sites
        .get(&clause)
        .ok_or(Error::MissingClauseSite)?;
    let identity = input.obligation.identity();
    input
        .obligation
        .id()
        .validate(identity, input.limits.identity)
        .map_err(Error::InvalidIdentity)?;
    if identity.owner != owner.id() {
        return Err(Error::OwnerMismatch);
    }
    if identity.spec != spec.id() {
        return Err(Error::SpecInstanceMismatch);
    }
    if identity.clause != clause || site.id() != clause {
        return Err(Error::ClauseMismatch);
    }
    if &identity.origin != site.location() {
        return Err(Error::OriginMismatch);
    }
    let owner_snapshot = snapshots
        .get(&owner.declaration().source)
        .ok_or(Error::MissingSnapshot)?;
    let recomputed = ResidualOwnerId::from_checked_source_declaration(
        owner_snapshot,
        owner.declaration(),
        owner.kind(),
    )
    .map_err(Error::InvalidIdentity)?;
    if recomputed != owner.id() {
        return Err(Error::OwnerMismatch);
    }
    let spec_snapshot = snapshots
        .get(&spec.declaration().source)
        .ok_or(Error::MissingSnapshot)?;
    spec.validate_snapshot(spec_snapshot, input.limits.identity)
        .map_err(Error::InvalidIdentity)?;
    let clause_snapshot = snapshots
        .get(&site.location().source)
        .ok_or(Error::MissingSnapshot)?;
    site.validate_snapshot(clause_snapshot, input.limits.identity)
        .map_err(Error::InvalidIdentity)?;
    let rule = site
        .rule(
            clause_snapshot,
            identity.rule.kind(),
            input.rule_suffix,
            input.limits.identity,
        )
        .map_err(Error::InvalidIdentity)?;
    if identity.rule != rule {
        return Err(Error::RuleMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use etas_code::{
        descriptor::PrimitiveInfo,
        residual::type_template::{
            StaticTypeNode, StaticTypeTemplate, TemplateField, TypeTemplateLimits,
        },
    };

    #[test]
    fn identity_work_counts_template_nodes_edges_and_field_bytes() {
        let template = StaticTypeTemplate::new(
            vec![
                StaticTypeNode::Primitive(PrimitiveInfo::I32),
                StaticTypeNode::Record(vec![TemplateField {
                    name: "abc".into(),
                    ty: 0,
                }]),
            ],
            1,
            TypeTemplateLimits::default(),
        )
        .unwrap();
        let recipe = StaticBindingRecipe::TypeTemplate(template);
        let mut exact = 19;
        assert_eq!(charge_recipe_work(&mut exact, &recipe), Ok(()));
        assert_eq!(exact, 0);
        let mut short = 18;
        assert_eq!(
            charge_recipe_work(&mut short, &recipe),
            Err(TracePlanSourceError::IdentityWorkLimit)
        );
    }
}
