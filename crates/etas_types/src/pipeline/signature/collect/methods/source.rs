use etas_hir::{HirItem, HirTreeView, Visibility};

use crate::{
    CheckedSpecRef, SymbolTypeFact, Type,
    method::{MethodEvidence, MethodOwner, MethodReceiver, MethodSignatureFact, MethodTarget},
    pipeline::{context::TypePipelineContext, signature::state::SignaturePipelineState},
};

pub(super) fn collect(
    ctx: &mut TypePipelineContext<'_>,
    state: &SignaturePipelineState,
) -> Vec<MethodSignatureFact> {
    let mut result = Vec::new();
    let view = ctx
        .hir_view
        .clone()
        .unwrap_or_else(|| HirTreeView::new(ctx.hir));
    let mut methods = state.type_impl_methods.values().collect::<Vec<_>>();
    methods.sort_by_key(|method| method.symbol);
    for method in methods {
        let HirItem::Flow(flow) = &ctx.hir.items[method.body] else {
            unreachable!("impl callable must be a flow")
        };
        let Some(SymbolTypeFact::Flow { signature }) = state.symbol_types.get(&method.symbol)
        else {
            continue;
        };
        let body = view
            .item(method.body)
            .and_then(|item| item.body())
            .expect("method declaration has an indexed body")
            .id();
        let receiver = if signature.params.first() == Some(&method.self_type) {
            MethodReceiver::Value { slot: 0 }
        } else {
            MethodReceiver::Associated
        };
        let constructor = match ctx.interner.store().get(method.self_type) {
            Some(Type::Applied { constructor, .. }) => crate::TypeId(constructor.0),
            _ => method.self_type,
        };
        let owner_params = ctx.type_generics(constructor).cloned().unwrap_or_default();
        let mut fact = MethodSignatureFact {
            name: method.name.clone(),
            owner: MethodOwner::Inherent(constructor),
            receiver,
            receiver_type: method.self_type,
            owner_params,
            signature: signature.clone(),
            parameter_names: flow
                .params
                .iter()
                .map(|param| {
                    Some(
                        ctx.hir
                            .symbols
                            .get(*param)
                            .expect("method parameter symbol")
                            .name
                            .clone(),
                    )
                })
                .collect(),
            target: MethodTarget::Source {
                symbol: method.symbol,
                callable: method.body,
                body,
            },
            spec_member: None,
            evidence: MethodEvidence::Inherent,
            defining_module: ctx.hir.item_module(method.body),
            public: ctx
                .hir
                .symbols
                .get(method.symbol)
                .expect("method declaration symbol")
                .visibility
                == Visibility::Public,
        };
        if method.inherent {
            result.push(fact);
        } else {
            for implementation in &state.spec_impls {
                if implementation
                    .methods
                    .iter()
                    .any(|member| member.symbol == method.symbol)
                {
                    let spec = CheckedSpecRef::Source(implementation.spec_symbol);
                    fact.spec_member = state
                        .spec_signatures
                        .get(&implementation.spec_symbol)
                        .or_else(|| {
                            ctx.signature_facts
                                .spec_signatures
                                .get(&implementation.spec_symbol)
                        })
                        .and_then(|spec| {
                            spec.methods
                                .iter()
                                .find(|required| required.name == method.name)
                        })
                        .map(|required| required.identity.clone());
                    fact.owner = MethodOwner::Spec(spec.clone());
                    fact.evidence = MethodEvidence::Implementation {
                        spec,
                        subject: implementation.self_type,
                        args: implementation.args.clone(),
                    };
                    // Availability follows the public spec surface, not free-flow visibility.
                    fact.public = ctx
                        .signature_facts
                        .external_specs
                        .aliases
                        .contains_key(&implementation.spec_symbol)
                        || ctx
                            .hir
                            .canonical_source_symbol(implementation.spec_symbol)
                            .and_then(|symbol| ctx.hir.symbols.get(symbol))
                            .is_some_and(|symbol| symbol.visibility == Visibility::Public);
                    result.push(fact.clone());
                }
            }
        }
    }
    let mut agents = state.agent_methods.values().collect::<Vec<_>>();
    agents.sort_by_key(|method| method.symbol);
    for method in agents {
        let mut signature = method.callable.clone();
        signature.params.insert(0, method.component);
        let parameters = match &ctx.hir.items[method.body] {
            HirItem::Flow(flow) => &flow.params,
            HirItem::Agent(agent) => &agent.params,
            _ => unreachable!("agent callable body must be a flow or source agent"),
        };
        let parameter_names = std::iter::once(None)
            .chain(parameters.iter().map(|param| {
                Some(
                    ctx.hir
                        .symbols
                        .get(*param)
                        .expect("method parameter symbol")
                        .name
                        .clone(),
                )
            }))
            .collect();
        result.push(MethodSignatureFact {
            name: method.name.clone(),
            owner: MethodOwner::Agent(method.component),
            receiver: MethodReceiver::Component,
            receiver_type: method.component,
            owner_params: Vec::new(),
            signature,
            parameter_names,
            target: MethodTarget::Agent {
                symbol: method.symbol,
                callable: method.body,
            },
            spec_member: None,
            evidence: MethodEvidence::Inherent,
            defining_module: ctx.hir.item_module(method.body),
            public: method.private_to.is_none(),
        });
    }
    let mut specs = state.spec_signatures.values().collect::<Vec<_>>();
    specs.sort_by_key(|spec| spec.symbol);
    for spec in specs {
        if spec.kind != crate::SpecKind::TypeSpec {
            continue;
        }
        for method in &spec.methods {
            let Some(mut signature) = method.signature.clone().or_else(|| {
                method.source_symbol().and_then(|symbol| {
                    match state
                        .symbol_types
                        .get(&symbol)
                        .or_else(|| ctx.symbol_fact(symbol))
                    {
                        Some(SymbolTypeFact::Flow { signature }) => Some(signature.clone()),
                        _ => None,
                    }
                })
            }) else {
                continue;
            };
            let Some(receiver_type) = signature.params.first().copied() else {
                continue;
            };
            let mut owner_params =
                super::super::callables::callable_generic_params(ctx, state, &spec.params);
            if let Some(index) = method.receiver_param {
                owner_params.push(signature.generic_params.remove(index as usize));
            }
            let spec_ref = CheckedSpecRef::Source(spec.symbol);
            let args = owner_params
                .iter()
                .take(spec.params.len())
                .map(|param| param.subject)
                .collect();
            result.push(MethodSignatureFact {
                name: method.name.clone(),
                owner: MethodOwner::Spec(spec_ref.clone()),
                receiver: MethodReceiver::Value { slot: 0 },
                receiver_type,
                owner_params,
                parameter_names: method
                    .source_symbol()
                    .and_then(|symbol| ctx.hir.symbols.get(symbol))
                    .and_then(|symbol| symbol.defining_item)
                    .and_then(|item| match &ctx.hir.items[item] {
                        HirItem::Flow(flow) => Some(
                            flow.params
                                .iter()
                                .map(|param| {
                                    Some(
                                        ctx.hir
                                            .symbols
                                            .get(*param)
                                            .expect("method parameter symbol")
                                            .name
                                            .clone(),
                                    )
                                })
                                .collect(),
                        ),
                        _ => None,
                    })
                    .unwrap_or_else(|| vec![None; signature.params.len()]),
                signature,
                target: MethodTarget::BoundSpec {
                    spec: spec_ref.clone(),
                    member: method.identity.clone(),
                    receiver_parameter: method.receiver_param,
                },
                spec_member: Some(method.identity.clone()),
                evidence: MethodEvidence::Bound {
                    spec: spec_ref,
                    subject: receiver_type,
                    args,
                },
                defining_module: Some(
                    ctx.hir
                        .symbols
                        .get(spec.symbol)
                        .expect("spec declaration symbol")
                        .defining_module,
                ),
                public: ctx
                    .hir
                    .symbols
                    .get(spec.symbol)
                    .expect("spec declaration symbol")
                    .visibility
                    == Visibility::Public,
            });
        }
    }
    result
}
