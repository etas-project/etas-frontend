use std::{cell::Cell, sync::Arc};

use etas_code::descriptor::*;
use etas_core::Span;
use etas_hir::{
    HirAnnotation, HirAnnotationArg, HirArg, HirExpr, HirExprId, HirFieldInit, HirLiteral,
    ResolveResult, SymbolId,
};

use crate::CheckedProgramData;

mod constructors;
pub(crate) mod package;

#[derive(Debug)]
pub(crate) struct AnnotationProjectionError {
    pub span: Span,
    pub reason: String,
    pub resource_limit: Option<(&'static str, usize)>,
}

impl std::fmt::Display for AnnotationProjectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {:?}", self.reason, self.span)
    }
}

pub(crate) struct AnnotationProjection<'a, F> {
    checked: &'a CheckedProgramData,
    reference: F,
    limits: DescriptorLimits,
    nodes: Cell<usize>,
    text: Cell<usize>,
    depth: Cell<usize>,
}

impl<'a, F> AnnotationProjection<'a, F> {
    pub fn new(checked: &'a CheckedProgramData, reference: F, limits: DescriptorLimits) -> Self {
        Self {
            checked,
            reference,
            limits,
            nodes: Cell::new(0),
            text: Cell::new(0),
            depth: Cell::new(0),
        }
    }
}

