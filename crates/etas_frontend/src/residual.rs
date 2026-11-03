//! Checked residual monitor plans of one compilation, as consumed by an
//! execution engine.
//!
//! Every plan here was verified against the analysis of the same compilation
//! and checked against the selected target's implemented semantics. A plan
//! bound to a checked revision describes what must be enforced; it is never
//! installation evidence, a grant or permission to dispatch an action. Only
//! the frontend constructs these values.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

mod application;
pub use application::CheckedResidualApplication;

use etas_code::{
    CallableId, CodeFingerprint, RevisionInfo, RevisionInputs,
    contract::CanonicalEffect,
    descriptor::{DescriptorLimits, GenericOwner},
    residual::{
        identity::{BehaviorContractRef, MonitorTemplateId, ResidualObligationRef},
        plan::{BoundResidualPlan, BoundResidualPlanError, SemanticResidualPlan},
        target::{ResidualBackendSupport, ResidualTargetCapabilities},
    },
};
use etas_hir::HirItemId;

use crate::{
    CheckedProgramData,
    code::{
        descriptor::{DefinitionDescriptorProjector, DescriptorInput, ProjectionError},
        identity::DefinitionIdentityIndex,
    },
};

/// Upper bound on obligations bound for one plan; plans are already bounded
/// by the effect lowering limits.
const MAX_BOUND_OBLIGATIONS: usize = 4096;

/// One verified, target-supported plan of one checked trace conformance.
pub struct CheckedResidualPlan {
    item: HirItemId,
    type_fact_index: usize,
    contract: BehaviorContractRef,
    monitor: MonitorTemplateId,
    plan: Arc<SemanticResidualPlan>,
    references: BTreeSet<ResidualObligationRef>,
    support: ResidualBackendSupport,
}

impl CheckedResidualPlan {
    pub fn item(&self) -> HirItemId {
        self.item
    }
    pub fn type_fact_index(&self) -> usize {
        self.type_fact_index
    }
    pub fn contract(&self) -> BehaviorContractRef {
        self.contract
    }
    pub fn monitor(&self) -> MonitorTemplateId {
        self.monitor
    }
    pub fn plan(&self) -> &Arc<SemanticResidualPlan> {
        &self.plan
    }
    pub fn references(&self) -> &BTreeSet<ResidualObligationRef> {
        &self.references
    }
    pub fn support(&self) -> ResidualBackendSupport {
        self.support
    }
}

impl std::fmt::Debug for CheckedResidualPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckedResidualPlan")
            .field("item", &self.item)
            .field("type_fact_index", &self.type_fact_index)
            .field("plan", &self.plan.hash())
            .field("obligations", &self.references.len())
            .finish_non_exhaustive()
    }
}

/// Final binding of every plan to the checked revision of its owner. It is
/// created only after the owner's contract facts were finalized.
#[derive(Clone, Debug)]
pub struct CheckedResidualBinding {
    revisions: BTreeMap<HirItemId, RevisionInfo>,
    bound: BTreeMap<usize, BoundResidualPlan>,
}

impl etas_core::memory::RetainedSize for CheckedResidualBinding {
    fn visit_retained<'a>(&'a self, visitor: &mut etas_core::memory::MemoryVisitor<'a>) {
        visitor.enqueue(&self.revisions);
    }
}

impl CheckedResidualBinding {
    pub fn revision(&self, item: HirItemId) -> Option<&RevisionInfo> {
        self.revisions.get(&item)
    }
    pub fn bound(&self, type_fact_index: usize) -> Option<&BoundResidualPlan> {
        self.bound.get(&type_fact_index)
    }
    pub fn is_empty(&self) -> bool {
        self.bound.is_empty()
    }

