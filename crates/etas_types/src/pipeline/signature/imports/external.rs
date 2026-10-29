use std::collections::HashMap;

mod enums;
mod specs;
#[cfg(test)]
mod tests;
mod types;

use crate::{
    CallableSignature, ExternalCallableSpecSatisfactionFact, ExternalPackageKey,
    ExternalSignatureInput, ExternalTraceSpecConformanceFact, ExternalTraceSpecConformanceTarget,
    SpecImplFact, SpecKind, SpecMethodFact, SpecMethodIdentity, SpecSignature, SpecSuperBoundFact,
    SymbolTypeFact, Type, TypeConstructorId, TypeId, TypeSpecSatisfactionFact,
    lower::external::{
        lower_external_action_signature, lower_external_callable_generic_params,
        lower_external_effect_row, lower_external_type, lower_external_type_declaration,
        resolve_external_spec,
    },
    pipeline::context::TypePipelineContext,
};

pub fn apply_external_signature_input(
    ctx: &mut TypePipelineContext<'_>,
    input: &ExternalSignatureInput,
) {
    if !types::prepare(ctx, input) {
        return;
    }
    specs::prepare(ctx, input);
    let mut symbols = HashMap::<(ExternalPackageKey, Vec<String>), SymbolTypeFact>::new();
    let mut actions =
        HashMap::<(ExternalPackageKey, Vec<String>), crate::EffectActionSignature>::new();
    let mut trace_specs = HashMap::<(ExternalPackageKey, Vec<String>), ()>::new();
    let mut spec_signatures =
        HashMap::<(ExternalPackageKey, Vec<String>), crate::ExternalSpecSignatureInput>::new();

    for metadata in &input.metadata {
        let Some(span) = package_binding_span(input, metadata.package) else {
            continue;
        };
        for ty in &metadata.behavior_types {
            lower_external_type(ctx, span, ty);
        }
        for item in &metadata.types {
            let ty = lower_external_type_declaration(
                ctx,
                span,
                &item.path,
                &item.generic_params,
                item.ty.as_ref(),
            );
            let Some(parameters) = lower_external_callable_generic_params(
                ctx,
                span,
                metadata.package,
                &item.path,
                &item.generic_params,
            ) else {
                continue;
            };
            let fact = match item.ty.as_ref() {
                Some(crate::ExternalTypeInput::Alias { .. }) => SymbolTypeFact::TypeAlias {
                    target: ty,
                    params: parameters.clone(),
                },
                _ => {
                    let representation = match ctx.interner.store().get(ty) {
                        Some(Type::Nominal(nominal)) => nominal.representation,
                        _ => None,
                    };
                    SymbolTypeFact::NominalType {
                        constructor: TypeConstructorId(ty.0),
                        params: nominal_params(ctx, ty),
                        representation,
                    }
                }
            };
            if !matches!(
                item.ty.as_ref(),
                Some(crate::ExternalTypeInput::Alias { .. })
            ) {
                ctx.signature_facts.type_generics.insert(ty, parameters);
            }
            symbols.insert((metadata.package, item.path.clone()), fact);
        }
        for item in &metadata.values {
            if let Some(ty) = item
                .ty
                .as_ref()
                .map(|ty| lower_external_type(ctx, span, ty))
            {
                symbols.insert(
                    (metadata.package, item.path.clone()),
                    SymbolTypeFact::Value { ty },
                );
            }
        }
        for item in &metadata.enums {
            if let Some(facts) = enums::lower(ctx, span, metadata.package, item) {
                symbols.extend(
                    facts
                        .into_iter()
                        .map(|(path, fact)| ((metadata.package, path), fact)),
                );
            }
        }
        for item in &metadata.flows {
            let Some(generic_params) = lower_external_callable_generic_params(
                ctx,
                span,
                metadata.package,
                &item.path,
                &item.generic_params,
            ) else {
                continue;
            };
            let signature = CallableSignature {
                generic_params,
                params: item
                    .params
                    .iter()
                    .map(|ty| lower_external_type(ctx, span, ty))
                    .collect(),
                output: lower_external_type(ctx, span, &item.output),
                effects: item
                    .effects
                    .as_ref()
                    .map(|row| lower_external_effect_row(ctx, span, row)),
                requested_actions: None,
            };
            symbols.insert(
                (metadata.package, item.path.clone()),
                SymbolTypeFact::Flow { signature },
            );
        }
        for item in &metadata.agents {
            let Some(generic_params) = lower_external_callable_generic_params(
                ctx,
                span,
                metadata.package,
                &item.path,
                &item.generic_params,
            ) else {
                continue;
            };
            let component = ctx
                .interner
                .intern(crate::Type::Nominal(crate::NominalTypeRef {
                    read_only_erased_view: false,
                    name: item.path.join("."),
                    params: Vec::new(),
                    representation: None,
                }));
            ctx.signature_facts
                .type_generics
                .insert(component, Vec::new());
            let default_entry = item.output.as_ref().map(|output| {
                let signature = CallableSignature {
                    generic_params,
                    params: item
                        .input
                        .iter()
                        .map(|ty| lower_external_type(ctx, span, ty))
                        .collect(),
                    output: lower_external_type(ctx, span, output),
                    effects: item
                        .effects
                        .as_ref()
                        .map(|row| lower_external_effect_row(ctx, span, row)),
                    requested_actions: None,
                };
                let callable_type = ctx.interner.intern(crate::Type::Function(crate::FlowType {
                    input: signature.params.clone(),
                    output: signature.output,
                    effects: signature.effects.clone(),
                }));
                std::sync::Arc::new(crate::AgentEntrySignature {
                    method: None,
                    callable_type,
                    callable: signature,
                })
            });
            let definition_type = crate::lower::code::agent_definition_type(
                &mut ctx.interner,
                &ctx.signature_facts.known_std_types.code,
                component,
            );
            let signature = crate::AgentSignature {
                component,
                definition_type,
                default_entry,
            };
            symbols.insert(
                (metadata.package, item.path.clone()),
                SymbolTypeFact::Agent { signature },
            );
        }
        for item in &metadata.tools {
            let Some(generic_params) = lower_external_callable_generic_params(
                ctx,
                span,
                metadata.package,
                &item.path,
                &item.generic_params,
            ) else {
                continue;
            };
            let signature = CallableSignature {
                generic_params,
                params: item
                    .input
                    .iter()
                    .map(|ty| lower_external_type(ctx, span, ty))
                    .collect(),
                output: lower_external_type(ctx, span, &item.output),
                effects: item
                    .effects
                    .as_ref()
                    .map(|row| lower_external_effect_row(ctx, span, row)),
                requested_actions: None,
            };
            symbols.insert(
                (metadata.package, item.path.clone()),
                SymbolTypeFact::Tool { signature },
            );
        }
        for item in &metadata.effects {
            symbols.insert(
                (metadata.package, item.path.clone()),
                SymbolTypeFact::Effect {
                    symbol: etas_hir::SymbolId(0),
                },
            );
        }
        for item in &metadata.trace_specs {
            trace_specs.insert((metadata.package, item.path.clone()), ());
        }
        for item in &metadata.spec_signatures {
            spec_signatures.insert((metadata.package, item.path.clone()), item.clone());
        }
        for action in &metadata.actions {
            if let Some(signature) =
                lower_external_action_signature(ctx, span, metadata.package, action)
            {
                actions.insert((metadata.package, action.path.clone()), signature);
            }
        }
    }

    for binding in &input.symbol_bindings {
        if let Some(fact) = symbols
            .get(&(binding.package, binding.path.clone()))
            .cloned()
        {
            if matches!(&fact, SymbolTypeFact::Agent { .. }) {
                ctx.signature_facts.external_agent_reflections.insert(
                    binding.symbol,
                    crate::facts::reflection::ExternalAgentReflectionFact::Unavailable,
                );
            }
            ctx.signature_facts
                .symbol_types
                .insert(binding.symbol, fact);
        } else if let Some(signature) =
            spec_signatures.get(&(binding.package, binding.path.clone()))
        {
            ctx.signature_facts.symbol_types.insert(
                binding.symbol,
                SymbolTypeFact::Spec {
                    symbol: binding.symbol,
                },
            );
            if let Some(lowered) = lower_external_spec_signature(
                ctx,
                binding.package,
                binding.symbol,
                binding.span,
                signature,
            ) {
                ctx.signature_facts
                    .spec_signatures
                    .insert(binding.symbol, lowered);
            }
        } else if trace_specs.contains_key(&(binding.package, binding.path.clone())) {
            ctx.signature_facts.symbol_types.insert(
                binding.symbol,
                SymbolTypeFact::Spec {
                    symbol: binding.symbol,
                },
            );
            if let Some(signature) = empty_external_trace_spec_signature(
                binding.symbol,
                &binding.path,
                binding.span,
                ctx,
            ) {
                ctx.signature_facts
                    .spec_signatures
                    .insert(binding.symbol, signature);
            }
        }
    }
    ctx.signature_facts
        .external_definition_bindings
        .extend(input.definition_bindings.iter().cloned());
    for (symbol, reflection) in &input.agent_reflections {
        if let crate::facts::reflection::ExternalAgentReflectionFact::Available { header, .. } =
            reflection
            && ctx.signature_facts.external_definition_bindings.get(symbol)
                != Some(&header.identity)
        {
            if let Some(binding) = input
                .symbol_bindings
                .iter()
                .find(|binding| binding.symbol == *symbol)
            {
                incomplete_external_type_facts(
                    ctx,
                    binding.span,
                    "external agent reflection disagrees with its canonical import identity".into(),
                );
            }
            continue;
        }
        if matches!(
            ctx.signature_facts.symbol_types.get(symbol),
            Some(SymbolTypeFact::Agent { .. })
        ) {
            ctx.signature_facts
                .external_agent_reflections
                .insert(*symbol, reflection.clone());
        } else if let Some(binding) = input
            .symbol_bindings
            .iter()
            .find(|binding| binding.symbol == *symbol)
        {
            incomplete_external_type_facts(
                ctx,
                binding.span,
                "external agent reflection has no checked agent signature".into(),
            );
        }
    }
    for binding in &input.action_bindings {
        if let Some(signature) = actions
            .get(&(binding.package, binding.path.clone()))
            .cloned()
        {
            ctx.signature_facts.symbol_types.insert(
                binding.symbol,
                SymbolTypeFact::EffectAction {
                    signature: signature.clone(),
                },
            );
            ctx.signature_facts
                .action_signatures
                .insert(binding.symbol, signature);
        }
    }
    apply_external_spec_facts(ctx, input);
}

