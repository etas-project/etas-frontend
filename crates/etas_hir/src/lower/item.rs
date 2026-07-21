use super::{
    binding::{action_key, item_key, item_symbol},
    program::LowerCtx,
};
use crate::*;
use etas_syntax::ast::{self, *};

impl LowerCtx {
    pub(super) fn predeclare_items(
        &mut self,
        items: &[ast::AnnotatedItem],
        module_scope: ScopeId,
        module_qualified_prefix: Option<&str>,
    ) {
        for item in items {
            if let Some((name, kind, span)) = item_symbol(&item.item) {
                let symbol = self.alloc_symbol_with_def(SymbolData {
                    name: name.clone(),
                    kind,
                    visibility: hir_visibility(item_visibility(&item.item)),
                    defining_module: self.current_module,
                    defining_item: None,
                    def: SymbolDef::Error,
                    declared_type: None,
                    definition_span: span,
                });
                self.insert_symbol(module_scope, &name, symbol, span);
                if let Some(prefix) = module_qualified_prefix {
                    self.insert_symbol(module_scope, &format!("{prefix}.{name}"), symbol, span);
                }
                self.top_item_symbols.insert(item_key(&item.item), symbol);
            }
        }

        for item in items {
            if let ast::Item::Effect(effect) = &item.item {
                for action in effect.body.actions() {
                    let qualified = format!("{}.{}", effect.name.text, action.name.text);
                    let symbol = self.alloc_symbol_with_def(SymbolData {
                        name: qualified.clone(),
                        kind: SymbolKind::EffectAction,
                        visibility: hir_visibility(effect.visibility),
                        defining_module: self.current_module,
                        defining_item: None,
                        def: SymbolDef::Error,
                        declared_type: None,
                        definition_span: action.name.span,
                    });
                    self.insert_symbol(module_scope, &qualified, symbol, action.name.span);
                    if let Some(prefix) = module_qualified_prefix {
                        self.insert_symbol(
                            module_scope,
                            &format!("{prefix}.{qualified}"),
                            symbol,
                            action.name.span,
                        );
                    }
                    self.action_symbols.insert(action_key(action), symbol);
                }
            }
        }
    }

    pub(super) fn lower_item(
        &mut self,
        annotated: &ast::AnnotatedItem,
        parent_scope: ScopeId,
    ) -> HirItemId {
        let item = &annotated.item;
        let item_id = self.hir.items.alloc(HirItem::Error {
            span: annotated.span(),
        });
        let annotations = self.lower_annotations(&annotated.annotations, parent_scope, item_id);
        if !annotations.is_empty() {
            self.hir.item_annotations.insert(item_id, annotations);
        }
        let hir_item = match item {
            ast::Item::Type(item) => {
                HirItem::Type(self.lower_type_decl(item, item_id, parent_scope))
            }
            ast::Item::Alias(item) => {
                HirItem::TypeAlias(self.lower_type_alias_decl(item, item_id, parent_scope))
            }
            ast::Item::Enum(item) => {
                HirItem::Enum(self.lower_enum_decl(item, item_id, parent_scope))
            }
            ast::Item::Spec(item) => {
                HirItem::Spec(self.lower_spec_decl(item, item_id, parent_scope))
            }
            ast::Item::Impl(item) => {
                HirItem::Impl(self.lower_impl_decl(item, item_id, parent_scope))
            }
            ast::Item::Effect(item) => {
                HirItem::Effect(self.lower_effect_decl(item, item_id, parent_scope))
            }
            ast::Item::TopLevelLet(item) => {
                HirItem::TopLevelLet(self.lower_top_level_let_decl(item, item_id, parent_scope))
            }
            ast::Item::Tool(item) => {
                HirItem::Tool(self.lower_tool_decl(item, item_id, parent_scope))
            }
            ast::Item::Agent(item) => {
                HirItem::Agent(self.lower_agent_decl(item, item_id, parent_scope))
            }
            ast::Item::Protocol(item) => {
                HirItem::Protocol(self.lower_protocol_decl(item, item_id, parent_scope))
            }
            ast::Item::Flow(item) => {
                HirItem::Flow(self.lower_flow_decl(item, item_id, parent_scope))
            }
            ast::Item::Error(item) => HirItem::Error { span: item.span },
        };
        *self.hir.items.get_mut(item_id).unwrap() = hir_item;
        item_id
    }

