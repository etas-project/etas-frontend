use std::collections::HashMap;

use crate::{
    CallableSignature, ExternalCallableSpecSatisfactionFact, ExternalPackageKey,
    ExternalSignatureInput, ExternalTraceSpecConformanceFact, ExternalTraceSpecConformanceTarget,
    SpecImplFact, SpecKind, SpecMethodFact, SpecMethodIdentity, SpecSignature, SpecSuperBoundFact,
    SymbolTypeFact, Type, TypeConstructorId, TypeId, TypeSpecSatisfactionFact,
    lower::external::{
        lower_external_action_signature, lower_external_effect_row, lower_external_type,
        lower_external_type_declaration,
    },
    pipeline::context::TypePipelineContext,
};

pub fn apply_external_signature_input(
    ctx: &mut TypePipelineContext<'_>,
    input: &ExternalSignatureInput,
) {
    let mut symbols = HashMap::<(ExternalPackageKey, Vec<String>), SymbolTypeFact>::new();
    let mut actions = HashMap::<Vec<String>, crate::EffectActionSignature>::new();
    let mut trace_specs = HashMap::<(ExternalPackageKey, Vec<String>), ()>::new();
    let mut spec_signatures =
        HashMap::<(ExternalPackageKey, Vec<String>), crate::ExternalSpecSignatureInput>::new();
    let binding_symbols = input
        .symbol_bindings
        .iter()
        .map(|binding| ((binding.package, binding.path.clone()), binding.symbol))
        .collect::<HashMap<_, _>>();

    for metadata in &input.metadata {
        for item in &metadata.types {
            let ty = lower_external_type_declaration(ctx, &item.path, item.ty.as_ref());
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
            if let Some(ty) = item.ty.as_ref().map(|ty| lower_external_type(ctx, ty)) {
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
                .map(|ty| lower_external_type(ctx, ty))
                .unwrap_or_else(|| named_external_type(ctx, &item.path));
            symbols.insert(
                (metadata.package, item.path.clone()),
                SymbolTypeFact::Type {
                    constructor: TypeConstructorId(ty.0),
                },
            );
        }
        for item in &metadata.flows {
            let signature = CallableSignature {
                params: item
                    .params
                    .iter()
                    .map(|ty| lower_external_type(ctx, ty))
                    .collect(),
                output: lower_external_type(ctx, &item.output),
                effects: item
                    .effects
                    .as_ref()
                    .map(|row| lower_external_effect_row(ctx, row)),
                requested_actions: None,
            };
            symbols.insert(
                (metadata.package, item.path.clone()),
                SymbolTypeFact::Flow { signature },
            );
        }
        for item in &metadata.agents {
            let signature = CallableSignature {
                params: item
                    .input
                    .iter()
                    .map(|ty| lower_external_type(ctx, ty))
                    .collect(),
                output: lower_external_type(ctx, &item.output),
                effects: item
                    .effects
                    .as_ref()
                    .map(|row| lower_external_effect_row(ctx, row)),
                requested_actions: None,
            };
            symbols.insert(
                (metadata.package, item.path.clone()),
                SymbolTypeFact::Agent { signature },
            );
        }
        for item in &metadata.tools {
            let signature = CallableSignature {
                params: item
                    .input
                    .iter()
                    .map(|ty| lower_external_type(ctx, ty))
                    .collect(),
                output: lower_external_type(ctx, &item.output),
                effects: item
                    .effects
                    .as_ref()
                    .map(|row| lower_external_effect_row(ctx, row)),
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
            actions.insert(
                action.path.clone(),
                lower_external_action_signature(ctx, action),
            );
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
            let lowered =
                lower_external_spec_signature(ctx, binding.symbol, signature, &binding_symbols);
            ctx.signature_facts
                .spec_signatures
                .insert(binding.symbol, lowered);
        } else if trace_specs.contains_key(&(binding.package, binding.path.clone())) {
            ctx.signature_facts.symbol_types.insert(
                binding.symbol,
                SymbolTypeFact::Spec {
                    symbol: binding.symbol,
                },
            );
            ctx.signature_facts.spec_signatures.insert(
                binding.symbol,
                empty_external_trace_spec_signature(binding.symbol, &binding.path),
            );
        }
    }
    for binding in &input.action_bindings {
        if let Some(signature) = actions.get(&binding.path).cloned() {
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
    apply_external_spec_facts(ctx, input, &binding_symbols);
}

fn apply_external_spec_facts(
    ctx: &mut TypePipelineContext<'_>,
    input: &ExternalSignatureInput,
    binding_symbols: &HashMap<(ExternalPackageKey, Vec<String>), etas_hir::SymbolId>,
) {
    for metadata in &input.metadata {
        for implementation in &metadata.spec_impls {
            let Some(spec_symbol) = binding_symbols
                .get(&(metadata.package, implementation.spec.clone()))
                .copied()
            else {
                continue;
            };
            let span = binding_span(input, metadata.package, &implementation.spec);
            let self_type = lower_external_type(ctx, &implementation.self_type);
            let args = implementation
                .args
                .iter()
                .map(|arg| lower_external_type(ctx, arg))
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
            let Some(spec_symbol) = binding_symbols
                .get(&(metadata.package, fact.spec.clone()))
                .copied()
            else {
                continue;
            };
            let self_type = lower_external_type(ctx, &fact.self_type);
            let args = fact
                .args
                .iter()
                .map(|arg| lower_external_type(ctx, arg))
                .collect();
            let span = binding_span(input, metadata.package, &fact.spec);
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
            let args = fact
                .args
                .iter()
                .map(|arg| lower_external_type(ctx, arg))
                .collect();
            ctx.signature_facts
                .external_callable_spec_satisfactions
                .push(ExternalCallableSpecSatisfactionFact {
                    item: fact.item.clone(),
                    spec: fact.spec.clone(),
                    args,
                    span: binding_span(input, metadata.package, &fact.item),
                });
        }
        for fact in &metadata.trace_spec_conformances {
            let target = match &fact.target {
                crate::ExternalTraceSpecConformanceTargetInput::Inline => {
                    ExternalTraceSpecConformanceTarget::Inline
                }
                crate::ExternalTraceSpecConformanceTargetInput::Named { spec, args } => {
                    ExternalTraceSpecConformanceTarget::Named {
                        spec: spec.clone(),
                        args: args
                            .iter()
                            .map(|arg| lower_external_type(ctx, arg))
                            .collect(),
                    }
                }
            };
            ctx.signature_facts.external_trace_spec_conformances.push(
                ExternalTraceSpecConformanceFact {
                    item: fact.item.clone(),
                    target,
                    span: binding_span(input, metadata.package, &fact.item),
                },
            );
        }
    }
}

fn binding_span(
    input: &ExternalSignatureInput,
    package: ExternalPackageKey,
    path: &[String],
) -> etas_core::Span {
    input
        .symbol_bindings
        .iter()
        .find(|binding| binding.package == package && binding.path == path)
        .map(|binding| binding.span)
        .unwrap_or_else(|| etas_core::Span::empty(etas_core::SourceId(0), etas_core::TextSize(0)))
}

fn lower_external_spec_signature(
    ctx: &mut TypePipelineContext<'_>,
    symbol: etas_hir::SymbolId,
    signature: &crate::ExternalSpecSignatureInput,
    binding_symbols: &HashMap<(ExternalPackageKey, Vec<String>), etas_hir::SymbolId>,
) -> SpecSignature {
    let package = package_for_bound(signature, binding_symbols);
    let methods = signature
        .methods
        .iter()
        .filter_map(|method| {
            let Some(package) = package else {
                ctx.diagnostics.push(etas_core::Diagnostic::type_check(
                    etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    etas_core::Span::empty(etas_core::SourceId(0), etas_core::TextSize(0)),
                    format!(
                        "external spec method `{}` is missing package identity",
                        method.path.join(".")
                    ),
                ));
                return None;
            };
            let signature = method
                .signature
                .as_ref()
                .map(|signature| CallableSignature {
                    params: signature
                        .params
                        .iter()
                        .map(|ty| lower_external_type(ctx, ty))
                        .collect(),
                    output: lower_external_type(ctx, &signature.output),
                    effects: signature
                        .effects
                        .as_ref()
                        .map(|row| lower_external_effect_row(ctx, row)),
                    requested_actions: None,
                });
            Some(SpecMethodFact {
                identity: SpecMethodIdentity::External {
                    package: package.0,
                    path: method.path.clone(),
                },
                name: method.name.clone(),
                signature,
            })
        })
        .collect();
    let super_specs = signature
        .super_specs
        .iter()
        .filter_map(|bound| {
            let super_spec_symbol = package
                .and_then(|package| binding_symbols.get(&(package, bound.spec.clone())).copied())?;
            Some(SpecSuperBoundFact {
                spec_symbol: symbol,
                super_spec_symbol,
                args: bound
                    .args
                    .iter()
                    .map(|arg| lower_external_type(ctx, arg))
                    .collect(),
            })
        })
        .collect();
    SpecSignature {
        symbol,
        name: signature
            .path
            .last()
            .cloned()
            .unwrap_or_else(|| format!("external_spec_{}", symbol.0)),
        kind: match signature.kind {
            crate::ExternalSpecKindInput::Type => SpecKind::TypeSpec,
            crate::ExternalSpecKindInput::Callable => SpecKind::CallableSpec,
            crate::ExternalSpecKindInput::Trace => SpecKind::TraceSpec,
        },
        params: Vec::new(),
        param_names: signature.param_names.clone(),
        callable: signature
            .callable
            .as_ref()
            .map(|callable| CallableSignature {
                params: callable
                    .params
                    .iter()
                    .map(|ty| lower_external_type(ctx, ty))
                    .collect(),
                output: lower_external_type(ctx, &callable.output),
                effects: callable
                    .effects
                    .as_ref()
                    .map(|row| lower_external_effect_row(ctx, row)),
                requested_actions: None,
            }),
        methods,
        super_specs,
    }
}

fn package_for_bound(
    signature: &crate::ExternalSpecSignatureInput,
    binding_symbols: &HashMap<(ExternalPackageKey, Vec<String>), etas_hir::SymbolId>,
) -> Option<ExternalPackageKey> {
    binding_symbols
        .keys()
        .find_map(|(package, path)| (*path == signature.path).then_some(*package))
}

fn empty_external_trace_spec_signature(
    symbol: etas_hir::SymbolId,
    path: &[String],
) -> SpecSignature {
    SpecSignature {
        symbol,
        name: path
            .last()
            .cloned()
            .unwrap_or_else(|| format!("external_spec_{}", symbol.0)),
        kind: SpecKind::TraceSpec,
        params: Vec::new(),
        param_names: Vec::new(),
        callable: None,
        methods: Vec::new(),
        super_specs: Vec::new(),
    }
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

fn named_external_type(ctx: &mut TypePipelineContext<'_>, path: &[String]) -> crate::TypeId {
    ctx.interner.intern(crate::Type::Named(crate::NamedTypeRef {
        name: path.join("."),
    }))
}
