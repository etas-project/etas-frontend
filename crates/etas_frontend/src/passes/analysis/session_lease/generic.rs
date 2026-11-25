//! Infer which type parameters must support copying, then carry that requirement
//! across calls. The existing move analysis handles branches, loops and places.
use super::*;
use crate::project::instantiation::{
    direct_targets, external_signature, owner_parameters, signature,
};
use etas_hir::{HirItem, HirItemId};
use etas_types::{
    CallableGenericParam, CallableGenericParamKind, GenericInstantiationFact, ItemSignature,
};
use std::sync::Arc;

type Binder = (HirItemId, TypeId);

#[derive(Clone, Copy)]
enum Contract {
    Local(Binder),
    Imported(Option<bool>),
}

impl Contract {
    fn requirement(self, inferred: &HashMap<Binder, Option<bool>>) -> Option<bool> {
        match self {
            Self::Local(binder) => inferred.get(&binder).copied().flatten(),
            Self::Imported(known) => known,
        }
    }
}

pub(super) struct Application {
    site: HirExprId,
    actual: TypeId,
    contract: Contract,
}

impl Application {
    pub(super) fn local_binding(&self) -> Option<(TypeId, TypeId)> {
        match self.contract {
            Contract::Local((_, subject)) => Some((subject, self.actual)),
            Contract::Imported(_) => None,
        }
    }
}