    fn lower_annotations(
        &mut self,
        annotations: &[ast::Annotation],
        scope: ScopeId,
        item_id: HirItemId,
    ) -> Vec<HirAnnotation> {
        annotations
            .iter()
            .map(|annotation| HirAnnotation {
                path: self.resolve_path(&annotation.path, scope, false),
                args: annotation
                    .args
                    .iter()
                    .map(|arg| match arg {
                        ast::AnnotationArg::Positional(value) => HirAnnotationArg::Positional {
                            value: self.lower_expr(value, scope, item_id),
                            span: value.span(),
                        },
                        ast::AnnotationArg::Named { name, value, span } => {
                            HirAnnotationArg::Named {
                                name: name.text.clone(),
                                name_span: name.span,
                                value: self.lower_expr(value, scope, item_id),
                                span: *span,
                            }
                        }
                    })
                    .collect(),
                span: annotation.span,
            })
            .collect()
    }

    pub(super) fn lower_type_alias_decl(
        &mut self,
        decl: &ast::TypeAliasDecl,
        item_id: HirItemId,
        parent_scope: ScopeId,
    ) -> HirTypeAliasDecl {
        let symbol = self.item_symbol(item_id, &ast::Item::Alias(decl.clone()));
        let scope = self
            .scopes
            .alloc(Some(parent_scope), ScopeOwner::Item(item_id), decl.span);
        let type_params = self.lower_type_params(&decl.type_params, scope, item_id);
        let target = self.lower_type(&decl.target, scope, item_id);
        self.set_symbol_item(symbol, item_id);
        self.map_item(item_id, decl.span);
        HirTypeAliasDecl {
            symbol,
            type_params,
            target,
            scope,
            span: decl.span,
        }
    }

    pub(super) fn lower_type_decl(
        &mut self,
        decl: &ast::TypeDecl,
        item_id: HirItemId,
        parent_scope: ScopeId,
    ) -> HirTypeDecl {
        let symbol = self.item_symbol(item_id, &ast::Item::Type(decl.clone()));
        let scope = self
            .scopes
            .alloc(Some(parent_scope), ScopeOwner::Item(item_id), decl.span);
        let type_params = self.lower_type_params(&decl.type_params, scope, item_id);
        let body = match &decl.body {
            ast::TypeDeclBody::Bodyless => HirTypeDeclBody::Bodyless,
            ast::TypeDeclBody::Representation(ty) => {
                HirTypeDeclBody::Representation(self.lower_type(ty, scope, item_id))
            }
        };
        self.set_symbol_item(symbol, item_id);
        self.map_item(item_id, decl.span);
        HirTypeDecl {
            symbol,
            type_params,
            body,
            scope,
            span: decl.span,
        }
    }

    pub(super) fn lower_enum_decl(
        &mut self,
        decl: &ast::EnumDecl,
        item_id: HirItemId,
        parent_scope: ScopeId,
    ) -> HirEnumDecl {
        let symbol = self.item_symbol(item_id, &ast::Item::Enum(decl.clone()));
        let scope = self
            .scopes
            .alloc(Some(parent_scope), ScopeOwner::Item(item_id), decl.span);
        let type_params = self.lower_type_params(&decl.type_params, scope, item_id);
        let variants = decl
            .variants
            .iter()
            .enumerate()
            .map(|(variant_index, variant)| {
                let variant_symbol = self.alloc_symbol_with_def(SymbolData {
                    name: variant.name.text.clone(),
                    kind: SymbolKind::EnumVariant,
                    visibility: crate::Visibility::Public,
                    defining_module: self.current_module,
                    defining_item: Some(item_id),
                    def: SymbolDef::EnumVariant {
                        enum_item: item_id,
                        variant_index: variant_index.min(u32::MAX as usize) as u32,
                    },
                    declared_type: None,
                    definition_span: variant.name.span,
                });
                self.insert_symbol(scope, &variant.name.text, variant_symbol, variant.name.span);
                let fields = variant
                    .fields
                    .iter()
                    .map(|field| self.lower_type(field, scope, item_id))
                    .collect();
                HirEnumVariant {
                    symbol: variant_symbol,
                    fields,
                    span: variant.span,
                }
            })
            .collect();
        self.set_symbol_item(symbol, item_id);
        self.map_item(item_id, decl.span);
        HirEnumDecl {
            symbol,
            type_params,
            variants,
            scope,
            span: decl.span,
        }
    }

