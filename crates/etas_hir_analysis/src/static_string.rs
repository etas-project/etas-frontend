use std::collections::{BTreeMap, BTreeSet};

use etas_hir::{
    HirArg, HirExpr, HirExprId, HirFieldInit, HirItemId, HirLiteral, HirProgram, ResolveResult,
    SymbolDef, SymbolId,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaticStringTransform {
    Trim,
    Lowercase,
    Uppercase,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StaticStringCallSemantics {
    StringTransform {
        argument: usize,
        transform: StaticStringTransform,
    },
    SourceFlow {
        item: HirItemId,
        params: Vec<SymbolId>,
        return_expr: HirExprId,
    },
}

pub trait StaticStringOracle {
    fn call_semantics(&self, callee: HirExprId) -> Option<StaticStringCallSemantics>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StaticStringEvaluationError {
    Cycle { expr: HirExprId },
    MissingExpression { expr: HirExprId },
    UnsupportedExpression { expr: HirExprId },
    MissingRecordField { expr: HirExprId, field: String },
    UnresolvedPath { expr: HirExprId },
    UnboundSymbol { expr: HirExprId, symbol: SymbolId },
    UnsupportedCall { expr: HirExprId },
    MissingCallArgument { expr: HirExprId, argument: usize },
}

impl std::fmt::Display for StaticStringEvaluationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cycle { expr } => write!(
                formatter,
                "static value evaluation contains a cycle at expression {expr:?}"
            ),
            Self::MissingExpression { expr } => {
                write!(formatter, "static value expression {expr:?} is missing")
            }
            Self::UnsupportedExpression { expr } => write!(
                formatter,
                "expression {expr:?} cannot be evaluated as a static value"
            ),
            Self::MissingRecordField { expr, field } => {
                write!(
                    formatter,
                    "record expression {expr:?} has no field `{field}`"
                )
            }
            Self::UnresolvedPath { expr } => {
                write!(formatter, "static value path {expr:?} is unresolved")
            }
            Self::UnboundSymbol { expr, symbol } => write!(
                formatter,
                "static value path {expr:?} refers to unbound symbol {symbol:?}"
            ),
            Self::UnsupportedCall { expr } => write!(
                formatter,
                "call expression {expr:?} has no checked static value semantics"
            ),
            Self::MissingCallArgument { expr, argument } => write!(
                formatter,
                "call expression {expr:?} is missing static argument {argument}"
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ExpressionRef {
    expr: HirExprId,
    environment: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct BindingEnvironment {
    bindings: BTreeMap<SymbolId, ExpressionRef>,
    active_calls: BTreeSet<HirItemId>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Query {
    expression: ExpressionRef,
    projection: Vec<String>,
}

enum Completion {
    Redirect {
        query: Query,
        dependency: Query,
    },
    Transform {
        query: Query,
        dependency: Query,
        transform: StaticStringTransform,
    },
}

enum WorkItem {
    Enter(Query),
    Complete(Completion),
}

pub struct StaticStringEvaluator<'a, O> {
    hir: &'a HirProgram,
    oracle: &'a O,
    environments: Vec<BindingEnvironment>,
}

impl<'a, O: StaticStringOracle> StaticStringEvaluator<'a, O> {
    pub fn new(hir: &'a HirProgram, oracle: &'a O) -> Self {
        Self {
            hir,
            oracle,
            environments: vec![BindingEnvironment::default()],
        }
    }

    pub fn evaluate_string(
        mut self,
        expr: HirExprId,
        projection: &[String],
    ) -> Result<String, StaticStringEvaluationError> {
        let root = Query {
            expression: ExpressionRef {
                expr,
                environment: 0,
            },
            projection: projection.to_vec(),
        };
        let mut worklist = vec![WorkItem::Enter(root.clone())];
        let mut active = BTreeSet::new();
        let mut results = BTreeMap::<Query, Result<String, StaticStringEvaluationError>>::new();

        while let Some(work) = worklist.pop() {
            match work {
                WorkItem::Enter(query) => {
                    if results.contains_key(&query) {
                        continue;
                    }
                    if !active.insert(query.clone()) {
                        results.insert(
                            query.clone(),
                            Err(StaticStringEvaluationError::Cycle {
                                expr: query.expression.expr,
                            }),
                        );
                        continue;
                    }
                    match self.expand(&query) {
                        Ok(QueryExpansion::Value(value)) => {
                            active.remove(&query);
                            results.insert(query, Ok(value));
                        }
                        Ok(QueryExpansion::Redirect(dependency)) => {
                            worklist.push(WorkItem::Complete(Completion::Redirect {
                                query: query.clone(),
                                dependency: dependency.clone(),
                            }));
                            worklist.push(WorkItem::Enter(dependency));
                        }
                        Ok(QueryExpansion::Transform {
                            dependency,
                            transform,
                        }) => {
                            worklist.push(WorkItem::Complete(Completion::Transform {
                                query: query.clone(),
                                dependency: dependency.clone(),
                                transform,
                            }));
                            worklist.push(WorkItem::Enter(dependency));
                        }
                        Err(error) => {
                            active.remove(&query);
                            results.insert(query, Err(error));
                        }
                    }
                }
                WorkItem::Complete(completion) => {
                    let (query, result) = match completion {
                        Completion::Redirect { query, dependency } => {
                            let result = results.get(&dependency).cloned().unwrap_or({
                                Err(StaticStringEvaluationError::MissingExpression {
                                    expr: dependency.expression.expr,
                                })
                            });
                            (query, result)
                        }
                        Completion::Transform {
                            query,
                            dependency,
                            transform,
                        } => {
                            let result = results.get(&dependency).cloned().unwrap_or({
                                Err(StaticStringEvaluationError::MissingExpression {
                                    expr: dependency.expression.expr,
                                })
                            });
                            let result =
                                result.map(|value| apply_string_transform(value, transform));
                            (query, result)
                        }
                    };
                    active.remove(&query);
                    results.insert(query, result);
                }
            }
        }

        results.remove(&root).unwrap_or({
            Err(StaticStringEvaluationError::MissingExpression {
                expr: root.expression.expr,
            })
        })
    }

    fn expand(&mut self, query: &Query) -> Result<QueryExpansion, StaticStringEvaluationError> {
        let expr = query.expression.expr;
        let Some(expression) = self.hir.exprs.get(expr) else {
            return Err(StaticStringEvaluationError::MissingExpression { expr });
        };
        match expression {
            HirExpr::Literal(HirLiteral::String { value, .. }) if query.projection.is_empty() => {
                Ok(QueryExpansion::Value(value.clone()))
            }
            HirExpr::Record(record) => {
                let Some((field_name, rest)) = query.projection.split_first() else {
                    return Err(StaticStringEvaluationError::UnsupportedExpression { expr });
                };
                let field = record.fields.iter().find_map(|field| match field {
                    HirFieldInit::Named { name, value, .. } if name == field_name => {
                        Some(ExpressionRef {
                            expr: *value,
                            environment: query.expression.environment,
                        })
                    }
                    HirFieldInit::Shorthand {
                        name, resolution, ..
                    } if name == field_name => {
                        let ResolveResult::Resolved(symbol) = resolution else {
                            return None;
                        };
                        self.expression_for_symbol(expr, *symbol, query.expression.environment)
                            .ok()
                    }
                    _ => None,
                });
                let Some(expression) = field else {
                    return Err(StaticStringEvaluationError::MissingRecordField {
                        expr,
                        field: field_name.clone(),
                    });
                };
                Ok(QueryExpansion::Redirect(Query {
                    expression,
                    projection: rest.to_vec(),
                }))
            }
            HirExpr::Field { base, field, .. } => {
                let mut projection = Vec::with_capacity(query.projection.len() + 1);
                projection.push(field.clone());
                projection.extend(query.projection.iter().cloned());
                Ok(QueryExpansion::Redirect(Query {
                    expression: ExpressionRef {
                        expr: *base,
                        environment: query.expression.environment,
                    },
                    projection,
                }))
            }
            HirExpr::Path(path) => {
                let (symbol, mut projection) = match &path.resolution {
                    ResolveResult::Resolved(symbol) => (*symbol, Vec::new()),
                    ResolveResult::PartiallyResolved(partial) => (
                        partial
                            .resolved_prefix
                            .ok_or(StaticStringEvaluationError::UnresolvedPath { expr })?,
                        partial.remaining.clone(),
                    ),
                    ResolveResult::Unresolved | ResolveResult::Ambiguous(_) => {
                        return Err(StaticStringEvaluationError::UnresolvedPath { expr });
                    }
                };
                projection.extend(query.projection.iter().cloned());
                Ok(QueryExpansion::Redirect(Query {
                    expression: self.expression_for_symbol(
                        expr,
                        symbol,
                        query.expression.environment,
                    )?,
                    projection,
                }))
            }
            HirExpr::Call { callee, args, .. } => self.expand_call(query, *callee, args),
            _ => Err(StaticStringEvaluationError::UnsupportedExpression { expr }),
        }
    }

    fn expand_call(
        &mut self,
        query: &Query,
        callee: HirExprId,
        args: &[HirArg],
    ) -> Result<QueryExpansion, StaticStringEvaluationError> {
        let expr = query.expression.expr;
        match self.oracle.call_semantics(callee) {
            Some(StaticStringCallSemantics::StringTransform {
                argument,
                transform,
            }) => {
                if !query.projection.is_empty() {
                    return Err(StaticStringEvaluationError::UnsupportedExpression { expr });
                }
                let argument_expr = args
                    .get(argument)
                    .map(arg_expr)
                    .ok_or(StaticStringEvaluationError::MissingCallArgument { expr, argument })?;
                Ok(QueryExpansion::Transform {
                    dependency: Query {
                        expression: ExpressionRef {
                            expr: argument_expr,
                            environment: query.expression.environment,
                        },
                        projection: Vec::new(),
                    },
                    transform,
                })
            }
            Some(StaticStringCallSemantics::SourceFlow {
                item,
                params,
                return_expr,
            }) => {
                let environment = &self.environments[query.expression.environment];
                if environment.active_calls.contains(&item) {
                    return Err(StaticStringEvaluationError::Cycle { expr });
                }
                let mut next = BindingEnvironment {
                    bindings: BTreeMap::new(),
                    active_calls: environment.active_calls.clone(),
                };
                next.active_calls.insert(item);
                for (index, param) in params.into_iter().enumerate() {
                    let argument_expr = args.get(index).map(arg_expr).ok_or(
                        StaticStringEvaluationError::MissingCallArgument {
                            expr,
                            argument: index,
                        },
                    )?;
                    next.bindings.insert(
                        param,
                        ExpressionRef {
                            expr: argument_expr,
                            environment: query.expression.environment,
                        },
                    );
                }
                let environment = self.intern_environment(next);
                Ok(QueryExpansion::Redirect(Query {
                    expression: ExpressionRef {
                        expr: return_expr,
                        environment,
                    },
                    projection: query.projection.clone(),
                }))
            }
            None => Err(StaticStringEvaluationError::UnsupportedCall { expr }),
        }
    }

    fn expression_for_symbol(
        &self,
        expr: HirExprId,
        symbol: SymbolId,
        environment: usize,
    ) -> Result<ExpressionRef, StaticStringEvaluationError> {
        if let Some(binding) = self.environments[environment].bindings.get(&symbol) {
            return Ok(*binding);
        }
        let Some(symbol_data) = self.hir.symbols.get(symbol) else {
            return Err(StaticStringEvaluationError::UnboundSymbol { expr, symbol });
        };
        let initializer = match symbol_data.def {
            SymbolDef::Local {
                initializer: Some(initializer),
                ..
            }
            | SymbolDef::PatternBinding {
                initializer: Some(initializer),
                ..
            } => initializer,
            _ => return Err(StaticStringEvaluationError::UnboundSymbol { expr, symbol }),
        };
        Ok(ExpressionRef {
            expr: initializer,
            environment,
        })
    }

    fn intern_environment(&mut self, environment: BindingEnvironment) -> usize {
        if let Some(index) = self
            .environments
            .iter()
            .position(|candidate| candidate == &environment)
        {
            return index;
        }
        self.environments.push(environment);
        self.environments.len() - 1
    }
}

enum QueryExpansion {
    Value(String),
    Redirect(Query),
    Transform {
        dependency: Query,
        transform: StaticStringTransform,
    },
}

fn arg_expr(arg: &HirArg) -> HirExprId {
    match arg {
        HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => *expr,
    }
}

fn apply_string_transform(value: String, transform: StaticStringTransform) -> String {
    match transform {
        StaticStringTransform::Trim => value.trim().to_owned(),
        StaticStringTransform::Lowercase => value.to_lowercase(),
        StaticStringTransform::Uppercase => value.to_uppercase(),
    }
}
