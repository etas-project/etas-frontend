use etas_core::{Diagnostic, LabelStyle, Severity, Span};

use crate::{
    HirArg, HirBlock, HirBlockId, HirDeclarationConformance, HirDeclarationConformanceTarget,
    HirElseBranch, HirExpr, HirExprId, HirGenericArg, HirHandlerArmId, HirImplTarget, HirItem,
    HirItemId, HirLambdaBody, HirMatchArmBody, HirNodeRef, HirOrigin, HirPat, HirPatId, HirProgram,
    HirSpecItem, HirStage, HirStmt, HirStmtId, HirToolBody, HirType, HirTypeDeclBody, HirTypeId,
    ResolveResult, ResolvedActionRef, ResolvedPath, SymbolDef, path_text,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HirDumpOptions {
    pub include_spans: bool,
    pub include_diagnostics: bool,
    pub include_symbols: bool,
    pub include_scopes: bool,
    pub include_source_map: bool,
}

impl Default for HirDumpOptions {
    fn default() -> Self {
        Self {
            include_spans: true,
            include_diagnostics: false,
            include_symbols: true,
            include_scopes: true,
            include_source_map: false,
        }
    }
}

pub fn dump_hir(program: &HirProgram, options: HirDumpOptions) -> String {
    let mut dump = Dump::new(program, options);
    dump.program();
    dump.finish()
}

struct Dump<'a> {
    program: &'a HirProgram,
    options: HirDumpOptions,
    output: String,
    indent: usize,
}

impl<'a> Dump<'a> {
    fn new(program: &'a HirProgram, options: HirDumpOptions) -> Self {
        Self {
            program,
            options,
            output: String::new(),
            indent: 0,
        }
    }

    fn finish(self) -> String {
        self.output
    }

    fn program(&mut self) {
        self.line("HirProgram", None);
        self.indented(|this| {
            if this.options.include_symbols {
                this.symbols();
            }
            if this.options.include_scopes {
                this.scopes();
            }
            this.modules();
            this.items();
            if this.options.include_source_map {
                this.source_map();
            }
            if this.options.include_diagnostics {
                this.diagnostics();
            }
        });
    }

    fn symbols(&mut self) {
        self.line("Symbols", None);
        self.indented(|this| {
            for symbol in this.program.symbols.iter() {
                let mut text = format!(
                    "s{} {:?} {}",
                    symbol.id.0,
                    symbol.kind,
                    dump_atom(&symbol.name)
                );
                push_symbol_def_details(&mut text, &symbol.def);
                this.push_span(&mut text, symbol.definition_span);
                this.line(&text, None);
            }
        });
    }

    fn scopes(&mut self) {
        self.line("Scopes", None);
        self.indented(|this| {
            for scope in this.program.scopes.iter() {
                let parent = scope
                    .parent
                    .map(|id| format!(" parent=sc{}", id.0))
                    .unwrap_or_default();
                let mut text = format!("sc{} {:?}{parent}", scope.id.0, scope.owner);
                if !scope.symbols.is_empty() {
                    text.push_str(" symbols=[");
                    text.push_str(
                        &scope
                            .symbols
                            .iter()
                            .map(|symbol| format!("s{}", symbol.0))
                            .collect::<Vec<_>>()
                            .join(","),
                    );
                    text.push(']');
                }
                this.push_span(&mut text, scope.span);
                this.line(&text, None);
            }
        });
    }

    fn modules(&mut self) {
        self.line("Modules", None);
        self.indented(|this| {
            for module_id in &this.program.modules {
                let module = &this.program.modules_arena[*module_id];
                let mut text = format!("m{} scope=sc{}", module.id.0, module.scope.0);
                if let Some(name) = &module.name {
                    text.push_str(&format!(" name={}", path_text(&name.syntax_path)));
                }
                this.push_span(&mut text, module.span);
                this.line(&text, None);
                this.indented(|this| {
                    for import in &module.imports {
                        let binding = import
                            .binding
                            .as_ref()
                            .map(|binding| {
                                format!(
                                    " binding={} symbol=s{} alias={}",
                                    binding.local_name, binding.symbol.0, binding.is_alias
                                )
                            })
                            .unwrap_or_default();
                        let mut text = format!(
                            "Import kind={:?} visibility={:?} target={}{}",
                            import.kind,
                            import.visibility,
                            path_text(&import.target.syntax_path),
                            binding
                        );
                        this.push_span(&mut text, import.span);
                        this.line(&text, None);
                    }
                    for item in &module.items {
                        this.line(&format!("Item i{}", item.0), None);
                    }
                });
            }
        });
    }

