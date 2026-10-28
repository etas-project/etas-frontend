use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_hir::{HirItem, SymbolDef};

use crate::pipeline::{context::TypePipelineContext, signature::state::SignaturePipelineState};
use crate::{AgentEntrySignature, FlowType, ItemSignature, SymbolTypeFact, Type};

pub fn collect_agent_methods(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut SignaturePipelineState,
) {
    let mut methods = ctx.hir.agent_methods.values().cloned().collect::<Vec<_>>();
    methods.sort_by_key(|method| method.symbol);
    for method in methods {
        let Some(agent_symbol) = ctx.symbols.canonical_symbol(ctx.hir, method.agent) else {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::IncompleteTypeFacts,
                ctx.hir.items[method.body].span(),
                "agent method owner identity cannot be resolved",
            ));
            continue;
        };
        let Some(SymbolDef::Item { item: agent }) =
            ctx.hir.symbols.get(agent_symbol).map(|symbol| &symbol.def)
        else {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::InvalidImplTargetKind,
                ctx.hir.items[method.body].span(),
                "source agent method requires a resolved agent declaration",
            ));
            continue;
        };
        let agent = *agent;
        let Some(ItemSignature::Agent(agent_signature)) =
            state.item_signatures.get(&agent).cloned()
        else {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::InvalidImplTargetKind,
                ctx.hir.items[method.body].span(),
                "impl agent target is not an agent declaration",
            ));
            continue;
        };
        let callable = match state.item_signatures.get(&method.body) {
            Some(ItemSignature::Flow(signature)) => signature.clone(),
            Some(ItemSignature::Agent(signature)) => {
                let Some(signature) = signature.callable() else {
                    ctx.diagnostics.push(Diagnostic::type_check(
                        TypeDiagnosticCode::IncompleteTypeFacts,
                        ctx.hir.items[method.body].span(),
                        "generated agent method has no checked callable signature",
                    ));
                    continue;
                };
                signature.clone()
            }
            _ => {
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::IncompleteTypeFacts,
                    ctx.hir.items[method.body].span(),
                    "agent method is missing its callable signature",
                ));
                continue;
            }
        };
        let callable_type = ctx.interner.intern(Type::Function(FlowType {
            input: callable.params.clone(),
            output: callable.output,
            effects: callable.effects.clone(),
        }));
        let Some(symbol) = ctx.hir.symbols.get(method.symbol) else {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::IncompleteTypeFacts,
                ctx.hir.items[method.body].span(),
                "agent method is missing its declaration symbol",
            ));
            continue;
        };
        let name = symbol.name.clone();
        state.agent_methods.insert(
            method.symbol,
            crate::facts::signatures::AgentMethodSignature {
                private_to: (symbol.visibility == etas_hir::Visibility::Private)
                    .then_some(symbol.defining_module),
                inherent: ctx.hir.agent_method_is_inherent(&method),
                agent,
                component: agent_signature.component,
                symbol: method.symbol,
                body: method.body,
                implementation: method.implementation,
                name,
                callable_type,
                callable: callable.clone(),
            },
        );
        state.symbol_types.insert(
            method.symbol,
            SymbolTypeFact::Flow {
                signature: callable,
            },
        );
    }
    let agents = state
        .item_signatures
        .iter()
        .filter_map(|(item, signature)| match signature {
            ItemSignature::Agent(signature) => Some((*item, signature.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    for (agent, mut signature) in agents {
        let inherent = state
            .agent_methods
            .values()
            .filter(|method| method.agent == agent && method.inherent)
            .collect::<Vec<_>>();
        let mut names = std::collections::HashSet::new();
        for method in &inherent {
            if !names.insert(&method.name) {
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::InvalidImplMemberKind,
                    ctx.hir.items[method.body].span(),
                    format!("duplicate inherent agent method `{}`", method.name),
                ));
            }
        }
        let HirItem::Agent(decl) = &ctx.hir.items[agent] else {
            continue;
        };
        signature.default_entry = ctx
            .hir
            .agent_method(decl.symbol, "run")
            .and_then(|method| state.agent_methods.get(&method.symbol))
            .map(|method| {
                std::sync::Arc::new(AgentEntrySignature {
                    method: Some(method.symbol),
                    callable_type: method.callable_type,
                    callable: method.callable.clone(),
                })
            });
        state.symbol_types.insert(
            decl.symbol,
            SymbolTypeFact::Agent {
                signature: signature.clone(),
            },
        );
        state
            .item_signatures
            .insert(agent, ItemSignature::Agent(signature));
    }
}

pub fn collect_agent_spec_satisfactions(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut SignaturePipelineState,
) {
    let implementations = ctx
        .hir
        .items
        .iter()
        .filter_map(|(item, decl)| match decl {
            HirItem::Impl(decl) if matches!(decl.target, etas_hir::HirImplTarget::Agent { .. }) => {
                Some((item, decl.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    for (implementation, decl) in implementations {
        let etas_hir::HirImplTarget::Agent { target, specs } = decl.target else {
            continue;
        };
        let component = match target.resolution {
            etas_hir::ResolveResult::Resolved(symbol) => ctx
                .symbols
                .canonical_symbol(ctx.hir, symbol)
                .and_then(|symbol| ctx.hir.symbols.get(symbol))
                .and_then(|symbol| match symbol.def {
                    SymbolDef::Item { item } => state.item_signatures.get(&item),
                    _ => None,
                })
                .and_then(|signature| match signature {
                    ItemSignature::Agent(signature) => Some(signature.component),
                    _ => None,
                }),
            _ => None,
        };
        let methods = state
            .agent_methods
            .values()
            .filter(|method| method.implementation == Some(implementation))
            .cloned()
            .collect::<Vec<_>>();
        for spec in specs {
            let etas_hir::ResolveResult::Resolved(symbol) = spec.spec_path.resolution else {
                continue;
            };
            let Some(symbol) = ctx.symbols.canonical_symbol(ctx.hir, symbol) else {
                continue;
            };
            let Some(signature) = state
                .spec_signatures
                .get(&symbol)
                .or_else(|| ctx.signature_facts.spec_signatures.get(&symbol))
            else {
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::InvalidImplTargetKind,
                    spec.span,
                    "impl agent conformance must resolve to a spec",
                ));
                continue;
            };
            let kind = signature.kind;
            let args = spec
                .spec_args
                .iter()
                .filter_map(|ty| crate::lower::type_ref::lower_type_ref(ctx, *ty))
                .collect::<Vec<_>>();
            if kind == crate::SpecKind::TypeSpec {
                let Some(self_type) = component else {
                    ctx.diagnostics.push(Diagnostic::type_check(
                        TypeDiagnosticCode::InvalidImplMemberKind,
                        spec.span,
                        "agent type spec implementation requires its checked nominal component",
                    ));
                    continue;
                };
                state.spec_impls.push(crate::SpecImplFact {
                    self_type,
                    spec_symbol: symbol,
                    args: args.clone(),
                    methods: methods
                        .iter()
                        .map(|method| crate::SpecImplMethodFact {
                            symbol: method.symbol,
                            name: method.name.clone(),
                        })
                        .collect(),
                    impl_group: implementation.0,
                    span: spec.span,
                });
                state
                    .type_spec_satisfactions
                    .push(crate::TypeSpecSatisfactionFact {
                        self_type,
                        spec_symbol: symbol,
                        args,
                        impl_group: implementation.0,
                        span: spec.span,
                    });
                continue;
            }
            let entries = methods
                .iter()
                .filter(|method| method.name == "run")
                .collect::<Vec<_>>();
            let [entry] = entries.as_slice() else {
                ctx.diagnostics.push(Diagnostic::type_check(TypeDiagnosticCode::InvalidImplMemberKind, spec.span,
                    "callable or trace spec implementation requires exactly one run method in this implementation"));
                continue;
            };
            match kind {
                crate::SpecKind::CallableSpec => {
                    state
                        .callable_spec_satisfactions
                        .push(crate::CallableSpecSatisfactionFact {
                            item: entry.body,
                            symbol: entry.symbol,
                            spec_symbol: symbol,
                            args,
                            effect_bound: None,
                            span: spec.span,
                        })
                }
                crate::SpecKind::TraceSpec => {
                    state
                        .trace_spec_conformances
                        .push(crate::TraceSpecConformanceFact {
                            item: entry.body,
                            symbol: entry.symbol,
                            target: crate::TraceSpecConformanceTarget::Named {
                                spec_symbol: symbol,
                                args,
                            },
                            span: spec.span,
                        })
                }
                crate::SpecKind::TypeSpec => {}
            }
        }
    }
}
