use etas_syntax::ast;

use super::program::LowerCtx;
use crate::{HirItemId, HirReflectExpr, HirReflectTarget, ScopeId};

impl LowerCtx {
    pub(super) fn lower_reflection(
        &mut self,
        reflection: &ast::ReflectExpr,
        scope: ScopeId,
        item: HirItemId,
    ) -> HirReflectExpr {
        let target = match &reflection.target {
            ast::ReflectTarget::Path(path) => {
                HirReflectTarget::Path(self.resolve_path(path, scope, true))
            }
            ast::ReflectTarget::SpecMethod {
                receiver,
                spec,
                spec_args,
                method,
            } => HirReflectTarget::SpecMethod {
                receiver: self.resolve_path(receiver, scope, true),
                spec: self.resolve_path(spec, scope, true),
                spec_args: spec_args
                    .iter()
                    .map(|arg| self.lower_type(arg, scope, item))
                    .collect(),
                method: method.text.clone(),
                method_span: method.span,
            },
        };
        HirReflectExpr {
            target,
            span: reflection.span,
        }
    }
}
