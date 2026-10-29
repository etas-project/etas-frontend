use std::collections::HashSet;

use etas_hir::{
    HirExpr, HirExprId, HirItem, HirItemId, HirRevision, HirStmt, ResolveResult, SymbolId,
};
use etas_types::{PrimitiveType, Type, TypeId, TypeOutput};

use crate::{EffectUnit, ParameterArgumentValue};

pub(super) fn checked_literal_value(
    hir: &HirRevision,
    types: &TypeOutput,
    expression: &HirExpr,
    ty: TypeId,
) -> Option<ParameterArgumentValue> {
    let Type::Primitive(primitive) = types.store.get(ty)? else {
        return None;
    };
    match expression {
        HirExpr::Literal(etas_hir::HirLiteral::Bool { value, .. })
            if *primitive == PrimitiveType::Bool =>
        {
            Some(ParameterArgumentValue::Bool(*value))
        }
        HirExpr::Literal(etas_hir::HirLiteral::Int { text, .. }) => {
            let value = etas_types::checked_integer_literal(text, *primitive).ok()?;
            Some(ParameterArgumentValue::Integer {
                negative: value.negative,
                magnitude: value.magnitude,
            })
        }
        HirExpr::Unary {
            op: etas_hir::HirUnaryOp::Neg,
            expr,
            ..
        } => {
            if types
                .facts
                .expr_types
                .get(expr)
                .and_then(|ty| types.store.get(*ty))
                != Some(&Type::Primitive(*primitive))
            {
                return None;
            }
            let HirExpr::Literal(etas_hir::HirLiteral::Int { text, .. }) = hir.exprs.get(*expr)?
            else {
                return None;
            };
            let value =
                etas_types::checked_integer_literal(&format!("-{text}"), *primitive).ok()?;
            Some(ParameterArgumentValue::Integer {
                negative: value.negative,
                magnitude: value.magnitude,
            })
        }
        _ => None,
    }
}

/// Whole-project checked write facts used only for by-value scalar inputs.
pub(crate) struct ArgumentWrites {
    symbols: HashSet<SymbolId>,
    unresolved_owners: HashSet<HirItemId>,
    unresolved_owner: bool,
}

