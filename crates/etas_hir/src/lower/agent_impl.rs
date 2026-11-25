use etas_core::Span;
use etas_syntax::ast;

use super::program::{LowerCtx, SyntaxKey};
use crate::*;

fn method_key(span: Span) -> SyntaxKey {
    SyntaxKey {
        source: span.source,
        start: span.range.start.0,
        end: span.range.end.0,
    }
}

impl LowerCtx {
    pub(super) fn predeclare_agent_methods(
        &mut self,
        items: &[ast::AnnotatedItem],
        scope: ScopeId,
    ) {
        for item in items {
            match &item.item {
                ast::Item::Agent(agent) if matches!(agent.body, ast::AgentBody::Source(_)) => {
                    let owner = self.top_item_symbols[&super::binding::item_key(&item.item)];
                    self.predeclare_agent_method(
                        owner,
                        "run",
                        agent.name.span,
                        true,
                        Visibility::Public,
                    );
                }
                ast::Item::Impl(decl) => {
                    let (target, legacy) = match &decl.target {
                        ast::ImplTarget::Agent { target, specs, .. } => {
                            (target, Some(specs.is_empty()))
                        }
                        ast::ImplTarget::Inherent { target, .. } => (target, None),
                        ast::ImplTarget::SpecSatisfaction { self_type, .. } => {
                            let ast::TypeExpr::Path { path, .. } = self_type else {
                                continue;
                            };
                            (path, None)
                        }
                        ast::ImplTarget::Error { .. } => continue,
                    };
                    let path = self.resolve_path(target, scope, false);
                    let ResolveResult::Resolved(owner) = path.resolution else {
                        continue;
                    };
                    for item in &decl.items {
                        match (legacy, item) {
                            (Some(inherent), ast::ImplItem::Flow(flow)) => {
                                self.predeclare_agent_method(
                                    owner,
                                    &flow.name.text,
                                    flow.name.span,
                                    inherent,
                                    super::item::hir_visibility(flow.visibility),
                                );
                            }
                            (None, ast::ImplItem::Tool(tool)) => {
                                self.predeclare_member_tool(tool, Some(owner));
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    pub(super) fn predeclare_member_tool(
        &mut self,
        tool: &ast::ToolDecl,
        owner: Option<SymbolId>,
    ) -> SymbolId {
        let key = method_key(tool.span);
        if let Some(symbol) = self.method_symbols.get(&key) {
            return *symbol;
        }
        let name = tool
            .path
            .segments
            .last()
            .map_or("<error>", |name| name.text.as_str());
        let symbol = self.alloc_symbol_with_def(SymbolData {
            name: name.to_owned(),
            kind: SymbolKind::Tool,
            visibility: super::item::hir_visibility(tool.visibility),
            defining_module: self.current_module,
            defining_item: None,
            def: SymbolDef::Error,
            declared_type: None,
            definition_span: tool.path.span,
        });
        if let Some(owner) = owner {
            if let ResolveResult::Resolved(previous) = self.symbols.resolve_member(owner, name) {
                self.diagnostics.duplicate_symbol(
                    name,
                    tool.path.span,
                    self.symbols
                        .get(previous)
                        .expect("member symbol")
                        .definition_span,
                );
            }
            self.symbols.insert_member(owner, name.to_owned(), symbol);
        }
        self.method_symbols.insert(key, symbol);
        symbol
    }

    fn predeclare_agent_method(
        &mut self,
        owner: SymbolId,
        name: &str,
        span: Span,
        inherent: bool,
        visibility: Visibility,
    ) -> SymbolId {
        let key = method_key(span);
        if let Some(symbol) = self.method_symbols.get(&key) {
            return *symbol;
        }
        let symbol = self.alloc_symbol_with_def(SymbolData {
            name: name.to_owned(),
            kind: SymbolKind::Flow,
            visibility,
            defining_module: self.current_module,
            defining_item: None,
            def: SymbolDef::Error,
            declared_type: None,
            definition_span: span,
        });
        if inherent {
            self.symbols.insert_member(owner, name.to_owned(), symbol);
        }
        self.method_symbols.insert(key, symbol);
        symbol
    }

    pub(super) fn register_default_agent_method(
        &mut self,
        agent: SymbolId,
        body: HirItemId,
        span: Span,
    ) {
        let symbol = self.predeclare_agent_method(agent, "run", span, true, Visibility::Public);
        self.set_symbol_item(symbol, body);
        self.hir.agent_methods.insert(
            symbol,
            HirAgentMethod {
                agent,
                symbol,
                body,
                implementation: None,
            },
        );
    }

    pub(super) fn lower_agent_impl(
        &mut self,
        decl: &ast::ImplDecl,
        path: &ast::Path,
        specs: &[ast::ImplSpecRef],
        implementation: HirItemId,
        parent_scope: ScopeId,
        scope: ScopeId,
    ) -> HirImplDecl {
        let target = self.resolve_path(path, parent_scope, true);
        let inherent = specs.is_empty();
        let specs = specs
            .iter()
            .map(|spec| HirImplSpecRef {
                spec_path: self.resolve_path(&spec.spec_path, parent_scope, true),
                spec_args: spec
                    .spec_args
                    .iter()
                    .map(|arg| self.lower_type(arg, scope, implementation))
                    .collect(),
                span: spec.span,
            })
            .collect();
        let ResolveResult::Resolved(agent) = target.resolution else {
            self.map_item(implementation, decl.span);
            return HirImplDecl {
                type_params: Vec::new(),
                target: HirImplTarget::Agent { target, specs },
                items: vec![HirImplItem::Error { span: decl.span }],
                scope,
                span: decl.span,
            };
        };
        for item in &decl.items {
            if let ast::ImplItem::Flow(flow) = item {
                let symbol = self.predeclare_agent_method(
                    agent,
                    &flow.name.text,
                    flow.name.span,
                    inherent,
                    super::item::hir_visibility(flow.visibility),
                );
                self.insert_symbol(scope, &flow.name.text, symbol, flow.name.span);
            }
        }
        let mut items = Vec::new();
        for item in &decl.items {
            match item {
                ast::ImplItem::Flow(flow) => {
                    let symbol = self.predeclare_agent_method(
                        agent,
                        &flow.name.text,
                        flow.name.span,
                        inherent,
                        super::item::hir_visibility(flow.visibility),
                    );
                    let body = self.hir.items.alloc(HirItem::Error { span: flow.span });
                    let callable = self.lower_flow_with_symbol(flow, body, scope, symbol, None);
                    *self.hir.items.get_mut(body).expect("reserved method item") =
                        HirItem::Flow(callable);
                    self.map_item(body, flow.span);
                    self.hir.agent_methods.insert(
                        symbol,
                        HirAgentMethod {
                            agent,
                            symbol,
                            body,
                            implementation: Some(implementation),
                        },
                    );
                    items.push(HirImplItem::Callable { item: body });
                }
                ast::ImplItem::Tool(tool) => {
                    self.diagnostics.invalid_impl_target(tool.span);
                    items.push(HirImplItem::Error { span: tool.span });
                }
                ast::ImplItem::Action(action) => {
                    self.diagnostics.invalid_impl_target(action.span);
                    items.push(HirImplItem::Error { span: action.span });
                }
                ast::ImplItem::Error(span) => items.push(HirImplItem::Error { span: *span }),
            }
        }
        self.map_item(implementation, decl.span);
        HirImplDecl {
            type_params: Vec::new(),
            target: HirImplTarget::Agent { target, specs },
            items,
            scope,
            span: decl.span,
        }
    }
}
