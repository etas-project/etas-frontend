use super::program::LowerCtx;
use crate::*;
use etas_syntax::ast::*;

impl LowerCtx {
    pub(super) fn lower_pattern(
        &mut self,
        pat: &Pattern,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirPatId {
        let span = pat.span();
        if let Pattern::Ident(name) = pat {
            let local = self.local_binding;
            let symbol = if let Some(local) = local {
                self.bind_symbol_with_def(
                    scope,
                    SymbolData {
                        name: name.text.clone(),
                        kind: SymbolKind::Local,
                        visibility: crate::Visibility::Private,
                        defining_module: self.current_module,
                        defining_item: Some(item_id),
                        def: SymbolDef::Local {
                            binding: local.binding,
                            pattern: HirPatId(0),
                            ty: local.ty,
                            initializer: local.initializer,
                        },
                        declared_type: local.ty,
                        definition_span: name.span,
                    },
                )
            } else {
                let binding = self
                    .pattern_binding
                    .expect("identifier pattern should have a source binding context");
                self.bind_symbol_with_def(
                    scope,
                    SymbolData {
                        name: name.text.clone(),
                        kind: SymbolKind::Local,
                        visibility: crate::Visibility::Private,
                        defining_module: self.current_module,
                        defining_item: Some(item_id),
                        def: SymbolDef::PatternBinding {
                            owner: binding.owner,
                            pattern: HirPatId(0),
                            ty: binding.ty,
                            initializer: binding.initializer,
                        },
                        declared_type: binding.ty,
                        definition_span: name.span,
                    },
                )
            };
            let id = self.hir.pats.alloc(HirPat::Binding {
                symbol,
                span: name.span,
            });
            if let Some(symbol) = self.symbols.get_mut(symbol) {
                match &mut symbol.def {
                    SymbolDef::Local { pattern, .. }
                    | SymbolDef::PatternBinding { pattern, .. } => {
                        *pattern = id;
                    }
                    _ => {}
                }
            }
            self.source_map.map_pat(id, span);
            return id;
        }
        let hir_pat = match pat {
            Pattern::Ident(_) => unreachable!("identifier patterns return early"),
            Pattern::Wildcard(span) => HirPat::Wildcard { span: *span },
            Pattern::Literal(literal) => HirPat::Literal(lower_pattern_literal(literal)),
            Pattern::Tuple { elems, span } => HirPat::Tuple {
                elems: elems
                    .iter()
                    .map(|elem| self.lower_pattern(elem, scope, item_id))
                    .collect(),
                span: *span,
            },
            Pattern::Record(record) => HirPat::Record {
                path: record
                    .path
                    .as_ref()
                    .map(|path| self.resolve_path(path, scope, true)),
                fields: record
                    .fields
                    .iter()
                    .map(|field| HirRecordPatField {
                        name: field.name.text.clone(),
                        pat: field
                            .pattern
                            .as_ref()
                            .map(|pat| self.lower_pattern(pat, scope, item_id)),
                        span: field.span,
                    })
                    .collect(),
                span: record.span,
            },
            Pattern::Variant(variant) => HirPat::Variant {
                path: self.resolve_path(&variant.path, scope, true),
                args: variant
                    .patterns
                    .iter()
                    .map(|pat| self.lower_pattern(pat, scope, item_id))
                    .collect(),
                span: variant.span,
            },
            Pattern::Error(span) => HirPat::Error { span: *span },
        };
        let id = self.hir.pats.alloc(hir_pat);
        self.source_map.map_pat(id, span);
        id
    }
}

fn lower_pattern_literal(literal: &Literal) -> HirLiteral {
    match literal {
        Literal::Bool { value, span } => HirLiteral::Bool {
            value: *value,
            span: *span,
        },
        Literal::Int { text, span } => HirLiteral::Int {
            text: text.clone(),
            span: *span,
        },
        Literal::Float { text, span } => HirLiteral::Float {
            text: text.clone(),
            span: *span,
        },
        Literal::String { value, span } => HirLiteral::String {
            value: value.clone(),
            span: *span,
        },
        Literal::Char { value, span } => HirLiteral::Char {
            value: *value,
            span: *span,
        },
    }
}
