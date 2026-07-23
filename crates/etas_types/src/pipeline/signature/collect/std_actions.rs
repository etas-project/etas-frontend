use etas_hir::{SymbolDef, SyntheticSymbolReason};
use etas_std::{StdDecl, StdSymbolKind};

use crate::{
    SymbolTypeFact,
    lower::std::{lower_std_effect_action_signature, lower_std_symbol},
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
        let signature = lower_std_effect_action_signature(ctx, &registry, decl);
        state
            .qualified_action_signatures
            .insert(format!("{}.{}", decl.owner, decl.name), signature.clone());
        state
            .qualified_action_signatures
            .insert(symbol.qualified_path.join("."), signature);
    }
    for symbol in ctx.hir.symbols.iter() {
        let SymbolDef::Synthetic {
            reason: SyntheticSymbolReason::QualifiedEffectAction,
        } = &symbol.def
        else {
            continue;
        };
        let Some((owner, action)) = symbol.name.split_once('.') else {
            continue;
        };
        let Some(std_symbol) = registry.symbols().find(|candidate| {
            if candidate.kind != StdSymbolKind::EffectAction {
                return false;
            }
            let StdDecl::EffectAction(decl) = &candidate.decl else {
                return false;
            };
            decl.owner == owner && decl.name == action
        }) else {
            continue;
        };
        let fact = lower_std_symbol(ctx, &registry, symbol.id, std_symbol);
        if let SymbolTypeFact::EffectAction { signature } = &fact {
            state.action_signatures.insert(symbol.id, signature.clone());
        }
        state.symbol_types.insert(symbol.id, fact);
    }
}
