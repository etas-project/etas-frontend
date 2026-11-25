use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_hir::{HirActionSelectorParam, HirEffectActionDecl, HirImplItem, HirItem, SymbolKind};

use crate::{
    EffectActionArgKind, EffectActionSignature, EffectArgRef, PrimitiveType, SymbolTypeFact, Type,
    lower::type_ref::lower_type_ref,
    pipeline::{context::TypePipelineContext, signature::state::SignaturePipelineState},
};

pub fn collect_effect_actions(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut SignaturePipelineState,
) {
    let items = ctx
        .hir
        .items
        .iter()
        .map(|(_, item)| item.clone())
        .collect::<Vec<_>>();
    for item in items {
        match item {
            HirItem::Effect(effect) => {
                state.symbol_types.insert(
                    effect.symbol,
                    SymbolTypeFact::Effect {
                        symbol: effect.symbol,
                    },
                );
                for action in effect.body.actions() {
                    collect_action(ctx, state, &effect.type_params, action);
                }
            }
            HirItem::Impl(impl_decl) => {
                for item in impl_decl.items {
                    if let HirImplItem::Action(action) = item {
                        collect_action(ctx, state, &[], &action);
                    }
                }
            }
            _ => {}
        }
    }
}

fn collect_action(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut SignaturePipelineState,
    owner_params: &[etas_hir::SymbolId],
    action: &HirEffectActionDecl,
) {
    let params = action
        .params
        .iter()
        .map(|symbol| {
            ctx.hir
                .symbols
                .get(*symbol)
                .and_then(|symbol| symbol.declared_type)
                .and_then(|ty| lower_type_ref(ctx, ty))
                .unwrap_or_else(|| ctx.interner.primitive(PrimitiveType::Never))
        })
        .collect::<Vec<_>>();
    let output = lower_type_ref(ctx, action.return_type)
        .unwrap_or_else(|| ctx.interner.primitive(PrimitiveType::Never));
    let returns_never = matches!(
        ctx.interner.store().get(output),
        Some(Type::Primitive(PrimitiveType::Never))
    );
    let (mut effect_args, mut selector_param_names, mut selector_defaults) =
        action_effect_selector_signature(ctx, action);
    let mut generic_params = Vec::new();
    // With no action-local selectors, the effect's type parameters select the
    // request. Existing action-local selector contracts retain their ABI.
    if action.selector_params.is_empty() {
        generic_params = super::callables::callable_generic_params(ctx, state, owner_params);
        for parameter in &generic_params {
            if parameter.kind == crate::CallableGenericParamKind::Type {
                effect_args.push(EffectActionArgKind::Type);
                selector_param_names.push(parameter.name.clone());
                selector_defaults.push(None);
            }
        }
    }
    generic_params.extend(super::callables::callable_generic_params(
        ctx,
        state,
        &action.type_params,
    ));
    let default_handler_effects = action
        .default_handler_effects
        .as_ref()
        .map(|row| crate::lower::effect_row::lower_effect_row(ctx, row));
    let signature = EffectActionSignature {
        parameter_interface: Some(crate::call_interface::source_parameters(
            ctx.hir,
            &action.params,
        )),
        default_handler_effects,
        generic_params,
        params,
        output,
        effect_args,
        selector_param_names,
        selector_defaults,
        returns_never,
    };
    state
        .action_signatures
        .insert(action.symbol, signature.clone());
    state
        .symbol_types
        .insert(action.symbol, SymbolTypeFact::EffectAction { signature });
}

fn action_effect_selector_signature(
    ctx: &mut TypePipelineContext<'_>,
    action: &HirEffectActionDecl,
) -> (
    Vec<EffectActionArgKind>,
    Vec<String>,
    Vec<Option<EffectArgRef>>,
) {
    let mut kinds = Vec::new();
    let mut names = Vec::new();
    let mut defaults = Vec::new();
    for selector in &action.selector_params {
        match selector {
            HirActionSelectorParam::Value { name, kind, .. } => {
                let kind = kind.join(".");
                kinds.push(match kind.as_str() {
                    "MemoryPlace" | "std.memory.MemoryPlace" => EffectActionArgKind::MemoryPlace,
                    "string" | "std.core.string" => EffectActionArgKind::StringPattern,
                    _ => EffectActionArgKind::StaticResourcePath { ty: kind },
                });
                // Value selectors do not substitute an inferred type binder.
                names.push(name.clone());
                defaults.push(None);
            }
            HirActionSelectorParam::Type { symbol } => {
                let Some(symbol_data) = ctx.hir.symbols.get(*symbol) else {
                    continue;
                };
                match symbol_data.kind {
                    SymbolKind::TypeParam => {
                        kinds.push(EffectActionArgKind::Type);
                        names.push(symbol_data.name.clone());
                        defaults.push(None);
                    }
                    SymbolKind::EffectParam => ctx.diagnostics.push(Diagnostic::type_check(
                        TypeDiagnosticCode::InvalidEffectArgument,
                        symbol_data.definition_span,
                        "effect action selector parameters must be type/static selector parameters; effect row parameters are not valid action selectors",
                    )),
                    _ => ctx.diagnostics.push(Diagnostic::type_check(
                        TypeDiagnosticCode::InvalidEffectArgument,
                        symbol_data.definition_span,
                        "effect action selector parameter must be a type parameter",
                    )),
                }
            }
        }
    }
    (kinds, names, defaults)
}
