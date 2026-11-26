use super::program::{LowerCtx, PreludeContext};
use crate::*;
use etas_syntax::ast::{self, *};

impl LowerCtx {
    pub(super) fn lower_type(
        &mut self,
        ty: &TypeExpr,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirTypeId {
        self.lower_type_with_field_annotations(ty, scope, item_id, false)
    }

    pub(super) fn lower_stored_type(
        &mut self,
        ty: &TypeExpr,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirTypeId {
        self.lower_type_with_field_annotations(ty, scope, item_id, true)
    }

    fn lower_type_with_field_annotations(
        &mut self,
        ty: &TypeExpr,
        scope: ScopeId,
        item_id: HirItemId,
        stored_fields: bool,
    ) -> HirTypeId {
        let hir_type = match ty {
            TypeExpr::Existential { bounds, span } => HirType::Existential {
                bounds: bounds
                    .iter()
                    .map(|bound| self.lower_type_param_bound(bound, scope, item_id))
                    .collect(),
                span: *span,
            },
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
                path: self.resolve_path_in_context(path, scope, true, PreludeContext::Type),
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
                        let mut injection = false;
                        for annotation in &field.annotations {
                            let is_inject = matches!(annotation.path.segments.as_slice(), [name] if name.text == "inject");
                            let reason = if !stored_fields {
                                Some("field annotations require a stored field of a named record type or agent")
                            } else if !is_inject {
                                Some("only `@inject` and `@inject(Action)` are supported on stored fields")
                            } else if injection {
                                Some("a stored field can have only one `@inject` annotation")
                            } else if !matches!(annotation.args.as_slice(), [] | [ast::AnnotationArg::Positional(ast::Expr::Perform(_))]) {
                                Some("`@inject` requires one action reference")
                            } else { None };
                            if let Some(reason) = reason {
                                self.diagnostics.push(etas_core::Diagnostic::type_check(
                                    etas_core::TypeDiagnosticCode::InvalidAnnotation,
                                    annotation.span,
                                    reason,
                                ));
                            }
                            injection |= is_inject;
                        }
                        let mut annotations = self.lower_annotations(&field.annotations, scope, item_id);
                        if stored_fields {
                            for annotation in &mut annotations {
                                if annotation.args.is_empty()
                                    && matches!(annotation.path.segments.as_slice(), [name] if name.name == "inject")
                                {
                                    let span = annotation.span;
                                    let (effect, action) = ast::EffectRef::typed_injection(field.ty.clone(), span);
                                    let value = self.lower_expr(&ast::Expr::Perform(ast::PerformExpr {
                                        effect,
                                        action,
                                        generic_args: Vec::new(),
                                        args: vec![ast::Arg::Positional(ast::Expr::Literal(ast::Literal::String {
                                            value: field.name.text.clone(), span: field.name.span,
                                        }))],
                                        span,
                                    }), scope, item_id);
                                    annotation.args.push(crate::HirAnnotationArg::Positional { value, span });
                                }
                            }
                        }
                        let ty = self.lower_type(&field.ty, scope, item_id);
                        let symbol = self.alloc_symbol_with_def(SymbolData {
                            name: field.name.text.clone(),
                            kind: SymbolKind::Field,
                            visibility: match field.visibility {
                                Some(ast::Visibility::Private) => crate::HirVisibility::Private,
                                _ => crate::HirVisibility::Public,
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
                            annotations,
                            symbol,
                            ty,
                            visibility: field.visibility.unwrap_or(ast::Visibility::Public),
                            span: field.span,
                        }
                    })
                    .collect(),
                span: record.span,
            },
            TypeExpr::Array { element, span } => HirType::Array {
                element: self.lower_type(element, scope, item_id),
                span: *span,
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
            path: self.resolve_path_in_context(&effect.path, scope, true, PreludeContext::Effect),
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
            ast::EffectArg::PatternBinding { name, bounds, span } => HirEffectArg::PatternBinding {
                name: name.text.clone(),
                bounds: bounds
                    .iter()
                    .map(|bound| self.lower_type_param_bound(bound, scope, item_id))
                    .collect(),
                span: *span,
            },
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
