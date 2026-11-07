use super::*;
use crate::HirOutput;
use etas_effects::EffectUnit;
use etas_hir::{HirLambdaBody, walk_block, walk_expr};
use etas_types::{SymbolTypeFact, Type, TypeOutput};

pub(super) fn analyze(
    hir: &HirOutput,
    types: &TypeOutput,
    effects: &etas_effects::EffectFacts,
    origins: &origin::CallableOrigins,
    callables: &HashMap<HirItemId, CpuParallelEligibilityFact>,
    item_work: &HashMap<HirItemId, Option<u64>>,
    registry: &etas_std::StdRegistry,
) -> Result<HashMap<HirExprId, CpuParallelEligibilityFact>, String> {
    let view = hir.view();
    let layouts = etas_hir_analysis::closures::analyze_closures(&view)?;
    let mut closures = HashMap::new();
    for unit in effects.unit_effects.keys() {
        let EffectUnit::AnonymousFlow {
            owner, value: expr, ..
        } = *unit
        else {
            continue;
        };
        let Some(HirExpr::Lambda { body, .. }) = hir.hir.exprs.get(expr) else {
            continue;
        };
        let summary = &effects
            .latent_effects
            .get(&expr)
            .ok_or("CPU closure has no checked latent effects")?
            .summary;
        let layout = layouts
            .get(&expr)
            .ok_or("anonymous CPU callable has no capture layout")?;
        let origin = origins.closure_owner(&hir.hir, owner, &types.facts);
        let captures = layout
            .captures
            .iter()
            .map(|symbol| {
                let ty = match types.facts.symbol_types.get(symbol)? {
                    SymbolTypeFact::Param { ty }
                    | SymbolTypeFact::Local { ty, .. }
                    | SymbolTypeFact::Value { ty }
                    | SymbolTypeFact::Field { ty }
                    | SymbolTypeFact::TopLevelLet { ty, .. } => *ty,
                    _ => return None,
                };
                Some(CpuCaptureContract {
                    symbol: *symbol,
                    ty,
                })
            })
            .collect::<Option<Vec<_>>>();
        let freeze_contract = match (
            types
                .facts
                .expr_types
                .get(&expr)
                .and_then(|ty| types.store.get(*ty)),
            captures,
        ) {
            (Some(Type::Function(signature)), Some(captures)) => Some(CpuFreezeContract {
                parameters: signature.input.clone(),
                output: signature.output,
                captures,
            }),
            _ => None,
        };
        let mut generics = match types.facts.item_signatures.get(&owner) {
            Some(ItemSignature::Flow(signature) | ItemSignature::Tool(signature)) => {
                signature.generic_params.clone()
            }
            Some(ItemSignature::Agent(agent)) => agent
                .callable()
                .map(|signature| signature.generic_params.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        if let Some(method) = hir.hir.items[owner]
            .symbol()
            .and_then(|symbol| types.facts.type_impl_methods.get(&symbol))
        {
            generics.extend(method.owner_params.iter().cloned());
        }
        let mut reads = CallableCaptures {
            hir: &hir.hir,
            registry,
            dependencies: Vec::new(),
            has_global: false,
            has_boundary: false,
        };
        match body {
            HirLambdaBody::Expr(body) => walk_expr(&view, *body, &mut reads),
            HirLambdaBody::Block(body) => walk_block(&view, *body, &mut reads),
        }
        let rejection = rejection(summary).or_else(|| {
            if reads.has_boundary {
                return Some(CpuParallelRejection::MutableRuntimeResource);
            }
            if origin.is_none() {
                return Some(CpuParallelRejection::DynamicCode);
            }
            (reads.has_global
                || freeze_contract
                    .as_ref()
                    .is_none_or(|contract| !contract.supports_template(&types.store, &generics))
                || reads
                    .dependencies
                    .iter()
                    .any(|item| callables.get(item).is_none_or(|fact| !fact.eligible)))
            .then_some(CpuParallelRejection::NonFreezableCapture)
        });
        closures.insert(
            expr,
            CpuParallelEligibilityFact {
                owner,
                eligible: rejection.is_none(),
                rejection,
                freeze_contract,
                origin,
                estimated_work: work::estimate_closure(&view, body.clone(), item_work),
            },
        );
    }
    Ok(closures)
}
