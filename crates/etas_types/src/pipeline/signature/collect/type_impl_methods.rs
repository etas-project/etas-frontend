use etas_hir::{HirImplItem, HirImplTarget, HirItem};

use crate::{
    facts::TypeImplMethodFact,
    lower::type_ref::{lower_path_type, lower_type_ref},
    pipeline::{context::TypePipelineContext, signature::state::SignaturePipelineState},
};

pub fn collect_type_impl_methods(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut SignaturePipelineState,
) {
    let implementations = ctx
        .hir
        .items
        .iter()
        .filter_map(|(item, declaration)| match declaration {
            HirItem::Impl(decl)
                if !matches!(
                    decl.target,
                    HirImplTarget::Agent { .. } | HirImplTarget::Error
                ) =>
            {
                Some((item, decl.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    for (implementation, declaration) in implementations {
        let methods = declaration
            .items
            .iter()
            .filter_map(|method| {
                let HirImplItem::Callable { item } = method else {
                    return None;
                };
                let Some(HirItem::Flow(flow)) = ctx.hir.items.get(*item) else {
                    return None;
                };
                let name = ctx
                    .hir
                    .symbols
                    .get(flow.symbol)?
                    .name
                    .rsplit('.')
                    .next()?
                    .to_owned();
                Some((*item, flow.symbol, name))
            })
            .collect::<Vec<_>>();
        let (self_type, inherent) = match &declaration.target {
            HirImplTarget::SpecSatisfaction { self_type, .. } => {
                let Some(ty) = lower_type_ref(ctx, *self_type) else {
                    continue;
                };
                (ty, false)
            }
            HirImplTarget::Inherent { target, type_args } => {
                (lower_path_type(ctx, target, type_args, target.span), true)
            }
            HirImplTarget::Agent { .. } | HirImplTarget::Error => continue,
        };
        let mut owner_params =
            super::callables::callable_generic_params(ctx, state, &declaration.type_params);
        inherit_owner_bounds(ctx, state, self_type, &mut owner_params, declaration.span);
        for (body, symbol, name) in methods {
            state.type_impl_methods.insert(
                symbol,
                TypeImplMethodFact {
                    symbol,
                    name,
                    implementation,
                    body,
                    self_type,
                    owner_params: owner_params.clone(),
                    inherent,
                },
            );
        }
    }
}

pub(super) fn inherit_owner_bounds(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut SignaturePipelineState,
    subject: crate::TypeId,
    parameters: &mut [crate::CallableGenericParam],
    span: etas_core::Span,
) {
    let Some(crate::Type::Applied { constructor, args }) = ctx.interner.store().get(subject) else {
        return;
    };
    let arguments = args.clone();
    let Some(declared) = ctx
        .signature_facts
        .type_generics
        .get(&crate::TypeId(constructor.0))
        .cloned()
    else {
        return;
    };
    let bindings = declared
        .iter()
        .zip(&arguments)
        .map(|(parameter, argument)| (parameter.subject, *argument))
        .collect();
    let symbols = parameters
        .iter()
        .filter_map(
            |parameter| match ctx.interner.store().get(parameter.subject) {
                Some(crate::Type::Named(crate::NamedTypeRef {
                    source_parameter: Some(symbol),
                    ..
                })) => Some((parameter.subject, *symbol)),
                _ => None,
            },
        )
        .collect::<std::collections::HashMap<_, _>>();
    let names = std::collections::HashMap::new();
    let mut substitution =
        crate::TypeSubstitutionEngine::materializing(&mut ctx.interner, &names, None)
            .with_type_parameters(&bindings);
    for (declared, argument) in declared.iter().zip(arguments) {
        let Some(parameter) = parameters
            .iter_mut()
            .find(|parameter| parameter.subject == argument)
        else {
            continue;
        };
        let Some(symbol) = symbols.get(&argument).copied() else {
            continue;
        };
        for bound in &declared.bounds {
            let args = match bound
                .args
                .iter()
                .map(|arg| substitution.substitute(*arg))
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(args) => args,
                Err(error) => {
                    ctx.diagnostics.push(etas_core::Diagnostic::type_check(
                        etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                        span,
                        error.to_string(),
                    ));
                    continue;
                }
            };
            let spec_symbol = match &bound.spec {
                crate::CheckedSpecRef::Source(symbol) => Some(*symbol),
                crate::CheckedSpecRef::External(identity) => ctx
                    .signature_facts
                    .external_specs
                    .aliases
                    .iter()
                    .find_map(|(symbol, candidate)| (candidate == identity).then_some(*symbol)),
                crate::CheckedSpecRef::Std(path) => ctx
                    .signature_facts
                    .std_spec_aliases
                    .iter()
                    .find_map(|(symbol, candidate)| (candidate == path).then_some(*symbol)),
                crate::CheckedSpecRef::Canonical(_) => None,
            };
            let Some(spec_symbol) = spec_symbol else {
                ctx.diagnostics.push(etas_core::Diagnostic::type_check(
                    etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    span,
                    "impl owner bound has no checked spec binding",
                ));
                continue;
            };
            let inherited = crate::CheckedSpecBound {
                spec: bound.spec.clone(),
                args: args.clone(),
            };
            if parameter.bounds.contains(&inherited) {
                continue;
            }
            parameter.bounds.push(inherited);
            let bounds = state.type_param_bounds.entry(symbol).or_default();
            if !bounds
                .iter()
                .any(|bound| bound.spec_symbol == spec_symbol && bound.args == args)
            {
                bounds.push(crate::TypeParamBoundFact {
                    param: symbol,
                    param_name: parameter.name.clone(),
                    spec_symbol,
                    args,
                });
            }
        }
    }
}
