use crate::{Diagnostic, Keyword, Punct, SyntaxDiagnosticCode, ast::*};

use crate::parser::Parser;

impl Parser<'_> {
    pub(crate) fn agent_decl(&mut self, visibility: Visibility) -> AgentDecl {
        let start = self.expect_keyword(Keyword::Agent, "expected `agent`");
        let name = self.name("expected agent name");
        let type_params = self.type_params();
        if self.cursor.eat_punct(Punct::Eq).is_some() {
            let mut state = self.type_expr();
            if let TypeExpr::Record(record) = &mut state {
                for field in &mut record.fields {
                    if field.visibility == Some(Visibility::Public) {
                        self.diagnostics.push(Diagnostic::syntax(
                            SyntaxDiagnosticCode::UnexpectedToken,
                            field.span,
                            "agent state fields are private",
                        ));
                    }
                    field.visibility = Some(Visibility::Private);
                }
            } else {
                self.diagnostics.push(Diagnostic::syntax(
                    SyntaxDiagnosticCode::UnexpectedToken,
                    state.span(),
                    "agent state must be a record declaration",
                ));
            }
            let semicolon_span =
                self.expect_punct(Punct::Semi, "expected `;` after agent state declaration");
            return AgentDecl {
                visibility,
                name,
                type_params,
                state: Some(state),
                params: Vec::new(),
                output_type: None,
                effects: None,
                conformances: Vec::new(),
                body: AgentBody::Decl { semicolon_span },
                span: start.cover(semicolon_span),
            };
        }
        if self.cursor.at_punct(Punct::Semi) {
            let semicolon_span =
                self.expect_punct(Punct::Semi, "expected `;` after agent declaration");
            return AgentDecl {
                visibility,
                name,
                type_params,
                state: None,
                params: Vec::new(),
                output_type: None,
                effects: None,
                conformances: Vec::new(),
                body: AgentBody::Decl { semicolon_span },
                span: start.cover(semicolon_span),
            };
        }
        let params = if self.cursor.at_punct(Punct::LParen) {
            self.param_list()
        } else {
            Vec::new()
        };
        self.expect_punct(Punct::Arrow, "expected agent output type");
        let output_type = Some(self.type_expr());
        let effects = self.effect_suffix();
        let conformances = self.declaration_conformances();
        self.recover_obsolete_agent_config_row();
        let body = if self.cursor.at_punct(Punct::LBrace) {
            let body_start = self.expect_punct(Punct::LBrace, "expected `{` in agent declaration");
            AgentBody::Source(self.block_after_lbrace(body_start))
        } else {
            let span = self.expect_punct(Punct::LBrace, "expected `{` in agent declaration");
            AgentBody::Error(span)
        };
        let span = start.cover(body.span());
        AgentDecl {
            visibility,
            name,
            type_params,
            state: None,
            params,
            output_type,
            effects,
            conformances,
            body,
            span,
        }
    }

    fn recover_obsolete_agent_config_row(&mut self) {
        if !self.cursor.at_punct(Punct::LBracket) {
            return;
        }
        let start = self.cursor.bump().span;
        self.diagnostics.push(Diagnostic::syntax(
            SyntaxDiagnosticCode::UnexpectedToken,
            start,
            "agent config rows are obsolete; use item annotations such as `@model`, `@tools`, and `@limits`",
        ));
        let mut depth = 1usize;
        while !self.cursor.at_eof() && depth > 0 {
            let token = self.cursor.bump();
            match token.kind {
                crate::TokenKind::Punct(Punct::LBracket) => depth += 1,
                crate::TokenKind::Punct(Punct::RBracket) => depth -= 1,
                _ => {}
            }
        }
    }
}
