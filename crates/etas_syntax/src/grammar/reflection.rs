use crate::{Diagnostic, Punct, Span, SyntaxDiagnosticCode, TokenKind, ast::*, parser::Parser};

impl Parser<'_> {
    pub(crate) fn reflect_expr(&mut self) -> Expr {
        let start = self.expect_punct(Punct::Caret, "expected `^`");
        if self.cursor.peek().kind != TokenKind::Ident {
            let span = start.cover(self.cursor.peek().span);
            self.bump_error(SyntaxDiagnosticCode::InvalidExpression,
                "reflection requires a definition or agent binding path, not an arbitrary expression");
            return Expr::Error(span);
        }
        let receiver = self.path();
        let mut end = receiver.span;
        let target = if self.cursor.eat_punct(Punct::ColonColon).is_some() {
            let mut spec = self.path();
            let (spec_args, method) = if self.cursor.at_punct(Punct::Lt) {
                let args = self.type_args();
                self.expect_punct(Punct::Dot, "expected `.` before reflected spec method");
                (args, self.name("expected reflected spec method"))
            } else if spec.segments.len() >= 2 {
                let method = spec.segments.remove(spec.segments.len() - 1);
                spec.span = spec.segments[0]
                    .span
                    .cover(spec.segments[spec.segments.len() - 1].span);
                (Vec::new(), method)
            } else {
                self.diagnostics.push(Diagnostic::syntax(
                    SyntaxDiagnosticCode::InvalidExpression,
                    spec.span,
                    "reflection of a spec selection requires `::Spec.method`",
                ));
                return Expr::Error(start.cover(spec.span));
            };
            end = method.span;
            ReflectTarget::SpecMethod {
                receiver,
                spec,
                spec_args,
                method,
            }
        } else {
            ReflectTarget::Path(receiver)
        };

        if self.cursor.at_punct(Punct::Lt) && self.bracket_group_followed_by(Punct::LParen) {
            self.generic_args();
        }
        if self.cursor.at_punct(Punct::LParen) {
            self.arg_list();
            return self.invalid_reflection_operation(start.cover(self.previous_span()), "call");
        }
        if self.cursor.at_punct(Punct::LBracket) {
            self.expect_punct(Punct::LBracket, "expected `[` ");
            self.expr();
            self.expect_punct(Punct::RBracket, "expected `]` after index");
            return self.invalid_reflection_operation(start.cover(self.previous_span()), "index");
        }
        Expr::Reflect(Box::new(ReflectExpr {
            target,
            span: start.cover(end),
        }))
    }

    fn invalid_reflection_operation(&mut self, span: Span, operation: &str) -> Expr {
        self.diagnostics.push(Diagnostic::syntax(
            SyntaxDiagnosticCode::InvalidExpression,
            span,
            format!("reflection selects a definition; a {operation} is not a reflection target"),
        ));
        Expr::Error(span)
    }
}