pub(super) fn infer_copy_requirements(
    hir: &HirRevision,
    types: &mut TypeOutput,
    layouts: &CaptureLayouts,
    applications: &[Application],
) -> Vec<Diagnostic> {
    let mut inferred = HashMap::new();
    let mut dependencies = Vec::new();
    let mut by_owner = HashMap::<_, Vec<_>>::new();
    for application in applications {
        if let Some(owner) = hir.index().expr_item.get(&application.site) {
            by_owner.entry(*owner).or_default().push(application);
        }
    }
    let mut symbols: HashMap<_, HashSet<_>> = hir
        .items
        .iter()
        .filter_map(|(item, _)| {
            let decl = signature(&types.facts, item)?;
            decl.generic_params
                .iter()
                .chain(owner_parameters(hir, &types.facts, item))
                .any(|p| p.kind == CallableGenericParamKind::Type)
                .then(|| (item, HashSet::new()))
        })
        .collect();
    if symbols.is_empty() && applications.is_empty() {
        return Vec::new();
    }
    for (expr, value) in hir.exprs.iter() {
        let Some(owner) = hir.index().expr_item.get(&expr) else {
            continue;
        };
        let Some(bindings) = symbols.get_mut(owner) else {
            continue;
        };
        if let HirExpr::Path(_) = value
            && let Some(Place {
                root: AliasTarget::Symbol(symbol),
                ..
            }) = place_of(hir, expr)
        {
            bindings.insert(symbol);
        }
        if let HirExpr::Record(record) = value {
            for field in &record.fields {
                if let HirFieldInit::Shorthand {
                    resolution: ResolveResult::Resolved(symbol),
                    ..
                } = field
                {
                    bindings.insert(*symbol);
                }
            }
        }
    }
    let context = HirAnalysisContext::new(hir);
    for (item, node) in hir.items.iter() {
        let Some(decl) = signature(&types.facts, item) else {
            continue;
        };
        let has_body = match node {
            HirItem::Flow(flow) => flow.body.as_block().is_some(),
            HirItem::Tool(tool) => {
                matches!(tool.body, etas_hir::HirToolBody::Source(body) if body.as_block().is_some())
            }
            HirItem::Agent(agent) => matches!(agent.body, etas_hir::HirAgentBody::Source { .. }),
            _ => false,
        };
        for param in decl
            .generic_params
            .iter()
            .chain(owner_parameters(hir, &types.facts, item))
            .filter(|p| p.kind == CallableGenericParamKind::Type)
        {
            let binder = (item, param.subject);
            if !has_body {
                inferred.insert(binder, None);
                continue;
            }
            let mut analysis = LeaseAnalysis::new(context.clone(), types, param.subject, layouts);
            let tracked = symbols
                .get(&item)
                .into_iter()
                .flatten()
                .copied()
                .filter(|symbol| {
                    types
                        .facts
                        .symbol_types
                        .get(symbol)
                        .and_then(value_type)
                        .is_some_and(|ty| analysis.contains_lease(ty))
                })
                .collect();
            analysis.captures.track_bindings(tracked);
            for application in by_owner.get(&item).into_iter().flatten() {
                if analysis.contains_lease(application.actual) {
                    dependencies.push((binder, application.contract));
                }
            }
            let mut interpreter = HirAbstractInterpreter::new(analysis);
            analyze_owner(hir, item, &mut interpreter);
            inferred.insert(
                binder,
                Some(!interpreter.into_semantics().moved_uses.is_empty()),
            );
        }
    }

    // Each binder changes at most twice: single-use, then unknown, then copy.
    loop {
        let mut changed = false;
        for (caller, callee) in &dependencies {
            let needed = callee.requirement(&inferred);
            let current = inferred.get_mut(caller).unwrap();
            if *current == Some(false) && needed != Some(false)
                || current.is_none() && needed == Some(true)
            {
                *current = needed;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut diagnostics = Vec::new();
    if let Some(lease) = types
        .facts
        .known_std_types
        .opaque_values
        .iter()
        .copied()
        .find(|ty| matches!(types.store.get(*ty), Some(Type::Nominal(n)) if n.name == LEASE))
    {
        let mut analysis = LeaseAnalysis::new(context, types, lease, layouts);
        for application in applications {
            if !analysis.contains_lease(application.actual) {
                continue;
            }
            let message = match application.contract.requirement(&inferred) {
                Some(false) => continue,
                Some(true) => {
                    "SessionLease cannot be passed to a generic parameter whose implementation copies its value"
                }
                None => "this declaration has no checked ownership contract for SessionLease",
            };
            diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::TypeMismatch,
                hir.exprs[application.site].span(&hir.blocks),
                message,
            ));
        }
    }
    if !inferred.is_empty() {
        let facts = Arc::make_mut(&mut types.facts);
        for (item, signature) in &mut facts.item_signatures {
            let signature = match signature {
                ItemSignature::Flow(signature) | ItemSignature::Tool(signature) => signature,
                ItemSignature::Agent(agent) => {
                    let Some(entry) = &mut agent.default_entry else {
                        continue;
                    };
                    &mut Arc::make_mut(entry).callable
                }
                ItemSignature::TopLevelLet(_) => continue,
            };
            record(*item, &mut signature.generic_params, &inferred);
        }
        for method in facts.type_impl_methods.values_mut() {
            record(method.body, &mut method.owner_params, &inferred);
        }
    }
    diagnostics
}

fn record(
    item: HirItemId,
    params: &mut [CallableGenericParam],
    inferred: &HashMap<Binder, Option<bool>>,
) {
    for param in params {
        param.requires_copy = inferred.get(&(item, param.subject)).copied().flatten();
    }
}

pub(super) fn applications(
    hir: &HirRevision,
    types: &TypeOutput,
) -> Result<Vec<Application>, String> {
    let facts = &types.facts;
    let targets = direct_targets(hir, types)?;
    let sites: BTreeSet<_> = facts
        .generic_instantiations
        .keys()
        .chain(facts.checked_method_calls.keys())
        .chain(facts.bound_method_values.keys())
        .copied()
        .collect();
    let mut uses = Vec::new();
    for site in sites {
        let method = facts.selected_method(site);
        let Some(bindings) = method
            .map(|m| &m.instantiation)
            .or_else(|| facts.generic_instantiations.get(&site))
        else {
            continue;
        };
        if bindings.type_bindings.is_empty()
            && method.is_none_or(|method| method.owner_bindings.is_empty())
        {
            continue;
        }
        let mut add = |params: &[CallableGenericParam],
                       bindings: &GenericInstantiationFact,
                       item: Option<HirItemId>,
                       owner: bool| {
            for param in params
                .iter()
                .filter(|p| p.kind == CallableGenericParamKind::Type)
            {
                let actual = if owner {
                    method.and_then(|m| {
                        m.owner_bindings
                            .iter()
                            .find(|(binder, _)| *binder == param.subject)
                            .map(|(_, ty)| *ty)
                    })
                } else {
                    bindings
                        .type_bindings
                        .iter()
                        .find(|(name, _)| name == &param.name)
                        .map(|(_, ty)| *ty)
                };
                if let Some(actual) = actual {
                    uses.push(Application {
                        site,
                        actual,
                        contract: item.map_or(Contract::Imported(param.requires_copy), |target| {
                            Contract::Local((target, param.subject))
                        }),
                    });
                }
            }
        };
        if let Some(items) = targets.get(&site) {
            for item in items {
                if let Some(decl) = signature(facts, *item) {
                    add(&decl.generic_params, bindings, Some(*item), false);
                    add(
                        owner_parameters(hir, facts, *item),
                        bindings,
                        Some(*item),
                        true,
                    );
                }
            }
        } else if let Some(method) = method {
            if let etas_types::method::MethodTarget::External { identity } = &method.target
                && let Some(decl) = facts
                    .external_methods
                    .iter()
                    .find(|decl| &decl.identity == identity)
            {
                add(&decl.signature.generic_params, bindings, None, false);
                add(&decl.owner_params, bindings, None, true);
            } else if matches!(
                method.target,
                etas_types::method::MethodTarget::BoundSpec { .. }
            ) {
                add(&method.signature.generic_params, bindings, None, false);
            }
        } else if let Some(decl) = external_signature(hir, types, site) {
            // A built-in enum constructor moves its fields directly into the
            // variant. It has no generic callable body to instantiate.
            if let HirExpr::Call { callee, .. } = &hir.exprs[site]
                && let Some(Place {
                    root: AliasTarget::Symbol(s),
                    ..
                }) = place_of(hir, *callee)
                && hir
                    .symbols
                    .get(s)
                    .is_some_and(|s| s.kind == etas_hir::SymbolKind::EnumVariant)
            {
                continue;
            }
            add(&decl.generic_params, bindings, None, false);
        }
    }
    Ok(uses)
}
