use std::collections::HashMap;

use crate::{
    CallableSignature, ExternalCallableSpecSatisfactionFact, ExternalPackageKey,
    ExternalSignatureInput, ExternalTraceSpecConformanceFact, ExternalTraceSpecConformanceTarget,
    SpecImplFact, SpecKind, SpecMethodFact, SpecMethodIdentity, SpecSignature, SpecSuperBoundFact,
    SymbolTypeFact, Type, TypeConstructorId, TypeId, TypeSpecSatisfactionFact,
    lower::external::{
        lower_external_action_signature, lower_external_callable_generic_params,
        lower_external_effect_row, lower_external_type, lower_external_type_declaration,
    },
    pipeline::context::TypePipelineContext,
};

use super::validated_external::ValidatedExternalMetadata;

pub fn apply_external_signature_input(
    ctx: &mut TypePipelineContext<'_>,
    input: &ExternalSignatureInput,
) {
    let validated = ValidatedExternalMetadata::validate(ctx, input);
    let mut symbols = HashMap::<(ExternalPackageKey, Vec<String>), SymbolTypeFact>::new();
    let mut actions =
        HashMap::<(ExternalPackageKey, Vec<String>), crate::EffectActionSignature>::new();
    let mut trace_specs = HashMap::<(ExternalPackageKey, Vec<String>), ()>::new();
    let mut spec_signatures =
        HashMap::<(ExternalPackageKey, Vec<String>), crate::ExternalSpecSignatureInput>::new();
    let binding_symbols = input
        .symbol_bindings
        .iter()
        .map(|binding| ((binding.package, binding.path.clone()), binding.symbol))
        .collect::<HashMap<_, _>>();

    for metadata in &input.metadata {
        if !validated.contains(metadata.package) {
            continue;
        }
        let Some(span) = package_binding_span(input, metadata.package) else {
            continue;
        };
        for item in &metadata.types {
            let ty = lower_external_type_declaration(ctx, span, &item.path, item.ty.as_ref());
            let fact = match item.ty.as_ref() {
                Some(crate::ExternalTypeInput::Alias { .. }) => {
                    let mut params = Vec::new();
                    collect_type_vars(ctx, ty, &mut params);
                    SymbolTypeFact::TypeAlias { target: ty, params }
                }
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
            let ty = item
                .ty
                .as_ref()
                .map(|ty| lower_external_type(ctx, span, ty))
                .unwrap_or_else(|| lower_external_type_declaration(ctx, span, &item.path, None));
            symbols.insert(
                (metadata.package, item.path.clone()),
                SymbolTypeFact::Type {
                    constructor: TypeConstructorId(ty.0),
                },
            );
        }
        for item in &metadata.flows {
            let Some(generic_params) = lower_external_callable_generic_params(
                ctx,
                span,
                metadata.package,
                &item.path,
                &item.generic_params,
                &binding_symbols,
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
                &binding_symbols,
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
                &binding_symbols,
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
            if let Some(signature) = lower_external_action_signature(
                ctx,
                span,
                metadata.package,
                action,
                &binding_symbols,
            ) {
                actions.insert((metadata.package, action.path.clone()), signature);
            }
        }
    }

    for binding in &input.symbol_bindings {
        if let Some(fact) = symbols
            .get(&(binding.package, binding.path.clone()))
            .cloned()
        {
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
                &binding_symbols,
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
    apply_external_spec_facts(ctx, input, &binding_symbols, &validated);
}

fn apply_external_spec_facts(
    ctx: &mut TypePipelineContext<'_>,
    input: &ExternalSignatureInput,
    binding_symbols: &HashMap<(ExternalPackageKey, Vec<String>), etas_hir::SymbolId>,
    validated: &ValidatedExternalMetadata,
) {
    for metadata in &input.metadata {
        if !validated.contains(metadata.package) {
            continue;
        }
        let Some(package_span) = package_binding_span(input, metadata.package) else {
            continue;
        };
        for implementation in &metadata.spec_impls {
            let Some(spec_symbol) = binding_symbols
                .get(&(metadata.package, implementation.spec.clone()))
                .copied()
            else {
                incomplete_external_type_facts(
                    ctx,
                    package_span,
                    format!(
                        "external spec implementation references `{}` without an imported spec binding",
                        implementation.spec.join(".")
                    ),
                );
                continue;
            };
            let span =
                binding_span(input, metadata.package, &implementation.spec).unwrap_or(package_span);
            let self_type = lower_external_type(ctx, span, &implementation.self_type);
            let args = implementation
                .args
                .iter()
                .map(|arg| lower_external_type(ctx, span, arg))
                .collect();
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
            let Some(spec_symbol) = binding_symbols
                .get(&(metadata.package, fact.spec.clone()))
                .copied()
            else {
                incomplete_external_type_facts(
                    ctx,
                    package_span,
                    format!(
                        "external type satisfaction references `{}` without an imported spec binding",
                        fact.spec.join(".")
                    ),
                );
                continue;
            };
            let self_type = lower_external_type(ctx, span, &fact.self_type);
            let args = fact
                .args
                .iter()
                .map(|arg| lower_external_type(ctx, span, arg))
                .collect();
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

fn lower_external_spec_signature(
    ctx: &mut TypePipelineContext<'_>,
    package: ExternalPackageKey,
    symbol: etas_hir::SymbolId,
    span: etas_core::Span,
    signature: &crate::ExternalSpecSignatureInput,
    binding_symbols: &HashMap<(ExternalPackageKey, Vec<String>), etas_hir::SymbolId>,
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
                    binding_symbols,
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
        let Some(super_spec_symbol) = binding_symbols.get(&(package, bound.spec.clone())).copied()
        else {
            incomplete_external_type_facts(
                ctx,
                span,
                format!(
                    "external spec `{}` references super spec `{}` without an imported binding",
                    signature.path.join("."),
                    bound.spec.join(".")
                ),
            );
            return None;
        };
        super_specs.push(SpecSuperBoundFact {
            spec_symbol: symbol,
            super_spec_symbol,
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
                binding_symbols,
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

fn collect_type_vars(ctx: &TypePipelineContext<'_>, ty: TypeId, out: &mut Vec<String>) {
    match ctx.interner.store().get(ty) {
        Some(Type::Named(named)) => {
            if !out.contains(&named.name) {
                out.push(named.name.clone());
            }
        }
        Some(Type::Array(inner))
        | Some(Type::List(inner))
        | Some(Type::Set(inner))
        | Some(Type::Slice(inner))
        | Some(Type::Option(inner))
        | Some(Type::Message(inner))
        | Some(Type::Schema(inner))
        | Some(Type::MemorySelection(inner))
        | Some(Type::MemoryRegion(inner))
        | Some(Type::Range { index: inner })
        | Some(Type::Trust { inner, .. }) => collect_type_vars(ctx, *inner, out),
        Some(Type::Map { key, value }) | Some(Type::Store { key, value }) => {
            collect_type_vars(ctx, *key, out);
            collect_type_vars(ctx, *value, out);
        }
        Some(Type::Result { ok, err }) => {
            collect_type_vars(ctx, *ok, out);
            collect_type_vars(ctx, *err, out);
        }
        Some(Type::Tuple(elements)) => {
            for element in elements {
                collect_type_vars(ctx, *element, out);
            }
        }
        Some(Type::Record(record)) => {
            for field in &record.fields {
                collect_type_vars(ctx, field.ty, out);
            }
        }
        Some(Type::Function(flow)) => {
            for input in &flow.input {
                collect_type_vars(ctx, *input, out);
            }
            collect_type_vars(ctx, flow.output, out);
        }
        Some(Type::Handler(handler)) => {
            if let Some(result) = handler.result {
                collect_type_vars(ctx, result, out);
            }
        }
        Some(Type::Applied { args, .. }) => {
            for arg in args {
                collect_type_vars(ctx, *arg, out);
            }
        }
        Some(Type::ResourceHandle(crate::ResourceHandleType::MemoryRegion { schema })) => {
            collect_type_vars(ctx, *schema, out);
        }
        Some(Type::ResourceHandle(crate::ResourceHandleType::ExternalTool { signature })) => {
            collect_type_vars(ctx, *signature, out);
        }
        Some(Type::ResourceHandle(crate::ResourceHandleType::Other { args, .. })) => {
            for arg in args {
                collect_type_vars(ctx, *arg, out);
            }
        }
        _ => {}
    }
}