    fn source_map(&mut self) {
        self.line("SourceMap", None);
        self.indented(|this| {
            let mut items = this
                .program
                .source_map
                .item_sources
                .iter()
                .map(|(id, source)| (id.0, "Item", source))
                .collect::<Vec<_>>();
            items.sort_by_key(|(id, _, _)| *id);
            for (id, kind, source) in items {
                let mut text = format!("{kind} i{id} {}", origin_label(source));
                this.push_span(&mut text, source.span());
                this.line(&text, None);
            }

            let mut blocks = this
                .program
                .source_map
                .block_sources
                .iter()
                .map(|(id, source)| (id.0, "Block", source))
                .collect::<Vec<_>>();
            blocks.sort_by_key(|(id, _, _)| *id);
            for (id, kind, source) in blocks {
                let mut text = format!("{kind} b{id} {}", origin_label(source));
                this.push_span(&mut text, source.span());
                this.line(&text, None);
            }

            let mut exprs = this
                .program
                .source_map
                .expr_sources
                .iter()
                .map(|(id, source)| (id.0, "Expr", source))
                .collect::<Vec<_>>();
            exprs.sort_by_key(|(id, _, _)| *id);
            for (id, kind, source) in exprs {
                let mut text = format!("{kind} e{id} {}", origin_label(source));
                this.push_span(&mut text, source.span());
                this.line(&text, None);
            }

            let mut handler_arms = this
                .program
                .source_map
                .handler_arm_sources
                .iter()
                .map(|(id, source)| (id.0, "HandlerArm", source))
                .collect::<Vec<_>>();
            handler_arms.sort_by_key(|(id, _, _)| *id);
            for (id, kind, source) in handler_arms {
                let mut text = format!("{kind} ha{id} {}", origin_label(source));
                this.push_span(&mut text, source.span());
                this.line(&text, None);
            }

            let mut stmts = this
                .program
                .source_map
                .stmt_sources
                .iter()
                .map(|(id, source)| (id.0, "Stmt", source))
                .collect::<Vec<_>>();
            stmts.sort_by_key(|(id, _, _)| *id);
            for (id, kind, source) in stmts {
                let mut text = format!("{kind} st{id} {}", origin_label(source));
                this.push_span(&mut text, source.span());
                this.line(&text, None);
            }

            let mut pats = this
                .program
                .source_map
                .pat_sources
                .iter()
                .map(|(id, source)| (id.0, "Pat", source))
                .collect::<Vec<_>>();
            pats.sort_by_key(|(id, _, _)| *id);
            for (id, kind, source) in pats {
                let mut text = format!("{kind} p{id} {}", origin_label(source));
                this.push_span(&mut text, source.span());
                this.line(&text, None);
            }

            let mut types = this
                .program
                .source_map
                .type_sources
                .iter()
                .map(|(id, source)| (id.0, "Type", source))
                .collect::<Vec<_>>();
            types.sort_by_key(|(id, _, _)| *id);
            for (id, kind, source) in types {
                let mut text = format!("{kind} t{id} {}", origin_label(source));
                this.push_span(&mut text, source.span());
                this.line(&text, None);
            }

            let mut symbols = this
                .program
                .source_map
                .symbol_sources
                .iter()
                .map(|(id, source)| (id.0, "Symbol", source))
                .collect::<Vec<_>>();
            symbols.sort_by_key(|(id, _, _)| *id);
            for (id, kind, source) in symbols {
                let mut text = format!("{kind} s{id} {}", origin_label(source));
                this.push_span(&mut text, source.span());
                this.line(&text, None);
            }

            let mut reverse = this
                .program
                .source_map
                .syntax_to_hir
                .iter()
                .map(|(id, nodes)| (id.0, nodes))
                .collect::<Vec<_>>();
            reverse.sort_by_key(|(id, _)| *id);
            for (id, nodes) in reverse {
                let nodes = nodes.iter().map(hir_node_ref).collect::<Vec<_>>().join(",");
                this.line(&format!("Syntax n{id} -> [{nodes}]"), None);
            }
        });
    }

    fn items(&mut self) {
        self.line("Items", None);
        self.indented(|this| {
            for (id, item) in this.program.items.iter() {
                this.annotations(id);
                this.item(id, item);
            }
        });
    }

    fn annotations(&mut self, id: HirItemId) {
        let Some(annotations) = self.program.item_annotations.get(&id) else {
            return;
        };
        for annotation in annotations {
            self.node(
                &format!("i{} Annotation {}", id.0, path_ref(&annotation.path)),
                annotation.span,
                |this| {
                    for arg in &annotation.args {
                        match arg {
                            crate::HirAnnotationArg::Positional { value, span } => {
                                this.node("PositionalArg", *span, |this| {
                                    this.expr_id("Value", *value);
                                });
                            }
                            crate::HirAnnotationArg::Named {
                                name, value, span, ..
                            } => {
                                this.node(&format!("NamedArg {name}"), *span, |this| {
                                    this.expr_id("Value", *value);
                                });
                            }
                        }
                    }
                },
            );
        }
    }

