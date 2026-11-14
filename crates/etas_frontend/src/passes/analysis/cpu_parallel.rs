use std::collections::HashMap;
use std::sync::Arc;

use etas_effects::{ActionTraceDomain, EffectSummary, RequestInstantiationSummary};
use etas_hir::{
    ExprView, HirExpr, HirExprId, HirItem, HirItemId, HirVisitor, ResolveResult, walk_item,
};
use etas_types::ItemSignature;
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::ProjectContext;
use crate::passes::artifacts::{
    CPU_PARALLEL_ELIGIBILITY, EFFECT_OUTPUT, HIR_OUTPUT, MODULE_INDEX, SOURCE_SET, TYPE_OUTPUT,
    VALIDATED_EXTERNAL_ENVIRONMENT,
};

mod closures;
mod freeze;
mod origin;
mod work;
pub use freeze::{CpuCaptureContract, CpuFreezeContract};
pub use origin::CpuCallableOrigin;

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
pub struct CpuParallelEligibilityFacts {
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub callables: HashMap<HirItemId, CpuParallelEligibilityFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub closures: HashMap<HirExprId, CpuParallelEligibilityFact>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub struct CpuParallelEligibilityFact {
    /// Source item that owns the callable's code and generic context.
    pub owner: HirItemId,
    pub eligible: bool,
    pub rejection: Option<CpuParallelRejection>,
    pub freeze_contract: Option<CpuFreezeContract>,
    pub origin: Option<CpuCallableOrigin>,
    /// Approximate HIR operations, including literal bounded loops and known
    /// callees. Recursive or dynamic work stays unknown and can run inline.
    pub estimated_work: Option<u64>,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub enum CpuParallelRejection {
    OpenEffectRow,
    HostAction,
    HandlerCapture,
    LiveContinuation,
    MutableRuntimeResource,
    NonFreezableCapture,
    DynamicCode,
}

pub struct AnalyzeCpuParallelEligibilityPass;

impl Pass<ProjectContext> for AnalyzeCpuParallelEligibilityPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("AnalyzeCpuParallelEligibilityPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                EFFECT_OUTPUT,
                HIR_OUTPUT,
                TYPE_OUTPUT,
                MODULE_INDEX,
                SOURCE_SET,
                VALIDATED_EXTERNAL_ENVIRONMENT,
            ]))
            .produces(ArtifactSet::one(CPU_PARALLEL_ELIGIBILITY))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let Some(effects) = context.effects.as_ref() else {
            return PassResult::failed("CPU parallel eligibility requires checked effect facts");
        };
        let (Some(hir), Some(types)) = (context.hir.as_ref(), context.types.as_ref()) else {
            return PassResult::failed("CPU parallel eligibility requires checked HIR and types");
        };
        let mut dependencies = HashMap::new();
        let Some(origins) = origin::CallableOrigins::new(context) else {
            return PassResult::failed(
                "CPU eligibility requires checked source and package ownership",
            );
        };
        let work = work::estimate(&hir.view());
        let mut callables: HashMap<_, _> = effects
            .facts
            .item_effects
            .iter()
            .map(|(callable, summary)| {
                let mut captures = CallableCaptures {
                    hir: &hir.hir,
                    registry: &context.std_registry,
                    dependencies: Vec::new(),
                    has_global: false,
                    has_boundary: false,
                };
                walk_item(&hir.view(), *callable, &mut captures);
                dependencies.insert(*callable, captures.dependencies);
                let origin = origins.callable(&hir.hir, *callable, &types.facts);
                let (freeze_contract, supported) = match types.facts.item_signatures.get(callable) {
                    Some(ItemSignature::Flow(signature)) => {
                        let contract = CpuFreezeContract {
                            parameters: signature.params.clone(),
                            output: signature.output,
                            captures: Vec::new(),
                        };
                        let mut parameters = signature.generic_params.clone();
                        if let Some(method) = hir.hir.items[*callable]
                            .symbol()
                            .and_then(|symbol| types.facts.type_impl_methods.get(&symbol))
                        {
                            parameters.extend(method.owner_params.iter().cloned());
                        }
                        let supported = contract.supports_template(&types.store, &parameters);
                        (Some(contract), supported)
                    }
                    _ => (None, false),
                };
                let rejection = rejection(summary).or_else(|| {
                    if captures.has_boundary {
                        return Some(CpuParallelRejection::MutableRuntimeResource);
                    }
                    if freeze_contract.is_none() || origin.is_none() {
                        return Some(CpuParallelRejection::DynamicCode);
                    }
                    (captures.has_global || !supported)
                        .then_some(CpuParallelRejection::NonFreezableCapture)
                });
                (
                    *callable,
                    CpuParallelEligibilityFact {
                        owner: *callable,
                        eligible: rejection.is_none(),
                        rejection,
                        freeze_contract,
                        origin,
                        estimated_work: work.get(callable).copied().flatten(),
                    },
                )
            })
            .collect();
        loop {
            let rejected = dependencies
                .iter()
                .filter_map(|(item, dependencies)| {
                    (callables[item].eligible
                        && dependencies.iter().any(|dependency| {
                            callables.get(dependency).is_some_and(|fact| !fact.eligible)
                        }))
                    .then_some(*item)
                })
                .collect::<Vec<_>>();
            if rejected.is_empty() {
                break;
            }
            for item in rejected {
                let fact = callables.get_mut(&item).expect("known callable");
                fact.eligible = false;
                fact.rejection = Some(CpuParallelRejection::NonFreezableCapture);
            }
        }
        let closures = match closures::analyze(
            hir,
            types,
            &effects.facts,
            &origins,
            &callables,
            &work,
            &context.std_registry,
        ) {
            Ok(closures) => closures,
            Err(reason) => return PassResult::failed(reason),
        };
        context.cpu_parallel = Some(Arc::new(CpuParallelEligibilityFacts {
            callables,
            closures,
        }));
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(CPU_PARALLEL_ELIGIBILITY),
        )
    }
}

