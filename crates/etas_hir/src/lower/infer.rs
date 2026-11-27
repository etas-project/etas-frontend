use super::program::{LowerCtx, PreludeContext};
use crate::*;
use etas_syntax::ast;

impl LowerCtx {
    pub(super) fn lower_infer_arguments(
        &mut self,
        infer: &ast::InferExpr,
        scope: ScopeId,
        item: HirItemId,
    ) -> Vec<HirArg> {
        let args: Vec<_> = infer
            .args
            .iter()
            .map(|arg| self.lower_arg(arg, scope, item))
            .collect();
        let [first, second] = args.as_slice() else {
            return args;
        };
        let (a, first) = infer_operand(first);
        let (b, second) = infer_operand(second);
        let names = match (a, b) {
            (None, None | Some("system")) | (Some("data"), Some("system")) => [DATA, SYSTEM],
            (Some("system"), Some("data")) => [SYSTEM, DATA],
            _ => return args,
        };

        // Bind both operands in source order before calling new().system(...).data(...).
        // These names cannot be spelled as Etas identifiers.
        let span = infer.span;
        let block = self.lower_block(
            &ast::Block {
                stmts: vec![],
                final_expr: None,
                span,
            },
            scope,
            item,
        );
        let inner = self.hir.blocks[block].scope;
        let [first, second] = [(names[0], first), (names[1], second)].map(|(name, value)| {
            let span = self.hir.exprs[value].span(&self.hir.blocks);
            let local = ast::Name {
                text: name.into(),
                span,
            };
            let binding = self.hir.stmts.alloc(HirStmt::Error { span });
            self.finish_let(
                binding,
                &ast::Pattern::Ident(local.clone()),
                value,
                None,
                inner,
                item,
            );
            self.hir.blocks[block].stmts.push(binding);
            let path = ast::Path {
                segments: vec![local],
                span,
            };
            let path = self.resolve_path(&path, inner, true);
            self.alloc_expr(HirExpr::Path(path), span)
        });
        let (data, system) = if names[0] == DATA {
            (first, second)
        } else {
            (second, first)
        };
        let path = ast::Path {
            segments: ["std", "agent", "prompt", "Prompt"]
                .into_iter()
                .map(|text| ast::Name {
                    text: text.into(),
                    span,
                })
                .collect(),
            span,
        };
        let path = self.resolve_path_in_context(&path, inner, true, PreludeContext::Type);
        let mut receiver = self.alloc_expr(HirExpr::Path(path), span);
        for (method, argument) in [
            ("new", None),
            ("system", Some(system)),
            ("data", Some(data)),
        ] {
            receiver = self.alloc_expr(
                HirExpr::MethodCall {
                    receiver,
                    method: method.into(),
                    generic_args: vec![],
                    args: argument.into_iter().map(HirArg::Positional).collect(),
                    span,
                },
                span,
            );
        }
        self.hir.blocks[block].final_expr = Some(receiver);
        vec![HirArg::Positional(
            self.alloc_expr(HirExpr::Block(block), span),
        )]
    }
}

const DATA: &str = "$infer_data";
const SYSTEM: &str = "$infer_system";

fn infer_operand(arg: &HirArg) -> (Option<&str>, HirExprId) {
    match arg {
        HirArg::Positional(value) => (None, *value),
        HirArg::Named { name, value, .. } => (Some(name), *value),
    }
}