impl ArgumentWrites {
    pub(crate) fn build(
        hir: &HirRevision,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<Self, etas_utils::PipelineInterruption> {
        let mut result = Self {
            symbols: HashSet::new(),
            unresolved_owners: HashSet::new(),
            unresolved_owner: false,
        };
        for (id, stmt) in hir.stmts.iter() {
            if let Some(control) = control {
                control.checkpoint()?;
            }
            let HirStmt::Assign { target, .. } = stmt else {
                continue;
            };
            match assignment_root(hir, *target, control)? {
                Some(symbol) => {
                    result.symbols.insert(symbol);
                }
                None => match hir.index().stmt_item.get(&id) {
                    Some(owner) => {
                        result.unresolved_owners.insert(*owner);
                    }
                    None => result.unresolved_owner = true,
                },
            }
        }
        Ok(result)
    }

    pub(super) fn input_value(
        &self,
        hir: &HirRevision,
        types: &TypeOutput,
        unit: EffectUnit,
        expression: &HirExpr,
        ty: TypeId,
    ) -> Option<ParameterArgumentValue> {
        if !matches!(types.store.get(ty), Some(Type::Primitive(primitive)) if !matches!(primitive, PrimitiveType::Never | PrimitiveType::String | PrimitiveType::Bytes))
        {
            return None;
        }
        let EffectUnit::Item(owner) = unit else {
            return None;
        };
        if self.unresolved_owner || self.unresolved_owners.contains(&owner) {
            return None;
        }
        let HirExpr::Path(path) = expression else {
            return None;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        if self.symbols.contains(&symbol) {
            return None;
        }
        let parameters = match hir.items.get(owner)? {
            HirItem::Flow(flow) => &flow.params,
            HirItem::Tool(tool) => &tool.params,
            HirItem::Agent(agent) => &agent.params,
            _ => return None,
        };
        let position = parameters
            .iter()
            .position(|parameter| *parameter == symbol)?;
        Some(ParameterArgumentValue::Input {
            position: u32::try_from(position).ok()?,
        })
    }
}

fn assignment_root(
    hir: &HirRevision,
    mut target: HirExprId,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<Option<SymbolId>, etas_utils::PipelineInterruption> {
    let mut visited = HashSet::new();
    while visited.insert(target) {
        if let Some(control) = control {
            control.checkpoint()?;
        }
        let Some(expr) = hir.exprs.get(target) else {
            return Ok(None);
        };
        match expr {
            HirExpr::Path(path) => {
                return Ok(match path.resolution {
                    ResolveResult::Resolved(symbol) => Some(symbol),
                    _ => None,
                });
            }
            HirExpr::Field { base, .. }
            | HirExpr::Index { base, .. }
            | HirExpr::Slice { base, .. } => target = *base,
            _ => return Ok(None),
        }
    }
    Ok(None)
}

pub(super) fn enclosing_handlers(
    hir: &HirRevision,
    expression: HirExprId,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<(Option<bool>, Option<Vec<HirExprId>>), etas_utils::PipelineInterruption> {
    use etas_hir::{HirBodyKind, HirOwner};
    let index = hir.index();
    let Some(mut owner) = index.expr_owner.get(&expression).copied() else {
        return Ok((None, None));
    };
    let mut child = HirOwner::Expr(expression);
    let mut visited = HashSet::new();
    let mut handlers = Vec::new();
    while visited.insert(owner) {
        if let Some(control) = control {
            control.checkpoint()?;
        }
        let parent = match owner {
            HirOwner::Item(_) => return Ok((Some(handlers.is_empty()), Some(handlers))),
            HirOwner::HandlerArm(_) => return Ok((Some(false), None)),
            HirOwner::Expr(expr) => {
                match hir.exprs.get(expr) {
                    Some(HirExpr::Handle { body, handler, .. }) => {
                        if child == HirOwner::Expr(*body) {
                            handlers.push(*handler);
                        } else if child != HirOwner::Expr(*handler) {
                            return Ok((None, None));
                        }
                    }
                    Some(HirExpr::Try { .. }) => return Ok((Some(false), None)),
                    Some(_) => {}
                    None => return Ok((None, None)),
                }
                let Some(parent) = index.expr_owner.get(&expr) else {
                    return Ok((None, None));
                };
                *parent
            }
            HirOwner::Stmt(stmt) => {
                let Some(block) = index.stmt_block.get(&stmt) else {
                    return Ok((None, None));
                };
                HirOwner::Block(*block)
            }
            HirOwner::Block(block) => {
                let Some(parent) = index.block_owner.get(&block) else {
                    return Ok((None, None));
                };
                *parent
            }
            HirOwner::Body(body) => {
                if matches!(body.kind, HirBodyKind::Lambda(_)) {
                    return Ok((Some(handlers.is_empty()), Some(handlers)));
                }
                let Some(parent) = index.body_owner.get(&body) else {
                    return Ok((None, None));
                };
                *parent
            }
            HirOwner::Pattern(pattern) => {
                let Some(parent) = index.pat_owner.get(&pattern) else {
                    return Ok((None, None));
                };
                *parent
            }
            HirOwner::Module(_) => return Ok((None, None)),
        };
        child = owner;
        owner = parent;
    }
    Ok((None, None))
}

impl super::engine::EffectSemantics<'_> {
    pub(super) fn enclosing_handled_row(
        &self,
        expression: HirExprId,
    ) -> Result<(Option<bool>, Option<crate::EffectRow>), etas_utils::PipelineInterruption> {
        let (free, handlers) = enclosing_handlers(self.hir, expression, self.query_control)?;
        let Some(handlers) = handlers else {
            return Ok((free, None));
        };
        let mut handled = crate::EffectRow::empty();
        for expression in handlers {
            if let Some(control) = self.query_control {
                control.checkpoint()?;
            }
            let Some(ty) = self.types.facts.expr_types.get(&expression) else {
                return Ok((free, None));
            };
            let Some(Type::Handler(handler)) = self.types.store.get(*ty) else {
                return Ok((free, None));
            };
            let row = self.row_from_type_ref(&handler.handled);
            if handled.open.is_some() && row.open.is_some() && handled.open != row.open {
                return Ok((free, None));
            }
            handled.union_assign(&row);
        }
        Ok((free, Some(handled)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_scope_excludes_factory_and_deferred_lambda_body() {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
            r#"
flow run() -> unit {
    handle {
        body_call();
        let later = () => { lambda_call(); return; };
    } with factory(factory_call());
    return;
}
"#,
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = HirRevision::try_new(etas_hir::lower_program(&parsed.value)).unwrap();
        let call = |name: &str| {
            hir.exprs
                .iter()
                .find_map(|(id, expr)| {
                    let HirExpr::Call { callee, .. } = expr else {
                        return None;
                    };
                    let HirExpr::Path(path) = hir.exprs.get(*callee)? else {
                        return None;
                    };
                    (path.segments.last()?.name == name).then_some(id)
                })
                .unwrap()
        };
        let (free, handlers) = enclosing_handlers(&hir, call("body_call"), None).unwrap();
        assert_eq!(free, Some(false));
        assert_eq!(handlers.unwrap().len(), 1);
        for name in ["factory_call", "lambda_call"] {
            let (free, handlers) = enclosing_handlers(&hir, call(name), None).unwrap();
            assert_eq!(free, Some(true), "{name}");
            assert!(handlers.unwrap().is_empty(), "{name}");
        }
    }

    #[test]
    fn nested_handler_scope_is_ordered_from_inner_to_outer() {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
            r#"
flow run() -> unit {
    handle {
        handle { inner_call(); } with inner_factory(inner_argument());
    } with outer_factory(outer_argument());
    return;
}
"#,
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = HirRevision::try_new(etas_hir::lower_program(&parsed.value)).unwrap();
        let call = |name: &str| {
            hir.exprs
                .iter()
                .find_map(|(id, expr)| {
                    let HirExpr::Call { callee, .. } = expr else {
                        return None;
                    };
                    let HirExpr::Path(path) = hir.exprs.get(*callee)? else {
                        return None;
                    };
                    (path.segments.last()?.name == name).then_some(id)
                })
                .unwrap()
        };
        for (name, expected) in [
            (
                "inner_call",
                vec![call("inner_factory"), call("outer_factory")],
            ),
            ("inner_argument", vec![call("outer_factory")]),
            ("outer_argument", vec![]),
        ] {
            let (free, handlers) = enclosing_handlers(&hir, call(name), None).unwrap();
            assert_eq!(free, Some(expected.is_empty()), "{name}");
            assert_eq!(handlers, Some(expected), "{name}");
        }
    }
}