fn rejection(summary: &EffectSummary) -> Option<CpuParallelRejection> {
    if !matches!(summary.support, etas_effects::InterpreterSupport::LocalOnly) {
        return Some(CpuParallelRejection::MutableRuntimeResource);
    }
    if summary.escaping_effects.open.is_some()
        || summary.requested_actions.open.is_some()
        || summary.default_actions.open.is_some()
        || summary.handled_actions.open.is_some()
    {
        return Some(CpuParallelRejection::OpenEffectRow);
    }
    if !summary.requested_actions.effects.is_empty()
        || !summary.default_actions.effects.is_empty()
        || !summary.action_trace.action_set().is_empty()
    {
        return Some(CpuParallelRejection::HostAction);
    }
    if summary.handler_requests != RequestInstantiationSummary::default()
        || !summary.handled_actions.effects.is_empty()
    {
        return Some(CpuParallelRejection::HandlerCapture);
    }
    if !summary.escaping_effects.effects.is_empty() {
        return Some(CpuParallelRejection::MutableRuntimeResource);
    }
    if contains_task_control(&summary.action_trace) {
        return Some(CpuParallelRejection::LiveContinuation);
    }
    if summary.action_trace.has_parameter_calls() {
        return Some(CpuParallelRejection::DynamicCode);
    }
    if !summary.residual_checks.is_empty() {
        return Some(CpuParallelRejection::DynamicCode);
    }
    None
}

struct CallableCaptures<'a> {
    hir: &'a etas_hir::HirProgram,
    registry: &'a etas_std::StdRegistry,
    dependencies: Vec<HirItemId>,
    has_global: bool,
    has_boundary: bool,
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for CallableCaptures<'_> {
    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        match expr.data() {
            HirExpr::Path(path) => self.read(&path.resolution),
            HirExpr::Reflect(reflection) => self.read(&reflection.target.root_path().resolution),
            HirExpr::Record(record) => {
                for field in &record.fields {
                    if let etas_hir::HirFieldInit::Shorthand { resolution, .. } = field {
                        self.read(resolution);
                    }
                }
            }
            _ => {}
        }
    }
}

impl CallableCaptures<'_> {
    fn read(&mut self, resolution: &ResolveResult) {
        let symbol = match resolution {
            ResolveResult::Resolved(symbol) => *symbol,
            ResolveResult::PartiallyResolved(partial) => match partial.resolved_prefix {
                Some(symbol) => symbol,
                None => return,
            },
            _ => return,
        };
        let Some(symbol) = self
            .hir
            .canonical_source_symbol(symbol)
            .and_then(|symbol| self.hir.symbols.get(symbol))
        else {
            return;
        };
        if let etas_hir::SymbolDef::ImportAlias { path, .. } = &symbol.def {
            self.has_boundary |= self
                .registry
                .lookup_qualified(path)
                .and_then(|symbol| symbol.intrinsic.as_ref())
                .is_some_and(|intrinsic| {
                    intrinsic.purity != etas_std::IntrinsicPurity::Pure
                        || intrinsic.runtime_requirement
                            != etas_std::IntrinsicRuntimeRequirement::None
                });
        }
        if etas_hir_analysis::closures::is_runtime_local(symbol.kind, &symbol.def) {
            return;
        }
        let Some(item) = symbol.defining_item else {
            return;
        };
        match self.hir.items.get(item) {
            Some(HirItem::TopLevelLet(_)) => self.has_global = true,
            Some(HirItem::Flow(_)) => self.dependencies.push(item),
            _ => {}
        }
    }
}

fn contains_task_control(trace: &ActionTraceDomain) -> bool {
    let mut pending = vec![trace];
    while let Some(trace) = pending.pop() {
        match trace {
            ActionTraceDomain::TaskStart { .. } | ActionTraceDomain::TaskAwait { .. } => {
                return true;
            }
            ActionTraceDomain::Seq(parts)
            | ActionTraceDomain::Choice(parts)
            | ActionTraceDomain::Parallel(parts) => pending.extend(parts),
            ActionTraceDomain::Repeat(inner) => pending.push(inner),
            ActionTraceDomain::Empty
            | ActionTraceDomain::Event(_)
            | ActionTraceDomain::ParameterCall { .. }
            | ActionTraceDomain::UnknownOrder(_)
            | ActionTraceDomain::Widened { .. } => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_pure_callable_is_eligible() {
        assert_eq!(rejection(&EffectSummary::default()), None);
    }

    #[test]
    fn open_effect_row_is_not_eligible() {
        let mut summary = EffectSummary::default();
        summary.requested_actions.open = Some(etas_effects::EffectVarId(1));
        assert_eq!(
            rejection(&summary),
            Some(CpuParallelRejection::OpenEffectRow)
        );
    }
}
