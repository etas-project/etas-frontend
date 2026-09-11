use crate::{Diagnostic, Keyword, Punct, SyntaxDiagnosticCode, TokenKind, ast::*};

use crate::parser::Parser;

impl Parser<'_> {
    pub(crate) fn program(&mut self) -> Program {
        let start = self.cursor.peek().span;
        let module = self
            .cursor
            .at_keyword(Keyword::Module)
            .then(|| self.module_decl());
        let mut imports = Vec::new();
        while self.at_import_decl_start() {
            imports.push(self.import_decl());
        }

        let mut items = Vec::new();
        while !self.cursor.at_eof() {
            let before = self.cursor.position();
            items.push(self.annotated_item());
            self.ensure_progress(before, "expected top-level item");
        }

        let end = self.cursor.peek().span;
        Program {
            module,
            imports,
            items,
            span: start.cover(end),
        }
    }

    pub(crate) fn module_decl(&mut self) -> ModuleDecl {
        let start = self.expect_keyword(Keyword::Module, "expected `module`");
        let path = self.path();
        let end = self.expect_punct(Punct::Semi, "expected `;` after module declaration");
        ModuleDecl {
            path,
            span: start.cover(end),
        }
    }

    pub(crate) fn import_decl(&mut self) -> ImportDecl {
        let visibility_token = if self.cursor.at_keyword(Keyword::Public)
            || self.cursor.at_keyword(Keyword::Private)
        {
            Some(self.cursor.bump())
        } else {
            None
        };
        let visibility = match visibility_token.as_ref().map(|token| token.kind.clone()) {
            Some(TokenKind::Keyword(Keyword::Public)) => Visibility::Public,
            _ => Visibility::Private,
        };
        let import_span = self.expect_keyword(Keyword::Import, "expected `import`");
        let start = visibility_token
            .as_ref()
            .map_or(import_span, |token| token.span);
        let tree = self.import_tree();
        let end = self.expect_punct(Punct::Semi, "expected `;` after import declaration");
        ImportDecl {
            visibility,
            tree,
            span: start.cover(end),
        }
    }

    fn at_import_decl_start(&self) -> bool {
        self.cursor.at_keyword(Keyword::Import)
            || ((self.cursor.at_keyword(Keyword::Public)
                || self.cursor.at_keyword(Keyword::Private))
                && self.cursor.nth(1).kind == TokenKind::Keyword(Keyword::Import))
    }

    fn import_tree(&mut self) -> ImportTree {
        let path = self.import_prefix_path();
        if path
            .segments
            .iter()
            .any(|segment| segment.text == "<error>")
        {
            return ImportTree::Error { span: path.span };
        }

        if let Some(dot) = self.cursor.eat_punct(Punct::Dot) {
            if let Some(star) = self.cursor.eat_punct(Punct::Star) {
                return ImportTree::Wildcard {
                    span: path.span.cover(star.span),
                    prefix: path,
                    star_span: star.span,
                };
            }
            if self.cursor.at_punct(Punct::LBrace) {
                let open = self.expect_punct(Punct::LBrace, "expected `{` in grouped import");
                let mut items = Vec::new();
                while !self.cursor.at_eof() && !self.cursor.at_punct(Punct::RBrace) {
                    let before = self.cursor.position();
                    if self.cursor.at_punct(Punct::Comma) {
                        self.cursor.bump();
                        continue;
                    }
                    items.push(self.import_item());
                    if self.cursor.eat_punct(Punct::Comma).is_none() {
                        break;
                    }
                    self.ensure_progress(before, "expected grouped import member");
                }
                let close = self.expect_punct(Punct::RBrace, "expected `}` after grouped import");
                return ImportTree::Group {
                    span: path.span.cover(dot.span).cover(open).cover(close),
                    prefix: path,
                    items,
                };
            }

            self.diagnostics.push(Diagnostic::syntax(
                SyntaxDiagnosticCode::UnexpectedToken,
                dot.span,
                "expected `*` or `{` after `.` in import tail",
            ));
            return ImportTree::Error {
                span: path.span.cover(dot.span),
            };
        }

        let alias = self
            .cursor
            .eat_keyword(Keyword::As)
            .map(|_| self.name("expected import alias"));
        let span = alias
            .as_ref()
            .map_or(path.span, |alias| path.span.cover(alias.span));
        ImportTree::Single { path, alias, span }
    }

    fn import_prefix_path(&mut self) -> Path {
        let first = self.name("expected import path segment");
        let mut span = first.span;
        let mut segments = vec![first];
        while self.cursor.at_punct(Punct::Dot) && self.cursor.nth(1).kind.is_ident_like() {
            self.cursor.bump();
            let segment = self.name("expected import path segment after `.`");
            span = span.cover(segment.span);
            segments.push(segment);
        }
        Path { segments, span }
    }

    fn import_item(&mut self) -> ImportItem {
        let name = self.name("expected grouped import member");
        let alias = self
            .cursor
            .eat_keyword(Keyword::As)
            .map(|_| self.name("expected grouped import alias"));
        let span = alias
            .as_ref()
            .map_or(name.span, |alias| name.span.cover(alias.span));
        ImportItem { name, alias, span }
    }

    pub(crate) fn annotated_item(&mut self) -> AnnotatedItem {
        let mut annotations = Vec::new();
        while self.cursor.at_punct(Punct::At) {
            annotations.push(self.annotation());
        }
        let item = self.item();
        let span = annotations
            .first()
            .map_or(item.span(), |annotation| annotation.span.cover(item.span()));
        AnnotatedItem {
            annotations,
            item,
            span,
        }
    }

    fn annotation(&mut self) -> Annotation {
        let start = self.expect_punct(Punct::At, "expected `@` before annotation");
        let path = self.path();
        let mut span = start.cover(path.span);
        let args = if self.cursor.at_punct(Punct::LParen) {
            let open = self.expect_punct(Punct::LParen, "expected annotation argument list");
            let args = self.comma_list(Punct::RParen, |this| this.annotation_arg());
            let close = self.expect_punct(Punct::RParen, "expected `)` after annotation arguments");
            span = span.cover(open).cover(close);
            args
        } else {
            Vec::new()
        };
        Annotation { path, args, span }
    }

    fn annotation_arg(&mut self) -> AnnotationArg {
        if self.cursor.at_ident_like() && self.cursor.nth(1).kind == TokenKind::Punct(Punct::Eq) {
            let name = self.name("expected annotation argument name");
            self.expect_punct(Punct::Eq, "expected `=` after annotation argument name");
            let value = self.expr();
            let span = name.span.cover(value.span());
            return AnnotationArg::Named { name, value, span };
        }
        AnnotationArg::Positional(self.expr())
    }

    pub(crate) fn item(&mut self) -> Item {
        let visibility = self.item_visibility();
        if self.cursor.at_keyword(Keyword::Alias) {
            return Item::Alias(self.type_alias_decl(visibility));
        }
        if self.cursor.at_keyword(Keyword::Type) {
            return Item::Type(self.type_decl(visibility));
        }
        if self.cursor.at_keyword(Keyword::Enum) {
            return Item::Enum(self.enum_decl(visibility));
        }
        if self.cursor.at_keyword(Keyword::Spec) {
            return Item::Spec(self.spec_decl(visibility));
        }
        if self.cursor.at_keyword(Keyword::Impl) {
            return Item::Impl(self.impl_decl());
        }
        if self.cursor.at_keyword(Keyword::Effect) {
            return Item::Effect(self.effect_decl(visibility));
        }
        if self.cursor.at_keyword(Keyword::Let) {
            return Item::TopLevelLet(self.top_level_let_decl(visibility));
        }
        if self.cursor.at_keyword(Keyword::Var) {
            return self.invalid_top_level_var_item();
        }
        if self.cursor.at_keyword(Keyword::Tool) {
            return Item::Tool(self.tool_decl(visibility));
        }
        if self.cursor.at_keyword(Keyword::Agent) {
            return Item::Agent(self.agent_decl(visibility));
        }
        if self.cursor.at_keyword(Keyword::Policy) {
            return self.obsolete_policy_item();
        }
        if self.cursor.at_keyword(Keyword::Protocol) {
            return Item::Protocol(self.protocol_decl(visibility));
        }
        if self.cursor.at_keyword(Keyword::Flow) {
            return Item::Flow(self.flow_decl(visibility));
        }
        if self.cursor.at_ident_like() && self.slice(self.cursor.peek().span) == "memory" {
            return self.obsolete_memory_item();
        }

        let start = self.cursor.peek().span;
        self.diagnostics.push(Diagnostic::syntax(
            SyntaxDiagnosticCode::InvalidItem,
            start,
            "expected top-level item",
        ));
        self.recover_item();
        Item::Error(ErrorItem { span: start })
    }

    fn item_visibility(&mut self) -> Visibility {
        if self.cursor.eat_keyword(Keyword::Public).is_some() {
            Visibility::Public
        } else {
            self.cursor.eat_keyword(Keyword::Private);
            Visibility::Private
        }
    }

    pub(crate) fn type_alias_decl(&mut self, visibility: Visibility) -> TypeAliasDecl {
        let start = self.expect_keyword(Keyword::Alias, "expected `alias`");
        let name = self.name("expected type name");
        let type_params = self.type_params();
        self.expect_punct(Punct::Eq, "expected `=` in alias declaration");
        let target = self.type_expr();
        let mut span = start.cover(target.span());
        if let Some(semi) = self.cursor.eat_punct(Punct::Semi) {
            span = span.cover(semi.span);
        }
        TypeAliasDecl {
            visibility,
            name,
            type_params,
            target,
            span,
        }
    }

    pub(crate) fn type_decl(&mut self, visibility: Visibility) -> TypeDecl {
        let start = self.expect_keyword(Keyword::Type, "expected `type`");
        let name = self.name("expected type name");
        let type_params = self.type_params();
        let (body, mut span) = if self.cursor.eat_punct(Punct::Eq).is_some() {
            let representation = self.type_expr();
            (
                TypeDeclBody::Representation(representation.clone()),
                start.cover(representation.span()),
            )
        } else {
            (TypeDeclBody::Bodyless, start.cover(name.span))
        };
        if let Some(semi) = self.cursor.eat_punct(Punct::Semi) {
            span = span.cover(semi.span);
        } else if !matches!(&body, TypeDeclBody::Representation(TypeExpr::Record(_))) {
            self.expect_punct(Punct::Semi, "expected `;` after type declaration");
        }
        TypeDecl {
            visibility,
            name,
            type_params,
            body,
            span,
        }
    }

    pub(crate) fn enum_decl(&mut self, visibility: Visibility) -> EnumDecl {
        let start = self.expect_keyword(Keyword::Enum, "expected `enum`");
        let name = self.name("expected enum name");
        let type_params = self.type_params();
        self.expect_punct(Punct::LBrace, "expected `{` in enum declaration");
        let mut variants = Vec::new();
        while !self.cursor.at_eof() && !self.cursor.at_punct(Punct::RBrace) {
            let before = self.cursor.position();
            let variant_name = self.name("expected enum variant");
            let mut fields = Vec::new();
            let mut field_names = None;
            let mut end = variant_name.span;
            if self.eat_open_punct(Punct::LParen).is_some() {
                fields = self.comma_list(Punct::RParen, |this| {
                    if this.cursor.at_ident_like()
                        && this.cursor.nth(1).kind == TokenKind::Punct(Punct::Colon)
                    {
                        this.name("expected payload label");
                        this.expect_punct(Punct::Colon, "expected `:` after payload label");
                    }
                    this.type_expr()
                });
                end = self.expect_punct(Punct::RParen, "expected `)` after enum variant fields");
            } else if self.eat_open_punct(Punct::LBrace).is_some() {
                let named = self.comma_list(Punct::RBrace, |this| {
                    let name = this.name("expected variant field name");
                    this.expect_punct(Punct::Colon, "expected `:` after variant field name");
                    (name, this.type_expr())
                });
                let (names, types) = named.into_iter().unzip();
                field_names = Some(names);
                fields = types;
                end = self.expect_punct(Punct::RBrace, "expected `}` after enum variant fields");
            }
            // Semicolons remain accepted for existing source enum declarations.
            if let Some(separator) = self
                .cursor
                .eat_punct(Punct::Comma)
                .or_else(|| self.cursor.eat_punct(Punct::Semi))
            {
                end = separator.span;
            } else if !self.cursor.at_punct(Punct::RBrace) {
                self.expect_punct(Punct::Comma, "expected `,` after enum variant");
            }
            variants.push(EnumVariant {
                span: variant_name.span.cover(end),
                name: variant_name,
                fields,
                field_names,
            });
            self.ensure_progress(before, "expected enum variant");
        }
        let end = self.expect_punct(Punct::RBrace, "expected `}` after enum declaration");
        EnumDecl {
            visibility,
            name,
            type_params,
            variants,
            span: start.cover(end),
        }
    }

    pub(crate) fn spec_decl(&mut self, visibility: Visibility) -> SpecDecl {
        let start = self.expect_keyword(Keyword::Spec, "expected `spec`");
        let name = self.name("expected spec name");
        let type_params = self.type_params();
        let kind_annotation = self.spec_kind_annotation();
        let bounds = if self.cursor.eat_punct(Punct::Tilde).is_some() {
            let mut bounds = Vec::new();
            loop {
                bounds.push(self.type_param_bound());
                if self.cursor.eat_punct(Punct::Plus).is_none() {
                    break;
                }
            }
            bounds
        } else {
            Vec::new()
        };
        if matches!(kind_annotation, Some(SpecKind::TraceSpec))
            && self.cursor.eat_punct(Punct::Eq).is_some()
        {
            let trace = self.spec_expr();
            let semi = self.expect_punct(Punct::Semi, "expected `;` after trace spec");
            return SpecDecl {
                visibility,
                name,
                type_params,
                bounds,
                kind: SpecKind::TraceSpec,
                callable: None,
                trace: Some(trace),
                items: Vec::new(),
                span: start.cover(semi),
            };
        }
        // Disambiguate TypeSpecDecl vs FlowSpecDecl:
        //   FlowSpecDecl: input type_expr => output type_expr ![effects] ;
        //   TypeSpecDecl: ; (bodyless) or { flow signatures... }
        if let Some(open) = self.eat_open_punct(Punct::LBrace) {
            let mut items = Vec::new();
            while !self.cursor.at_eof() && !self.cursor.at_punct(Punct::RBrace) {
                let before = self.cursor.position();
                if self.cursor.at_keyword(Keyword::Flow) {
                    items.push(SpecItem::FlowSignature(Box::new(self.flow_signature())));
                } else {
                    let span = self.cursor.peek().span;
                    self.diagnostics.push(Diagnostic::syntax(
                        SyntaxDiagnosticCode::UnexpectedToken,
                        span,
                        "expected spec item",
                    ));
                    self.recover_stmt();
                    items.push(SpecItem::Error(span));
                }
                self.ensure_progress(before, "expected spec item");
            }
            let close = self.expect_punct(Punct::RBrace, "expected `}` after spec block");
            let _ = open;
            return SpecDecl {
                visibility,
                name,
                type_params,
                bounds,
                kind: kind_annotation.unwrap_or(SpecKind::TypeSpec),
                callable: None,
                trace: None,
                items,
                span: start.cover(close),
            };
        }
        if self.cursor.at_punct(Punct::Semi) {
            let semi = self.expect_punct(Punct::Semi, "expected `;` after bodyless spec");
            return SpecDecl {
                visibility,
                name,
                type_params,
                bounds,
                kind: kind_annotation.unwrap_or(SpecKind::TypeSpec),
                callable: None,
                trace: None,
                items: Vec::new(),
                span: start.cover(semi),
            };
        }
        // FlowSpecDecl form: input => output ![effects] ;
        let input = self.type_expr();
        self.expect_punct(Punct::FatArrow, "expected `=>` in flow spec signature");
        let output = self.type_expr();
        let effects = self.effect_suffix();
        let sig_span = effects.as_ref().map_or_else(
            || input.span().cover(output.span()),
            |effects| input.span().cover(effects.span),
        );
        let callable = SpecCallableSignature {
            input,
            output,
            effects,
            span: sig_span,
        };
        let semi = self.expect_punct(Punct::Semi, "expected `;` after flow spec");
        SpecDecl {
            visibility,
            name,
            type_params,
            bounds,
            kind: kind_annotation.unwrap_or(SpecKind::CallableSpec),
            callable: Some(callable),
            trace: None,
            items: Vec::new(),
            span: start.cover(semi),
        }
    }

    fn spec_kind_annotation(&mut self) -> Option<SpecKind> {
        self.cursor.eat_punct(Punct::Colon)?;
        if self.cursor.at_keyword(Keyword::Type) {
            self.cursor.bump();
            return Some(SpecKind::TypeSpec);
        }
        let token = self.cursor.peek();
        if token.kind == TokenKind::Ident {
            let text = self.slice(token.span);
            let kind = match text {
                "callable" => SpecKind::CallableSpec,
                "trace" => SpecKind::TraceSpec,
                _ => {
                    self.diagnostics.push(Diagnostic::syntax(
                        SyntaxDiagnosticCode::UnexpectedToken,
                        token.span,
                        "expected spec kind `type`, `callable`, or `trace`",
                    ));
                    self.cursor.bump();
                    return Some(SpecKind::TypeSpec);
                }
            };
            self.cursor.bump();
            return Some(kind);
        }
        self.diagnostics.push(Diagnostic::syntax(
            SyntaxDiagnosticCode::UnexpectedToken,
            token.span,
            "expected spec kind `type`, `callable`, or `trace`",
        ));
        Some(SpecKind::TypeSpec)
    }

    fn spec_expr(&mut self) -> SpecExpr {
        self.spec_or()
    }

    fn spec_or(&mut self) -> SpecExpr {
        let mut expr = self.spec_and();
        while self.cursor.eat_punct(Punct::Pipe).is_some() {
            let rhs = self.spec_and();
            let span = expr.span().cover(rhs.span());
            expr = SpecExpr::Or {
                lhs: Box::new(expr),
                rhs: Box::new(rhs),
                span,
            };
        }
        expr
    }

    fn spec_and(&mut self) -> SpecExpr {
        let mut expr = self.spec_unary();
        while self.cursor.eat_punct(Punct::Amp).is_some() {
            let rhs = self.spec_unary();
            let span = expr.span().cover(rhs.span());
            expr = SpecExpr::And {
                lhs: Box::new(expr),
                rhs: Box::new(rhs),
                span,
            };
        }
        expr
    }

    fn spec_unary(&mut self) -> SpecExpr {
        if let Some(plus) = self.cursor.eat_punct(Punct::Plus) {
            let pattern = self.effect_ref();
            return SpecExpr::Allow {
                span: plus.span.cover(pattern.span),
                pattern,
            };
        }
        if let Some(minus) = self.cursor.eat_punct(Punct::Minus) {
            let pattern = self.effect_ref();
            return SpecExpr::Deny {
                span: minus.span.cover(pattern.span),
                pattern,
            };
        }
        self.spec_temporal()
    }

    fn spec_temporal(&mut self) -> SpecExpr {
        let lhs = self.spec_primary();
        if self.eat_trace_temporal_operator(Punct::Gt) {
            let rhs = self.spec_primary();
            let span = lhs.span().cover(rhs.span());
            return SpecExpr::Before {
                before: Box::new(lhs),
                after: Box::new(rhs),
                span,
            };
        }
        if self.eat_trace_temporal_operator(Punct::Lt) {
            let rhs = self.spec_primary();
            let span = lhs.span().cover(rhs.span());
            return SpecExpr::After {
                after: Box::new(lhs),
                before: Box::new(rhs),
                span,
            };
        }
        lhs
    }

    fn eat_trace_temporal_operator(&mut self, punct: Punct) -> bool {
        if !self.cursor.at_punct(punct) || self.cursor.nth(1).kind != TokenKind::Punct(punct) {
            return false;
        }
        self.cursor.bump();
        self.cursor.bump();
        true
    }

    fn spec_primary(&mut self) -> SpecExpr {
        if self.eat_open_punct(Punct::LParen).is_some() {
            let expr = self.spec_expr();
            self.expect_punct(Punct::RParen, "expected `)` after spec expression");
            return expr;
        }
        SpecExpr::Atom(self.effect_ref())
    }

    pub(crate) fn flow_signature(&mut self) -> FlowSignature {
        let start = self.expect_keyword(Keyword::Flow, "expected `flow`");
        let name = self.name("expected flow name");
        let type_params = self.type_params();
        let params = self.param_list();
        let return_type = self
            .cursor
            .eat_punct(Punct::Arrow)
            .map(|_| self.type_expr());
        let declared_effects = self.effect_suffix();
        let end = self.expect_punct(Punct::Semi, "expected `;` after spec flow signature");
        FlowSignature {
            name,
            type_params,
            params,
            return_type,
            declared_effects,
            span: start.cover(end),
        }
    }

    pub(crate) fn impl_decl(&mut self) -> ImplDecl {
        let start = self.expect_keyword(Keyword::Impl, "expected `impl`");
        let self_type = self.type_expr();
        let target = if self.cursor.at_punct(Punct::Comma) || self.cursor.at_keyword(Keyword::For) {
            self.legacy_impl_for_target(self_type)
        } else if self.cursor.eat_punct(Punct::Tilde).is_some() {
            let mut specs = vec![self.impl_spec_ref()];
            while self.cursor.eat_punct(Punct::Plus).is_some() {
                specs.push(self.impl_spec_ref());
            }
            let span = specs.last().map_or_else(
                || self_type.span(),
                |spec_ref| self_type.span().cover(spec_ref.span),
            );
            ImplTarget::SpecSatisfaction {
                specs,
                self_type,
                span,
            }
        } else {
            match inherent_impl_target_from_type(self_type) {
                Some((target, type_args, span)) => ImplTarget::Inherent {
                    target,
                    type_args,
                    span,
                },
                None => {
                    let span = self.cursor.peek().span;
                    self.diagnostics.push(Diagnostic::syntax(
                        SyntaxDiagnosticCode::InvalidItem,
                        span,
                        "expected `~` for spec impl or `{` for inherent/effect impl",
                    ));
                    ImplTarget::Error { span }
                }
            }
        };
        if let Some(semi) = self.cursor.eat_punct(Punct::Semi) {
            return ImplDecl {
                span: start.cover(semi.span),
                target,
                items: Vec::new(),
            };
        }
        self.expect_punct(Punct::LBrace, "expected `{` in impl block");
        let mut items = Vec::new();
        let owner_path = target.owner_path().cloned();
        while !self.cursor.at_eof() && !self.cursor.at_punct(Punct::RBrace) {
            let before = self.cursor.position();
            if self.cursor.at_keyword(Keyword::Flow) {
                items.push(ImplItem::Flow(Box::new(
                    self.flow_decl(Visibility::Private),
                )));
            } else if self.cursor.at_keyword(Keyword::Action) {
                let owner = owner_path.as_ref().and_then(|owner_path| {
                    owner_path
                        .segments
                        .last()
                        .map(|segment| segment.text.as_str())
                });
                items.push(ImplItem::Action(Box::new(self.effect_action_decl(owner))));
            } else {
                let span = self.cursor.peek().span;
                self.diagnostics.push(Diagnostic::syntax(
                    SyntaxDiagnosticCode::UnexpectedToken,
                    span,
                    "expected `flow` or `action` in impl block",
                ));
                self.recover_stmt();
                items.push(ImplItem::Error(span));
            }
            self.ensure_progress(before, "expected impl item");
        }
        let end = self.expect_punct(Punct::RBrace, "expected `}` after impl block");
        ImplDecl {
            target,
            items,
            span: start.cover(end),
        }
    }

    fn legacy_impl_for_target(&mut self, first_spec: TypeExpr) -> ImplTarget {
        let mut specs = Vec::new();
        let mut span = first_spec.span();
        if let Some(spec) = self.impl_spec_ref_from_type(first_spec) {
            span = span.cover(spec.span);
            specs.push(spec);
        }
        while self.cursor.eat_punct(Punct::Comma).is_some() {
            let spec_ty = self.type_expr();
            span = span.cover(spec_ty.span());
            if let Some(spec) = self.impl_spec_ref_from_type(spec_ty) {
                span = span.cover(spec.span);
                specs.push(spec);
            }
        }
        self.expect_keyword(Keyword::For, "expected `for` in legacy spec impl");
        let self_type = self.type_expr();
        span = span.cover(self_type.span());
        if specs.is_empty() {
            return ImplTarget::Error { span };
        }
        ImplTarget::SpecSatisfaction {
            specs,
            self_type,
            span,
        }
    }

    fn impl_spec_ref_from_type(&mut self, ty: TypeExpr) -> Option<ImplSpecRef> {
        let span = ty.span();
        match ty {
            TypeExpr::Path { path, args, span } => Some(ImplSpecRef {
                spec_path: path,
                spec_args: args,
                span,
            }),
            _ => {
                self.diagnostics.push(Diagnostic::syntax(
                    SyntaxDiagnosticCode::InvalidItem,
                    span,
                    "expected spec path in legacy `impl Spec for Type` syntax",
                ));
                None
            }
        }
    }

    fn impl_spec_ref(&mut self) -> ImplSpecRef {
        let spec_path = self.path();
        let spec_args = self.type_args();
        let span = spec_args
            .last()
            .map_or(spec_path.span, |arg| spec_path.span.cover(arg.span()));
        ImplSpecRef {
            spec_path,
            spec_args,
            span,
        }
    }

    pub(crate) fn effect_decl(&mut self, visibility: Visibility) -> EffectDecl {
        let start = self.expect_keyword(Keyword::Effect, "expected `effect`");
        let name = self.name("expected effect name");
        let type_params = self.type_params();
        let extends = self
            .cursor
            .eat_keyword(Keyword::Extends)
            .map(|_| self.effect_ref());
        let (body, end) = if let Some(open) = self.eat_open_punct(Punct::LBrace) {
            let mut actions = Vec::new();
            while !self.cursor.at_eof() && !self.cursor.at_punct(Punct::RBrace) {
                let before = self.cursor.position();
                if self.cursor.at_keyword(Keyword::Action) {
                    actions.push(self.effect_action_decl(Some(name.text.as_str())));
                } else {
                    let span = self.cursor.peek().span;
                    self.diagnostics.push(Diagnostic::syntax(
                        SyntaxDiagnosticCode::UnexpectedToken,
                        span,
                        "expected effect action",
                    ));
                    self.recover_stmt();
                }
                self.ensure_progress(before, "expected effect action");
            }
            let close = self.expect_punct(Punct::RBrace, "expected `}` after effect block");
            (
                EffectBody::Block {
                    actions,
                    span: open.span.cover(close),
                },
                close,
            )
        } else {
            let semi = self.expect_punct(Punct::Semi, "expected `;` or effect block");
            (EffectBody::Empty { span: semi }, semi)
        };
        EffectDecl {
            visibility,
            name,
            type_params,
            extends,
            body,
            span: start.cover(end),
        }
    }

    pub(crate) fn effect_action_decl(&mut self, owner: Option<&str>) -> EffectActionDecl {
        let start = self.expect_keyword(Keyword::Action, "expected `action`");
        let name = self.name("expected action name");
        let selector_params = self.action_selector_params();
        let type_params = selector_params
            .iter()
            .map(|param| match param {
                ActionSelectorParam::Type(param) => param.clone(),
            })
            .collect();
        let params = self.param_list();
        self.expect_punct(Punct::Arrow, "expected action return type");
        let return_type = self.type_expr();
        let end = if self.cursor.at_punct(Punct::LBrace) {
            let body_span = self.recover_braced_block();
            let label = owner
                .map(|owner| format!("{owner}.{}", name.text))
                .unwrap_or_else(|| name.text.clone());
            self.diagnostics.push(Diagnostic::syntax(
                SyntaxDiagnosticCode::InvalidItem,
                body_span,
                format!("ActionSignatureHasBody: action signature `{label}` must not have a body"),
            ));
            body_span
        } else {
            self.expect_punct(Punct::Semi, "expected `;` after action declaration")
        };
        EffectActionDecl {
            name,
            selector_params,
            type_params,
            params,
            return_type,
            span: start.cover(end),
        }
    }

    fn action_selector_params(&mut self) -> Vec<ActionSelectorParam> {
        if !self.cursor.at_punct(Punct::Lt) {
            return Vec::new();
        }
        self.cursor.bump();
        let params = self.comma_list(Punct::Gt, |this| {
            let param = this.type_param();
            if param.name.text == "_" {
                this.diagnostics.push(Diagnostic::syntax(
                    SyntaxDiagnosticCode::InvalidItem,
                    param.name.span,
                    "action selector declaration cannot use `_`; omit the selector at perform/handler sites to request wildcard matching",
                ));
            }
            ActionSelectorParam::Type(param)
        });
        self.expect_punct(Punct::Gt, "expected `>` after action selectors");
        params
            .into_iter()
            .filter(|param| match param {
                ActionSelectorParam::Type(param) => param.name.text != "_",
            })
            .collect()
    }

    pub(crate) fn flow_decl(&mut self, visibility: Visibility) -> FlowDecl {
        let start = self.expect_keyword(Keyword::Flow, "expected `flow`");
        let name = self.name("expected flow name");
        let type_params = self.type_params();
        let params = self.param_list();
        let return_type = self
            .cursor
            .eat_punct(Punct::Arrow)
            .map(|_| self.type_expr());
        let declared_effects = self.effect_suffix();
        let mut conformances = self.declaration_conformances();
        loop {
            if self.cursor.at_keyword(Keyword::Effect) {
                self.obsolete_effect_clause();
                continue;
            }
            if self.cursor.at_keyword(Keyword::Policy) {
                conformances.push(self.obsolete_flow_policy_clause());
                continue;
            }
            if self.cursor.at_keyword(Keyword::Follows) {
                conformances.push(self.obsolete_follows_clause());
                continue;
            }
            break;
        }
        let body = if self.cursor.at_punct(Punct::Eq) {
            let eq = self.expect_punct(Punct::Eq, "expected `=` before expression flow body");
            let expr = self.expr();
            let end = self
                .cursor
                .eat_punct(Punct::Semi)
                .map_or(expr.span(), |semi| semi.span);
            FlowBody::Expr {
                expr,
                span: eq.cover(end),
            }
        } else {
            FlowBody::Block(self.block())
        };
        let trailing_handler = if self.cursor.eat_keyword(Keyword::With).is_some() {
            Some(self.handler_arg_expr())
        } else {
            None
        };
        let span = start.cover(
            trailing_handler
                .as_ref()
                .map_or_else(|| body.span(), |handler| handler.span()),
        );
        FlowDecl {
            visibility,
            name,
            type_params,
            params,
            return_type,
            declared_effects,
            conformances,
            body,
            trailing_handler,
            span,
        }
    }

    pub(crate) fn declaration_conformances(&mut self) -> Vec<DeclarationConformance> {
        let mut conformances = Vec::new();
        while self.cursor.eat_punct(Punct::Tilde).is_some() {
            conformances.push(self.declaration_conformance_ref());
            while self.cursor.eat_punct(Punct::Plus).is_some() {
                conformances.push(self.declaration_conformance_ref());
            }
        }
        conformances
    }

    fn declaration_conformance_ref(&mut self) -> DeclarationConformance {
        if let Some(policy_span) = self.cursor.eat_keyword(Keyword::Policy) {
            self.diagnostics.push(Diagnostic::syntax(
                SyntaxDiagnosticCode::UnexpectedToken,
                policy_span.span,
                "`policy` inline conformance is obsolete; use `~ TraceSpec` or `~ (TraceSpecExpr)`",
            ));
            let span = if self.cursor.at_punct(Punct::LBrace) {
                policy_span.span.cover(self.recover_braced_block())
            } else {
                policy_span.span
            };
            return DeclarationConformance {
                span,
                target: DeclarationConformanceTarget::Error(span),
            };
        }
        if self.eat_open_punct(Punct::LParen).is_some() {
            let expr = self.spec_expr();
            let close = self.expect_punct(Punct::RParen, "expected `)` after inline trace spec");
            return DeclarationConformance {
                span: expr.span().cover(close),
                target: DeclarationConformanceTarget::InlineTraceSpec(expr),
            };
        }
        let spec = self.impl_spec_ref();
        DeclarationConformance {
            span: spec.span,
            target: DeclarationConformanceTarget::Path(spec),
        }
    }

    fn obsolete_flow_policy_clause(&mut self) -> DeclarationConformance {
        let policy_span = self.expect_keyword(Keyword::Policy, "expected `policy`");
        self.diagnostics.push(Diagnostic::syntax(
            SyntaxDiagnosticCode::UnexpectedToken,
            policy_span,
            "`policy` flow clauses are obsolete; use declaration conformance with `~`",
        ));
        let path = self.path();
        DeclarationConformance {
            span: policy_span.cover(path.span),
            target: DeclarationConformanceTarget::Error(policy_span.cover(path.span)),
        }
    }

    fn obsolete_follows_clause(&mut self) -> DeclarationConformance {
        let follows_span = self.expect_keyword(Keyword::Follows, "expected `follows`");
        self.diagnostics.push(Diagnostic::syntax(
            SyntaxDiagnosticCode::UnexpectedToken,
            follows_span,
            "`follows` is obsolete; use declaration conformance with `~`",
        ));
        if self.cursor.at_keyword(Keyword::Policy) {
            let _ = self.cursor.eat_keyword(Keyword::Policy);
            if self.cursor.at_punct(Punct::LBrace) {
                let _ = self.recover_braced_block();
            }
        } else if !self.cursor.at_punct(Punct::LBrace) && !self.cursor.at_punct(Punct::Semi) {
            let _ = self.path();
        }
        DeclarationConformance {
            span: follows_span,
            target: DeclarationConformanceTarget::Error(follows_span),
        }
    }

    pub(crate) fn tool_decl(&mut self, visibility: Visibility) -> ToolDecl {
        let start = self.expect_keyword(Keyword::Tool, "expected `tool`");
        let path = self.path();
        let type_params = self.type_params();
        let params = self.param_list();
        self.expect_punct(Punct::Arrow, "expected tool return type");
        let return_type = self.type_expr();
        let effects = self.effect_suffix();
        let conformances = self.declaration_conformances();
        while !self.cursor.at_eof()
            && !self.cursor.at_punct(Punct::Semi)
            && !self.cursor.at_punct(Punct::LBrace)
        {
            let before = self.cursor.position();
            if let Some(effect) = self.cursor.eat_keyword(Keyword::Effect) {
                self.diagnostics.push(Diagnostic::syntax(
                    SyntaxDiagnosticCode::UnexpectedToken,
                    effect.span,
                    "tool effect clauses are obsolete; write the effect contract as `![...]` after the return type",
                ));
                let _ = self.effect_row();
            } else if self.cursor.eat_keyword(Keyword::Require).is_some() {
                let expr = self.expr();
                self.diagnostics.push(Diagnostic::syntax(
                    SyntaxDiagnosticCode::UnexpectedToken,
                    expr.span(),
                    obsolete_source_requirement(&expr).unwrap_or(
                        "tool requirement clauses are obsolete; use declaration conformance with `~`",
                    ),
                ));
            } else if self.cursor.eat_keyword(Keyword::Follows).is_some() {
                let follows_span = self.previous_span();
                self.diagnostics.push(Diagnostic::syntax(
                    SyntaxDiagnosticCode::UnexpectedToken,
                    follows_span,
                    "`follows` is obsolete; use declaration conformance with `~`",
                ));
                if self.cursor.eat_keyword(Keyword::Policy).is_some() {
                    let _ = self.recover_braced_block();
                }
            } else {
                self.bump_error(
                    SyntaxDiagnosticCode::UnexpectedToken,
                    "expected tool clause",
                );
            }
            self.ensure_progress(before, "expected tool clause");
        }
        let body = if self.cursor.at_punct(Punct::LBrace) {
            ToolBody::Source(FlowBody::Block(self.block()))
        } else if let Some(semi) = self.cursor.eat_punct(Punct::Semi) {
            ToolBody::Decl {
                semicolon_span: semi.span,
            }
        } else {
            let span = self.cursor.peek().span;
            self.bump_error(
                SyntaxDiagnosticCode::MissingToken,
                "expected tool body or `;` after tool declaration",
            );
            ToolBody::Error(span)
        };
        let end = body.span();
        ToolDecl {
            visibility,
            path,
            type_params,
            params,
            return_type,
            effects,
            conformances,
            body,
            span: start.cover(end),
        }
    }

    fn obsolete_effect_clause(&mut self) {
        let effect = self.expect_keyword(Keyword::Effect, "expected `effect`");
        self.diagnostics.push(Diagnostic::syntax(
            SyntaxDiagnosticCode::UnexpectedToken,
            effect,
            "flow effect clauses are obsolete; write the effect contract as `![...]` after the return type",
        ));
        let _ = self.effect_row();
    }

    pub(crate) fn top_level_let_decl(&mut self, visibility: Visibility) -> TopLevelLetDecl {
        let start = self.expect_keyword(Keyword::Let, "expected `let`");
        let name = self.name("expected top-level `let` name");
        let type_annotation = self.type_annotation();
        self.expect_punct(Punct::Eq, "expected `=` in top-level `let`");
        let value = self.expr();
        let end = self.expect_punct(Punct::Semi, "expected `;` after top-level `let`");
        TopLevelLetDecl {
            visibility,
            name,
            type_annotation,
            value,
            span: start.cover(end),
        }
    }

    fn obsolete_memory_item(&mut self) -> Item {
        let start = self.cursor.peek().span;
        self.diagnostics.push(Diagnostic::syntax(
            SyntaxDiagnosticCode::InvalidItem,
            start,
            "`memory` is obsolete source syntax; use top-level `let` with std memory support types",
        ));
        self.recover_item();
        Item::Error(ErrorItem { span: start })
    }

    fn obsolete_policy_item(&mut self) -> Item {
        let start = self.expect_keyword(Keyword::Policy, "expected `policy`");
        self.diagnostics.push(Diagnostic::syntax(
            SyntaxDiagnosticCode::InvalidItem,
            start,
            "`policy` declarations are obsolete; use `spec Name: trace = ...` and declaration conformance with `~`",
        ));
        if self.cursor.at_ident_like() {
            let _ = self.name("expected obsolete policy name");
        }
        let span = if self.cursor.at_punct(Punct::LBrace) {
            start.cover(self.recover_braced_block())
        } else {
            self.recover_item();
            start
        };
        Item::Error(ErrorItem { span })
    }

    fn invalid_top_level_var_item(&mut self) -> Item {
        let start = self.cursor.peek().span;
        self.diagnostics.push(Diagnostic::syntax(
            SyntaxDiagnosticCode::InvalidItem,
            start,
            "top-level `var` is invalid; use immutable top-level `let`",
        ));
        self.recover_item();
        Item::Error(ErrorItem { span: start })
    }

    pub(crate) fn protocol_decl(&mut self, visibility: Visibility) -> ProtocolDecl {
        let start = self.expect_keyword(Keyword::Protocol, "expected `protocol`");
        let name = self.name("expected protocol name");
        self.expect_punct(Punct::LBrace, "expected `{` in protocol declaration");
        let mut messages = Vec::new();
        while !self.cursor.at_eof() && !self.cursor.at_punct(Punct::RBrace) {
            let before = self.cursor.position();
            let from = self.path();
            self.expect_punct(Punct::Arrow, "expected `->` in protocol message");
            let to = self.path();
            self.expect_punct(Punct::Colon, "expected `:` before protocol payload");
            let payload = self.type_expr();
            let end = self.expect_punct(Punct::Semi, "expected `;` after protocol message");
            messages.push(ProtocolMsg {
                span: from.span.cover(end),
                from,
                to,
                payload,
            });
            self.ensure_progress(before, "expected protocol message");
        }
        let end = self.expect_punct(Punct::RBrace, "expected `}` after protocol declaration");
        ProtocolDecl {
            visibility,
            name,
            messages,
            span: start.cover(end),
        }
    }
}

fn inherent_impl_target_from_type(ty: TypeExpr) -> Option<(Path, Vec<TypeExpr>, crate::Span)> {
    match ty {
        TypeExpr::Path { path, args, span } => Some((path, args, span)),
        _ => None,
    }
}

fn obsolete_source_requirement(expr: &Expr) -> Option<&'static str> {
    let Expr::Call(CallExpr { callee, .. }) = expr else {
        return None;
    };
    let Expr::Path(path) = callee.as_ref() else {
        return None;
    };
    if path.segments.len() != 1 {
        return None;
    }
    match path.segments[0].text.as_str() {
        "Capability" => Some(
            "source-level `Capability(...)` requirements are obsolete and were removed from the language SPEC",
        ),
        "Sandbox" => Some(
            "source-level `require Sandbox(...)` is obsolete; use an action parameter such as `![Command.run[DefaultCommandSandbox]]`",
        ),
        _ => None,
    }
}
