use crate::{Keyword, Punct, SyntaxDiagnosticCode, TokenKind, ast::*};

use crate::parser::Parser;

impl Parser<'_> {
    pub(crate) fn type_expr(&mut self) -> TypeExpr {
        if self.cursor.at_punct(Punct::Bang) {
            return self.handler_type();
        }
        let input = self.primary_type();
        if self.cursor.eat_punct(Punct::Arrow).is_some() {
            let output = self.type_expr();
            let effect = self.effect_suffix();
            let span = effect
                .as_ref()
                .map_or(input.span().cover(output.span()), |effect| {
                    input.span().cover(effect.span)
                });
            TypeExpr::Arrow {
                effect,
                input: Box::new(input),
                output: Box::new(output),
                span,
            }
        } else {
            input
        }
    }

    pub(crate) fn primary_type(&mut self) -> TypeExpr {
        if self.cursor.at_punct(Punct::Bang) {
            return self.handler_type();
        }
        if self.eat_open_punct(Punct::LBrace).is_some() {
            return self.record_type();
        }
        if self.eat_open_punct(Punct::LParen).is_some() {
            let elems = self.comma_list(Punct::RParen, |this| this.type_expr());
            let end = self.expect_punct(Punct::RParen, "expected `)` after tuple type");
            return if elems.len() == 1 {
                elems.into_iter().next().unwrap()
            } else {
                let start = end;
                TypeExpr::Tuple {
                    elems,
                    span: start.cover(end),
                }
            };
        }
        if self.cursor.at_ident_like() {
            let path = self.path();
            let args = self.type_args();
            let mut ty = if args.is_empty() && path.segments.len() == 1 {
                if let Some(kind) = PrimitiveType::from_name(&path.segments[0].text) {
                    TypeExpr::Primitive {
                        kind,
                        span: path.span,
                    }
                } else {
                    TypeExpr::Path {
                        span: path.span,
                        path,
                        args,
                    }
                }
            } else {
                let span = args.last().map_or(path.span, TypeExpr::span);
                TypeExpr::Path {
                    span: path.span.cover(span),
                    path,
                    args,
                }
            };
            if self.cursor.eat_keyword(Keyword::Where).is_some() {
                let predicate = self.expr();
                let span = ty.span().cover(predicate.span());
                ty = TypeExpr::Refined {
                    base: Box::new(ty),
                    predicate: Box::new(predicate),
                    span,
                };
            }
            return ty;
        }
        let span = self.cursor.peek().span;
        self.bump_error(
            SyntaxDiagnosticCode::InvalidType,
            "expected type expression",
        );
        TypeExpr::Error(span)
    }

    pub(crate) fn handler_type(&mut self) -> TypeExpr {
        let bang = self.expect_punct(Punct::Bang, "expected `!` before handler type");
        let open = self.expect_punct(Punct::LBracket, "expected `[` after `!`");
        let handled = self.effect_row_until(open, |this| {
            this.cursor.at_punct(Punct::FatArrow)
                || this.cursor.at_keyword(Keyword::For)
                || this.cursor.at_punct(Punct::RBracket)
        });
        let produced = if self.cursor.eat_punct(Punct::FatArrow).is_some() {
            HandlerProducedEffects::Explicit(self.handler_produced_effect_row())
        } else {
            HandlerProducedEffects::Infer
        };
        let result = self
            .cursor
            .eat_keyword(Keyword::For)
            .map(|_| Box::new(self.type_expr()));
        let end = self.expect_punct(Punct::RBracket, "expected `]` after handler type");
        TypeExpr::Handler(HandlerType {
            handled,
            produced,
            result,
            span: bang.cover(end),
        })
    }

    pub(crate) fn record_type(&mut self) -> TypeExpr {
        let start = self.cursor.nth(0).span;
        let mut fields = Vec::new();
        while !self.cursor.at_eof() && !self.cursor.at_punct(Punct::RBrace) {
            let before = self.cursor.position();
            let visibility = if self.cursor.eat_keyword(Keyword::Private).is_some() {
                Some(Visibility::Private)
            } else if self.cursor.eat_keyword(Keyword::Public).is_some() {
                Some(Visibility::Public)
            } else {
                None
            };
            let name = self.name("expected record field");
            self.expect_punct(Punct::Colon, "expected `:` after record field");
            let ty = self.type_expr();
            let span = name.span.cover(ty.span());
            fields.push(crate::ast::ty::FieldDecl {
                visibility,
                name,
                ty,
                span,
            });
            self.cursor.eat_punct(Punct::Comma);
            self.ensure_progress(before, "expected record field");
        }
        let end = self.expect_punct(Punct::RBrace, "expected `}` after record type");
        TypeExpr::Record(RecordType {
            fields,
            span: start.cover(end),
        })
    }

    pub(crate) fn type_annotation(&mut self) -> Option<TypeExpr> {
        self.cursor
            .eat_punct(Punct::Colon)
            .map(|_| self.type_expr())
    }

    pub(crate) fn type_params(&mut self) -> Vec<TypeParam> {
        if !self.cursor.at_punct(Punct::Lt) || !self.looks_like_name_list_angle() {
            return Vec::new();
        }
        self.cursor.bump();
        let params = self.comma_list(Punct::Gt, |this| this.type_param());
        self.expect_punct(Punct::Gt, "expected `>` after type parameters");
        params
    }

    pub(crate) fn type_param(&mut self) -> TypeParam {
        if self.cursor.at_keyword(Keyword::Effect) && self.cursor.nth(1).kind.is_ident_like() {
            let effect = self.expect_keyword(Keyword::Effect, "expected `effect`");
            let name = self.name("expected effect type parameter name");
            let span = effect.cover(name.span);
            return TypeParam {
                name,
                kind: TypeParamKind::Effect,
                bounds: Vec::new(),
                span,
            };
        }
        let name = self.name("expected type parameter");
        let kind = TypeParamKind::Type;
        let mut bounds = Vec::new();
        let mut span = name.span;
        if self.cursor.eat_punct(Punct::Tilde).is_some() {
            loop {
                let bound = self.type_param_bound();
                span = span.cover(bound.span);
                bounds.push(bound);
                if self.cursor.eat_punct(Punct::Plus).is_none() {
                    break;
                }
            }
        }
        TypeParam {
            name,
            kind,
            bounds,
            span,
        }
    }

    pub(crate) fn type_param_bound(&mut self) -> TypeParamBound {
        let path = self.path();
        let args = self.type_args();
        let span = args.last().map_or(path.span, TypeExpr::span);
        TypeParamBound {
            span: path.span.cover(span),
            path,
            args,
        }
    }

    pub(crate) fn type_args(&mut self) -> Vec<TypeExpr> {
        if !self.cursor.at_punct(Punct::Lt) || self.looks_like_named_field_brackets() {
            return Vec::new();
        }
        self.cursor.bump();
        let args = self.comma_list(Punct::Gt, |this| this.type_expr());
        self.expect_punct(Punct::Gt, "expected `>` after type arguments");
        args
    }

    pub(crate) fn generic_args(&mut self) -> Vec<crate::ast::GenericArg> {
        if !self.cursor.at_punct(Punct::Lt) || self.looks_like_named_field_brackets() {
            return Vec::new();
        }
        self.cursor.bump();
        let args = self.comma_list(Punct::Gt, |this| this.generic_arg());
        self.expect_punct(Punct::Gt, "expected `>` after generic arguments");
        args
    }

    fn generic_arg(&mut self) -> crate::ast::GenericArg {
        if self.cursor.peek().kind == TokenKind::Ident && self.slice(self.cursor.peek().span) == "_"
        {
            let span = self.cursor.bump().span;
            return crate::ast::GenericArg::Wildcard { span };
        }
        if self.cursor.eat_punct(Punct::Bang).is_some() {
            crate::ast::GenericArg::EffectRow(self.effect_row())
        } else {
            crate::ast::GenericArg::Type(self.type_expr())
        }
    }

    pub(crate) fn effect_suffix(&mut self) -> Option<EffectRow> {
        self.cursor
            .eat_punct(Punct::Bang)
            .map(|_| self.effect_row())
    }

    pub(crate) fn effect_row(&mut self) -> EffectRow {
        let start = self.expect_punct(Punct::LBracket, "expected effect row");
        let effects = self.comma_list(Punct::RBracket, |this| this.effect_ref());
        let end = self.expect_punct(Punct::RBracket, "expected `]` after effect row");
        EffectRow {
            effects,
            span: start.cover(end),
        }
    }

    pub(crate) fn effect_ref(&mut self) -> EffectRef {
        let path = self.path();
        let args = self.effect_args();
        let span = args.last().map_or(path.span, EffectArg::span);
        EffectRef {
            span: path.span.cover(span),
            path,
            args,
        }
    }

    pub(crate) fn effect_args(&mut self) -> Vec<EffectArg> {
        if !self.cursor.at_punct(Punct::Lt)
            || self.cursor.nth(1).kind == TokenKind::Punct(Punct::Lt)
        {
            return Vec::new();
        }
        self.cursor.bump();
        let args = self.comma_list(Punct::Gt, |this| this.effect_arg());
        self.expect_punct(Punct::Gt, "expected `>` after effect arguments");
        args
    }

    pub(crate) fn effect_arg(&mut self) -> EffectArg {
        if self.cursor.peek().kind == TokenKind::Ident && self.slice(self.cursor.peek().span) == "_"
        {
            let span = self.cursor.bump().span;
            return EffectArg::Wildcard { span };
        }
        match self.cursor.peek().kind {
            TokenKind::StringLit => {
                let token = self.cursor.bump();
                EffectArg::String {
                    value: super::expr::unquote_string(self.slice(token.span)),
                    span: token.span,
                }
            }
            TokenKind::IntLit => {
                let token = self.cursor.bump();
                EffectArg::Int {
                    text: self.slice(token.span).to_string(),
                    span: token.span,
                }
            }
            _ => match self.type_expr() {
                TypeExpr::Path { path, args, span } if args.is_empty() && span == path.span => {
                    EffectArg::Path(path)
                }
                ty => EffectArg::Type(ty),
            },
        }
    }

    fn handler_produced_effect_row(&mut self) -> EffectRow {
        if self.cursor.at_punct(Punct::LBracket)
            && self.cursor.nth(1).kind == crate::TokenKind::Punct(Punct::RBracket)
        {
            let start = self.expect_punct(Punct::LBracket, "expected empty produced effect row");
            let end = self.expect_punct(Punct::RBracket, "expected `]` after empty effect row");
            return EffectRow {
                effects: Vec::new(),
                span: start.cover(end),
            };
        }
        let start = self.cursor.peek().span;
        self.effect_row_until(start, |this| {
            this.cursor.at_keyword(Keyword::For) || this.cursor.at_punct(Punct::RBracket)
        })
    }

    fn effect_row_until(&mut self, start: crate::Span, stop: impl Fn(&Self) -> bool) -> EffectRow {
        let mut effects = Vec::new();
        while !self.cursor.at_eof() && !stop(self) {
            let before = self.cursor.position();
            if self.cursor.at_punct(Punct::Comma) {
                self.cursor.bump();
                continue;
            }
            effects.push(self.effect_ref());
            if self.cursor.eat_punct(Punct::Comma).is_none() {
                break;
            }
            self.ensure_progress(before, "expected effect reference");
        }
        let span = effects
            .last()
            .map_or(start, |effect| start.cover(effect.span));
        EffectRow { effects, span }
    }
}