    /// Revalidates every bound plan against its semantic plan, its owner's
    /// revision and its exact obligation references. A binding that omits a
    /// plan, binds an unknown plan or mismatches any linkage is rejected.
    pub fn validate(&self, plans: &CheckedResidualPlans) -> Result<(), ResidualBindingError> {
        if self.bound.len() != plans.plans.len() {
            return Err(ResidualBindingError::Coverage);
        }
        for plan in &plans.plans {
            let bound = self
                .bound
                .get(&plan.type_fact_index)
                .ok_or(ResidualBindingError::Coverage)?;
            let revision = self
                .revisions
                .get(&plan.item)
                .ok_or(ResidualBindingError::MissingRevision(plan.item))?;
            BoundResidualPlan::from_data(
                bound.data().clone(),
                &plan.plan,
                revision,
                &plan.references,
                MAX_BOUND_OBLIGATIONS,
            )
            .map_err(ResidualBindingError::Bound)?;
        }
        if self
            .revisions
            .keys()
            .any(|item| !plans.by_item.contains_key(item))
        {
            return Err(ResidualBindingError::Coverage);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResidualBindingError {
    Coverage,
    MissingRevision(HirItemId),
    Owner(String),
    Bound(BoundResidualPlanError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResidualActionError {
    MissingIdentityIndex,
    UnknownOwner(HirItemId),
    Projection(String),
}

/// All residual plans required by one checked program.
pub struct CheckedResidualPlans {
    target: ResidualTargetCapabilities,
    plans: Vec<Arc<CheckedResidualPlan>>,
    by_item: BTreeMap<HirItemId, Vec<usize>>,
    identities: Option<Arc<DefinitionIdentityIndex>>,
    foreign_type_bindings: bool,
    limits: DescriptorLimits,
    binding: Option<CheckedResidualBinding>,
}

impl Default for CheckedResidualPlans {
    fn default() -> Self {
        Self {
            target: ResidualTargetCapabilities::none(),
            plans: Vec::new(),
            by_item: BTreeMap::new(),
            identities: None,
            foreign_type_bindings: false,
            limits: DescriptorLimits::default(),
            binding: None,
        }
    }
}

impl std::fmt::Debug for CheckedResidualPlans {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckedResidualPlans")
            .field("target", &self.target.fingerprint())
            .field("plans", &self.plans)
            .field(
                "bound",
                &self.binding.as_ref().map(|binding| binding.bound.len()),
            )
            .finish_non_exhaustive()
    }
}

impl etas_core::memory::RetainedSize for CheckedResidualPlans {
    fn visit_retained<'a>(&'a self, visitor: &mut etas_core::memory::MemoryVisitor<'a>) {
        if let Some(identities) = &self.identities {
            visitor.visit_shared_arc(identities);
        }
    }
}

impl CheckedResidualPlans {
    pub fn is_empty(&self) -> bool {
        self.plans.is_empty()
    }
    /// The capabilities every plan was checked against.
    pub fn target(&self) -> &ResidualTargetCapabilities {
        &self.target
    }
    pub fn plans(&self) -> impl Iterator<Item = &Arc<CheckedResidualPlan>> {
        self.plans.iter()
    }
    pub fn items(&self) -> impl Iterator<Item = HirItemId> + '_ {
        self.by_item.keys().copied()
    }
    pub fn item_plans(&self, item: HirItemId) -> impl Iterator<Item = &Arc<CheckedResidualPlan>> {
        self.by_item
            .get(&item)
            .into_iter()
            .flatten()
            .filter_map(|index| self.plans.get(*index))
    }
    /// The final binding of an ordinary program. Candidate programs carry
    /// their binding with the emitted candidate instead.
    pub fn binding(&self) -> Option<&CheckedResidualBinding> {
        self.binding.as_ref()
    }

    /// The exact obligation references owned by `item`.
    pub fn item_references(&self, item: HirItemId) -> BTreeSet<&ResidualObligationRef> {
        self.item_plans(item)
            .flat_map(|plan| plan.references.iter())
            .collect()
    }

    /// Checks exact obligation instances against the verified plans and the
    /// target support captured by this checked program. This is installability,
    /// not native engine installation or permission to run an invocation.
    pub fn supports_references(&self, required: &BTreeSet<ResidualObligationRef>) -> bool {
        if required.len() > MAX_BOUND_OBLIGATIONS {
            return false;
        }
        required.iter().all(|reference| {
            let mut covering = self.plans.iter().filter(|plan| {
                plan.plan
                    .obligations()
                    .get(reference.obligation())
                    .is_some_and(|obligation| obligation.accepts_reference(reference))
            });
            let Some(plan) = covering.next() else {
                return false;
            };
            covering.next().is_none()
                && plan.support.matches(&plan.plan, &self.target)
                && plan
                    .plan
                    .obligations()
                    .get(reference.obligation())
                    .is_some()
        })
    }

    /// Projects a concrete action of `program` to the canonical identity used
    /// by the plans' action patterns. `owner` must own a plan.
    pub fn canonical_action(
        &self,
        program: &CheckedProgramData,
        owner: HirItemId,
        effect: &etas_effects::Effect,
    ) -> Result<CanonicalEffect, ResidualActionError> {
        if !self.by_item.contains_key(&owner) {
            return Err(ResidualActionError::UnknownOwner(owner));
        }
        let identities = self
            .identities
            .as_deref()
            .ok_or(ResidualActionError::MissingIdentityIndex)?;
        let generic_owner = item_owner(identities, &program.hir, owner)
            .map_err(|_| ResidualActionError::UnknownOwner(owner))?;
        DefinitionDescriptorProjector::from_input(
            self.descriptor_input(program),
            identities,
            self.limits,
        )
        .concrete_action_instance(generic_owner, effect)
        .map_err(|error| ResidualActionError::Projection(error.to_string()))
    }

    fn descriptor_input<'a>(&'a self, program: &'a CheckedProgramData) -> DescriptorInput<'a> {
        DescriptorInput {
            hir: &program.hir,
            sources: None,
            types: &program.types,
            type_store: &program.type_store,
            std_registry: &program.std_registry,
            resolved_imports: &program.resolved_imports,
            module_index: &program.module_index,
            effect_registry: &program.effect_registry,
            effects: None,
            foreign_type_bindings: self
                .foreign_type_bindings
                .then_some(program.foreign_type_bindings.as_ref()),
        }
    }

    /// Plans of one compilation checked against `target`; unbound.
    pub(crate) fn new(
        plans: &etas_effects::residual::VerifiedResidualPlans,
        support: &crate::passes::execution::ResidualBackendSupportFacts,
        target: &ResidualTargetCapabilities,
        identities: Option<Arc<DefinitionIdentityIndex>>,
        foreign_type_bindings: bool,
        limits: DescriptorLimits,
    ) -> Result<Self, &'static str> {
        if !support.covers(plans, target) {
            return Err("residual backend support does not cover the verified plans");
        }
        if !plans.is_empty() && identities.is_none() {
            return Err("residual plans require their checked identity index");
        }
        let mut checked = Self {
            target: target.clone(),
            identities,
            foreign_type_bindings,
            limits,
            ..Self::default()
        };
        for plan in plans.plans() {
            let support = support
                .support(plan.type_fact_index())
                .ok_or("residual plan has no backend support result")?;
            checked
                .by_item
                .entry(plan.item())
                .or_default()
                .push(checked.plans.len());
            checked.plans.push(Arc::new(CheckedResidualPlan {
                item: plan.item(),
                type_fact_index: plan.type_fact_index(),
                contract: plan.contract(),
                monitor: plan.monitor(),
                plan: plan.shared_plan().clone(),
                references: plan.references().clone(),
                support,
            }));
        }
        Ok(checked)
    }

    /// Binds every plan of an ordinary program to its owner's checked
    /// revision. Must run after contract facts were finalized.
    pub(crate) fn bind_ordinary(
        &self,
        program: &CheckedProgramData,
    ) -> Result<CheckedResidualBinding, ResidualBindingError> {
        let mut revisions = BTreeMap::new();
        for item in self.by_item.keys() {
            revisions.insert(*item, self.checked_item_revision(program, *item)?);
        }
        self.bind_revisions(revisions)
    }

    /// The edited entry uses its validated frozen-contract revision. Retained
    /// dependencies keep their own freshly checked source-owner revisions;
    /// their scopes must never be rebound to the entry's callable identity.
    pub(crate) fn bind_candidate(
        &self,
        program: &CheckedProgramData,
        entry: HirItemId,
        revision: &RevisionInfo,
    ) -> Result<CheckedResidualBinding, ResidualBindingError> {
        let revisions = self
            .by_item
            .keys()
            .map(|item| {
                Ok((
                    *item,
                    if *item == entry {
                        revision.clone()
                    } else {
                        self.checked_item_revision(program, *item)?
                    },
                ))
            })
            .collect::<Result<_, ResidualBindingError>>()?;
        self.bind_revisions(revisions)
    }

    pub(crate) fn with_binding(mut self, binding: CheckedResidualBinding) -> Self {
        self.binding = Some(binding);
        self
    }

    fn bind_revisions(
        &self,
        revisions: BTreeMap<HirItemId, RevisionInfo>,
    ) -> Result<CheckedResidualBinding, ResidualBindingError> {
        let mut bound = BTreeMap::new();
        for plan in &self.plans {
            let revision = revisions
                .get(&plan.item)
                .ok_or(ResidualBindingError::MissingRevision(plan.item))?;
            bound.insert(
                plan.type_fact_index,
                BoundResidualPlan::bind(
                    &plan.plan,
                    revision,
                    &plan.references,
                    MAX_BOUND_OBLIGATIONS,
                )
                .map_err(ResidualBindingError::Bound)?,
            );
        }
        let binding = CheckedResidualBinding { revisions, bound };
        binding.validate(self)?;
        Ok(binding)
    }

    /// The checked revision of an ordinary source owner: its canonical
    /// callable identity, exact source revision, validated environment, its
    /// finalized checked effect contract including the verified plans, and
    /// the compilation configuration. It never hashes a bound plan.
    fn checked_item_revision(
        &self,
        program: &CheckedProgramData,
        item: HirItemId,
    ) -> Result<RevisionInfo, ResidualBindingError> {
        let owner_error = |reason: &str| ResidualBindingError::Owner(reason.into());
        let identities = self
            .identities
            .as_deref()
            .ok_or_else(|| owner_error("residual identity index is missing"))?;
        let owner = item_owner(identities, &program.hir, item)
            .map_err(|error| ResidualBindingError::Owner(error.to_string()))?;
        let definition = match &owner {
            GenericOwner::Declaration(key) => CallableId::declaration(key.clone())
                .map_err(|error| ResidualBindingError::Owner(format!("{error:?}")))?,
            GenericOwner::Method(method) => CallableId::method(method.clone()),
        };
        let declaration = program
            .hir
            .items
            .get(item)
            .ok_or_else(|| owner_error("residual owner declaration is missing"))?;
        let span = declaration.span();
        let file = program
            .sources
            .sources
            .iter()
            .find(|file| file.id == span.source)
            .ok_or_else(|| owner_error("residual owner source is missing"))?;
        let key = program
            .sources
            .stable_source_key(span.source)
            .map_err(|_| owner_error("residual owner has no stable source identity"))?;
        let source = etas_code::SourceRevisionKey::new(key, file.text.as_bytes());
        let content = CodeFingerprint::of_parts(
            "etas.frontend.residual.checked-item-content.v1",
            [
                source.fingerprint().as_bytes().as_slice(),
                &u64::from(span.range.start.0).to_le_bytes(),
                &u64::from(span.range.end.0).to_le_bytes(),
            ],
        );
        let summary = program
            .effects
            .item_effects
            .get(&item)
            .ok_or_else(|| owner_error("residual owner has no checked effect summary"))?;
        let row = |row: &etas_effects::EffectRow| -> Result<CodeFingerprint, ResidualBindingError> {
            let mut projector = DefinitionDescriptorProjector::from_input(
                self.descriptor_input(program),
                identities,
                self.limits,
            );
            match declaration {
                etas_hir::HirItem::Flow(flow) if !flow.type_params.is_empty() => {}
                etas_hir::HirItem::Tool(tool) if !tool.type_params.is_empty() => {}
                _ => {
                    return projector
                        .finish_analysis_row_contract_fingerprint(owner.clone(), row)
                        .map_err(projection_owner_error);
                }
            }
            let input = self.descriptor_input(program);
            let signature = crate::code::definition::callable_signature(input, item)
                .ok_or_else(|| owner_error("residual owner signature is missing"))?;
            let (names, declared) = crate::code::definition::callable_source_header(input, item)
                .map_err(projection_owner_error)?;
            projector
                .project_signature(owner.clone(), signature, &names, declared)
                .map_err(projection_owner_error)?;
            projector
                .finish_scoped_analysis_row_contract_fingerprint(&owner, row)
                .map_err(projection_owner_error)
        };
        let requested = row(&summary.requested_actions)?;
        let defaults = row(&summary.default_actions)?;
        let mut contract_parts = vec![
            definition.fingerprint().as_bytes().to_vec(),
            requested.as_bytes().to_vec(),
            defaults.as_bytes().to_vec(),
        ];
        for plan in self.item_plans(item) {
            contract_parts.push((plan.type_fact_index as u64).to_le_bytes().to_vec());
            contract_parts.push(plan.plan.hash().fingerprint().as_bytes().to_vec());
            contract_parts.push(plan.monitor.fingerprint().as_bytes().to_vec());
            contract_parts.push(match plan.contract {
                BehaviorContractRef::SourceConstraint => vec![1],
                BehaviorContractRef::FrozenInput(id) => {
                    let mut bytes = vec![2];
                    bytes.extend_from_slice(id.fingerprint().as_bytes());
                    bytes
                }
            });
            for reference in &plan.references {
                contract_parts.push(reference.fingerprint().as_bytes().to_vec());
            }
        }
        let contract = CodeFingerprint::of_parts(
            "etas.frontend.residual.checked-item-contract.v1",
            contract_parts.iter().map(Vec::as_slice),
        );
        let environment = CodeFingerprint::of_bytes(
            "etas.frontend.residual.checked-environment.v1",
            program.project_environment_fingerprint.as_bytes(),
        );
        let options = program.compile_options.canonical_options_fingerprint();
        let target = self.target.fingerprint();
        let compilation = CodeFingerprint::of_parts(
            "etas.frontend.residual.checked-compilation.v1",
            [
                program.compiler_version.as_bytes(),
                options.as_bytes(),
                target.as_bytes().as_slice(),
            ],
        );
        Ok(RevisionInfo::new(RevisionInputs {
            definition,
            content,
            environment,
            contract,
            compilation,
            parent: None,
        }))
    }
}

fn projection_owner_error(error: ProjectionError) -> ResidualBindingError {
    ResidualBindingError::Owner(error.to_string())
}

fn item_owner(
    identities: &DefinitionIdentityIndex,
    hir: &etas_hir::HirProgram,
    item: HirItemId,
) -> Result<GenericOwner, ProjectionError> {
    let symbol = hir
        .items
        .get(item)
        .and_then(etas_hir::HirItem::symbol)
        .ok_or_else(|| ProjectionError::MissingIdentity("residual owner declaration".into()))?;
    if let Some(method) = identities.method(symbol) {
        Ok(GenericOwner::Method(method.clone()))
    } else {
        identities
            .symbol(symbol)
            .cloned()
            .map(GenericOwner::Declaration)
            .ok_or_else(|| ProjectionError::MissingIdentity("residual owner identity".into()))
    }
}
