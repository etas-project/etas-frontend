use etas_syntax::ast;

use crate::{
    HirDiagnostics, HirEffectRef, ResolveResult, ResolvedActionRef, ScopeId, ScopeTree,
    SymbolTable, path_text, resolve_path,
};

pub fn resolve_action_ref(
    effect: &ast::EffectRef,
    action: &ast::Name,
    scope: ScopeId,
    scopes: &ScopeTree,
    symbols: &SymbolTable,
    diagnostics: &mut HirDiagnostics,
) -> ResolvedActionRef {
    let effect_path = resolve_path(&effect.path, scope, scopes, symbols, diagnostics, true);
    let effect_ref = HirEffectRef {
        path: effect_path,
        args: Vec::new(),
        span: effect.span,
    };
    let qualified = format!("{}.{}", path_text(&effect.path), action.text);
    let matches = symbols.find_by_name(&qualified);
    let action_symbol = match matches.as_slice() {
        [symbol] => ResolveResult::Resolved(*symbol),
        [] => {
            diagnostics.unresolved_effect_action(&qualified, action.span);
            ResolveResult::Unresolved
        }
        many => {
            diagnostics.ambiguous_name(&qualified, action.span);
            ResolveResult::Ambiguous(many.to_vec())
        }
    };

    ResolvedActionRef {
        effect: effect_ref,
        action: action.text.clone(),
        action_symbol,
        span: effect.span.cover(action.span),
    }
}
