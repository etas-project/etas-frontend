use super::expr::{unquote_char, unquote_string};
use crate::{Diagnostic, Keyword, Punct, Span, SyntaxDiagnosticCode, TokenKind, ast::*};

use crate::parser::Parser;

impl Parser<'_> {
    pub(crate) fn block(&mut self) -> Block {
        let start = self.expect_punct(Punct::LBrace, "expected block");
        self.block_after_lbrace(start)
    }

    pub(crate) fn block_after_lbrace(&mut self, start: Span) -> Block {
        let mut stmts = Vec::new();
        let mut final_expr = None;
        while !self.cursor.at_eof() && !self.cursor.at_punct(Punct::RBrace) {
            let before = self.cursor.position();
            if self.starts_stmt_keyword() {
                stmts.push(self.stmt());
                self.ensure_progress(before, "expected statement");
                continue;
            }
            if self.starts_assignment_stmt() {
                stmts.push(self.assign_stmt());
                self.ensure_progress(before, "expected assignment statement");
                continue;
            }
            let expr = self.expr();
            if let Expr::Error(span) = expr {
                self.recover_stmt();
                stmts.push(Stmt::Error(span));
            } else if self.cursor.eat_punct(Punct::Semi).is_some() {
                let span = expr.span();
                stmts.push(Stmt::Expr(ExprStmt { expr, span }));
            } else {
                final_expr = Some(Box::new(expr));
                break;
            }
            self.ensure_progress(before, "expected statement or final expression");
        }
        let end = self.expect_punct(Punct::RBrace, "expected `}` after block");
        Block {
            stmts,
            final_expr,
            span: start.cover(end),
        }
    }

    pub(crate) fn stmt(&mut self) -> Stmt {
        if self.cursor.at_keyword(Keyword::Let) {
            return Stmt::Let(self.let_stmt());
        }
        if self.cursor.at_keyword(Keyword::Var) {
            return Stmt::Var(self.var_stmt());
        }
        if self.cursor.at_keyword(Keyword::If) {
            let stmt = Stmt::If(self.if_stmt());
            return self.with_optional_trailing_semicolon(stmt);
        }
        if self.cursor.at_keyword(Keyword::Match) {
            let stmt = Stmt::Match(self.match_stmt());
            return self.with_optional_trailing_semicolon(stmt);
        }
        if self.cursor.at_keyword(Keyword::For) {
            let stmt = Stmt::For(self.for_stmt());
            return self.with_optional_trailing_semicolon(stmt);
        }
        if self.cursor.at_keyword(Keyword::While) {
            let stmt = Stmt::While(self.while_stmt());
            return self.with_optional_trailing_semicolon(stmt);
        }
        if self.cursor.at_keyword(Keyword::Retry) {
            let stmt = Stmt::Retry(self.retry_stmt());
            return self.with_optional_trailing_semicolon(stmt);
        }
        if self.cursor.at_keyword(Keyword::Resume) {
            return Stmt::Resume(self.resume_stmt());
        }
        if self.cursor.at_keyword(Keyword::Finish) {
            return Stmt::Finish(self.finish_stmt());
        }
        if self.cursor.at_keyword(Keyword::Return) {
            return Stmt::Return(self.return_stmt());
        }
        if let Some(token) = self.cursor.eat_keyword(Keyword::Break) {
            let end = self.expect_punct(Punct::Semi, "expected `;` after break");
            return Stmt::Break(token.span.cover(end));
        }
        if let Some(token) = self.cursor.eat_keyword(Keyword::Continue) {
            let end = self.expect_punct(Punct::Semi, "expected `;` after continue");
            return Stmt::Continue(token.span.cover(end));
        }

        if self.starts_assignment_stmt() {
            return self.assign_stmt();
        }

        let expr = self.expr();
        if let Expr::Error(span) = expr {
            self.recover_stmt();
            Stmt::Error(span)
        } else {
            let end = self.expect_punct(Punct::Semi, "expected `;` after expression");
            Stmt::Expr(ExprStmt {
                span: expr.span().cover(end),
                expr,
            })
        }
    }

    fn with_optional_trailing_semicolon(&mut self, stmt: Stmt) -> Stmt {
        let Some(semi) = self.cursor.eat_punct(Punct::Semi) else {
            return stmt;
        };
        match stmt {
            Stmt::If(mut stmt) => {
                stmt.span = stmt.span.cover(semi.span);
                Stmt::If(stmt)
            }
            Stmt::Match(mut stmt) => {
                stmt.span = stmt.span.cover(semi.span);
                Stmt::Match(stmt)
            }
            Stmt::For(mut stmt) => {
                stmt.span = stmt.span.cover(semi.span);
                Stmt::For(stmt)
            }
            Stmt::While(mut stmt) => {
                stmt.span = stmt.span.cover(semi.span);
                Stmt::While(stmt)
            }
            Stmt::Retry(mut stmt) => {
                stmt.span = stmt.span.cover(semi.span);
                Stmt::Retry(stmt)
            }
            other => other,
        }
    }

    pub(crate) fn assign_stmt(&mut self) -> Stmt {
        let target = self.assign_target_expr();
        self.expect_punct(Punct::Eq, "expected `=` in assignment statement");
        let value = self.expr();
        let end = self.expect_punct(Punct::Semi, "expected `;` after assignment");
        Stmt::Assign(AssignStmt {
            span: target.span().cover(end),
            target,
            value,
        })
    }

    pub(crate) fn assign_target_expr(&mut self) -> Expr {
        let mut target = Expr::Path(self.path());
        loop {
            if self.eat_open_punct(Punct::LBracket).is_some() {
                let index = self.expr();
                let end = self.expect_punct(Punct::RBracket, "expected `]` after index");
                let span = target.span().cover(end);
                target = Expr::Index(IndexExpr {
                    receiver: Box::new(target),
                    index: Box::new(index),
                    span,
                });
            } else if self.cursor.eat_punct(Punct::Dot).is_some() {
                let field = self.name("expected field name after `.`");
                let span = target.span().cover(field.span);
                target = Expr::Field(FieldExpr {
                    receiver: Box::new(target),
                    field,
                    span,
                });
            } else {
                break;
            }
        }
        target
    }

    pub(crate) fn let_stmt(&mut self) -> LetStmt {
        let start = self.expect_keyword(Keyword::Let, "expected `let`");
        let pattern = self.pattern();
        let ty = self.type_annotation();
        self.expect_punct(Punct::Eq, "expected `=` in let statement");
        let value = self.expr();
        let end = self.expect_punct(Punct::Semi, "expected `;` after let statement");
        LetStmt {
            pattern,
            ty,
            value,
            span: start.cover(end),
        }
    }

    pub(crate) fn var_stmt(&mut self) -> VarStmt {
        let start = self.expect_keyword(Keyword::Var, "expected `var`");
        let pattern = self.pattern();
        let ty = self.type_annotation();
        self.expect_punct(Punct::Eq, "expected `=` in var statement");
        let value = self.expr();
        let end = self.expect_punct(Punct::Semi, "expected `;` after var statement");
        VarStmt {
            pattern,
            ty,
            value,
            span: start.cover(end),
        }
    }

    pub(crate) fn if_stmt(&mut self) -> IfStmt {
        let start = self.expect_keyword(Keyword::If, "expected `if`");
        let condition = self.expr_before_block();
        let then_branch = self.block();
        let else_branch = if self.cursor.eat_keyword(Keyword::Else).is_some() {
            if self.cursor.at_keyword(Keyword::If) {
                Some(ElseBranch::If(Box::new(self.if_stmt())))
            } else {
                Some(ElseBranch::Block(self.block()))
            }
        } else {
            None
        };
        let span = else_branch
            .as_ref()
            .map_or(then_branch.span, |branch| match branch {
                ElseBranch::If(stmt) => stmt.span,
                ElseBranch::Block(block) => block.span,
            });
        IfStmt {
            condition,
            then_branch,
            else_branch,
            span: start.cover(span),
        }
    }

    pub(crate) fn match_stmt(&mut self) -> MatchStmt {
        let start = self.expect_keyword(Keyword::Match, "expected `match`");
        let scrutinee = self.expr_before_block();
        let arms = self.match_arms();
        let span = arms.last().map_or(scrutinee.span(), |arm| arm.span);
        MatchStmt {
            scrutinee,
            arms,
            span: start.cover(span),
        }
    }

    pub(crate) fn match_arms(&mut self) -> Vec<MatchArm> {
        self.expect_punct(Punct::LBrace, "expected `{` in match");
        let mut arms = Vec::new();
        while !self.cursor.at_eof() && !self.cursor.at_punct(Punct::RBrace) {
            let before = self.cursor.position();
            let pattern = self.pattern();
            self.expect_punct(Punct::FatArrow, "expected `=>` in match arm");
            let body = if self.cursor.at_punct(Punct::LBrace) {
                MatchArmBody::Block(self.block())
            } else {
                MatchArmBody::Expr(self.expr())
            };
            let span = match &body {
                MatchArmBody::Expr(expr) => pattern.span().cover(expr.span()),
                MatchArmBody::Block(block) => pattern.span().cover(block.span),
            };
            arms.push(MatchArm {
                pattern,
                body,
                span,
            });
            self.cursor.eat_punct(Punct::Comma);
            self.ensure_progress(before, "expected match arm");
        }
        self.expect_punct(Punct::RBrace, "expected `}` after match arms");
        arms
    }

    pub(crate) fn for_stmt(&mut self) -> ForStmt {
        let start = self.expect_keyword(Keyword::For, "expected `for`");
        let pattern = self.pattern();
        self.expect_keyword(Keyword::In, "expected `in` in for statement");
        let iter = self.expr_before_block();
        let limits = self.limit_clauses();
        let body = self.block();
        ForStmt {
            pattern,
            iter,
            limits,
            span: start.cover(body.span),
            body,
        }
    }

    pub(crate) fn while_stmt(&mut self) -> WhileStmt {
        let start = self.expect_keyword(Keyword::While, "expected `while`");
        let condition = self.expr_before_block();
        let limits = self.limit_clauses();
        let body = self.block();
        WhileStmt {
            condition,
            limits,
            span: start.cover(body.span),
            body,
        }
    }

    pub(crate) fn retry_stmt(&mut self) -> RetryStmt {
        let start = self.expect_keyword(Keyword::Retry, "expected `retry`");
        let limits = self.limit_clauses();
        let body = self.block();
        RetryStmt {
            limits,
            span: start.cover(body.span),
            body,
        }
    }

    pub(crate) fn limit_clauses(&mut self) -> Vec<Expr> {
        let mut limits = Vec::new();
        while self.cursor.eat_keyword(Keyword::Limit).is_some() {
            limits.extend(self.limit_list());
        }
        limits
    }

    pub(crate) fn limit_list(&mut self) -> Vec<Expr> {
        let mut limits = Vec::new();
        loop {
            limits.push(self.expr());
            if self.cursor.eat_punct(Punct::Comma).is_none() {
                break;
            }
            if self.cursor.at_punct(Punct::LBrace) {
                break;
            }
        }
        limits
    }

    pub(crate) fn starts_assignment_stmt(&self) -> bool {
        if !self.cursor.at_ident_like() {
            return false;
        }

        let mut depth = 0usize;
        for idx in 0..64 {
            match self.cursor.nth(idx).kind {
                crate::TokenKind::Punct(Punct::LParen | Punct::LBracket | Punct::LBrace) => {
                    depth += 1;
                }
                crate::TokenKind::Punct(Punct::RParen | Punct::RBracket | Punct::RBrace) => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                crate::TokenKind::Punct(Punct::Eq) if depth == 0 => return true,
                crate::TokenKind::Punct(Punct::Semi) if depth == 0 => return false,
                crate::TokenKind::Eof => return false,
                _ => {}
            }
        }
        false
    }

    pub(crate) fn resume_stmt(&mut self) -> ResumeStmt {
        let start = self.expect_keyword(Keyword::Resume, "expected `resume`");
        let value = (!self.cursor.at_punct(Punct::Semi)).then(|| self.expr());
        let end = self.expect_punct(Punct::Semi, "expected `;` after resume statement");
        ResumeStmt {
            value,
            span: start.cover(end),
        }
    }

    pub(crate) fn finish_stmt(&mut self) -> FinishStmt {
        let start = self.expect_keyword(Keyword::Finish, "expected `finish`");
        let value = self.expr();
        let end = self.expect_punct(Punct::Semi, "expected `;` after finish statement");
        FinishStmt {
            value,
            span: start.cover(end),
        }
    }

    pub(crate) fn return_stmt(&mut self) -> ReturnStmt {
        let start = self.expect_keyword(Keyword::Return, "expected `return`");
        let value = (!self.cursor.at_punct(Punct::Semi)).then(|| self.expr());
        let end = self.expect_punct(Punct::Semi, "expected `;` after return statement");
        ReturnStmt {
            value,
            span: start.cover(end),
        }
    }

    pub(crate) fn pattern(&mut self) -> Pattern {
        if let Some(token) = self.eat_open_punct(Punct::LParen) {
            let elems = self.comma_list(Punct::RParen, |this| this.pattern());
            let end = self.expect_punct(Punct::RParen, "expected `)` after tuple pattern");
            return Pattern::Tuple {
                elems,
                span: token.span.cover(end),
            };
        }
        if self.cursor.at_keyword(Keyword::True) || self.cursor.at_keyword(Keyword::False) {
            let token = self.cursor.bump();
            return Pattern::Literal(Literal::Bool {
                value: token.kind == TokenKind::Keyword(Keyword::True),
                span: token.span,
            });
        }
        match self.cursor.peek().kind {
            TokenKind::IntLit => {
                let token = self.cursor.bump();
                return Pattern::Literal(Literal::Int {
                    text: self.slice(token.span).to_string(),
                    span: token.span,
                });
            }
            TokenKind::StringLit => {
                let token = self.cursor.bump();
                return Pattern::Literal(Literal::String {
                    value: unquote_string(self.slice(token.span)),
                    span: token.span,
                });
            }
            TokenKind::CharLit => {
                let token = self.cursor.bump();
                return match unquote_char(self.slice(token.span)) {
                    Some(value) => Pattern::Literal(Literal::Char {
                        value,
                        span: token.span,
                    }),
                    None => {
                        self.diagnostics.push(Diagnostic::syntax(
                            SyntaxDiagnosticCode::InvalidPattern,
                            token.span,
                            "character literal pattern must contain exactly one valid character",
                        ));
                        Pattern::Error(token.span)
                    }
                };
            }
            TokenKind::FloatLit => {
                let span = self.cursor.peek().span;
                self.bump_error(
                    SyntaxDiagnosticCode::InvalidPattern,
                    "floating-point literal patterns are not supported",
                );
                return Pattern::Error(span);
            }
            _ => {}
        }
        if self.cursor.at_ident_like() {
            let path = self.path();
            if self.cursor.at_punct(Punct::LBrace) {
                return self.record_pattern(Some(path));
            }
            if self.eat_open_punct(Punct::LParen).is_some() {
                let patterns = self.comma_list(Punct::RParen, |this| this.pattern());
                let end = self.expect_punct(Punct::RParen, "expected `)` after variant pattern");
                return Pattern::Variant(VariantPattern {
                    span: path.span.cover(end),
                    path,
                    patterns,
                });
            }
            if path.segments.len() == 1 {
                let name = path.segments.into_iter().next().unwrap();
                if name.text == "_" {
                    Pattern::Wildcard(name.span)
                } else {
                    Pattern::Ident(name)
                }
            } else {
                Pattern::Variant(VariantPattern {
                    span: path.span,
                    path,
                    patterns: Vec::new(),
                })
            }
        } else if self.cursor.at_punct(Punct::LBrace) {
            self.record_pattern(None)
        } else {
            let span = self.cursor.peek().span;
            self.bump_error(SyntaxDiagnosticCode::InvalidPattern, "expected pattern");
            Pattern::Error(span)
        }
    }

    pub(crate) fn record_pattern(&mut self, path: Option<Path>) -> Pattern {
        let start = path
            .as_ref()
            .map_or(self.cursor.peek().span, |path| path.span);
        self.expect_punct(Punct::LBrace, "expected `{` in record pattern");
        let mut fields = Vec::new();
        while !self.cursor.at_eof() && !self.cursor.at_punct(Punct::RBrace) {
            let before = self.cursor.position();
            let name = self.name("expected pattern field");
            let pattern = self.cursor.eat_punct(Punct::Colon).map(|_| self.pattern());
            let span = pattern
                .as_ref()
                .map_or(name.span, |pattern| name.span.cover(pattern.span()));
            fields.push(PatternField {
                name,
                pattern,
                span,
            });
            self.cursor.eat_punct(Punct::Comma);
            self.ensure_progress(before, "expected pattern field");
        }
        let end = self.expect_punct(Punct::RBrace, "expected `}` after record pattern");
        Pattern::Record(RecordPattern {
            path,
            fields,
            span: start.cover(end),
        })
    }
}
