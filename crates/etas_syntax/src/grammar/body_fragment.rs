use crate::{
    Diagnostic, Span, SyntaxDiagnosticCode, TextRange, TextSize, ast::Block, parser::Parser,
};

impl Parser<'_> {
    pub(crate) fn retained_fragment(
        &mut self,
        form: crate::RetainedFragmentForm,
    ) -> crate::retained_fragment::FragmentContents {
        use crate::{Keyword, Punct, RetainedFragmentForm as F, retained_fragment::FragmentBody};
        let body = match form {
            F::BlockContents => return (FragmentBody::Block(Box::new(self.body_contents())), None),
            F::Expression | F::HandledExpression => FragmentBody::Expression(Box::new(self.expr())),
            F::HandledBlock => FragmentBody::Block(Box::new(self.block())),
        };
        let handler = if matches!(form, F::HandledBlock | F::HandledExpression) {
            if form == F::HandledExpression {
                self.cursor.eat_punct(Punct::Semi);
            }
            self.expect_keyword(Keyword::With, "expected `with` in retained handled body");
            Some(self.handler_arg_expr())
        } else {
            None
        };
        if !self.cursor.at_eof() {
            self.diagnostics.push(Diagnostic::syntax(
                SyntaxDiagnosticCode::UnexpectedToken,
                self.cursor.peek().span,
                "unexpected input after retained body",
            ));
        }
        (body, handler)
    }

    pub(crate) fn body_contents(&mut self) -> Block {
        let (stmts, final_expr) = self.block_elements(None);
        if !self.cursor.at_eof() {
            let start = self.cursor.peek().span;
            let mut end = start;
            while !self.cursor.at_eof() {
                end = self.cursor.bump().span;
            }
            self.diagnostics.push(Diagnostic::syntax(
                SyntaxDiagnosticCode::UnexpectedToken,
                start.cover(end),
                "unexpected input after body tail expression",
            ));
        }
        Block {
            stmts,
            final_expr,
            span: Span::new(
                self.source.id,
                TextRange::new(TextSize::ZERO, TextSize::new(self.source.text().len())),
            ),
        }
    }
}