    pub(super) fn lower_impl_decl(
        &mut self,
        decl: &ast::ImplDecl,
        item_id: HirItemId,
        parent_scope: ScopeId,
    ) -> HirImplDecl {
        if decl
            .target
            .owner_path()
            .map_or(true, |path| path.segments.is_empty())
        {
            self.diagnostics.invalid_impl_target(decl.span);
        }
        let scope = self
            .scopes
            .alloc(Some(parent_scope), ScopeOwner::Item(item_id), decl.span);
        let target = match &decl.target {
            ast::ImplTarget::Inherent {
                target, type_args, ..
            } => HirImplTarget::Inherent {
                target: self.resolve_path(target, parent_scope, true),
                type_args: type_args
                    .iter()
                    .map(|arg| self.lower_type(arg, scope, item_id))
                    .collect(),
            },
            ast::ImplTarget::SpecSatisfaction {
                specs, self_type, ..
            } => HirImplTarget::SpecSatisfaction {
                specs: specs
                    .iter()
                    .map(|spec_ref| HirImplSpecRef {
                        spec_path: self.resolve_path(&spec_ref.spec_path, parent_scope, true),
                        spec_args: spec_ref
                            .spec_args
                            .iter()
                            .map(|arg| self.lower_type(arg, scope, item_id))
                            .collect(),
                        span: spec_ref.span,
                    })
                    .collect(),
                self_type: self.lower_type(self_type, scope, item_id),
            },
            ast::ImplTarget::Error { .. } => HirImplTarget::Error,
        };
        let owner = decl
            .target
            .owner_path()
            .map(path_text)
            .unwrap_or_else(|| "<invalid-impl>".to_string());
        let owner_effect = match &target {
            HirImplTarget::Inherent { target, .. } => match &target.resolution {
                ResolveResult::Resolved(symbol)
                    if self
                        .symbols
                        .get(*symbol)
                        .is_some_and(|symbol| symbol.kind == SymbolKind::Effect) =>
                {
                    Some(*symbol)
                }
                _ => None,
            },
            HirImplTarget::SpecSatisfaction { .. } | HirImplTarget::Error => None,
        };
        let items = decl
            .items
            .iter()
            .enumerate()
            .map(|item| match item {
                (_, ImplItem::Flow(flow)) => {
                    HirImplItem::Flow(self.lower_flow_like(flow, item_id, scope, Some(&owner)))
                }
                (item_index, ImplItem::Action(action)) => {
                    HirImplItem::Action(self.lower_action_decl(
                        action,
                        item_id,
                        scope,
                        Some(&owner),
                        owner_effect,
                        item_index,
                    ))
                }
                (_, ImplItem::Error(span)) => HirImplItem::Error { span: *span },
            })
            .collect();
        self.map_item(item_id, decl.span);
        HirImplDecl {
            target,
            items,
            scope,
            span: decl.span,
        }
    }