fn apply_external_spec_facts(ctx: &mut TypePipelineContext<'_>, input: &ExternalSignatureInput) {
    for metadata in &input.metadata {
        let Some(package_span) = package_binding_span(input, metadata.package) else {
            continue;
        };
        for implementation in &metadata.spec_impls {
            let spec = match resolve_external_spec(
                ctx,
                metadata.package,
                &implementation.spec,
                implementation.args.len(),
            ) {
                Ok(spec) => spec,
                Err(reason) => {
                    incomplete_external_type_facts(
                        ctx,
                        package_span,
                        format!(
                            "external spec implementation references `{}`: {reason}",
                            implementation.spec.join(".")
                        ),
                    );
                    continue;
                }
            };
            let span =
                binding_span(input, metadata.package, &implementation.spec).unwrap_or(package_span);
            let self_type = lower_external_type(ctx, span, &implementation.self_type);
            let args = implementation
                .args
                .iter()
                .map(|arg| lower_external_type(ctx, span, arg))
                .collect();
            let spec_symbol = match spec {
                crate::CheckedSpecRef::Source(symbol) => symbol,
                crate::CheckedSpecRef::Std(path) => {
                    record_std_spec_evidence(ctx, self_type, path, args);
                    continue;
                }
                crate::CheckedSpecRef::External(spec) => {
                    ctx.signature_facts.external_specs.impls.push(
                        crate::facts::specs::ExternalSpecImplFact {
                            self_type,
                            spec,
                            args,
                        },
                    );
                    continue;
                }
            };
            ctx.signature_facts.spec_impls.push(SpecImplFact {
                self_type,
                spec_symbol,
                args,
                methods: Vec::new(),
                impl_group: ctx.signature_facts.spec_impls.len() as u32,
                span,
            });
        }
        for fact in &metadata.type_spec_satisfactions {
            let span = binding_span(input, metadata.package, &fact.spec).unwrap_or(package_span);
            let spec =
                match resolve_external_spec(ctx, metadata.package, &fact.spec, fact.args.len()) {
                    Ok(spec) => spec,
                    Err(reason) => {
                        incomplete_external_type_facts(
                            ctx,
                            package_span,
                            format!(
                                "external type satisfaction references `{}`: {reason}",
                                fact.spec.join(".")
                            ),
                        );
                        continue;
                    }
                };
            let self_type = lower_external_type(ctx, span, &fact.self_type);
            let args = fact
                .args
                .iter()
                .map(|arg| lower_external_type(ctx, span, arg))
                .collect();
            let spec_symbol = match spec {
                crate::CheckedSpecRef::Source(symbol) => symbol,
                crate::CheckedSpecRef::Std(path) => {
                    record_std_spec_evidence(ctx, self_type, path, args);
                    continue;
                }
                crate::CheckedSpecRef::External(spec) => {
                    ctx.signature_facts.external_specs.impls.push(
                        crate::facts::specs::ExternalSpecImplFact {
                            self_type,
                            spec,
                            args,
                        },
                    );
                    continue;
                }
            };
            ctx.signature_facts
                .type_spec_satisfactions
                .push(TypeSpecSatisfactionFact {
                    self_type,
                    spec_symbol,
                    args,
                    impl_group: ctx.signature_facts.type_spec_satisfactions.len() as u32,
                    span,
                });
        }
        for fact in &metadata.callable_spec_satisfactions {
            let span = binding_span(input, metadata.package, &fact.item).unwrap_or(package_span);
            let args = fact
                .args
                .iter()
                .map(|arg| lower_external_type(ctx, span, arg))
                .collect();
            ctx.signature_facts
                .external_callable_spec_satisfactions
                .push(ExternalCallableSpecSatisfactionFact {
                    item: fact.item.clone(),
                    spec: fact.spec.clone(),
                    args,
                    span: binding_span(input, metadata.package, &fact.item).unwrap_or(package_span),
                });
        }
        for fact in &metadata.trace_spec_conformances {
            let span = binding_span(input, metadata.package, &fact.item).unwrap_or(package_span);
            let target = match &fact.target {
                crate::ExternalTraceSpecConformanceTargetInput::Inline => {
                    ExternalTraceSpecConformanceTarget::Inline
                }
                crate::ExternalTraceSpecConformanceTargetInput::Named { spec, args } => {
                    ExternalTraceSpecConformanceTarget::Named {
                        spec: spec.clone(),
                        args: args
                            .iter()
                            .map(|arg| lower_external_type(ctx, span, arg))
                            .collect(),
                    }
                }
            };
            ctx.signature_facts.external_trace_spec_conformances.push(
                ExternalTraceSpecConformanceFact {
                    item: fact.item.clone(),
                    target,
                    span: binding_span(input, metadata.package, &fact.item).unwrap_or(package_span),
                },
            );
        }
    }
}

