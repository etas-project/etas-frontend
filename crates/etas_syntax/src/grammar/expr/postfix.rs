use super::*;

impl Parser<'_> {
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
            if self.call_follows_generic_arguments(idx + 2) {
                return true;
            }
            idx += 1;
        }
    }

    pub(super) fn call_follows_generic_arguments(&self, mut index: usize) -> bool {
        if self.cursor.nth(index).kind == TokenKind::Punct(Punct::Lt) {
            let mut depth = 0usize;
            loop {
                match self.cursor.nth(index).kind {
                    TokenKind::Punct(Punct::Lt) => depth += 1,
                    TokenKind::Punct(Punct::Gt) => {
                        depth -= 1;
                        if depth == 0 {
                            index += 1;
                            break;
                        }
                    }
                    TokenKind::Eof => return false,
                    _ => {}
                }
                index += 1;
            }
        }
        self.cursor.nth(index).kind == TokenKind::Punct(Punct::LParen)
    }
    pub(crate) fn postfix_expr(&mut self) -> Expr {
        let suppress_handler_with = std::mem::take(&mut self.suppress_next_handler_with);
        let mut expr = self.primary_expr();
        let mut chain = 0;
        loop {
            if !self.body_chain_step(&mut chain) {
                break;
            }
            let generic_args = if self.cursor.at_punct(Punct::Lt)
                && self.bracket_group_followed_by(Punct::LParen)
            {
                self.generic_args()
            } else {
                Vec::new()
            };
            if self.looks_like_spec_method_selection() {
                expr = self.spec_method_call_expr(expr);
            } else if self.cursor.at_punct(Punct::LParen) && self.open_closed_slice_follows() {
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
                let start = self.with_record_expressions(Self::expr);
                if self.cursor.eat_punct(Punct::Comma).is_some() {
                    let end_expr = self.with_record_expressions(Self::expr);
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
                && self.call_follows_generic_arguments(2)
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
        let generic_args = self.generic_args();
        let args = self.arg_list();
        let span = receiver.span().cover(self.previous_span());
        Expr::SpecMethodCall(SpecMethodCallExpr {
            receiver: Box::new(receiver),
            spec_path,
            spec_args,
            method,
            generic_args,
            args,
            span,
        })
    }
}