    fn item(&mut self, id: HirItemId, item: &HirItem) {
        match item {
            HirItem::TypeAlias(item) => {
                self.node(
                    &format!(
                        "i{} TypeAlias s{} scope=sc{}",
                        id.0, item.symbol.0, item.scope.0
                    ),
                    item.span,
                    |this| {
                        this.type_id("Target", item.target);
                    },
                );
            }
            HirItem::Type(item) => {
                self.node(
                    &format!("i{} Type s{} scope=sc{}", id.0, item.symbol.0, item.scope.0),
                    item.span,
                    |this| match item.body {
                        HirTypeDeclBody::Bodyless => {
                            this.line("Bodyless", Some(item.span));
                        }
                        HirTypeDeclBody::Representation(ty) => {
                            this.type_id("Representation", ty);
                        }
                    },
                );
            }
            HirItem::Enum(item) => {
                self.node(
                    &format!("i{} Enum s{} scope=sc{}", id.0, item.symbol.0, item.scope.0),
                    item.span,
                    |this| {
                        for variant in &item.variants {
                            this.line(
                                &format!("Variant s{}", variant.symbol.0),
                                Some(variant.span),
                            );
                        }
                    },
                );
            }
            HirItem::Spec(item) => {
                self.node(
                    &format!(
                        "i{} Spec s{} scope=sc{} kind={}",
                        id.0,
                        item.symbol.0,
                        item.scope.0,
                        spec_kind_text(&item.kind)
                    ),
                    item.span,
                    |this| {
                        for bound in &item.bounds {
                            this.line(
                                &format!(
                                    "SuperSpec {} args={}",
                                    path_ref(&bound.path),
                                    bound.args.len()
                                ),
                                Some(bound.span),
                            );
                        }
                        if let Some(callable) = &item.callable {
                            this.node("SpecCallableSignature", callable.span, |this| {
                                this.type_id("Input", callable.input);
                                this.type_id("Output", callable.output);
                                if let Some(effects) = &callable.effects {
                                    this.line(
                                        &format!("EffectRow len={}", effects.effects.len()),
                                        Some(effects.span),
                                    );
                                }
                            });
                        }
                        if let Some(trace) = &item.trace {
                            this.spec_expr(trace);
                        }
                        for item in &item.items {
                            match item {
                                HirSpecItem::FlowSignature(signature) => {
                                    this.node(
                                        &format!(
                                            "FlowSignature s{} scope=sc{}",
                                            signature.symbol.0, signature.scope.0
                                        ),
                                        signature.span,
                                        |this| {
                                            if let Some(effects) = &signature.effects {
                                                this.line(
                                                    &format!(
                                                        "EffectRow len={}",
                                                        effects.effects.len()
                                                    ),
                                                    Some(effects.span),
                                                );
                                                if let Some(tail) = &effects.tail {
                                                    this.line(
                                                        &format!(
                                                            "EffectRowTail {}",
                                                            path_ref(tail)
                                                        ),
                                                        Some(tail.span),
                                                    );
                                                }
                                            }
                                        },
                                    );
                                }
                                HirSpecItem::Error { span } => {
                                    this.line("ErrorSpecItem", Some(*span));
                                }
                            }
                        }
                    },
                );
            }
            HirItem::Impl(item) => {
                let target = match &item.target {
                    HirImplTarget::Inherent { target, .. } => {
                        format!("target={}", path_ref(target))
                    }
                    HirImplTarget::SpecSatisfaction { specs, self_type } => format!(
                        "specs=[{}] self=t{}",
                        specs
                            .iter()
                            .map(|spec_ref| path_ref(&spec_ref.spec_path))
                            .collect::<Vec<_>>()
                            .join(","),
                        self_type.0
                    ),
                    HirImplTarget::Error => "target=<error>".to_string(),
                };
                self.node(
                    &format!("i{} Impl scope=sc{} {}", id.0, item.scope.0, target),
                    item.span,
                    |this| {
                        for item in &item.items {
                            this.line(
                                &format!("ImplItem {:?}", std::mem::discriminant(item)),
                                None,
                            );
                        }
                    },
                );
            }
            HirItem::Effect(item) => {
                self.node(
                    &format!(
                        "i{} Effect s{} scope=sc{}",
                        id.0, item.symbol.0, item.scope.0
                    ),
                    item.span,
                    |this| {
                        for action in item.body.actions() {
                            this.line(
                                &format!("Action s{} scope=sc{}", action.symbol.0, action.scope.0),
                                Some(action.span),
                            );
                        }
                    },
                );
            }
            HirItem::TopLevelLet(item) => {
                self.node(
                    &format!(
                        "i{} TopLevelLet s{} visibility={:?} classification={:?}",
                        id.0, item.symbol.0, item.visibility, item.classification
                    ),
                    item.span,
                    |this| {
                        if let Some(type_annotation) = item.type_annotation {
                            this.type_id("TypeAnnotation", type_annotation);
                        }
                        this.expr_id("Value", item.value);
                    },
                );
            }
            HirItem::Tool(item) => {
                self.node(
                    &format!("i{} Tool s{} scope=sc{}", id.0, item.symbol.0, item.scope.0),
                    item.span,
                    |this| {
                        for param in &item.type_params {
                            this.line(&format!("TypeParam s{}", param.0), None);
                        }
                        if let Some(row) = &item.effects {
                            this.line(
                                &format!("EffectRow len={}", row.effects.len()),
                                Some(row.span),
                            );
                            if let Some(tail) = &row.tail {
                                this.line(
                                    &format!("EffectRowTail {}", path_ref(tail)),
                                    Some(tail.span),
                                );
                            }
                        }
                        for conformance in &item.conformances {
                            this.declaration_conformance(conformance);
                        }
                        match &item.body {
                            HirToolBody::Source(body) => {
                                this.node("ToolSourceBody", item.span, |this| {
                                    this.block_id("Body", body.block());
                                });
                            }
                            HirToolBody::Decl { span } => this.line("ToolDeclBody", Some(*span)),
                            HirToolBody::Error { span } => this.line("ErrorToolBody", Some(*span)),
                        }
                    },
                );
            }
            HirItem::Agent(item) => {
                self.node(
                    &format!(
                        "i{} Agent s{} scope=sc{}",
                        id.0, item.symbol.0, item.scope.0
                    ),
                    item.span,
                    |this| {
                        if let Some(output_type) = item.output_type {
                            this.type_id("Output", output_type);
                        }
                        if let Some(row) = &item.effects {
                            this.line(
                                &format!("EffectRow len={}", row.effects.len()),
                                Some(row.span),
                            );
                            if let Some(tail) = &row.tail {
                                this.line(
                                    &format!("EffectRowTail {}", path_ref(tail)),
                                    Some(tail.span),
                                );
                            }
                        }
                        for conformance in &item.conformances {
                            this.declaration_conformance(conformance);
                        }
                        match item.body {
                            crate::HirAgentBody::Source { block } => this.block_id("Body", block),
                            crate::HirAgentBody::Decl { span } => {
                                this.line("AgentDeclBody", Some(span));
                            }
                            crate::HirAgentBody::Error { span } => {
                                this.line("ErrorAgentBody", Some(span));
                            }
                        }
                    },
                );
            }
            HirItem::Protocol(item) => {
                self.node(
                    &format!(
                        "i{} Protocol s{} scope=sc{}",
                        id.0, item.symbol.0, item.scope.0
                    ),
                    item.span,
                    |this| {
                        for message in &item.messages {
                            this.line(
                                &format!(
                                    "Message from={} to={}",
                                    path_ref(&message.from),
                                    path_ref(&message.to)
                                ),
                                Some(message.span),
                            );
                        }
                    },
                );
            }
            HirItem::Flow(item) => {
                let params = item
                    .params
                    .iter()
                    .map(|symbol| format!("s{}", symbol.0))
                    .collect::<Vec<_>>()
                    .join(",");
                self.node(
                    &format!(
                        "i{} Flow s{} scope=sc{} params=[{}]",
                        id.0, item.symbol.0, item.scope.0, params
                    ),
                    item.span,
                    |this| {
                        for conformance in &item.conformances {
                            this.declaration_conformance(conformance);
                        }
                        if let Some(return_type) = item.return_type {
                            this.type_id("Return", return_type);
                        }
                        if let Some(row) = &item.effects {
                            this.line(
                                &format!("EffectRow len={}", row.effects.len()),
                                Some(row.span),
                            );
                        }
                        this.block_id("Body", item.body.block());
                    },
                );
            }
            HirItem::Error { span } => self.line(&format!("i{} Error", id.0), Some(*span)),
        }
    }