impl<F, R> AnnotationProjection<'_, F>
where
    F: Fn(SymbolId) -> Result<R, String>,
{
    pub fn project(
        &self,
        annotation: &HirAnnotation,
    ) -> Result<AnnotationInfo<R>, AnnotationProjectionError> {
        let path = annotation
            .path
            .segments
            .iter()
            .map(|segment| segment.name.clone())
            .collect::<Vec<_>>();
        for segment in &path {
            self.text(segment, annotation.span)?;
        }
        let arguments = annotation
            .args
            .iter()
            .map(|argument| {
                let (name, value) = match argument {
                    HirAnnotationArg::Positional { value, .. } => (None, *value),
                    HirAnnotationArg::Named { name, value, .. } => (Some(name.clone()), *value),
                };
                self.argument(name, value, annotation.span)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(AnnotationInfo { path, arguments })
    }

    fn argument(
        &self,
        name: Option<String>,
        expr: HirExprId,
        anchor: Span,
    ) -> Result<AnnotationArgumentInfo<R>, AnnotationProjectionError> {
        if let Some(name) = &name {
            self.text(name, anchor)?;
        }
        Ok(AnnotationArgumentInfo {
            name,
            value: self.value(expr, anchor)?,
        })
    }

    fn value(
        &self,
        expression: HirExprId,
        anchor: Span,
    ) -> Result<Arc<StaticValueInfo<R>>, AnnotationProjectionError> {
        let expr = self.checked.hir.exprs.get(expression).ok_or_else(|| {
            self.error(anchor, "annotation expression is missing from checked HIR")
        })?;
        let span = expr.span(&self.checked.hir.blocks);
        self.node(span)?;
        if self.depth.get() >= self.limits.max_expansion_depth {
            return Err(self.limit_error(
                span,
                "annotation expansion depth",
                self.limits.max_expansion_depth,
            ));
        }
        self.depth.set(self.depth.get() + 1);
        let _depth = Depth(&self.depth);
        let value = match expr {
            HirExpr::Literal(literal) => StaticValueInfo::Literal(self.literal(literal, span)?),
            HirExpr::Path(path) => {
                StaticValueInfo::Reference(self.resolved(&path.resolution, span)?)
            }
            HirExpr::Array { elems, .. } => {
                self.sequence(AnnotationSequenceKind::Array, elems, span)?
            }
            HirExpr::List { elems, .. } => {
                self.sequence(AnnotationSequenceKind::List, elems, span)?
            }
            HirExpr::Set { elems, .. } => {
                self.sequence(AnnotationSequenceKind::Set, elems, span)?
            }
            HirExpr::Tuple { elems, .. } => {
                self.sequence(AnnotationSequenceKind::Tuple, elems, span)?
            }
            HirExpr::EmptySequence { .. } => StaticValueInfo::Sequence {
                kind: AnnotationSequenceKind::Array,
                elements: Vec::new(),
            },
            HirExpr::EmptyRecordOrMap { .. } => StaticValueInfo::Record(Vec::new()),
            HirExpr::Record(record) if record.path.is_none() && record.generic_args.is_empty() => {
                let fields = record
                    .fields
                    .iter()
                    .map(|field| {
                        let (name, value) = match field {
                            HirFieldInit::Named { name, value, .. } => {
                                (name.clone(), self.value(*value, span)?)
                            }
                            HirFieldInit::Shorthand {
                                name, resolution, ..
                            } => {
                                self.node(span)?;
                                (
                                    name.clone(),
                                    Arc::new(StaticValueInfo::Reference(
                                        self.resolved(resolution, span)?,
                                    )),
                                )
                            }
                        };
                        self.text(&name, span)?;
                        Ok(AnnotationFieldInfo { name, value })
                    })
                    .collect::<Result<Vec<_>, AnnotationProjectionError>>()?;
                StaticValueInfo::Record(fields)
            }
            HirExpr::Call {
                callee,
                generic_args,
                args,
                ..
            } if generic_args.is_empty() => {
                let Some(HirExpr::Path(path)) = self.checked.hir.exprs.get(*callee) else {
                    return Err(self.error(
                        span,
                        "annotation constructor must be a resolved standard declaration",
                    ));
                };
                let kind = constructors::kind(self.checked, path)
                    .map_err(|reason| self.error(span, reason))?;
                let target = self.resolved(&path.resolution, span)?;
                let arguments = args
                    .iter()
                    .map(|arg| match arg {
                        HirArg::Positional(value) => self.argument(None, *value, span),
                        HirArg::Named { name, value, .. } => {
                            self.argument(Some(name.clone()), *value, span)
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                StaticValueInfo::Constructor {
                    kind,
                    target,
                    arguments,
                }
            }
            _ => {
                return Err(self.error(
                    span,
                    "annotation argument has no checked static representation",
                ));
            }
        };
        Ok(Arc::new(value))
    }

    fn sequence(
        &self,
        kind: AnnotationSequenceKind,
        elems: &[HirExprId],
        span: Span,
    ) -> Result<StaticValueInfo<R>, AnnotationProjectionError> {
        let elements = elems
            .iter()
            .map(|expr| self.value(*expr, span))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StaticValueInfo::Sequence { kind, elements })
    }

    fn literal(
        &self,
        literal: &HirLiteral,
        span: Span,
    ) -> Result<AnnotationLiteralInfo, AnnotationProjectionError> {
        Ok(match literal {
            HirLiteral::Bool { value, .. } => AnnotationLiteralInfo::Bool(*value),
            HirLiteral::Int { text, .. } => {
                self.text(text, span)?;
                AnnotationLiteralInfo::Integer(text.clone())
            }
            HirLiteral::Float { text, .. } => {
                self.text(text, span)?;
                AnnotationLiteralInfo::Float(text.clone())
            }
            HirLiteral::String { value, .. } => {
                self.text(value, span)?;
                AnnotationLiteralInfo::String(value.clone())
            }
            HirLiteral::Char { value, .. } => {
                self.text(&value.to_string(), span)?;
                AnnotationLiteralInfo::Character(*value)
            }
        })
    }

    fn resolved(
        &self,
        resolution: &ResolveResult,
        span: Span,
    ) -> Result<R, AnnotationProjectionError> {
        let ResolveResult::Resolved(symbol) = resolution else {
            return Err(self.error(span, "annotation reference has no resolved symbol identity"));
        };
        (self.reference)(*symbol).map_err(|reason| self.error(span, reason))
    }

    fn node(&self, span: Span) -> Result<(), AnnotationProjectionError> {
        let count = self.nodes.get().saturating_add(1);
        if count > self.limits.max_nodes {
            return Err(self.limit_error(span, "annotation nodes", self.limits.max_nodes));
        }
        self.nodes.set(count);
        Ok(())
    }
    fn text(&self, value: &str, span: Span) -> Result<(), AnnotationProjectionError> {
        let count = self.text.get().saturating_add(value.len());
        if count > self.limits.max_text_bytes {
            return Err(self.limit_error(
                span,
                "annotation text bytes",
                self.limits.max_text_bytes,
            ));
        }
        self.text.set(count);
        Ok(())
    }
    fn error(&self, span: Span, reason: impl Into<String>) -> AnnotationProjectionError {
        AnnotationProjectionError {
            span,
            reason: reason.into(),
            resource_limit: None,
        }
    }

    fn limit_error(
        &self,
        span: Span,
        resource: &'static str,
        limit: usize,
    ) -> AnnotationProjectionError {
        AnnotationProjectionError {
            span,
            reason: format!("static {resource} limit exceeded"),
            resource_limit: Some((resource, limit)),
        }
    }
}

struct Depth<'a>(&'a Cell<usize>);
impl Drop for Depth<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

#[cfg(test)]
mod tests;