fn binding_span(
    input: &ExternalSignatureInput,
    package: ExternalPackageKey,
    path: &[String],
) -> Option<etas_core::Span> {
    input
        .symbol_bindings
        .iter()
        .find(|binding| binding.package == package && binding.path == path)
        .map(|binding| binding.span)
        .or_else(|| {
            input
                .action_bindings
                .iter()
                .find(|binding| binding.package == package && binding.path == path)
                .map(|binding| binding.span)
        })
}

fn package_binding_span(
    input: &ExternalSignatureInput,
    package: ExternalPackageKey,
) -> Option<etas_core::Span> {
    if let Some((_, span)) = input
        .package_anchors
        .iter()
        .find(|(key, _)| *key == package)
    {
        return Some(*span);
    }
    input
        .symbol_bindings
        .iter()
        .find(|binding| binding.package == package)
        .map(|binding| binding.span)
        .or_else(|| {
            input
                .action_bindings
                .iter()
                .find(|binding| binding.package == package)
                .map(|binding| binding.span)
        })
}

fn record_std_spec_evidence(
    ctx: &mut TypePipelineContext<'_>,
    self_type: TypeId,
    spec: Vec<String>,
    args: Vec<TypeId>,
) {
    let evidence = crate::CheckedStdSpecImplFact {
        self_type,
        spec,
        args,
    };
    // Metadata may contain both an impl and its materialized satisfaction.
    if !ctx.signature_facts.std_spec_impls.contains(&evidence) {
        ctx.signature_facts.std_spec_impls.push(evidence);
    }
}

