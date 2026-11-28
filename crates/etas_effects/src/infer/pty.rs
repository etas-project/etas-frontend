//! Checked origins of managed terminal handles. The profile is a selector,
//! while the terminal itself remains an ordinary action argument.
use etas_hir::{HirArg, HirExpr, HirExprId, HirProgram, ResolveResult, SymbolDef};
use etas_std::StdRegistry;
use etas_types::{Type, TypeOutput};

pub(super) use etas_types::sandbox_profile::{
    TYPE_NAME as PROFILE, declaration_path as static_profile_path,
};
pub(super) const HANDLE: &str = "std.host.command.PtyHandle";
pub(super) const START: &str = "std.effects.Command.start_pty";

/// Recover the source constraint independently of the inferred request. A
/// dynamic formal can have an unknown origin even with an explicit selector.
pub fn checked_pty_profile_constraint(
    hir: &HirProgram,
    registry: &StdRegistry,
    expr: HirExprId,
) -> Result<Option<Vec<String>>, &'static str> {
    use etas_hir::{HirGenericArg, HirType};
    let Some(HirExpr::Perform { generic_args, .. }) = hir.exprs.get(expr) else {
        return Err("expected a checked PTY perform");
    };
    let ty = match generic_args.as_slice() {
        [] | [HirGenericArg::Wildcard { .. }] => return Ok(None),
        [HirGenericArg::Type(ty)] => *ty,
        _ => return Err("terminal action expects one sandbox selector"),
    };
    let Some(HirType::Path { path, .. }) = hir.types.get(ty) else {
        return Err("terminal action selector must name a sandbox profile");
    };
    etas_types::sandbox_profile::selector_path(hir, registry, path)
        .map(Some)
        .ok_or("terminal action selector has no checked sandbox profile identity")
}

pub(super) fn payload_argument(args: &[HirArg], index: usize, name: &str) -> Option<HirExprId> {
    args.iter()
        .find_map(|arg| match arg {
            HirArg::Named {
                name: parameter,
                value,
                ..
            } if parameter == name => Some(*value),
            _ => None,
        })
        .or_else(|| match args.get(index)? {
            HirArg::Positional(value) => Some(*value),
            _ => None,
        })
}

pub(super) fn start_profile(
    hir: &HirProgram,
    types: &TypeOutput,
    registry: &StdRegistry,
    expr: HirExprId,
) -> Option<HirExprId> {
    let args = match hir.exprs.get(expr)? {
        HirExpr::Call { args, callee, .. } => {
            let ty = types.facts.expression_producer_type(expr)?;
            if !matches!(types.store.get(ty), Some(Type::Nominal(owner)) if owner.name == HANDLE) {
                return None;
            }
            let source_path;
            let (module, declaration) =
                if let Some(selected) = types.facts.library_flow_calls.get(&expr) {
                    let key = selected.callable.as_declaration()?;
                    (key.module(), key.declaration())
                } else {
                    let symbol = etas_types::source_execution::callable_symbol(hir, *callee)?;
                    source_path = hir.declaration_path(symbol)?;
                    if let Some(key) = registry
                        .lookup_qualified(&source_path)
                        .and_then(|decl| registry.declaration_key(decl.id))
                    {
                        (key.module(), key.declaration())
                    } else {
                        source_path.split_at(source_path.len().checked_sub(1)?)
                    }
                };
            if module != ["std", "host", "command"] || declaration != ["start_pty"] {
                return None;
            }
            args
        }
        HirExpr::Perform { action, args, .. } if action.action == "start_pty" => {
            let ResolveResult::Resolved(action_symbol) = action.action_symbol else {
                return None;
            };
            let action_symbol = hir.canonical_source_symbol(action_symbol)?;
            let is_start = if let Some(selected) = hir.standard_action_targets.get(&action_symbol) {
                registry.symbol(*selected)?.qualified_path
                    == ["std", "effects", "Command", "start_pty"]
            } else {
                let SymbolDef::EffectAction {
                    owner_effect: Some(owner),
                    ..
                } = hir.symbols.get(action_symbol)?.def
                else {
                    return None;
                };
                hir.declaration_path(owner)? == ["std", "effects", "Command"]
            };
            if !is_start {
                return None;
            }
            args
        }
        _ => return None,
    };
    payload_argument(args, 1, "sandbox")
}
