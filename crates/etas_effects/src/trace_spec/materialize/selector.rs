use etas_hir::{
    HirEffectArg, HirEffectRef, ImportAliasOrigin, PartialResolutionReason, ResolveResult,
    SymbolDef, SymbolId, resolved_static_type_selector_symbol, static_selector_path_segments,
};
use etas_types::{EffectArgRef, SymbolTypeFact, TypeId};

use crate::trace_spec::model::TraceSpecPattern;
use crate::{
    ActionInstanceRef, ActionRef, Effect, EffectActionArgKind, EffectPipelineError, EffectRow,
    EffectSet,
};

use super::{TraceSpecMaterializer, effect_label, effect_ref_span_for_missing_type};

impl TraceSpecMaterializer<'_> {
    pub(super) fn pattern_from_effect_ref(
        &mut self,
        effect_ref: &HirEffectRef,
    ) -> Result<Option<TraceSpecPattern>, EffectPipelineError> {
        let Some(effect) = self.effect_from_ref(effect_ref)? else {
            return Ok(None);
        };
        Ok(Some(TraceSpecPattern {
            row: EffectRow::closed(EffectSet::one(effect)),
            label: effect_label(effect_ref),
            arg_bounds: Vec::new(),
        }))
    }

    fn effect_from_ref(
        &mut self,
        effect_ref: &HirEffectRef,
    ) -> Result<Option<Effect>, EffectPipelineError> {
        let resolved = match &effect_ref.path.resolution {
            ResolveResult::Resolved(symbol) => *symbol,
            ResolveResult::PartiallyResolved(partial)
                if partial.reason == PartialResolutionReason::MemberRequiresTypeChecking =>
            {
                if let Some(effect) = self.effect_from_partial_member_ref(effect_ref, partial)? {
                    return Ok(Some(effect));
                }
                self.diagnostic(
                    effect_ref.span,
                    "trace spec action pattern must resolve to a checked effect or action",
                );
                return Ok(None);
            }
            _ => {
                if let Some(effect) = self.effect_from_standard_path(effect_ref)? {
                    return Ok(Some(effect));
                }
                self.diagnostic(
                    effect_ref.span,
                    "trace spec action pattern must resolve to a checked effect or action",
                );
                return Ok(None);
            }
        };
        if let Some(signature) = self.registry.action(resolved) {
            let action = ActionRef {
                tag: signature.owner,
                action: signature.id,
            };
            let Some(args) = self.action_args(effect_ref, &action)? else {
                return Ok(None);
            };
            return if args.is_empty() {
                Ok(Some(Effect::Action(action)))
            } else {
                Ok(Some(Effect::AppliedAction(ActionInstanceRef {
                    action,
                    args,
                })))
            };
        }
        if let Some(tag) = self.registry.tag_by_symbol(resolved) {
            let Some(args) = self.effect_type_args(effect_ref) else {
                return Ok(None);
            };
            return if args.is_empty() {
                Ok(Some(Effect::Tag(tag)))
            } else {
                Ok(Some(Effect::Applied { tag, args }))
            };
        }
        if let Some(effect) = self.effect_from_import_alias(resolved, effect_ref)? {
            return Ok(Some(effect));
        }
        self.diagnostic(
            effect_ref.span,
            "trace spec action pattern must resolve to an effect tag or action",
        );
        Ok(None)
    }

    fn effect_from_standard_path(
        &mut self,
        effect_ref: &HirEffectRef,
    ) -> Result<Option<Effect>, EffectPipelineError> {
        let qualified = effect_label(effect_ref);
        if let Some(action) = self
            .registry
            .standard_action_by_name(&qualified)
            .or_else(|| {
                self.registry.action_by_name(&qualified).filter(|action| {
                    self.registry.is_carried_action(action)
                        && self.registry.standard_tag_by_id(action.tag.0).is_some()
                })
            })
        {
            let Some(args) = self.action_args(effect_ref, &action)? else {
                return Ok(None);
            };
            return Ok(Some(if args.is_empty() {
                Effect::Action(action)
            } else {
                Effect::AppliedAction(ActionInstanceRef { action, args })
            }));
        }
        if let Some(tag) = self.registry.standard_tag_by_name(&qualified) {
            let Some(args) = self.effect_type_args(effect_ref) else {
                return Ok(None);
            };
            return Ok(Some(if args.is_empty() {
                Effect::Tag(tag)
            } else {
                Effect::Applied { tag, args }
            }));
        }
        Ok(None)
    }

    fn effect_from_partial_member_ref(
        &mut self,
        effect_ref: &HirEffectRef,
        partial: &etas_hir::PartialResolution,
    ) -> Result<Option<Effect>, EffectPipelineError> {
        let Some(prefix) = partial.resolved_prefix else {
            return Ok(None);
        };
        let Some(prefix) = self.hir.canonical_source_symbol(prefix) else {
            return Ok(None);
        };
        let tag = self.registry.tag_by_symbol(prefix).or_else(|| {
            let symbol = self.hir.symbols.get(prefix)?;
            let SymbolDef::ImportAlias { path, origin } = &symbol.def else {
                return None;
            };
            matches!(
                origin,
                ImportAliasOrigin::StdPrelude | ImportAliasOrigin::SourceImport
            )
            .then(|| self.registry.tag_by_name(&path.join(".")))
            .flatten()
        });
        let Some(tag) = tag else {
            return Ok(None);
        };
        if partial.remaining.len() != 1 {
            return Ok(None);
        }
        let action_name = format!(
            "{}.{}",
            self.registry.tag_name(tag).expect("checked effect tag"),
            partial.remaining[0]
        );
        let Some(action) = self.registry.action_by_name(&action_name) else {
            return Ok(None);
        };
        if action.tag != tag {
            return Ok(None);
        }
        let Some(args) = self.action_args(effect_ref, &action)? else {
            return Ok(None);
        };
        Ok(Some(if args.is_empty() {
            Effect::Action(action)
        } else {
            Effect::AppliedAction(ActionInstanceRef { action, args })
        }))
    }

    fn effect_from_import_alias(
        &mut self,
        symbol: SymbolId,
        effect_ref: &HirEffectRef,
    ) -> Result<Option<Effect>, EffectPipelineError> {
        let Some(symbol) = self.hir.symbols.get(symbol) else {
            return Ok(None);
        };
        let SymbolDef::ImportAlias { path, origin } = &symbol.def else {
            return Ok(None);
        };
        if !matches!(
            origin,
            ImportAliasOrigin::StdPrelude | ImportAliasOrigin::SourceImport
        ) {
            return Ok(None);
        }
        let qualified = path.join(".");
        if let Some(action) = self.registry.action_by_name(&qualified) {
            let Some(args) = self.action_args(effect_ref, &action)? else {
                return Ok(None);
            };
            return Ok(Some(if args.is_empty() {
                Effect::Action(action)
            } else {
                Effect::AppliedAction(ActionInstanceRef { action, args })
            }));
        }
        if let Some(tag) = self.registry.tag_by_name(&qualified) {
            let Some(args) = self.effect_type_args(effect_ref) else {
                return Ok(None);
            };
            return Ok(Some(if args.is_empty() {
                Effect::Tag(tag)
            } else {
                Effect::Applied { tag, args }
            }));
        }
        Ok(None)
    }

    fn action_args(
        &mut self,
        effect_ref: &HirEffectRef,
        action: &ActionRef,
    ) -> Result<Option<Vec<EffectArgRef>>, EffectPipelineError> {
        let Some(selectors) = self.registry.action_selectors(action) else {
            self.diagnostic(
                effect_ref.span,
                "trace spec action is missing checked selectors",
            );
            return Ok(None);
        };
        let defaults = self
            .registry
            .action_signature(action)
            .map_or(&[][..], |signature| signature.selector_defaults.as_slice());
        if !effect_ref.args.is_empty() && effect_ref.args.len() != selectors.len() {
            self.diagnostic(
                effect_ref.span,
                format!(
                    "trace spec action pattern expects {} static selector argument(s), got {}",
                    selectors.len(),
                    effect_ref.args.len()
                ),
            );
            return Ok(None);
        }
        if effect_ref.args.is_empty() {
            return Ok(Some(
                selectors
                    .iter()
                    .enumerate()
                    .map(|(index, _)| {
                        defaults
                            .get(index)
                            .and_then(Clone::clone)
                            .unwrap_or(EffectArgRef::Wildcard)
                    })
                    .collect(),
            ));
        }
        let args = selectors
            .iter()
            .zip(&effect_ref.args)
            .map(|(kind, arg)| self.effect_arg(arg, kind))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(args.into_iter().collect())
    }

    fn effect_type_args(&mut self, effect_ref: &HirEffectRef) -> Option<Vec<TypeId>> {
        effect_ref
            .args
            .iter()
            .map(|arg| match arg {
                HirEffectArg::Type(ty) => {
                    self.types.facts.type_refs.get(ty).copied().or_else(|| {
                        self.diagnostic(
                            effect_ref.span,
                            "trace spec type argument is missing checked type facts",
                        );
                        None
                    })
                }
                _ => {
                    self.diagnostic(
                        effect_ref.span,
                        "effect tag trace spec arguments must be types",
                    );
                    None
                }
            })
            .collect()
    }

    fn effect_arg(
        &mut self,
        arg: &HirEffectArg,
        kind: &EffectActionArgKind,
    ) -> Result<Option<EffectArgRef>, EffectPipelineError> {
        match arg {
            HirEffectArg::PatternBinding { span, .. } => {
                self.diagnostic(
                    *span,
                    "trace selector bounds require checked summary and metadata support",
                );
                Ok(None)
            }
            HirEffectArg::Wildcard { .. } => Ok(Some(EffectArgRef::Wildcard)),
            HirEffectArg::String { value, .. } => Ok(Some(EffectArgRef::String(value.clone()))),
            HirEffectArg::Int { text, .. } => Ok(Some(EffectArgRef::Int(text.clone()))),
            HirEffectArg::Type(ty) => match self.types.facts.type_refs.get(ty).copied() {
                Some(type_id) => Ok(Some(EffectArgRef::Type(type_id))),
                None => {
                    let span = effect_ref_span_for_missing_type(self.hir, *ty)?;
                    self.diagnostic(
                        span,
                        "trace spec selector type argument is missing checked type facts",
                    );
                    Ok(None)
                }
            },
            HirEffectArg::Path(path) => match kind {
                EffectActionArgKind::Type => Ok(self
                    .type_arg_from_path(path)
                    .map(EffectArgRef::Type)
                    .or_else(|| {
                        self.diagnostic(path.span, "trace spec selector must name a checked type");
                        None
                    })),
                EffectActionArgKind::MemoryPlace
                | EffectActionArgKind::StaticResourcePath { .. }
                | EffectActionArgKind::StringPattern => Ok(self
                    .static_path_segments(path)
                    .map(EffectArgRef::Path)
                    .or_else(|| {
                        self.diagnostic(
                            path.span,
                            "trace spec selector path must resolve to a static resource",
                        );
                        None
                    })),
            },
        }
    }

    fn type_arg_from_path(&self, path: &etas_hir::ResolvedPath) -> Option<TypeId> {
        let symbol = resolved_static_type_selector_symbol(path, |symbol| {
            self.hir.canonical_source_symbol(symbol)
        })
        .or_else(|| {
            let ResolveResult::PartiallyResolved(partial) = &path.resolution else {
                return None;
            };
            let [name] = partial.remaining.as_slice() else {
                return None;
            };
            let owner = self.hir.canonical_source_symbol(partial.resolved_prefix?)?;
            let SymbolTypeFact::Agent { signature } = self.types.facts.symbol_types.get(&owner)?
            else {
                return None;
            };
            self.types
                .facts
                .agent_method(signature.component, name)
                .map(|method| method.symbol)
        })?;
        if let Some(method) = self.types.facts.agent_methods.get(&symbol) {
            return Some(method.context_type);
        }
        if let Some(symbol_data) = self.hir.symbols.get(symbol)
            && matches!(symbol_data.def, etas_hir::SymbolDef::TypeParam { .. })
        {
            return self.types.facts.symbol_types.get(&symbol).and_then(|fact| {
                if let SymbolTypeFact::Param { ty } = fact {
                    Some(*ty)
                } else {
                    None
                }
            });
        }
        match self.types.facts.symbol_types.get(&symbol) {
            Some(SymbolTypeFact::Agent { signature }) => Some(signature.component),
            Some(SymbolTypeFact::TypeAlias { target, .. }) => Some(*target),
            Some(SymbolTypeFact::Type { constructor })
            | Some(SymbolTypeFact::NominalType { constructor, .. }) => Some(TypeId(constructor.0)),
            _ => None,
        }
    }

    fn static_path_segments(&self, path: &etas_hir::ResolvedPath) -> Option<Vec<String>> {
        static_selector_path_segments(self.hir, path, Some)
    }
}
