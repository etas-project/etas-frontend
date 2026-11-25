use std::collections::HashSet;

use etas_hir::{HirArg, HirExpr, HirExprId, HirPat, HirStmt, ResolveResult, SymbolDef};
use etas_hir_analysis::interprocedural::CallSite;

use crate::{
    ActionInstanceRef, Effect, EffectSummary, EffectUnit, RequirementFact, RequirementSet,
};

use super::engine::EffectSemantics;

const OPEN: &str = "std.effects.Fs.open";
const READ: &str = "std.effects.Fs.read";
const WRITE: &str = "std.effects.Fs.write";

impl EffectSemantics<'_> {
    pub(super) fn specialize_filesystem_open_call(
        &self,
        site: &CallSite<EffectUnit>,
        summary: &mut EffectSummary,
    ) {
        let is_filesystem_open =
            if let Some(call) = self.types.facts.library_flow_calls.get(&site.call) {
                call.callable
                    .as_declaration()
                    .is_some_and(|id| id.module() == ["std", "fs"] && id.declaration() == ["open"])
            } else {
                etas_types::source_execution::callable_symbol(self.hir, site.callee_expr)
                    .and_then(|symbol| self.hir.declaration_path(symbol))
                    .is_some_and(|path| path == ["std", "fs", "open"])
            };
        if !is_filesystem_open {
            return;
        }
        let Ok(arguments) = self.call_argument_slots(site, 2) else {
            return;
        };
        let Some(mode) = arguments
            .get(1)
            .copied()
            .flatten()
            .and_then(|expr| self.file_mode_variant(expr))
        else {
            return;
        };
        let actions = &summary.requested_actions.effects;
        if actions.len() != 1 || summary.requested_actions.open.is_some() {
            return;
        }
        let Some(open @ Effect::AppliedAction(instance)) = actions.iter().next() else {
            return;
        };
        if self.registry.action_by_name(OPEN).as_ref() != Some(&instance.action) {
            return;
        }
        let (Ok(all), Ok(required)) = (
            self.filesystem_rights(&instance.args, None),
            self.filesystem_rights(&instance.args, Some(mode)),
        ) else {
            return;
        };
        let Some(behavior) = &mut summary.default_behavior else {
            return;
        };
        for (guards, footprint) in &mut behavior.guarded {
            if guards.contains(open) {
                footprint.requirements = footprint
                    .requirements
                    .iter()
                    .filter(|fact| !all.contains(fact) || required.contains(fact))
                    .cloned()
                    .collect();
            }
        }
        summary.refresh_default_behavior();
    }

    pub(super) fn filesystem_open_authority(
        &self,
        invocation: &Effect,
        args: &[HirArg],
    ) -> Result<Option<RequirementSet>, &'static str> {
        let Effect::AppliedAction(open) = invocation else {
            return Ok(None);
        };
        if self.registry.action_by_name(OPEN).as_ref() != Some(&open.action) {
            return Ok(None);
        }
        let mode = args
            .iter()
            .find_map(|arg| match arg {
                HirArg::Named { name, value, .. } if name == "mode" => Some(*value),
                _ => None,
            })
            .or_else(|| match args.get(1) {
                Some(HirArg::Positional(mode)) => Some(*mode),
                _ => None,
            });
        self.filesystem_rights(
            &open.args,
            mode.and_then(|expr| self.file_mode_variant(expr)),
        )
        .map(Some)
    }

    fn filesystem_rights(
        &self,
        region: &[etas_types::EffectArgRef],
        mode: Option<&str>,
    ) -> Result<RequirementSet, &'static str> {
        let names: &[&str] = match mode {
            Some("Read") => &[READ],
            Some("WriteTruncate" | "Append" | "CreateNew") => &[WRITE],
            _ => &[READ, WRITE],
        };
        names
            .iter()
            .map(|name| {
                let action = self.registry.action_by_name(name).ok_or(
                    "Fs.open authority requires checked filesystem read and write actions",
                )?;
                Ok(RequirementFact::HostAuthority(Effect::AppliedAction(
                    ActionInstanceRef {
                        action,
                        args: region.to_vec(),
                    },
                )))
            })
            .collect()
    }

    fn file_mode_variant(&self, mut expr: HirExprId) -> Option<&str> {
        let ty = self.types.facts.expression_producer_type(expr)?;
        let etas_types::Type::Enum(owner) = self.types.store.get(ty)? else {
            return None;
        };
        if owner.name != "std.fs.FileMode" {
            return None;
        }

        let mut visited = HashSet::new();
        while visited.insert(expr) {
            let HirExpr::Path(path) = self.hir.exprs.get(expr)? else {
                return None;
            };
            let ResolveResult::Resolved(symbol) = path.resolution else {
                return None;
            };
            let symbol = self.type_symbols.canonical_symbol(self.hir, symbol)?;
            let definition = self.hir.symbols.get(symbol)?;
            match &definition.def {
                SymbolDef::Local {
                    binding,
                    initializer: Some(value),
                    ..
                } if matches!(self.hir.stmts.get(*binding), Some(HirStmt::Let { pat, .. })
                        if matches!(self.hir.pats.get(*pat), Some(HirPat::Binding { symbol: declared, .. }) if *declared == symbol)) =>
                {
                    expr = *value;
                }
                SymbolDef::TopLevelLet { initializer, .. } => expr = *initializer,
                _ => {
                    let (constructor, _, variant) = self
                        .types
                        .facts
                        .enum_constructor(&self.types.store, definition)?;
                    return (constructor == ty && variant.fields.is_empty())
                        .then_some(variant.name.as_str());
                }
            }
        }
        None
    }
}