    fn block_id(&mut self, label: &str, id: HirBlockId) {
        let block = &self.program.blocks[id];
        self.node(
            &format!("{label} b{} scope=sc{}", id.0, block.scope.0),
            block.span,
            |this| {
                this.block(block);
            },
        );
    }

    fn block(&mut self, block: &HirBlock) {
        for stmt in &block.stmts {
            self.stmt_id(*stmt);
        }
        if let Some(expr) = block.final_expr {
            self.expr_id("Final", expr);
        }
    }

    fn stmt_id(&mut self, id: HirStmtId) {
        let stmt = &self.program.stmts[id];
        match stmt {
            HirStmt::Let {
                pat,
                type_annotation,
                value,
                span,
            } => self.node(&format!("st{} Let", id.0), *span, |this| {
                this.pat_id("Pat", *pat);
                if let Some(ty) = type_annotation {
                    this.type_id("Type", *ty);
                }
                this.expr_id("Value", *value);
            }),
            HirStmt::Var {
                pat,
                type_annotation,
                value,
                span,
            } => self.node(&format!("st{} Var", id.0), *span, |this| {
                this.pat_id("Pat", *pat);
                if let Some(ty) = type_annotation {
                    this.type_id("Type", *ty);
                }
                this.expr_id("Value", *value);
            }),
            HirStmt::Assign {
                target,
                value,
                span,
            } => self.node(&format!("st{} Assign", id.0), *span, |this| {
                this.expr_id("Target", *target);
                this.expr_id("Value", *value);
            }),
            HirStmt::If(expr) => self.expr_id(&format!("st{} If", id.0), *expr),
            HirStmt::Match(expr) => self.expr_id(&format!("st{} Match", id.0), *expr),
            HirStmt::For {
                pat,
                iter,
                limits,
                body,
                span,
            } => self.node(&format!("st{} For", id.0), *span, |this| {
                this.pat_id("Pat", *pat);
                this.expr_id("Iter", *iter);
                for limit in limits {
                    this.expr_id("Limit", *limit);
                }
                this.block_id("Body", *body);
            }),
            HirStmt::While {
                cond,
                limits,
                body,
                span,
            } => self.node(&format!("st{} While", id.0), *span, |this| {
                this.expr_id("Cond", *cond);
                for limit in limits {
                    this.expr_id("Limit", *limit);
                }
                this.block_id("Body", *body);
            }),
            HirStmt::Retry { limits, body, span } => {
                self.node(&format!("st{} Retry", id.0), *span, |this| {
                    for limit in limits {
                        this.expr_id("Limit", *limit);
                    }
                    this.block_id("Body", *body);
                })
            }
            HirStmt::Resume { value, span } => {
                self.node(&format!("st{} Resume", id.0), *span, |this| {
                    if let Some(value) = value {
                        this.expr_id("Value", *value);
                    }
                })
            }
            HirStmt::Finish { value, span } => {
                self.node(&format!("st{} Finish", id.0), *span, |this| {
                    this.expr_id("Value", *value);
                })
            }
            HirStmt::Return { value, span } => {
                self.node(&format!("st{} Return", id.0), *span, |this| {
                    if let Some(value) = value {
                        this.expr_id("Value", *value);
                    }
                })
            }
            HirStmt::Break { span } => self.line(&format!("st{} Break", id.0), Some(*span)),
            HirStmt::Continue { span } => self.line(&format!("st{} Continue", id.0), Some(*span)),
            HirStmt::Expr { expr, span } => self.node(&format!("st{} Expr", id.0), *span, |this| {
                this.expr_id("Expr", *expr);
            }),
            HirStmt::Error { span } => self.line(&format!("st{} Error", id.0), Some(*span)),
        }
    }

