use super::*;
use etas_std::IntrinsicCompletionSemantics as Completion;

impl EffectSemantics<'_> {
    pub(crate) fn checked_total_operation(&self, expr: HirExprId) -> bool {
        use etas_hir::{HirBinaryOp as B, HirUnaryOp as U};
        let typed = |id| {
            self.types
                .facts
                .expr_types
                .get(&id)
                .and_then(|ty| self.types.store.get(*ty))
        };
        let boolean = |id| matches!(typed(id), Some(Type::Primitive(PrimitiveType::Bool)));
        if matches!(
            self.hir.exprs.get(expr),
            Some(HirExpr::Unary { op: U::Neg, .. })
        ) && self.types.facts.expr_types.get(&expr).is_some_and(|ty| {
            matches!(
                super::super::argument_values::checked_literal_value(
                    self.hir,
                    self.types,
                    &self.hir.exprs[expr],
                    *ty,
                ),
                Some(crate::ParameterArgumentValue::Integer { .. })
            )
        }) {
            return true;
        }
        if !boolean(expr) {
            return false;
        }
        match self.hir.exprs.get(expr) {
            Some(HirExpr::Binary {
                lhs,
                op: B::EqEq | B::BangEq,
                rhs,
                ..
            }) => typed(*lhs).is_some() && typed(*rhs).is_some(),
            Some(HirExpr::Binary {
                lhs,
                op: B::AndAnd | B::OrOr,
                rhs,
                ..
            }) => boolean(*lhs) && boolean(*rhs),
            Some(HirExpr::Unary {
                op: U::Not,
                expr: operand,
                ..
            }) => boolean(*operand),
            _ => false,
        }
    }

    pub(crate) fn note_external_outcomes(&self, expr: HirExprId, state: &mut EffectState) {
        if self.checked_intrinsic_completion(expr).is_none() {
            state.exits.gaps.insert(Gap::ExternalCall(expr));
        }
    }

    pub(crate) fn checked_intrinsic_completion(&self, expr: HirExprId) -> Option<Completion> {
        let (declaration, effects, overload) =
            if let Some(fact) = self.types.facts.std_intrinsic_calls.get(&expr) {
                if !matches!(
                    self.hir.exprs.get(expr)?,
                    HirExpr::Call { .. } | HirExpr::MethodCall { .. }
                ) {
                    return None;
                }
                let declaration = self.std_registry.lookup_qualified(&fact.qualified_path)?;
                if declaration.intrinsic.as_ref()?.id.0 != fact.intrinsic {
                    return None;
                }
                let Type::Function(signature) = self.types.store.get(fact.callable_type)? else {
                    return None;
                };
                if signature.input != fact.signature.params
                    || signature.output != fact.signature.output
                    || signature.effects != fact.signature.effects
                {
                    return None;
                }
                (declaration, signature.effects.as_ref(), fact.overload)
            } else {
                match self.hir.exprs.get(expr)? {
                    HirExpr::Call { callee, .. } => {
                        let path = self.std_qualified_path_for_expr(*callee)?;
                        let declaration = self.std_registry.lookup_qualified(&path)?;
                        // An overloaded symbol needs the solver's selected declaration;
                        // its primary signature is not evidence for another overload.
                        if self
                            .std_registry
                            .flow_declarations(declaration.id)
                            .nth(1)
                            .is_some()
                        {
                            return None;
                        }
                        let signature = self
                            .callee_symbol(*callee)
                            .and_then(|symbol| self.type_symbols.canonical_symbol(self.hir, symbol))
                            .and_then(|symbol| {
                                self.type_symbols
                                    .symbol_fact(self.hir, &self.types.facts, symbol)
                            })
                            .and_then(|fact| match fact {
                                etas_types::SymbolTypeFact::Flow { signature } => Some(signature),
                                _ => None,
                            });
                        // A namespace-qualified pure descriptor has no row to instantiate.
                        // Effectful/generic descriptors still require their checked bindings.
                        (
                            declaration,
                            signature.and_then(|signature| signature.effects.as_ref()),
                            0,
                        )
                    }
                    _ => return None,
                }
            };
        let flow = self
            .std_registry
            .flow_declarations(declaration.id)
            .nth(overload)?;
        let completion =
            etas_std::intrinsic_completion_semantics(declaration.intrinsic.as_ref()?.id)?;
        self.types
            .store
            .get(*self.types.facts.expr_types.get(&expr)?)?;
        if !flow.type_params.is_empty() {
            let bindings = self.types.facts.generic_instantiations.get(&expr)?;
            if !bindings.deferred_effect_row_obligations.is_empty()
                || flow.type_params.iter().any(|param| {
                    !bindings.type_bindings.iter().any(|(name, ty)| {
                        name == &param.name && self.types.store.get(*ty).is_some()
                    })
                })
            {
                return None;
            }
        }
        let row = match effects {
            Some(row) if row.tail.is_none() => self.row_from_type_ref(row),
            None if flow.public_effects.is_empty() => crate::EffectRow::empty(),
            _ => return None,
        };
        match completion {
            Completion::Returns if row.is_empty() => Some(completion),
            Completion::ReturnsOrRaisesDeclaredErrors
                if !row.effects.is_empty()
                    && row
                        .effects
                        .iter()
                        .all(|effect| matches!(effect, Effect::Error(_))) =>
            {
                Some(completion)
            }
            _ => None,
        }
    }

    pub(crate) fn discarded_try_completion(&self, expr: HirExprId) -> bool {
        let Some(etas_hir::HirOwner::Stmt(stmt)) = self.context.tree_index().expr_owner.get(&expr)
        else {
            return false;
        };
        matches!(self.hir.stmts.get(*stmt), Some(etas_hir::HirStmt::Expr { expr: value, .. }) if *value == expr)
            && self.types.facts.try_facts.get(&expr).is_some_and(|fact| {
                self.checked_intrinsic_completion(fact.operand)
                    == Some(Completion::ReturnsOrRaisesDeclaredErrors)
            })
    }
}
