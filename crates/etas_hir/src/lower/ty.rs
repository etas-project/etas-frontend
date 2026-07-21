use super::program::LowerCtx;
use crate::*;
use etas_syntax::ast::{self, *};

impl LowerCtx {
    pub(super) fn lower_type(
        &mut self,
        ty: &TypeExpr,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirTypeId {
        let hir_type = match ty {
            TypeExpr::Handler(handler) => {
                let handled = self.lower_effect_row(&handler.handled, scope, item_id);
                let produced = match &handler.produced {
                    HandlerProducedEffects::Infer => HirHandlerProducedEffects::Infer,
                    HandlerProducedEffects::Explicit(produced) => {
                        HirHandlerProducedEffects::Explicit(
                            self.lower_effect_row(produced, scope, item_id),
                        )
                    }
                };
                let result = handler
                    .result
                    .as_ref()
                    .map(|result| self.lower_type(result, scope, item_id));
                HirType::Handler {
                    handled,
                    produced,
                    result,
                    span: handler.span,
                }
            }
            TypeExpr::Arrow {
                effect,
                input,
                output,
                span,
            } => HirType::Arrow {
                effect: effect
                    .as_ref()
                    .map(|effect| self.lower_effect_row(effect, scope, item_id)),
                input: self.lower_type(input, scope, item_id),
                output: self.lower_type(output, scope, item_id),
                span: *span,
            },
            TypeExpr::Primitive { kind, span } => HirType::Primitive {
                kind: *kind,
                span: *span,
            },
            TypeExpr::Path { path, args, span } => HirType::Path {
                path: self.resolve_path(path, scope, true),
                args: args
                    .iter()
                    .map(|arg| self.lower_type(arg, scope, item_id))
                    .collect(),
                span: *span,
            },
            TypeExpr::Record(record) => HirType::Record {
                fields: record
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(field_index, field)| {
                        let ty = self.lower_type(&field.ty, scope, item_id);
                        let symbol = self.alloc_symbol_with_def(SymbolData {
                            name: field.name.text.clone(),
                            kind: SymbolKind::Field,
                            visibility: match field.visibility {
                                Some(ast::Visibility::Public) => crate::HirVisibility::Public,
                                _ => crate::HirVisibility::Private,
                            },
                            defining_module: self.current_module,
                            defining_item: Some(item_id),
                            def: SymbolDef::Field {
                                owner: item_id,
                                field_index: field_index.min(u32::MAX as usize) as u32,
                                ty,
                            },
                            declared_type: Some(ty),
                            definition_span: field.name.span,
                        });
                        HirFieldDecl {
                            symbol,
                            ty,
                            visibility: field.visibility.unwrap_or(ast::Visibility::Private),
                            span: field.span,
                        }
                    })
                    .collect(),
                span: record.span,
            },
            TypeExpr::Tuple { elems, span } => HirType::Tuple {
                elems: elems
                    .iter()
                    .map(|elem| self.lower_type(elem, scope, item_id))
                    .collect(),
                span: *span,
            },
            TypeExpr::Refined {
                base,
                predicate,
                span,
            } => HirType::Refined {
                base: self.lower_type(base, scope, item_id),
                predicate: self.lower_expr(predicate, scope, item_id),
                span: *span,
            },
            TypeExpr::Error(span) => HirType::Error { span: *span },
        };
        let id = self.hir.types.alloc(hir_type);
        self.source_map.map_type(id, ty.span());
        id
    }

    pub(super) fn lower_generic_arg(
        &mut self,
        arg: &ast::GenericArg,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirGenericArg {
        match arg {
            ast::GenericArg::Type(ty) => HirGenericArg::Type(self.lower_type(ty, scope, item_id)),
            ast::GenericArg::EffectRow(row) => {
                HirGenericArg::EffectRow(self.lower_effect_row(row, scope, item_id))
            }
            ast::GenericArg::Wildcard { span } => HirGenericArg::Wildcard { span: *span },
        }
    }

    pub(super) fn lower_effect_row(
        &mut self,
        row: &ast::EffectRow,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirEffectRow {
        let mut effects = Vec::new();
        let mut tail = None;
        for effect in &row.effects {
            let lowered = self.lower_effect_ref(effect, scope, item_id);
            let is_effect_param = lowered.args.is_empty()
                && matches!(
                    lowered.path.resolution,
                    ResolveResult::Resolved(symbol)
                        if self
                            .symbols
                            .get(symbol)
                            .is_some_and(|symbol| symbol.kind == SymbolKind::EffectParam)
                );
            if is_effect_param {
                tail = Some(lowered.path);
            } else {
                effects.push(lowered);
            }
        }
        HirEffectRow {
            effects,
            tail,
            span: row.span,
        }
    }

    pub(super) fn lower_effect_ref(
        &mut self,
        effect: &ast::EffectRef,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirEffectRef {
        HirEffectRef {
            path: self.resolve_path(&effect.path, scope, true),
            args: effect
                .args
                .iter()
                .map(|arg| self.lower_effect_arg(arg, scope, item_id))
                .collect(),
            span: effect.span,
        }
    }

    pub(super) fn lower_effect_arg(
        &mut self,
        arg: &ast::EffectArg,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirEffectArg {
        match arg {
            ast::EffectArg::Type(ty) => HirEffectArg::Type(self.lower_type(ty, scope, item_id)),
            ast::EffectArg::Path(path) => HirEffectArg::Path(self.resolve_path(path, scope, false)),
            ast::EffectArg::Wildcard { span } => HirEffectArg::Wildcard { span: *span },
            ast::EffectArg::String { value, span } => HirEffectArg::String {
                value: value.clone(),
                span: *span,
            },
            ast::EffectArg::Int { text, span } => HirEffectArg::Int {
                text: text.clone(),
                span: *span,
            },
        }
    }
}
