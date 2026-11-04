use super::program::{LowerCtx, PreludeContext};
use crate::{HirItemId, ResolveResult, ScopeId, SymbolId};
use etas_syntax::ast;
use std::collections::HashSet;

impl LowerCtx {
    pub(super) fn lower_impl_type_params(
        &mut self,
        target: &ast::ImplTarget,
        scope: ScopeId,
        item: HirItemId,
    ) -> Vec<SymbolId> {
        let arguments = match target {
            ast::ImplTarget::Inherent { type_args, .. } => type_args.as_slice(),
            ast::ImplTarget::SpecSatisfaction {
                self_type: ast::TypeExpr::Path { args, .. },
                ..
            } => args.as_slice(),
            _ => &[],
        };
        let mut pending = arguments.iter().rev().collect::<Vec<_>>();
        let mut names = HashSet::new();
        let mut parameters = Vec::new();
        while let Some(ty) = pending.pop() {
            match ty {
                ast::TypeExpr::Path { path, args, .. } => {
                    if args.is_empty()
                        && path.segments.len() == 1
                        && matches!(
                            self.resolve_path_in_context(path, scope, false, PreludeContext::Type)
                                .resolution,
                            ResolveResult::Unresolved
                        )
                        && names.insert(path.segments[0].text.clone())
                    {
                        parameters.push(ast::TypeParam {
                            name: path.segments[0].clone(),
                            kind: ast::TypeParamKind::Type,
                            bounds: Vec::new(),
                            span: path.span,
                        });
                    }
                    pending.extend(args.iter().rev());
                }
                ast::TypeExpr::Tuple { elems, .. } => pending.extend(elems.iter().rev()),
                ast::TypeExpr::Record(record) => {
                    pending.extend(record.fields.iter().rev().map(|field| &field.ty));
                }
                ast::TypeExpr::Arrow { input, output, .. } => {
                    pending.extend([output.as_ref(), input.as_ref()]);
                }
                ast::TypeExpr::Refined { base, .. } => pending.push(base),
                ast::TypeExpr::Handler(handler) => pending.extend(handler.result.as_deref()),
                ast::TypeExpr::Existential { bounds, .. } => {
                    pending.extend(
                        bounds
                            .iter()
                            .rev()
                            .flat_map(|bound| bound.args.iter().rev()),
                    );
                }
                ast::TypeExpr::Primitive { .. } | ast::TypeExpr::Error(_) => {}
            }
        }
        self.lower_type_params(&parameters, scope, item)
    }
}