    fn expr_id(&mut self, label: &str, id: HirExprId) {
        let expr = &self.program.exprs[id];
        self.node(
            &format!("{label} e{}", id.0),
            expr_span(expr, self.program),
            |this| {
                this.expr(expr);
            },
        );
    }

    fn expr(&mut self, expr: &HirExpr) {
        match expr {
            HirExpr::Literal(lit) => self.line(&format!("Literal {:?}", lit), None),
            HirExpr::Path(path) => self.line(&format!("Path {}", path_ref(path)), Some(path.span)),
            HirExpr::Record(record) => {
                if let Some(path) = &record.path {
                    self.line(
                        &format!("Record path={}", path_ref(path)),
                        Some(record.span),
                    );
                } else {
                    self.line("Record", Some(record.span));
                }
            }
            HirExpr::EmptyRecordOrMap { .. } => self.line("EmptyRecordOrMap", None),
            HirExpr::Tuple { elems, .. } => self.expr_list("Tuple", elems),
            HirExpr::Array { elems, .. } => self.expr_list("Array", elems),
            HirExpr::List { elems, .. } => self.expr_list("List", elems),
            HirExpr::ListCons { head, tail, .. } => {
                self.line("ListCons", None);
                self.indented(|this| {
                    this.expr_id("Head", *head);
                    this.expr_id("Tail", *tail);
                });
            }
            HirExpr::EmptySequence { .. } => self.line("EmptySequence", None),
            HirExpr::Map { entries, .. } => {
                self.line("Map", None);
                self.indented(|this| {
                    for entry in entries {
                        this.line("MapEntry", Some(entry.span));
                        this.indented(|this| {
                            this.expr_id("Key", entry.key);
                            this.expr_id("Value", entry.value);
                        });
                    }
                });
            }
            HirExpr::Set { elems, .. } => self.expr_list("Set", elems),
            HirExpr::Range {
                start, end, bounds, ..
            } => {
                self.line(&format!("Range bounds={bounds:?}"), None);
                self.expr_id("Start", *start);
                self.expr_id("End", *end);
            }
            HirExpr::Call {
                callee,
                generic_args,
                args,
                ..
            } => {
                self.expr_id("Callee", *callee);
                for arg in generic_args {
                    self.generic_arg(arg);
                }
                for arg in args {
                    self.arg(arg);
                }
            }
            HirExpr::MethodCall {
                receiver,
                method,
                generic_args,
                args,
                ..
            } => {
                self.line(&format!("Method {}", dump_atom(method)), None);
                self.expr_id("Receiver", *receiver);
                for arg in generic_args {
                    self.generic_arg(arg);
                }
                for arg in args {
                    self.arg(arg);
                }
            }
            HirExpr::SpecMethodCall {
                receiver,
                spec_path,
                spec_args,
                method,
                args,
                ..
            } => {
                self.line(
                    &format!(
                        "SpecMethod {} spec={}",
                        dump_atom(method),
                        path_ref(spec_path)
                    ),
                    None,
                );
                self.expr_id("Receiver", *receiver);
                for arg in spec_args {
                    self.type_id("SpecTypeArg", *arg);
                }
                for arg in args {
                    self.arg(arg);
                }
            }
            HirExpr::Perform {
                action,
                generic_args,
                args,
                ..
            } => {
                self.action(action);
                for arg in generic_args {
                    self.generic_arg(arg);
                }
                for arg in args {
                    self.arg(arg);
                }
            }
            HirExpr::Handler { handlers, .. } => {
                self.line("HandlerValue", None);
                self.handler_arms(handlers);
            }
            HirExpr::Handle { body, handler, .. } => {
                self.expr_id("Body", *body);
                self.expr_id("Handler", *handler);
            }
            HirExpr::StageCompose { stages, .. } => self.stages(stages),
            HirExpr::Pipeline { input, stages, .. } => {
                self.expr_id("Input", *input);
                self.stages(stages);
            }
            HirExpr::Field { base, field, .. } => {
                self.line(&format!("Field {}", dump_atom(field)), None);
                self.expr_id("Base", *base);
            }
            HirExpr::Index { base, index, .. } => {
                self.expr_id("Base", *base);
                self.expr_id("Index", *index);
            }
            HirExpr::Slice {
                base,
                start,
                end,
                bounds,
                ..
            } => {
                self.line(&format!("Slice bounds={bounds:?}"), None);
                self.expr_id("Base", *base);
                self.expr_id("Start", *start);
                self.expr_id("End", *end);
            }
            HirExpr::Try { expr, .. } => {
                self.line("Try", None);
                self.expr_id("Expr", *expr);
            }
            HirExpr::Unary { op, expr, .. } => {
                self.line(&format!("Op {:?}", op), None);
                self.expr_id("Expr", *expr);
            }
            HirExpr::Binary { op, lhs, rhs, .. } => {
                self.line(&format!("Op {:?}", op), None);
                self.expr_id("Lhs", *lhs);
                self.expr_id("Rhs", *rhs);
            }
            HirExpr::If {
                cond,
                then_block,
                else_branch,
                ..
            } => {
                self.expr_id("Cond", *cond);
                self.block_id("Then", *then_block);
                if let Some(else_branch) = else_branch {
                    match else_branch {
                        HirElseBranch::If(expr) => self.expr_id("ElseIf", *expr),
                        HirElseBranch::Block(block) => self.block_id("Else", *block),
                    }
                }
            }
            HirExpr::Match {
                scrutinee, arms, ..
            } => {
                self.expr_id("Scrutinee", *scrutinee);
                for arm in arms {
                    self.line(&format!("Arm scope=sc{}", arm.scope.0), Some(arm.span));
                    self.indented(|this| {
                        this.pat_id("Pat", arm.pat);
                        match arm.body {
                            HirMatchArmBody::Expr(expr) => this.expr_id("Body", expr),
                            HirMatchArmBody::Block(block) => this.block_id("Body", block),
                        }
                    });
                }
            }
            HirExpr::Lambda {
                params,
                body,
                scope,
                ..
            } => {
                self.line(
                    &format!(
                        "Lambda scope=sc{} params=[{}]",
                        scope.0,
                        params
                            .iter()
                            .map(|symbol| format!("s{}", symbol.0))
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                    None,
                );
                match body {
                    HirLambdaBody::Expr(expr) => self.expr_id("Body", *expr),
                    HirLambdaBody::Block(block) => self.block_id("Body", *block),
                }
            }
            HirExpr::Block(block) => self.block_id("Block", *block),
            HirExpr::Error { .. } => self.line("ErrorExpr", None),
        }
    }

    fn expr_list(&mut self, label: &str, elems: &[HirExprId]) {
        self.line(label, None);
        self.indented(|this| {
            for elem in elems {
                this.expr_id("Elem", *elem);
            }
        });
    }

    fn stages(&mut self, stages: &[HirStage]) {
        for stage in stages {
            self.line("Stage", Some(stage.span));
            self.indented(|this| {
                this.expr_id("Expr", stage.expr);
                for limit in &stage.limits {
                    this.expr_id("Limit", *limit);
                }
            });
        }
    }

    fn handler_arms(&mut self, handlers: &[HirHandlerArmId]) {
        for handler_id in handlers {
            let handler = &self.program.handler_arms[*handler_id];
            self.node(
                &format!(
                    "Handler ha{} scope=sc{} {}",
                    handler_id.0,
                    handler.scope.0,
                    action_ref(&handler.action)
                ),
                handler.span,
                |this| {
                    for arg in &handler.generic_args {
                        this.generic_arg(arg);
                    }
                    this.block_id("Body", handler.body);
                },
            );
        }
    }

    fn arg(&mut self, arg: &HirArg) {
        match arg {
            HirArg::Positional(expr) => self.expr_id("Arg", *expr),
            HirArg::Named { name, value, span } => {
                self.node(&format!("NamedArg {}", dump_atom(name)), *span, |this| {
                    this.expr_id("Value", *value);
                });
            }
        }
    }

    fn pat_id(&mut self, label: &str, id: HirPatId) {
        let pat = &self.program.pats[id];
        match pat {
            HirPat::Binding { symbol, span } => {
                self.line(
                    &format!("{label} p{} Binding s{}", id.0, symbol.0),
                    Some(*span),
                );
            }
            HirPat::Wildcard { span } => {
                self.line(&format!("{label} p{} Wildcard", id.0), Some(*span))
            }
            HirPat::Literal(literal) => self.line(
                &format!("{label} p{} Literal {:?}", id.0, literal),
                Some(literal.span()),
            ),
            HirPat::Tuple { elems, span } => {
                self.node(&format!("{label} p{} Tuple", id.0), *span, |this| {
                    for elem in elems {
                        this.pat_id("Elem", *elem);
                    }
                })
            }
            HirPat::Record { path, fields, span } => {
                self.node(&format!("{label} p{} Record", id.0), *span, |this| {
                    if let Some(path) = path {
                        this.line(&format!("Path {}", path_ref(path)), Some(path.span));
                    }
                    for field in fields {
                        this.line(
                            &format!("Field {}", dump_atom(&field.name)),
                            Some(field.span),
                        );
                        if let Some(pat) = field.pat {
                            this.pat_id("Pat", pat);
                        }
                    }
                })
            }
            HirPat::Variant { path, args, span } => self.node(
                &format!("{label} p{} Variant {}", id.0, path_ref(path)),
                *span,
                |this| {
                    for arg in args {
                        this.pat_id("Arg", *arg);
                    }
                },
            ),
            HirPat::Error { span } => self.line(&format!("{label} p{} Error", id.0), Some(*span)),
        }
    }

    fn type_id(&mut self, label: &str, id: HirTypeId) {
        let ty = &self.program.types[id];
        self.node(&format!("{label} t{}", id.0), ty.span(), |this| {
            this.ty(ty);
        });
    }

    fn generic_arg(&mut self, arg: &HirGenericArg) {
        match arg {
            HirGenericArg::Type(ty) => self.type_id("GenericTypeArg", *ty),
            HirGenericArg::EffectRow(row) => {
                self.node(
                    &format!("GenericEffectRowArg len={}", row.effects.len()),
                    row.span,
                    |this| {
                        for effect in &row.effects {
                            this.line(
                                &format!("Effect {}", path_ref(&effect.path)),
                                Some(effect.span),
                            );
                        }
                        if let Some(tail) = &row.tail {
                            this.line(
                                &format!("EffectRowTail {}", path_ref(tail)),
                                Some(tail.span),
                            );
                        }
                    },
                );
            }
            HirGenericArg::Wildcard { span } => {
                self.line("GenericWildcardArg", Some(*span));
            }
        }
    }

    fn ty(&mut self, ty: &HirType) {
        match ty {
            HirType::Handler {
                handled,
                produced,
                result,
                ..
            } => {
                self.line(
                    &format!("HandlerType handled={}", handled.effects.len()),
                    Some(handled.span),
                );
                match produced {
                    crate::HirHandlerProducedEffects::Infer => {
                        self.line("ProducedEffects Infer", None);
                    }
                    crate::HirHandlerProducedEffects::Explicit(produced) => {
                        self.line(
                            &format!("ProducedEffects len={}", produced.effects.len()),
                            Some(produced.span),
                        );
                    }
                }
                if let Some(result) = result {
                    self.type_id("Result", *result);
                }
            }
            HirType::Arrow {
                effect,
                input,
                output,
                ..
            } => {
                if let Some(effect) = effect {
                    self.line(
                        &format!("EffectRow len={}", effect.effects.len()),
                        Some(effect.span),
                    );
                }
                self.type_id("Input", *input);
                self.type_id("Output", *output);
            }
            HirType::Primitive { kind, .. } => self.line(&format!("Primitive {:?}", kind), None),
            HirType::Path { path, args, .. } => {
                self.line(&format!("Path {}", path_ref(path)), Some(path.span));
                for arg in args {
                    self.type_id("TypeArg", *arg);
                }
            }
            HirType::Record { fields, .. } => {
                for field in fields {
                    self.line(&format!("Field s{}", field.symbol.0), Some(field.span));
                    self.type_id("Type", field.ty);
                }
            }
            HirType::Tuple { elems, .. } => {
                for elem in elems {
                    self.type_id("Elem", *elem);
                }
            }
            HirType::Refined {
                base, predicate, ..
            } => {
                self.type_id("Base", *base);
                self.expr_id("Predicate", *predicate);
            }
            HirType::Error { .. } => self.line("ErrorType", None),
        }
    }

    fn declaration_conformance(&mut self, conformance: &HirDeclarationConformance) {
        match &conformance.target {
            HirDeclarationConformanceTarget::Path(spec_ref) => {
                self.line(
                    &format!(
                        "Conformance {} args={}",
                        path_ref(&spec_ref.spec_path),
                        spec_ref.spec_args.len()
                    ),
                    Some(conformance.span),
                );
            }
            HirDeclarationConformanceTarget::InlineTraceSpec(expr) => {
                self.line("InlineTraceSpecConformance", Some(conformance.span));
                self.indented(|this| this.spec_expr(expr));
            }
            HirDeclarationConformanceTarget::Error { span } => {
                self.line("ErrorDeclarationConformance", Some(*span));
            }
        }
    }

    fn spec_expr(&mut self, expr: &crate::HirSpecExpr) {
        match expr {
            crate::HirSpecExpr::Atom(pattern) => {
                self.effect_pattern("SpecAtom", pattern);
            }
            crate::HirSpecExpr::Allow { pattern, span } => {
                self.node("TraceAllow", *span, |this| {
                    this.effect_pattern("Pattern", pattern)
                });
            }
            crate::HirSpecExpr::Deny { pattern, span } => {
                self.node("TraceDeny", *span, |this| {
                    this.effect_pattern("Pattern", pattern)
                });
            }
            crate::HirSpecExpr::And { lhs, rhs, span } => {
                self.node("TraceAnd", *span, |this| {
                    this.spec_expr(lhs);
                    this.spec_expr(rhs);
                });
            }
            crate::HirSpecExpr::Or { lhs, rhs, span } => {
                self.node("TraceOr", *span, |this| {
                    this.spec_expr(lhs);
                    this.spec_expr(rhs);
                });
            }
            crate::HirSpecExpr::Before {
                before,
                after,
                span,
            } => {
                self.node("TraceBefore", *span, |this| {
                    this.spec_expr(before);
                    this.spec_expr(after);
                });
            }
            crate::HirSpecExpr::After {
                after,
                before,
                span,
            } => {
                self.node("TraceAfter", *span, |this| {
                    this.spec_expr(after);
                    this.spec_expr(before);
                });
            }
        }
    }

    fn effect_pattern(&mut self, label: &str, pattern: &crate::HirEffectRef) {
        self.line(
            &format!(
                "{label} {} args={}",
                path_ref(&pattern.path),
                pattern.args.len()
            ),
            Some(pattern.span),
        );
    }

    fn action(&mut self, action: &ResolvedActionRef) {
        self.line(&format!("Action {}", action_ref(action)), Some(action.span));
    }

    fn diagnostics(&mut self) {
        if self.program.diagnostics.is_empty() {
            return;
        }
        self.line("Diagnostics", None);
        self.indented(|this| {
            for diagnostic in &this.program.diagnostics {
                this.diagnostic(diagnostic);
            }
        });
    }

    fn diagnostic(&mut self, diagnostic: &Diagnostic) {
        let mut text = format!(
            "Diagnostic code={:?} severity={}",
            diagnostic.code,
            severity_text(diagnostic.severity)
        );
        self.push_span(&mut text, diagnostic.primary.span);
        self.line(&text, None);
        self.indented(|this| {
            for label in &diagnostic.labels {
                let mut text = format!("Label style={}", label_style_text(label.style));
                this.push_span(&mut text, label.span);
                this.line(&text, None);
            }
        });
    }

    fn node(&mut self, text: &str, span: Span, body: impl FnOnce(&mut Self)) {
        self.line(text, Some(span));
        self.indented(body);
    }

    fn indented(&mut self, body: impl FnOnce(&mut Self)) {
        self.indent += 1;
        body(self);
        self.indent -= 1;
    }

    fn line(&mut self, text: &str, span: Option<Span>) {
        for _ in 0..self.indent {
            self.output.push_str("  ");
        }
        self.output.push_str(text);
        if let Some(span) = span {
            self.push_span_to_output(span);
        }
        self.output.push('\n');
    }

    fn push_span(&self, text: &mut String, span: Span) {
        if self.options.include_spans {
            text.push_str(&format!(
                " @{}..{}",
                span.range.start.to_usize(),
                span.range.end.to_usize()
            ));
        }
    }

    fn push_span_to_output(&mut self, span: Span) {
        if self.options.include_spans {
            self.output.push_str(&format!(
                " @{}..{}",
                span.range.start.to_usize(),
                span.range.end.to_usize()
            ));
        }
    }
}

fn expr_span(expr: &HirExpr, program: &HirProgram) -> Span {
    match expr {
        HirExpr::Literal(lit) => lit.span(),
        HirExpr::Path(path) => path.span,
        HirExpr::Record(record) => record.span,
        HirExpr::Tuple { span, .. }
        | HirExpr::Array { span, .. }
        | HirExpr::List { span, .. }
        | HirExpr::ListCons { span, .. }
        | HirExpr::EmptySequence { span }
        | HirExpr::EmptyRecordOrMap { span }
        | HirExpr::Map { span, .. }
        | HirExpr::Set { span, .. }
        | HirExpr::Range { span, .. }
        | HirExpr::Call { span, .. }
        | HirExpr::MethodCall { span, .. }
        | HirExpr::SpecMethodCall { span, .. }
        | HirExpr::Perform { span, .. }
        | HirExpr::Handler { span, .. }
        | HirExpr::Handle { span, .. }
        | HirExpr::StageCompose { span, .. }
        | HirExpr::Pipeline { span, .. }
        | HirExpr::Field { span, .. }
        | HirExpr::Index { span, .. }
        | HirExpr::Slice { span, .. }
        | HirExpr::Try { span, .. }
        | HirExpr::Unary { span, .. }
        | HirExpr::Binary { span, .. }
        | HirExpr::If { span, .. }
        | HirExpr::Match { span, .. }
        | HirExpr::Lambda { span, .. }
        | HirExpr::Error { span } => *span,
        HirExpr::Block(block) => program.blocks[*block].span,
    }
}

fn path_ref(path: &ResolvedPath) -> String {
    format!(
        "{}({})",
        path_text(&path.syntax_path),
        resolution_text(&path.resolution)
    )
}

fn action_ref(action: &ResolvedActionRef) -> String {
    format!(
        "{}.{}({})",
        path_text(&action.effect.path.syntax_path),
        action.action,
        resolution_text(&action.action_symbol)
    )
}

fn origin_label(origin: &HirOrigin) -> String {
    match origin {
        HirOrigin::Direct(source) => format!("Direct(n{}:{:?})", source.id.0, source.kind),
        HirOrigin::Desugared {
            primary,
            desugaring,
            ..
        } => {
            format!("Desugared(n{}:{desugaring:?})", primary.id.0)
        }
        HirOrigin::Synthetic { reason, .. } => format!("Synthetic({reason:?})"),
        HirOrigin::Error { .. } => "Error".to_owned(),
    }
}

fn hir_node_ref(node: &HirNodeRef) -> String {
    match node {
        HirNodeRef::Item(id) => format!("i{}", id.0),
        HirNodeRef::Expr(id) => format!("e{}", id.0),
        HirNodeRef::HandlerArm(id) => format!("ha{}", id.0),
        HirNodeRef::Stmt(id) => format!("st{}", id.0),
        HirNodeRef::Pat(id) => format!("p{}", id.0),
        HirNodeRef::Type(id) => format!("t{}", id.0),
        HirNodeRef::Block(id) => format!("b{}", id.0),
        HirNodeRef::Symbol(id) => format!("s{}", id.0),
    }
}

fn resolution_text(resolution: &ResolveResult) -> String {
    match resolution {
        ResolveResult::Resolved(symbol) => format!("s{}", symbol.0),
        ResolveResult::PartiallyResolved(partial) => format!(
            "partial:s{}+{}:{:?}:{}",
            partial
                .resolved_prefix
                .map_or_else(|| "none".to_owned(), |symbol| symbol.0.to_string()),
            partial.resolved_segments,
            partial.reason,
            partial.remaining.join(".")
        ),
        ResolveResult::Unresolved => "unresolved".to_owned(),
        ResolveResult::Ambiguous(symbols) => format!(
            "ambiguous:[{}]",
            symbols
                .iter()
                .map(|symbol| format!("s{}", symbol.0))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn dump_atom(text: &str) -> String {
    if text
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
        && !text.is_empty()
    {
        text.to_owned()
    } else {
        format!("{text:?}")
    }
}

fn push_symbol_def_details(text: &mut String, def: &SymbolDef) {
    if let SymbolDef::ImportAlias { path, origin } = def {
        text.push_str(&format!(" origin={origin:?} target={}", path.join(".")));
    }
}

fn severity_text(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
        Severity::Hint => "hint",
    }
}

fn label_style_text(style: LabelStyle) -> &'static str {
    match style {
        LabelStyle::Primary => "primary",
        LabelStyle::Secondary => "secondary",
    }
}

fn spec_kind_text(kind: &crate::HirSpecKind) -> &'static str {
    match kind {
        crate::HirSpecKind::TypeSpec => "type",
        crate::HirSpecKind::CallableSpec => "callable",
        crate::HirSpecKind::TraceSpec => "trace",
    }
}
