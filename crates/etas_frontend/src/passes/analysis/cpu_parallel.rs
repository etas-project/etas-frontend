use std::collections::HashMap;
use std::sync::Arc;

use etas_effects::{ActionTraceDomain, EffectSummary, RequestInstantiationSummary};
use etas_hir::{ExprView, HirExpr, HirItem, HirItemId, HirVisitor, ResolveResult, walk_item};
use etas_types::{ItemSignature, Type, TypeId, TypeStore};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::ProjectContext;
use crate::passes::artifacts::{CPU_PARALLEL_ELIGIBILITY, EFFECT_OUTPUT, HIR_OUTPUT, TYPE_OUTPUT};

mod work;

#[derive(Clone, Debug, Default, PartialEq, Eq, etas_core::RetainedSize)]
pub struct CpuParallelEligibilityFacts {
    pub callables: HashMap<HirItemId, CpuParallelEligibilityFact>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct CpuParallelEligibilityFact {
    pub callable: HirItemId,
    pub eligible: bool,
    pub rejection: Option<CpuParallelRejection>,
    /// Approximate HIR operations, including literal bounded loops and known
    /// callees. Recursive or dynamic work stays unknown and can run inline.
    pub estimated_work: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
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
            .requires(ArtifactSet::from([EFFECT_OUTPUT, HIR_OUTPUT, TYPE_OUTPUT]))
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
        let work = work::estimate(&hir.view());
        let mut callables: HashMap<_, _> = effects
            .facts
            .item_effects
            .iter()
            .map(|(callable, summary)| {
                let mut captures = CallableCaptures {
                    hir: &hir.hir,
                    dependencies: Vec::new(),
                    has_global: false,
                };
                walk_item(&hir.view(), *callable, &mut captures);
                dependencies.insert(*callable, captures.dependencies);
                let rejection = rejection(summary).or_else(|| {
                    let Some(ItemSignature::Flow(signature)) =
                        types.facts.item_signatures.get(callable)
                    else {
                        return Some(CpuParallelRejection::DynamicCode);
                    };
                    (captures.has_global || !freezable_type(&types.store, signature.output))
                        .then_some(CpuParallelRejection::NonFreezableCapture)
                });
                (
                    *callable,
                    CpuParallelEligibilityFact {
                        callable: *callable,
                        eligible: rejection.is_none(),
                        rejection,
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
        context.cpu_parallel = Some(Arc::new(CpuParallelEligibilityFacts { callables }));
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
    None
}

fn freezable_type(store: &TypeStore, ty: TypeId) -> bool {
    match store.get(ty) {
        Some(Type::Primitive(_) | Type::IntegerLiteral { .. }) => true,
        Some(Type::Array(inner) | Type::Option(inner)) => freezable_type(store, *inner),
        Some(Type::Tuple(parts)) => parts.iter().all(|ty| freezable_type(store, *ty)),
        Some(Type::Record(record)) => record
            .fields
            .iter()
            .all(|field| freezable_type(store, field.ty)),
        _ => false,
    }
}

struct CallableCaptures<'a> {
    hir: &'a etas_hir::HirProgram,
    dependencies: Vec<HirItemId>,
    has_global: bool,
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for CallableCaptures<'_> {
    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        let HirExpr::Path(path) = expr.data() else {
            return;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return;
        };
        let Some(item) = self
            .hir
            .canonical_source_symbol(symbol)
            .and_then(|symbol| self.hir.symbols.get(symbol))
            .and_then(|symbol| symbol.defining_item)
        else {
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