    pub(super) fn lower_spec_decl(
        &mut self,
        decl: &ast::SpecDecl,
        item_id: HirItemId,
        parent_scope: ScopeId,
    ) -> HirSpecDecl {
        let symbol = self.item_symbol(item_id, &ast::Item::Spec(decl.clone()));
        let scope = self
            .scopes
            .alloc(Some(parent_scope), ScopeOwner::Item(item_id), decl.span);
        let type_params = self.lower_type_params(&decl.type_params, scope, item_id);
        let bounds = decl
            .bounds
            .iter()
            .map(|bound| self.lower_type_param_bound(bound, scope, item_id))
            .collect();
        let callable = decl.callable.as_ref().map(|signature| {
            let input = self.lower_type(&signature.input, scope, item_id);
            let output = self.lower_type(&signature.output, scope, item_id);
            let effects = signature
                .effects
                .as_ref()
                .map(|effects| self.lower_effect_row(effects, scope, item_id));
            HirSpecCallableSignature {
                input,
                output,
                effects,
                span: signature.span,
            }
        });
        let trace = decl
            .trace
            .as_ref()
            .map(|trace| self.lower_spec_expr(trace, scope, item_id));
        let items = decl
            .items
            .iter()
            .enumerate()
            .map(|(item_index, item)| match item {
                ast::SpecItem::FlowSignature(signature) => HirSpecItem::FlowSignature(
                    self.lower_flow_signature(signature, item_id, scope, item_index),
                ),
                ast::SpecItem::Error(span) => HirSpecItem::Error { span: *span },
            })
            .collect();
        self.set_symbol_item(symbol, item_id);
        self.map_item(item_id, decl.span);
        HirSpecDecl {
            symbol,
            type_params,
            bounds,
            kind: match decl.kind {
                ast::SpecKind::TypeSpec => HirSpecKind::TypeSpec,
                ast::SpecKind::CallableSpec => HirSpecKind::CallableSpec,
                ast::SpecKind::TraceSpec => HirSpecKind::TraceSpec,
            },
            callable,
            trace,
            items,
            scope,
            span: decl.span,
        }
    }