fn lower_external_spec_signature(
    ctx: &mut TypePipelineContext<'_>,
    package: ExternalPackageKey,
    symbol: etas_hir::SymbolId,
    span: etas_core::Span,
    signature: &crate::ExternalSpecSignatureInput,
) -> Option<SpecSignature> {
    let Some(name) = signature.path.last().cloned() else {
        incomplete_external_type_facts(
            ctx,
            span,
            "external spec signature has an empty canonical path".to_owned(),
        );
        return None;
    };
    let methods = signature
        .methods
        .iter()
        .map(|method| {
            let signature = method.signature.as_ref().and_then(|signature| {
                let generic_params = lower_external_callable_generic_params(
                    ctx,
                    span,
                    package,
                    &signature.path,
                    &signature.generic_params,
                )?;
                Some(CallableSignature {
                    generic_params,
                    params: signature
                        .params
                        .iter()
                        .map(|ty| lower_external_type(ctx, span, ty))
                        .collect(),
                    output: lower_external_type(ctx, span, &signature.output),
                    effects: signature
                        .effects
                        .as_ref()
                        .map(|row| lower_external_effect_row(ctx, span, row)),
                    requested_actions: None,
                })
            });
            SpecMethodFact {
                identity: SpecMethodIdentity::External {
                    package: package.0,
                    path: method.path.clone(),
                },
                name: method.name.clone(),
                signature,
            }
        })
        .collect();
    let mut super_specs = Vec::with_capacity(signature.super_specs.len());
    for bound in &signature.super_specs {
        let super_spec = match resolve_external_spec(ctx, package, &bound.spec, bound.args.len()) {
            Ok(spec) => spec,
            Err(reason) => {
                incomplete_external_type_facts(ctx, span, reason);
                return None;
            }
        };
        super_specs.push(SpecSuperBoundFact {
            spec_symbol: symbol,
            super_spec,
            args: bound
                .args
                .iter()
                .map(|arg| lower_external_type(ctx, span, arg))
                .collect(),
        });
    }
    Some(SpecSignature {
        symbol,
        name,
        kind: match signature.kind {
            crate::ExternalSpecKindInput::Type => SpecKind::TypeSpec,
            crate::ExternalSpecKindInput::Callable => SpecKind::CallableSpec,
            crate::ExternalSpecKindInput::Trace => SpecKind::TraceSpec,
        },
        params: Vec::new(),
        param_names: signature.param_names.clone(),
        callable: signature.callable.as_ref().and_then(|callable| {
            let generic_params = lower_external_callable_generic_params(
                ctx,
                span,
                package,
                &callable.path,
                &callable.generic_params,
            )?;
            Some(CallableSignature {
                generic_params,
                params: callable
                    .params
                    .iter()
                    .map(|ty| lower_external_type(ctx, span, ty))
                    .collect(),
                output: lower_external_type(ctx, span, &callable.output),
                effects: callable
                    .effects
                    .as_ref()
                    .map(|row| lower_external_effect_row(ctx, span, row)),
                requested_actions: None,
            })
        }),
        methods,
        super_specs,
    })
}

fn empty_external_trace_spec_signature(
    symbol: etas_hir::SymbolId,
    path: &[String],
    span: etas_core::Span,
    ctx: &mut TypePipelineContext<'_>,
) -> Option<SpecSignature> {
    let Some(name) = path.last().cloned() else {
        incomplete_external_type_facts(
            ctx,
            span,
            "external trace spec has an empty canonical path".to_owned(),
        );
        return None;
    };
    Some(SpecSignature {
        symbol,
        name,
        kind: SpecKind::TraceSpec,
        params: Vec::new(),
        param_names: Vec::new(),
        callable: None,
        methods: Vec::new(),
        super_specs: Vec::new(),
    })
}

fn incomplete_external_type_facts(
    ctx: &mut TypePipelineContext<'_>,
    span: etas_core::Span,
    message: String,
) {
    ctx.diagnostics.push(etas_core::Diagnostic::type_check(
        etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
        span,
        message,
    ));
}

fn nominal_params(ctx: &TypePipelineContext<'_>, ty: crate::TypeId) -> Vec<String> {
    match ctx.interner.store().get(ty) {
        Some(crate::Type::Nominal(nominal)) => nominal.params.clone(),
        _ => Vec::new(),
    }
}
