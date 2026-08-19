use crate::{Diagnostic, Keyword, Punct, Span, SyntaxDiagnosticCode, TokenKind, ast::*};

use crate::parser::Parser;

impl Parser<'_> {
    pub(crate) fn expr(&mut self) -> Expr {
        self.lambda_or_pipeline()
    }

    pub(crate) fn expr_before_block(&mut self) -> Expr {
        let previous = self.record_expr_enabled;
        self.record_expr_enabled = false;
        let expr = self.expr();
        self.record_expr_enabled = previous;
        expr
    }

    pub(crate) fn lambda_or_pipeline(&mut self) -> Expr {
        if self.cursor.at_ident_like()
            && self.cursor.nth(1).kind == TokenKind::Punct(Punct::FatArrow)
        {
            let name = self.name("expected lambda parameter");
            self.cursor.bump();
            let body = self.lambda_body();
            let span = name.span.cover(lambda_body_span(&body));
            return Expr::Lambda(LambdaExpr {
                params: LambdaParams::Ident(name),
                body,
                span,
            });
        }

        if self.cursor.at_punct(Punct::LParen) && self.looks_like_lambda_param_list() {
            let start = self.cursor.peek().span;
            let params = self.param_list();
            self.expect_punct(Punct::FatArrow, "expected `=>` after lambda parameters");
            let body = self.lambda_body();
            let span = start.cover(lambda_body_span(&body));
            return Expr::Lambda(LambdaExpr {
                params: LambdaParams::ParamList(params),
                body,
                span,
            });
        }

        self.pipeline_expr()
    }

    pub(crate) fn lambda_body(&mut self) -> LambdaBody {
        if self.cursor.at_punct(Punct::LBrace) {
            LambdaBody::Block(self.block())
        } else {
            LambdaBody::Expr(Box::new(self.expr()))
        }
    }

    pub(crate) fn pipeline_expr(&mut self) -> Expr {
        let input = self.list_cons_expr();
        let mut stages = Vec::new();
        while self.cursor.eat_punct(Punct::TildeArrow).is_some() {
            stages.push(self.pipeline_stage());
        }
        if stages.is_empty() {
            input
        } else {
            let span = input.span().cover(stages.last().unwrap().span);
            Expr::Pipeline(PipelineExpr {
                input: Box::new(input),
                stages,
                span,
            })
        }
    }

    pub(crate) fn list_cons_expr(&mut self) -> Expr {
        let head = self.stage_compose_expr();
        if self.looks_like_spec_method_selection() {
            return self.spec_method_call_expr(head);
        }
        if self.cursor.eat_punct(Punct::ColonColon).is_none() {
            return head;
        }
        let tail = self.list_cons_expr();
        let span = head.span().cover(tail.span());
        Expr::ListCons {
            head: Box::new(head),
            tail: Box::new(tail),
            span,
        }
    }

    fn spec_method_call_expr(&mut self, receiver: Expr) -> Expr {
        self.expect_punct(
            Punct::ColonColon,
            "expected `::` before spec method selection",
        );
        let first = self.name("expected spec name after `::`");
        let mut span = receiver.span().cover(first.span);
        let mut segments = vec![first];
        let mut spec_args = Vec::new();
        loop {
            if self.cursor.at_punct(Punct::Lt) {
                spec_args = self.type_args();
                if let Some(last) = spec_args.last() {
                    span = span.cover(last.span());
                }
            }
            if self.cursor.at_punct(Punct::Dot)
                && self.cursor.nth(1).kind.is_ident_like()
                && self.cursor.nth(2).kind == TokenKind::Punct(Punct::LParen)
            {
                break;
            }
            if self.cursor.eat_punct(Punct::Dot).is_none() {
                break;
            }
            let segment = self.name("expected spec path segment");
            span = span.cover(segment.span);
            segments.push(segment);
        }
        let spec_path = Path { segments, span };
        self.expect_punct(Punct::Dot, "expected `.` before selected spec method");
        let method = self.name("expected spec method name");
        let args = self.arg_list();
        let span = receiver.span().cover(self.previous_span());
        Expr::SpecMethodCall(SpecMethodCallExpr {
            receiver: Box::new(receiver),
            spec_path,
            spec_args,
            method,
            args,
            span,
        })
    }

    pub(crate) fn stage_compose_expr(&mut self) -> Expr {
        let first = self.binary_expr(0);
        if !self.cursor.at_punct(Punct::Pipe) {
            return first;
        }
        let first_span = first.span();
        let mut stages = vec![PipelineStage {
            expr: Box::new(first),
            limits: Vec::new(),
            span: first_span,
        }];
        while self.cursor.eat_punct(Punct::Pipe).is_some() {
            stages.push(self.pipeline_stage());
        }
        let span = stages
            .first()
            .unwrap()
            .span
            .cover(stages.last().unwrap().span);
        Expr::StageCompose(StageComposeExpr { stages, span })
    }

    pub(crate) fn pipeline_stage(&mut self) -> PipelineStage {
        let expr = self.list_cons_expr();
        let mut span = expr.span();
        let mut limits = Vec::new();
        while self.cursor.eat_keyword(Keyword::Limit).is_some() {
            limits.extend(self.limit_list());
            if let Some(limit) = limits.last() {
                span = span.cover(limit.span());
            }
        }
        PipelineStage {
            expr: Box::new(expr),
            limits,
            span,
        }
    }

    pub(crate) fn binary_expr(&mut self, min_prec: u8) -> Expr {
        let mut lhs = self.unary_expr();
        while let Some((op, prec)) = self.binary_op() {
            if prec < min_prec {
                break;
            }
            self.cursor.bump();
            let rhs = self.binary_expr(prec + 1);
            let span = lhs.span().cover(rhs.span());
            lhs = Expr::Binary(BinaryExpr {
                lhs: Box::new(lhs),
                op,
                rhs: Box::new(rhs),
                span,
            });
        }
        lhs
    }

    pub(crate) fn unary_expr(&mut self) -> Expr {
        let op = if self.cursor.eat_punct(Punct::Bang).is_some() {
            Some(UnaryOp::Not)
        } else if self.cursor.eat_punct(Punct::Minus).is_some() {
            Some(UnaryOp::Neg)
        } else {
            None
        };
        if let Some(op) = op {
            let expr = self.unary_expr();
            let span = expr.span();
            Expr::Unary(UnaryExpr {
                op,
                expr: Box::new(expr),
                span,
            })
        } else {
            self.postfix_expr()
        }
    }

    pub(crate) fn postfix_expr(&mut self) -> Expr {
        let suppress_handler_with = std::mem::take(&mut self.suppress_next_handler_with);
        let mut expr = self.primary_expr();
        loop {
            let generic_args = if self.cursor.at_punct(Punct::Lt)
                && self.bracket_group_followed_by(Punct::LParen)
            {
                self.generic_args()
            } else {
                Vec::new()
            };
            if self.cursor.at_punct(Punct::LParen) && self.open_closed_slice_follows() {
                let _ = self.eat_open_punct(Punct::LParen);
                let start = self.expr();
                self.expect_punct(Punct::Comma, "expected `,` in slice");
                let end_expr = self.expr();
                let end = self.expect_punct(Punct::RBracket, "expected `]` after slice");
                let span = expr.span().cover(end);
                expr = Expr::Slice(SliceExpr {
                    receiver: Box::new(expr),
                    start: Box::new(start),
                    end: Box::new(end_expr),
                    bounds: RangeBounds::OpenClosed,
                    span,
                });
            } else if self.cursor.at_punct(Punct::LParen) {
                let args = self.arg_list();
                let span = expr.span().cover(self.previous_span());
                expr = Expr::Call(CallExpr {
                    callee: Box::new(expr),
                    generic_args,
                    args,
                    span,
                });
            } else if self.cursor.eat_punct(Punct::Dot).is_some() {
                let name = self.name("expected field or method name");
                let generic_args = if self.cursor.at_punct(Punct::Lt)
                    && self.bracket_group_followed_by(Punct::LParen)
                {
                    self.generic_args()
                } else {
                    Vec::new()
                };
                if self.cursor.at_punct(Punct::LParen) {
                    let args = self.arg_list();
                    let span = expr.span().cover(self.previous_span());
                    expr = Expr::MethodCall(MethodCallExpr {
                        receiver: Box::new(expr),
                        method: name,
                        generic_args,
                        args,
                        span,
                    });
                } else if !generic_args.is_empty() {
                    self.diagnostics.push(Diagnostic::syntax(
                        SyntaxDiagnosticCode::InvalidExpression,
                        name.span,
                        "type arguments require a method call",
                    ));
                    let span = expr.span().cover(name.span);
                    expr = Expr::Field(FieldExpr {
                        receiver: Box::new(expr),
                        field: name,
                        span,
                    });
                } else {
                    let span = expr.span().cover(name.span);
                    expr = Expr::Field(FieldExpr {
                        receiver: Box::new(expr),
                        field: name,
                        span,
                    });
                }
            } else if self.eat_open_punct(Punct::LBracket).is_some() {
                let start = self.expr();
                if self.cursor.eat_punct(Punct::Comma).is_some() {
                    let end_expr = self.expr();
                    let end = self.expect_punct(Punct::RParen, "expected `)` after slice");
                    let span = expr.span().cover(end);
                    expr = Expr::Slice(SliceExpr {
                        receiver: Box::new(expr),
                        start: Box::new(start),
                        end: Box::new(end_expr),
                        bounds: RangeBounds::ClosedOpen,
                        span,
                    });
                } else {
                    let end = self.expect_punct(Punct::RBracket, "expected `]` after index");
                    let span = expr.span().cover(end);
                    expr = Expr::Index(IndexExpr {
                        receiver: Box::new(expr),
                        index: Box::new(start),
                        span,
                    });
                }
            } else if let Some(question) = self.cursor.eat_punct(Punct::Question) {
                let span = expr.span().cover(question.span);
                expr = Expr::Try(TryExpr {
                    expr: Box::new(expr),
                    span,
                });
            } else if self.handler_with_enabled
                && !suppress_handler_with
                && self.cursor.eat_keyword(Keyword::With).is_some()
            {
                let handler = self.handler_arg_expr();
                let span = expr.span().cover(handler.span());
                expr = Expr::Handle(HandleExpr {
                    body: Box::new(expr),
                    handler,
                    span,
                });
            } else if !generic_args.is_empty() {
                self.diagnostics.push(Diagnostic::syntax(
                    SyntaxDiagnosticCode::InvalidExpression,
                    expr.span(),
                    "generic arguments require a call",
                ));
            } else {
                break;
            }
        }
        expr
    }

    pub(crate) fn primary_expr(&mut self) -> Expr {
        if self.cursor.at_keyword(Keyword::True) || self.cursor.at_keyword(Keyword::False) {
            let token = self.cursor.bump();
            return Expr::Literal(Literal::Bool {
                value: token.kind == TokenKind::Keyword(Keyword::True),
                span: token.span,
            });
        }
        match self.cursor.peek().kind {
            TokenKind::IntLit => {
                let token = self.cursor.bump();
                Expr::Literal(Literal::Int {
                    text: self.slice(token.span).to_string(),
                    span: token.span,
                })
            }
            TokenKind::FloatLit => {
                let token = self.cursor.bump();
                Expr::Literal(Literal::Float {
                    text: self.slice(token.span).to_string(),
                    span: token.span,
                })
            }
            TokenKind::StringLit => {
                let token = self.cursor.bump();
                Expr::Literal(Literal::String {
                    value: unquote_string(self.slice(token.span)),
                    span: token.span,
                })
            }
            TokenKind::CharLit => {
                let token = self.cursor.bump();
                match unquote_char(self.slice(token.span)) {
                    Some(value) => Expr::Literal(Literal::Char {
                        value,
                        span: token.span,
                    }),
                    None => {
                        self.diagnostics.push(Diagnostic::syntax(
                            SyntaxDiagnosticCode::InvalidExpression,
                            token.span,
                            "character literal must contain exactly one valid character",
                        ));
                        Expr::Error(token.span)
                    }
                }
            }
            TokenKind::Keyword(Keyword::If) => self.if_expr(),
            TokenKind::Keyword(Keyword::Match) => self.match_expr(),
            TokenKind::Keyword(Keyword::Perform) => self.perform_expr(),
            TokenKind::Keyword(Keyword::Handle) => self.handle_expr(),
            TokenKind::Keyword(Keyword::Handler) => self.handler_expr(),
            TokenKind::Punct(Punct::LBrace)
                if self.record_expr_enabled && self.looks_like_brace_literal_body() =>
            {
                self.brace_literal_expr()
            }
            TokenKind::Punct(Punct::LBrace) => Expr::Block(BlockExpr {
                block: self.block(),
            }),
            TokenKind::Punct(Punct::LBracket) => self.list_expr(),
            TokenKind::Punct(Punct::LParen) => self.tuple_or_group_expr(),
            TokenKind::Punct(Punct::Hash) => self.set_expr(),
            _ if self.cursor.at_ident_like() => self.path_or_record_expr(),
            _ => {
                let span = self.cursor.peek().span;
                self.bump_error(
                    SyntaxDiagnosticCode::InvalidExpression,
                    "expected expression",
                );
                Expr::Error(span)
            }
        }
    }

    pub(crate) fn path_or_record_expr(&mut self) -> Expr {
        let path = self.expr_path();
        let generic_args = if self.record_expr_enabled
            && self.cursor.at_punct(Punct::Lt)
            && self.bracket_group_followed_by(Punct::LBrace)
        {
            self.generic_args()
        } else {
            Vec::new()
        };
        if self.record_expr_enabled
            && self.cursor.at_punct(Punct::LBrace)
            && self.looks_like_record_expr_body()
        {
            return self.record_expr(Some(path), generic_args);
        }
        Expr::Path(path)
    }

    fn expr_path(&mut self) -> Path {
        let first = self.name("expected path segment");
        let mut span = first.span;
        let mut segments = vec![first];
        while self.cursor.at_punct(Punct::Dot) && self.cursor.nth(1).kind.is_ident_like() {
            if matches!(
                self.cursor.nth(2).kind,
                TokenKind::Punct(Punct::LParen | Punct::LBracket)
            ) {
                break;
            }
            self.cursor.bump();
            let segment = self.name("expected path segment after `.`");
            span = span.cover(segment.span);
            segments.push(segment);
        }
        Path { segments, span }
    }

    pub(crate) fn record_expr(
        &mut self,
        path: Option<Path>,
        generic_args: Vec<GenericArg>,
    ) -> Expr {
        let start = path
            .as_ref()
            .map_or(self.cursor.peek().span, |path| path.span);
        self.expect_punct(Punct::LBrace, "expected `{` in record expression");
        if path.is_none() && self.cursor.at_punct(Punct::RBrace) {
            let end = self.expect_punct(Punct::RBrace, "expected `}` after empty brace literal");
            return Expr::EmptyRecordOrMap {
                span: start.cover(end),
            };
        }
        let mut fields = Vec::new();
        while !self.cursor.at_eof() && !self.cursor.at_punct(Punct::RBrace) {
            let before = self.cursor.position();
            fields.push(self.field_init());
            self.cursor.eat_punct(Punct::Comma);
            self.ensure_progress(before, "expected record field initializer");
        }
        let end = self.expect_punct(Punct::RBrace, "expected `}` after record expression");
        Expr::Record(RecordExpr {
            path,
            generic_args,
            fields,
            span: start.cover(end),
        })
    }

    pub(crate) fn brace_literal_expr(&mut self) -> Expr {
        if self.looks_like_map_expr_body() {
            return self.map_expr();
        }
        self.record_expr(None, Vec::new())
    }

    pub(crate) fn looks_like_record_expr_body(&self) -> bool {
        if !self.cursor.at_punct(Punct::LBrace) {
            return false;
        }
        match (&self.cursor.nth(1).kind, &self.cursor.nth(2).kind) {
            (TokenKind::Punct(Punct::RBrace), _) => true,
            (TokenKind::Ident, TokenKind::Punct(Punct::Eq | Punct::Comma | Punct::RBrace)) => true,
            (TokenKind::Ident, TokenKind::Ident | TokenKind::Keyword(_)) => false,
            _ => false,
        }
    }

    pub(crate) fn looks_like_brace_literal_body(&self) -> bool {
        if self.looks_like_handler_arm_block_body() {
            return false;
        }
        self.looks_like_record_expr_body() || self.looks_like_map_expr_body()
    }

    pub(crate) fn looks_like_map_expr_body(&self) -> bool {
        if !self.cursor.at_punct(Punct::LBrace) {
            return false;
        }
        if self.looks_like_handler_arm_block_body() {
            return false;
        }
        let mut depth = 0usize;
        let mut idx = 1usize;
        loop {
            match self.cursor.nth(idx).kind {
                TokenKind::Eof => return false,
                TokenKind::Punct(Punct::FatArrow) if depth == 0 => return true,
                TokenKind::Punct(Punct::Comma | Punct::RBrace) if depth == 0 => return false,
                TokenKind::Punct(Punct::LParen | Punct::LBracket | Punct::LBrace) => depth += 1,
                TokenKind::Punct(Punct::RParen | Punct::RBracket | Punct::RBrace) => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            idx += 1;
        }
    }

    pub(crate) fn looks_like_handler_arm_block_body(&self) -> bool {
        if !self.cursor.at_punct(Punct::LBrace) {
            return false;
        }
        let mut idx = 1usize;
        if !self.cursor.nth(idx).kind.is_ident_like() {
            return false;
        }
        idx += 1;
        let mut saw_dot = false;
        while self.cursor.nth(idx).kind == TokenKind::Punct(Punct::Dot) {
            if !self.cursor.nth(idx + 1).kind.is_ident_like() {
                return false;
            }
            saw_dot = true;
            idx += 2;
        }
        if !saw_dot || self.cursor.nth(idx).kind != TokenKind::Punct(Punct::LParen) {
            return false;
        }
        let mut depth = 0usize;
        loop {
            match self.cursor.nth(idx).kind {
                TokenKind::Punct(Punct::LParen) => depth += 1,
                TokenKind::Punct(Punct::RParen) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return self.cursor.nth(idx + 1).kind == TokenKind::Punct(Punct::FatArrow);
                    }
                }
                TokenKind::Eof | TokenKind::Punct(Punct::RBrace) => return false,
                _ => {}
            }
            idx += 1;
        }
    }

    pub(crate) fn map_expr(&mut self) -> Expr {
        let start = self.expect_punct(Punct::LBrace, "expected `{` in map expression");
        if self.cursor.at_punct(Punct::RBrace) {
            let end = self.expect_punct(Punct::RBrace, "expected `}` after empty brace literal");
            return Expr::EmptyRecordOrMap {
                span: start.cover(end),
            };
        }

        let mut entries = Vec::new();
        while !self.cursor.at_eof() && !self.cursor.at_punct(Punct::RBrace) {
            let before = self.cursor.position();
            let key = self.expr();
            self.expect_punct(Punct::FatArrow, "expected `=>` in map entry");
            let value = self.expr();
            let span = key.span().cover(value.span());
            entries.push(MapEntry { key, value, span });
            if self.cursor.eat_punct(Punct::Comma).is_none() {
                break;
            }
            self.ensure_progress(before, "expected map entry");
        }
        let end = self.expect_punct(Punct::RBrace, "expected `}` after map expression");
        Expr::Map(MapExpr {
            entries,
            span: start.cover(end),
        })
    }

    pub(crate) fn set_expr(&mut self) -> Expr {
        let hash = self.expect_punct(Punct::Hash, "expected `#` in set expression");
        self.expect_punct(Punct::LBrace, "expected `{` after `#` in set expression");
        let elems = self.comma_list(Punct::RBrace, |this| this.expr());
        let end = self.expect_punct(Punct::RBrace, "expected `}` after set expression");
        Expr::Set {
            elems,
            span: hash.cover(end),
        }
    }

    pub(crate) fn field_init(&mut self) -> FieldInit {
        let name = self.name("expected field name");
        if self.cursor.eat_punct(Punct::Eq).is_some() {
            let value = self.expr();
            FieldInit::Named { name, value }
        } else {
            FieldInit::Shorthand(name)
        }
    }

    pub(crate) fn list_expr(&mut self) -> Expr {
        let start = self.expect_punct(Punct::LBracket, "expected `[`");
        if self.cursor.at_punct(Punct::RBracket) {
            let end = self.expect_punct(Punct::RBracket, "expected `]` after sequence literal");
            return Expr::EmptySequence {
                span: start.cover(end),
            };
        }

        let first = self.expr();
        if self.cursor.eat_punct(Punct::Semi).is_some() {
            let mut elems = vec![first];
            while !self.cursor.at_eof() && !self.cursor.at_punct(Punct::RBracket) {
                let before = self.cursor.position();
                elems.push(self.expr());
                if self.cursor.eat_punct(Punct::Semi).is_none() {
                    break;
                }
                self.ensure_progress(before, "expected list element");
            }
            let end = self.expect_punct(Punct::RBracket, "expected `]` after list");
            return Expr::List {
                elems,
                span: start.cover(end),
            };
        }

        let mut elems = vec![first];
        if self.cursor.eat_punct(Punct::Comma).is_some() && !self.cursor.at_punct(Punct::RBracket) {
            let end_expr = self.expr();
            if self.cursor.at_punct(Punct::RParen) {
                let end = self.expect_punct(Punct::RParen, "expected `)` after range");
                return Expr::Range(RangeExpr {
                    start: Box::new(elems.remove(0)),
                    end: Box::new(end_expr),
                    bounds: RangeBounds::ClosedOpen,
                    span: start.cover(end),
                });
            }
            elems.push(end_expr);
            while !self.cursor.at_eof() && !self.cursor.at_punct(Punct::RBracket) {
                let before = self.cursor.position();
                if self.cursor.eat_punct(Punct::Comma).is_none() {
                    break;
                }
                if self.cursor.at_punct(Punct::RBracket) {
                    break;
                }
                elems.push(self.expr());
                self.ensure_progress(before, "expected array element");
            }
        }
        let end = self.expect_punct(Punct::RBracket, "expected `]` after list");
        Expr::Array {
            elems,
            span: start.cover(end),
        }
    }

    pub(crate) fn tuple_or_group_expr(&mut self) -> Expr {
        let start = self.expect_punct(Punct::LParen, "expected `(`");
        if self.cursor.eat_punct(Punct::RParen).is_some() {
            return Expr::Tuple {
                elems: Vec::new(),
                span: start.cover(self.previous_span()),
            };
        }
        let first = self.expr();
        if self.cursor.eat_punct(Punct::Comma).is_some() {
            if self.cursor.at_punct(Punct::RParen) {
                let end = self.expect_punct(Punct::RParen, "expected `)` after tuple");
                return Expr::Tuple {
                    elems: vec![first],
                    span: start.cover(end),
                };
            }
            let end_expr = self.expr();
            if self.cursor.at_punct(Punct::RBracket) {
                let end = self.expect_punct(Punct::RBracket, "expected `]` after range");
                return Expr::Range(RangeExpr {
                    start: Box::new(first),
                    end: Box::new(end_expr),
                    bounds: RangeBounds::OpenClosed,
                    span: start.cover(end),
                });
            }
            let mut elems = vec![first];
            elems.push(end_expr);
            while !self.cursor.at_eof() && !self.cursor.at_punct(Punct::RParen) {
                let before = self.cursor.position();
                if self.cursor.eat_punct(Punct::Comma).is_none() {
                    break;
                }
                if self.cursor.at_punct(Punct::RParen) {
                    break;
                }
                elems.push(self.expr());
                self.ensure_progress(before, "expected tuple element");
            }
            let end = self.expect_punct(Punct::RParen, "expected `)` after tuple");
            Expr::Tuple {
                elems,
                span: start.cover(end),
            }
        } else {
            self.expect_punct(Punct::RParen, "expected `)` after expression");
            first
        }
    }

    pub(crate) fn if_expr(&mut self) -> Expr {
        let start = self.expect_keyword(Keyword::If, "expected `if`");
        let condition = self.expr_before_block();
        let then_branch = self.block();
        self.expect_keyword(Keyword::Else, "expected `else` in if expression");
        let else_branch = self.block();
        Expr::If(IfExpr {
            span: start.cover(else_branch.span),
            condition: Box::new(condition),
            then_branch,
            else_branch,
        })
    }

    pub(crate) fn match_expr(&mut self) -> Expr {
        let start = self.expect_keyword(Keyword::Match, "expected `match`");
        let scrutinee = self.expr_before_block();
        let arms = self.match_arms();
        let span = arms.last().map_or(scrutinee.span(), |arm| arm.span);
        Expr::Match(MatchExpr {
            scrutinee: Box::new(scrutinee),
            arms,
            span: start.cover(span),
        })
    }

    pub(crate) fn perform_expr(&mut self) -> Expr {
        let start = self.expect_keyword(Keyword::Perform, "expected `perform`");
        let (effect, action) = self.effect_action_ref("expected effect action path");
        let generic_args = self.generic_args();
        let args = self.arg_list();
        let span = start.cover(self.previous_span());
        Expr::Perform(PerformExpr {
            effect,
            action,
            generic_args,
            args,
            span,
        })
    }

    pub(crate) fn handle_expr(&mut self) -> Expr {
        let start = self.expect_keyword(Keyword::Handle, "expected `handle`");
        self.suppress_next_handler_with = true;
        let body = self.expr();
        self.expect_keyword(Keyword::With, "expected `with` after handle body");
        let handler = self.handler_arg_expr();
        let span = start.cover(handler.span());
        Expr::Handle(HandleExpr {
            body: Box::new(body),
            handler,
            span,
        })
    }

    pub(crate) fn handler_arg_expr(&mut self) -> HandlerArg {
        if self.cursor.at_punct(Punct::LBrace) {
            return HandlerArg::Block(self.handler_block());
        }
        if self.cursor.at_keyword(Keyword::Handler) {
            let span = self.cursor.peek().span;
            self.diagnostics.push(Diagnostic::syntax(
                SyntaxDiagnosticCode::InvalidExpression,
                span,
                "`with handler { ... }` is not valid handler application syntax; use `with { ... }`",
            ));
        }
        HandlerArg::Expr(Box::new(self.expr()))
    }

    pub(crate) fn handler_expr(&mut self) -> Expr {
        let start = self.expect_keyword(Keyword::Handler, "expected `handler`");
        let block = self.handler_block();
        Expr::Handler(HandlerExpr {
            span: start.cover(block.span),
            block,
        })
    }

    pub(crate) fn handler_block(&mut self) -> HandlerBlock {
        let start = self.expect_punct(Punct::LBrace, "expected handler block");
        let mut handlers = Vec::new();
        while !self.cursor.at_eof() && !self.cursor.at_punct(Punct::RBrace) {
            handlers.push(self.handler_arm());
            self.cursor.eat_punct(Punct::Comma);
            self.cursor.eat_punct(Punct::Semi);
        }
        let end = self.expect_punct(Punct::RBrace, "expected `}` after handler block");
        HandlerBlock {
            arms: handlers,
            span: start.cover(end),
        }
    }

    pub(crate) fn handler_arm(&mut self) -> HandlerArm {
        let before = self.cursor.position();
        let (effect, action) = self.effect_action_ref("expected handler action path");
        let generic_args = self.generic_args();
        self.expect_punct(Punct::LParen, "expected handler pattern list");
        let patterns = self.comma_list(Punct::RParen, |this| this.pattern());
        self.expect_punct(Punct::RParen, "expected `)` after handler patterns");
        self.expect_punct(Punct::FatArrow, "expected `=>` before handler body");
        let body = if self.cursor.at_punct(Punct::LBrace) {
            HandlerArmBody::Block(self.block())
        } else {
            if !self.cursor.at_keyword(Keyword::Resume) && !self.cursor.at_keyword(Keyword::Finish)
            {
                let span = self.cursor.peek().span;
                self.diagnostics.push(Diagnostic::syntax(
                    SyntaxDiagnosticCode::InvalidExpression,
                    span,
                    "handler arm shorthand must be `resume value;` or `finish value;`; use `{ ... }` for a handler arm block",
                ));
            }
            HandlerArmBody::Stmt(Box::new(self.stmt()))
        };
        let span = effect.span.cover(body.span());
        self.ensure_progress(before, "expected handler arm");
        HandlerArm {
            effect,
            action,
            generic_args,
            patterns,
            body,
            span,
        }
    }

    pub(crate) fn binary_op(&self) -> Option<(BinaryOp, u8)> {
        let op = match self.cursor.peek().kind {
            TokenKind::Punct(Punct::PipePipe) => (BinaryOp::OrOr, 1),
            TokenKind::Punct(Punct::AmpAmp) => (BinaryOp::AndAnd, 2),
            TokenKind::Punct(Punct::EqEq) => (BinaryOp::EqEq, 3),
            TokenKind::Punct(Punct::BangEq) => (BinaryOp::BangEq, 3),
            TokenKind::Punct(Punct::Lt) => (BinaryOp::Lt, 4),
            TokenKind::Punct(Punct::LtEq) => (BinaryOp::LtEq, 4),
            TokenKind::Punct(Punct::Gt) => (BinaryOp::Gt, 4),
            TokenKind::Punct(Punct::GtEq) => (BinaryOp::GtEq, 4),
            TokenKind::Punct(Punct::Plus) => (BinaryOp::Add, 5),
            TokenKind::Punct(Punct::Minus) => (BinaryOp::Sub, 5),
            TokenKind::Punct(Punct::Star) => (BinaryOp::Mul, 6),
            TokenKind::Punct(Punct::Slash) => (BinaryOp::Div, 6),
            TokenKind::Punct(Punct::Percent) => (BinaryOp::Rem, 6),
            _ => return None,
        };
        Some(op)
    }

    pub(crate) fn looks_like_name_list_angle(&self) -> bool {
        let mut idx = 1usize;
        while matches!(
            self.cursor.nth(idx).kind,
            TokenKind::Ident | TokenKind::Keyword(Keyword::Effect)
        ) {
            if matches!(
                self.cursor.nth(idx).kind,
                TokenKind::Keyword(Keyword::Effect)
            ) {
                idx += 1;
                if !self.cursor.nth(idx).kind.is_ident_like() {
                    return false;
                }
                idx += 1;
                if matches!(self.cursor.nth(idx).kind, TokenKind::Punct(Punct::Comma)) {
                    idx += 1;
                    continue;
                }
                break;
            }
            idx += 1;
            if matches!(self.cursor.nth(idx).kind, TokenKind::Punct(Punct::Tilde)) {
                idx += 1;
                if matches!(
                    self.cursor.nth(idx).kind,
                    TokenKind::Keyword(Keyword::Effect)
                ) {
                    idx += 1;
                } else {
                    while self.cursor.nth(idx).kind.is_ident_like() {
                        idx += 1;
                        while matches!(self.cursor.nth(idx).kind, TokenKind::Punct(Punct::Dot)) {
                            idx += 1;
                            if !self.cursor.nth(idx).kind.is_ident_like() {
                                return false;
                            }
                            idx += 1;
                        }
                        if matches!(self.cursor.nth(idx).kind, TokenKind::Punct(Punct::Lt)) {
                            let mut depth = 0usize;
                            loop {
                                match self.cursor.nth(idx).kind {
                                    TokenKind::Punct(Punct::Lt) => depth += 1,
                                    TokenKind::Punct(Punct::Gt) => {
                                        depth = depth.saturating_sub(1);
                                        idx += 1;
                                        if depth == 0 {
                                            break;
                                        }
                                        continue;
                                    }
                                    TokenKind::Eof => return false,
                                    _ => {}
                                }
                                idx += 1;
                            }
                        }
                        if matches!(self.cursor.nth(idx).kind, TokenKind::Punct(Punct::Plus)) {
                            idx += 1;
                        } else {
                            break;
                        }
                    }
                }
            }
            if matches!(self.cursor.nth(idx).kind, TokenKind::Punct(Punct::Comma)) {
                idx += 1;
            } else {
                break;
            }
        }
        matches!(self.cursor.nth(idx).kind, TokenKind::Punct(Punct::Gt))
    }

    fn looks_like_spec_method_selection(&self) -> bool {
        if self.cursor.nth(0).kind != TokenKind::Punct(Punct::ColonColon) {
            return false;
        }
        let mut idx = 1usize;
        if !self.cursor.nth(idx).kind.is_ident_like() {
            return false;
        }
        loop {
            idx += 1;
            if self.cursor.nth(idx).kind == TokenKind::Punct(Punct::Lt) {
                let mut depth = 0usize;
                loop {
                    match self.cursor.nth(idx).kind {
                        TokenKind::Punct(Punct::Lt) => depth += 1,
                        TokenKind::Punct(Punct::Gt) => {
                            depth = depth.saturating_sub(1);
                            idx += 1;
                            if depth == 0 {
                                break;
                            }
                            continue;
                        }
                        TokenKind::Eof => return false,
                        _ => {}
                    }
                    idx += 1;
                }
            }
            if self.cursor.nth(idx).kind != TokenKind::Punct(Punct::Dot) {
                return false;
            }
            if !self.cursor.nth(idx + 1).kind.is_ident_like() {
                return false;
            }
            if self.cursor.nth(idx + 2).kind == TokenKind::Punct(Punct::LParen) {
                return true;
            }
            idx += 1;
            if idx > 96 {
                return false;
            }
        }
    }

    pub(crate) fn looks_like_lambda_param_list(&self) -> bool {
        let mut depth = 0usize;
        for idx in 0..96 {
            match self.cursor.nth(idx).kind {
                TokenKind::Punct(Punct::LParen) => depth += 1,
                TokenKind::Punct(Punct::RParen) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return matches!(
                            self.cursor.nth(idx + 1).kind,
                            TokenKind::Punct(Punct::FatArrow)
                        );
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
        }
        false
    }

    pub(crate) fn bracket_group_followed_by(&self, punct: Punct) -> bool {
        let mut depth = 0usize;
        for idx in 0..96 {
            match self.cursor.nth(idx).kind {
                TokenKind::Punct(Punct::LBracket) | TokenKind::Punct(Punct::Lt) => depth += 1,
                TokenKind::Punct(Punct::RBracket) | TokenKind::Punct(Punct::Gt) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return self.cursor.nth(idx + 1).kind == TokenKind::Punct(punct);
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
        }
        false
    }

    fn open_closed_slice_follows(&self) -> bool {
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        let mut brace_depth = 0usize;
        let mut saw_comma = false;
        for idx in 0..96 {
            match self.cursor.nth(idx).kind {
                TokenKind::Punct(Punct::LParen) => paren_depth += 1,
                TokenKind::Punct(Punct::RParen) => {
                    if paren_depth == 1 && bracket_depth == 0 && brace_depth == 0 {
                        return false;
                    }
                    paren_depth = paren_depth.saturating_sub(1);
                }
                TokenKind::Punct(Punct::LBracket) => bracket_depth += 1,
                TokenKind::Punct(Punct::RBracket) => {
                    if paren_depth == 1 && bracket_depth == 0 && brace_depth == 0 {
                        return saw_comma;
                    }
                    bracket_depth = bracket_depth.saturating_sub(1);
                }
                TokenKind::Punct(Punct::LBrace) => brace_depth += 1,
                TokenKind::Punct(Punct::RBrace) => brace_depth = brace_depth.saturating_sub(1),
                TokenKind::Punct(Punct::Comma)
                    if paren_depth == 1 && bracket_depth == 0 && brace_depth == 0 =>
                {
                    saw_comma = true;
                }
                TokenKind::Eof => return false,
                _ => {}
            }
        }
        false
    }
}

fn lambda_body_span(body: &LambdaBody) -> Span {
    match body {
        LambdaBody::Expr(expr) => expr.span(),
        LambdaBody::Block(block) => block.span,
    }
}

pub(super) fn unquote_string(text: &str) -> String {
    let text = text.strip_prefix('"').unwrap_or(text);
    let text = text.strip_suffix('"').unwrap_or(text);
    text.replace("\\\"", "\"")
        .replace("\\n", "\n")
        .replace("\\r", "\r")
        .replace("\\t", "\t")
}

pub(super) fn unquote_char(text: &str) -> Option<char> {
    let inner = text.strip_prefix('\'')?.strip_suffix('\'')?;
    match inner {
        "\\n" => Some('\n'),
        "\\r" => Some('\r'),
        "\\t" => Some('\t'),
        "\\'" => Some('\''),
        "\\\\" => Some('\\'),
        _ => {
            let mut chars = inner.chars();
            let value = chars.next()?;
            chars.next().is_none().then_some(value)
        }
    }
}