    fn lower_spec_expr(
        &mut self,
        expr: &ast::SpecExpr,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirSpecExpr {
        match expr {
            ast::SpecExpr::Atom(pattern) => {
                HirSpecExpr::Atom(self.lower_effect_ref(pattern, scope, item_id))
            }
            ast::SpecExpr::Allow { pattern, span } => HirSpecExpr::Allow {
                pattern: self.lower_effect_ref(pattern, scope, item_id),
                span: *span,
            },
            ast::SpecExpr::Deny { pattern, span } => HirSpecExpr::Deny {
                pattern: self.lower_effect_ref(pattern, scope, item_id),
                span: *span,
            },
            ast::SpecExpr::And { lhs, rhs, span } => HirSpecExpr::And {
                lhs: Box::new(self.lower_spec_expr(lhs, scope, item_id)),
                rhs: Box::new(self.lower_spec_expr(rhs, scope, item_id)),
                span: *span,
            },
            ast::SpecExpr::Or { lhs, rhs, span } => HirSpecExpr::Or {
                lhs: Box::new(self.lower_spec_expr(lhs, scope, item_id)),
                rhs: Box::new(self.lower_spec_expr(rhs, scope, item_id)),
                span: *span,
            },
            ast::SpecExpr::Before {
                before,
                after,
                span,
            } => HirSpecExpr::Before {
                before: Box::new(self.lower_spec_expr(before, scope, item_id)),
                after: Box::new(self.lower_spec_expr(after, scope, item_id)),
                span: *span,
            },
            ast::SpecExpr::After {
                after,
                before,
                span,
            } => HirSpecExpr::After {
                after: Box::new(self.lower_spec_expr(after, scope, item_id)),
                before: Box::new(self.lower_spec_expr(before, scope, item_id)),
                span: *span,
            },
        }
    }

    fn lower_flow_signature(
        &mut self,
        signature: &ast::FlowSignature,
        item_id: HirItemId,
        parent_scope: ScopeId,
        method_index: usize,
    ) -> HirFlowSignature {
        let scope = self.scopes.alloc(
            Some(parent_scope),
            ScopeOwner::Item(item_id),
            signature.span,
        );
        let symbol = self.alloc_symbol_with_def(SymbolData {
            name: signature.name.text.clone(),
            kind: SymbolKind::Flow,
            visibility: crate::Visibility::Public,
            defining_module: self.current_module,
            defining_item: Some(item_id),
            def: SymbolDef::Param {
                owner: item_id,
                param_index: method_index.min(u32::MAX as usize) as u32,
                pattern: None,
                ty: None,
            },
            declared_type: None,
            definition_span: signature.name.span,
        });
        self.insert_symbol(scope, &signature.name.text, symbol, signature.name.span);
        let type_params = self.lower_type_params(&signature.type_params, scope, item_id);
        let params = self.lower_params(&signature.params, scope, item_id);
        let return_type = signature
            .return_type
            .as_ref()
            .map(|ty| self.lower_type(ty, scope, item_id));
        let effects = signature
            .declared_effects
            .as_ref()
            .map(|effects| self.lower_effect_row(effects, scope, item_id));
        HirFlowSignature {
            symbol,
            type_params,
            params,
            return_type,
            effects,
            scope,
            span: signature.span,
        }
    }

    pub(super) fn lower_effect_decl(
        &mut self,
        decl: &ast::EffectDecl,
        item_id: HirItemId,
        parent_scope: ScopeId,
    ) -> HirEffectDecl {
        let symbol = self.item_symbol(item_id, &ast::Item::Effect(decl.clone()));
        let scope = self
            .scopes
            .alloc(Some(parent_scope), ScopeOwner::Item(item_id), decl.span);
        let type_params = self.lower_type_params(&decl.type_params, scope, item_id);
        let extends = decl
            .extends
            .as_ref()
            .map(|effect| self.lower_effect_ref(effect, scope, item_id));
        let body = match &decl.body {
            ast::EffectBody::Empty { span } => HirEffectBody::Empty { span: *span },
            ast::EffectBody::Block { actions, span } => HirEffectBody::Block {
                actions: actions
                    .iter()
                    .enumerate()
                    .map(|(action_index, action)| {
                        self.lower_action_decl(
                            action,
                            item_id,
                            scope,
                            Some(&decl.name.text),
                            Some(symbol),
                            action_index,
                        )
                    })
                    .collect(),
                span: *span,
            },
        };
        self.set_symbol_item(symbol, item_id);
        self.map_item(item_id, decl.span);
        HirEffectDecl {
            symbol,
            type_params,
            extends,
            body,
            scope,
            span: decl.span,
        }
    }

    pub(super) fn lower_top_level_let_decl(
        &mut self,
        decl: &ast::TopLevelLetDecl,
        item_id: HirItemId,
        parent_scope: ScopeId,
    ) -> HirTopLevelLetDecl {
        let symbol = self.item_symbol(item_id, &ast::Item::TopLevelLet(decl.clone()));
        let type_annotation = decl
            .type_annotation
            .as_ref()
            .map(|ty| self.lower_type(ty, parent_scope, item_id));
        let value = self.lower_expr(&decl.value, parent_scope, item_id);
        self.set_symbol_item(symbol, item_id);
        if let Some(symbol_data) = self.symbols.get_mut(symbol) {
            symbol_data.declared_type = type_annotation;
            symbol_data.def = SymbolDef::TopLevelLet {
                item: item_id,
                ty: type_annotation,
                initializer: value,
                classification: TopLevelLetClassification::Unknown,
            };
        }
        self.map_item(item_id, decl.span);
        HirTopLevelLetDecl {
            symbol,
            visibility: hir_visibility(decl.visibility),
            type_annotation,
            value,
            classification: TopLevelLetClassification::Unknown,
            span: decl.span,
        }
    }

    pub(super) fn lower_tool_decl(
        &mut self,
        decl: &ast::ToolDecl,
        item_id: HirItemId,
        parent_scope: ScopeId,
    ) -> HirToolDecl {
        let symbol = self.item_symbol(item_id, &ast::Item::Tool(decl.clone()));
        let scope = self
            .scopes
            .alloc(Some(parent_scope), ScopeOwner::Item(item_id), decl.span);
        let type_params = self.lower_type_params(&decl.type_params, scope, item_id);
        let params = self.lower_params(&decl.params, scope, item_id);
        let return_type = self.lower_type(&decl.return_type, scope, item_id);
        let effects = decl
            .effects
            .as_ref()
            .map(|effects| self.lower_effect_row(effects, scope, item_id));
        let conformances = decl
            .conformances
            .iter()
            .map(|conformance| self.lower_declaration_conformance(conformance, scope, item_id))
            .collect::<Vec<_>>();
        let body = match &decl.body {
            ToolBody::Source(body) => {
                HirToolBody::Source(self.lower_flow_body(body, scope, item_id))
            }
            ToolBody::Decl { semicolon_span } => HirToolBody::Decl {
                span: *semicolon_span,
            },
            ToolBody::Error(span) => HirToolBody::Error { span: *span },
        };
        self.set_symbol_item(symbol, item_id);
        self.map_item(item_id, decl.span);
        HirToolDecl {
            symbol,
            type_params,
            params,
            return_type,
            effects,
            conformances,
            body,
            scope,
            span: decl.span,
        }
    }

    pub(super) fn lower_agent_decl(
        &mut self,
        decl: &ast::AgentDecl,
        item_id: HirItemId,
        parent_scope: ScopeId,
    ) -> HirAgentDecl {
        let symbol = self.item_symbol(item_id, &ast::Item::Agent(decl.clone()));
        let scope = self
            .scopes
            .alloc(Some(parent_scope), ScopeOwner::Item(item_id), decl.span);
        let params = self.lower_params(&decl.params, scope, item_id);
        let output_type = decl
            .output_type
            .as_ref()
            .map(|output_type| self.lower_type(output_type, scope, item_id));
        let effects = decl
            .effects
            .as_ref()
            .map(|effects| self.lower_effect_row(effects, scope, item_id));
        let conformances = decl
            .conformances
            .iter()
            .map(|conformance| self.lower_declaration_conformance(conformance, scope, item_id))
            .collect::<Vec<_>>();
        let body = match &decl.body {
            ast::AgentBody::Source(block) => HirAgentBody::Source {
                block: self.lower_block(block, scope, item_id),
            },
            ast::AgentBody::Decl { semicolon_span } => HirAgentBody::Decl {
                span: *semicolon_span,
            },
            ast::AgentBody::Error(span) => HirAgentBody::Error { span: *span },
        };
        self.set_symbol_item(symbol, item_id);
        self.map_item(item_id, decl.span);
        HirAgentDecl {
            symbol,
            params,
            output_type,
            effects,
            conformances,
            body,
            scope,
            span: decl.span,
        }
    }

    pub(super) fn lower_protocol_decl(
        &mut self,
        decl: &ast::ProtocolDecl,
        item_id: HirItemId,
        parent_scope: ScopeId,
    ) -> HirProtocolDecl {
        let symbol = self.item_symbol(item_id, &ast::Item::Protocol(decl.clone()));
        let scope = self
            .scopes
            .alloc(Some(parent_scope), ScopeOwner::Item(item_id), decl.span);
        let messages = decl
            .messages
            .iter()
            .map(|message| HirProtocolMsg {
                from: self.resolve_path(&message.from, scope, true),
                to: self.resolve_path(&message.to, scope, true),
                payload: self.lower_type(&message.payload, scope, item_id),
                span: message.span,
            })
            .collect();
        self.set_symbol_item(symbol, item_id);
        self.map_item(item_id, decl.span);
        HirProtocolDecl {
            symbol,
            messages,
            scope,
            span: decl.span,
        }
    }

    pub(super) fn lower_flow_decl(
        &mut self,
        decl: &ast::FlowDecl,
        item_id: HirItemId,
        parent_scope: ScopeId,
    ) -> HirFlowDecl {
        let flow = self.lower_flow_like(decl, item_id, parent_scope, None);
        self.map_item(item_id, decl.span);
        flow
    }

    pub(super) fn lower_flow_like(
        &mut self,
        decl: &ast::FlowDecl,
        item_id: HirItemId,
        parent_scope: ScopeId,
        owner_qualifier: Option<&str>,
    ) -> HirFlowDecl {
        let symbol = if let Some(owner_qualifier) = owner_qualifier {
            self.alloc_symbol_with_def(SymbolData {
                name: format!("{}.{}", owner_qualifier, decl.name.text),
                kind: SymbolKind::Flow,
                visibility: crate::Visibility::Public,
                defining_module: self.current_module,
                defining_item: Some(item_id),
                def: SymbolDef::Item { item: item_id },
                declared_type: None,
                definition_span: decl.name.span,
            })
        } else {
            self.item_symbol(item_id, &ast::Item::Flow(decl.clone()))
        };
        let scope = self
            .scopes
            .alloc(Some(parent_scope), ScopeOwner::Item(item_id), decl.span);
        let type_params = self.lower_type_params(&decl.type_params, scope, item_id);
        let params = self.lower_params(&decl.params, scope, item_id);
        let return_type = decl
            .return_type
            .as_ref()
            .map(|ty| self.lower_type(ty, scope, item_id));
        let effects = decl
            .declared_effects
            .as_ref()
            .map(|effects| self.lower_effect_row(effects, scope, item_id));
        let conformances = decl
            .conformances
            .iter()
            .map(|conformance| self.lower_declaration_conformance(conformance, scope, item_id))
            .collect();
        let body = self.lower_flow_body(&decl.body, scope, item_id);
        let body = if let Some(handler) = &decl.trailing_handler {
            self.wrap_flow_body_with_handler(body, handler, scope, item_id)
        } else {
            body
        };
        self.set_symbol_item(symbol, item_id);
        HirFlowDecl {
            symbol,
            type_params,
            params,
            return_type,
            effects,
            conformances,
            body,
            scope,
            span: decl.span,
        }
    }

    fn lower_declaration_conformance(
        &mut self,
        conformance: &ast::DeclarationConformance,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirDeclarationConformance {
        let target = match &conformance.target {
            ast::DeclarationConformanceTarget::Path(spec_ref) => {
                HirDeclarationConformanceTarget::Path(HirSpecRef {
                    spec_path: self.resolve_path(&spec_ref.spec_path, scope, true),
                    spec_args: spec_ref
                        .spec_args
                        .iter()
                        .map(|arg| self.lower_type(arg, scope, item_id))
                        .collect(),
                    span: spec_ref.span,
                })
            }
            ast::DeclarationConformanceTarget::InlineTraceSpec(expr) => {
                HirDeclarationConformanceTarget::InlineTraceSpec(
                    self.lower_spec_expr(expr, scope, item_id),
                )
            }
            ast::DeclarationConformanceTarget::Error(span) => {
                HirDeclarationConformanceTarget::Error { span: *span }
            }
        };
        HirDeclarationConformance {
            target,
            span: conformance.span,
        }
    }

    fn lower_flow_body(
        &mut self,
        body: &ast::FlowBody,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirFlowBody {
        match body {
            ast::FlowBody::Block(block) => {
                HirFlowBody::Block(self.lower_block(block, scope, item_id))
            }
            ast::FlowBody::Expr { expr, span } => {
                let block_id = self.lower_expr_body_block(expr, scope, item_id);
                let lowered_expr = self.hir.blocks[block_id]
                    .final_expr
                    .expect("expression body block should have a final expression");
                HirFlowBody::Expr {
                    expr: lowered_expr,
                    lowered_block: block_id,
                    span: *span,
                }
            }
        }
    }

    fn wrap_flow_body_with_handler(
        &mut self,
        body: HirFlowBody,
        handler: &ast::HandlerArg,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirFlowBody {
        let (body_expr, body_span) = match body {
            HirFlowBody::Block(block) => {
                let span = self.hir.blocks[block].span;
                (self.alloc_expr(HirExpr::Block(block), span), span)
            }
            HirFlowBody::Expr { expr, span, .. } => (expr, span),
        };
        let handler_expr = self.lower_handler_arg(handler, scope, item_id);
        let span = body_span.cover(handler.span());
        let handle_expr = self.alloc_expr(
            HirExpr::Handle {
                body: body_expr,
                handler: handler_expr,
                span,
            },
            span,
        );
        let block_id = self.hir.blocks.alloc(HirBlock {
            id: HirBlockId(0),
            stmts: Vec::new(),
            final_expr: None,
            scope: ScopeId(0),
            span,
        });
        let block_scope = self
            .scopes
            .alloc(Some(scope), ScopeOwner::Block(block_id), span);
        self.source_map.map_block(block_id, span);
        *self.hir.blocks.get_mut(block_id).unwrap() = HirBlock {
            id: block_id,
            stmts: Vec::new(),
            final_expr: Some(handle_expr),
            scope: block_scope,
            span,
        };
        HirFlowBody::Expr {
            expr: handle_expr,
            lowered_block: block_id,
            span,
        }
    }

    pub(super) fn lower_action_decl(
        &mut self,
        decl: &ast::EffectActionDecl,
        item_id: HirItemId,
        parent_scope: ScopeId,
        owner_qualifier: Option<&str>,
        owner_effect: Option<SymbolId>,
        action_index: usize,
    ) -> HirEffectActionDecl {
        let name = owner_qualifier
            .map(|owner| format!("{owner}.{}", decl.name.text))
            .unwrap_or_else(|| decl.name.text.clone());
        let symbol = self
            .action_symbols
            .get(&action_key(decl))
            .copied()
            .unwrap_or_else(|| {
                self.alloc_symbol_with_def(SymbolData {
                    name,
                    kind: SymbolKind::EffectAction,
                    visibility: crate::Visibility::Public,
                    defining_module: self.current_module,
                    defining_item: Some(item_id),
                    def: SymbolDef::Item { item: item_id },
                    declared_type: None,
                    definition_span: decl.name.span,
                })
            });
        self.set_symbol_item(symbol, item_id);
        if let Some(symbol_data) = self.symbols.get_mut(symbol) {
            symbol_data.def = SymbolDef::EffectAction {
                declaring_item: item_id,
                owner_effect,
                action_index: action_index.min(u32::MAX as usize) as u32,
            };
        }
        let scope = self
            .scopes
            .alloc(Some(parent_scope), ScopeOwner::Item(item_id), decl.span);
        let type_params = self.lower_type_params(&decl.type_params, scope, item_id);
        let mut type_param_symbols = type_params.iter().copied();
        let selector_params = decl
            .selector_params
            .iter()
            .map(|param| match param {
                ast::ActionSelectorParam::Type(_) => HirActionSelectorParam::Type {
                    symbol: type_param_symbols
                        .next()
                        .expect("action type selector lowered into a type parameter symbol"),
                },
            })
            .collect();
        let params = self.lower_params(&decl.params, scope, item_id);
        let return_type = self.lower_type(&decl.return_type, scope, item_id);
        HirEffectActionDecl {
            symbol,
            selector_params,
            type_params,
            params,
            return_type,
            scope,
            span: decl.span,
        }
    }
}

fn item_visibility(item: &ast::Item) -> ast::Visibility {
    match item {
        ast::Item::Alias(item) => item.visibility,
        ast::Item::Type(item) => item.visibility,
        ast::Item::Enum(item) => item.visibility,
        ast::Item::Spec(item) => item.visibility,
        ast::Item::Effect(item) => item.visibility,
        ast::Item::TopLevelLet(item) => item.visibility,
        ast::Item::Tool(item) => item.visibility,
        ast::Item::Agent(item) => item.visibility,
        ast::Item::Protocol(item) => item.visibility,
        ast::Item::Flow(item) => item.visibility,
        ast::Item::Impl(_) | ast::Item::Error(_) => ast::Visibility::Private,
    }
}

fn hir_visibility(visibility: ast::Visibility) -> crate::Visibility {
    match visibility {
        ast::Visibility::Private => crate::Visibility::Private,
        ast::Visibility::Public => crate::Visibility::Public,
    }
}
