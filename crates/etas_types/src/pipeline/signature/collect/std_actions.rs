use etas_hir::{SymbolDef, SyntheticSymbolReason};
use etas_std::{StdDecl, StdSymbolKind};

use crate::{
    SymbolTypeFact, lower::std::lower_std_effect_action_signature,
    pipeline::context::TypePipelineContext,
};

use super::super::state::SignaturePipelineState;

pub fn collect_std_actions(ctx: &mut TypePipelineContext<'_>, state: &mut SignaturePipelineState) {
    let registry = ctx.std_registry.clone();
    for symbol in registry.symbols() {
        if symbol.kind != StdSymbolKind::EffectAction {
            continue;
        }
        let StdDecl::EffectAction(decl) = &symbol.decl else {
            continue;
        };
        let native = lower_std_effect_action_signature(ctx, &registry, decl);
        ctx.signature_facts.std_callable_signatures.insert(
            symbol.id,
            crate::CallableSignature {
                generic_params: native.generic_params.clone(),
                params: native.params.clone(),
                output: native.output,
                effects: None,
                requested_actions: None,
            },
        );
        let signature = ctx
            .hir
            .standard_action_targets
            .iter()
            .find_map(|(source, target)| {
                (*target == symbol.id
                    && matches!(
                        ctx.hir.symbols.get(*source).map(|s| &s.def),
                        Some(SymbolDef::EffectAction { .. })
                    ))
                .then(|| state.action_signatures.get(source).cloned())
                .flatten()
            })
            .unwrap_or(native);
        state
            .qualified_action_signatures
            .insert(format!("{}.{}", decl.owner, decl.name), signature.clone());
        state
            .qualified_action_signatures
            .insert(symbol.qualified_path.join("."), signature);
    }
    for symbol in ctx.hir.symbols.iter() {
        let std_symbol = match &symbol.def {
            SymbolDef::ImportAlias { path, .. } => registry.lookup_qualified(path),
            SymbolDef::Synthetic { reason: SyntheticSymbolReason::QualifiedEffectAction } => {
                symbol.name.split_once('.').and_then(|(owner, action)| registry.symbols().find(|candidate| {
                    matches!(&candidate.decl, StdDecl::EffectAction(decl) if decl.owner == owner && decl.name == action)
                }))
            }
            _ => None,
        };
        let Some(std_symbol) =
            std_symbol.filter(|symbol| matches!(symbol.decl, StdDecl::EffectAction(_)))
        else {
            continue;
        };
        let fact = SymbolTypeFact::EffectAction {
            signature: state.qualified_action_signatures[&std_symbol.qualified_path.join(".")]
                .clone(),
        };
        if let SymbolTypeFact::EffectAction { signature } = &fact {
            state.action_signatures.insert(symbol.id, signature.clone());
        }
        state.symbol_types.insert(symbol.id, fact);
    }
}
