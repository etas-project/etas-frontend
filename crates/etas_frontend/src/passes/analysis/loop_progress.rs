use etas_core::{AnalysisDiagnosticCode, Diagnostic};
use etas_hir::{
    HirBinaryOp, HirBlockId, HirElseBranch, HirExpr, HirExprId, HirStmt, ResolveResult, SymbolId,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::ProjectContext;
use crate::passes::artifacts::{DIAGNOSTICS, HIR_OUTPUT};

pub struct AnalyzeLoopProgressPass;

impl Pass<ProjectContext> for AnalyzeLoopProgressPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("AnalyzeLoopProgressPass", PassKind::Analysis)
            .requires(ArtifactSet::one(HIR_OUTPUT))
            .produces(ArtifactSet::one(DIAGNOSTICS))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let hir = context
            .hir
            .as_ref()
            .expect("HIR output should exist before loop progress analysis");
        let mut diagnostics = Vec::new();
        let analyzer = LoopProgressAnalyzer { hir: &hir.hir };
        for stmt in hir.hir.stmts.iter().map(|(id, _)| id) {
            analyzer.check_stmt(stmt, &mut diagnostics);
        }
        context.diagnostics.extend(diagnostics);
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(DIAGNOSTICS))
    }
}

struct LoopProgressAnalyzer<'a> {
    hir: &'a etas_hir::HirProgram,
}

impl LoopProgressAnalyzer<'_> {
    fn check_stmt(&self, stmt: etas_hir::HirStmtId, diagnostics: &mut Vec<Diagnostic>) {
        let Some(HirStmt::While {
            cond, body, span, ..
        }) = self.hir.stmts.get(stmt)
        else {
            return;
        };
        let Some(symbol) = self.condition_progress_symbol(*cond) else {
            return;
        };
        if self.block_assigns_symbol(*body, symbol) {
            return;
        }
        let name = self
            .hir
            .symbols
            .get(symbol)
            .map(|symbol| symbol.name.as_str())
            .unwrap_or("loop variable");
        diagnostics.push(Diagnostic::analysis(
            AnalysisDiagnosticCode::NonProgressingLoop,
            *span,
            format!("loop body does not advance {name}"),
        ));
    }

    fn condition_progress_symbol(&self, expr: HirExprId) -> Option<SymbolId> {
        let HirExpr::Binary { op, lhs, .. } = &self.hir.exprs[expr] else {
            return None;
        };
        if !matches!(
            op,
            HirBinaryOp::Lt | HirBinaryOp::LtEq | HirBinaryOp::Gt | HirBinaryOp::GtEq
        ) {
            return None;
        }
        self.path_symbol(*lhs)
    }

    fn block_assigns_symbol(&self, block: HirBlockId, symbol: SymbolId) -> bool {
        self.hir.blocks.get(block).is_some_and(|block| {
            block
                .stmts
                .iter()
                .any(|stmt| self.stmt_assigns_symbol(*stmt, symbol))
        })
    }

    fn stmt_assigns_symbol(&self, stmt: etas_hir::HirStmtId, symbol: SymbolId) -> bool {
        match &self.hir.stmts[stmt] {
            HirStmt::Assign { target, .. } => self.path_symbol(*target) == Some(symbol),
            HirStmt::If(expr) | HirStmt::Match(expr) | HirStmt::Expr { expr, .. } => {
                self.expr_assigns_symbol(*expr, symbol)
            }
            HirStmt::For { body, .. }
            | HirStmt::While { body, .. }
            | HirStmt::Retry { body, .. } => self.block_assigns_symbol(*body, symbol),
            _ => false,
        }
    }

    fn expr_assigns_symbol(&self, expr: HirExprId, symbol: SymbolId) -> bool {
        match &self.hir.exprs[expr] {
            HirExpr::If {
                then_block,
                else_branch,
                ..
            } => {
                self.block_assigns_symbol(*then_block, symbol)
                    || else_branch
                        .as_ref()
                        .is_some_and(|branch| self.else_branch_assigns_symbol(branch, symbol))
            }
            HirExpr::Match { arms, .. } => arms.iter().any(|arm| match &arm.body {
                etas_hir::HirMatchArmBody::Expr(expr) => self.expr_assigns_symbol(*expr, symbol),
                etas_hir::HirMatchArmBody::Block(block) => {
                    self.block_assigns_symbol(*block, symbol)
                }
            }),
            HirExpr::Block(block) => self.block_assigns_symbol(*block, symbol),
            _ => false,
        }
    }

    fn else_branch_assigns_symbol(&self, branch: &HirElseBranch, symbol: SymbolId) -> bool {
        match branch {
            HirElseBranch::Block(block) => self.block_assigns_symbol(*block, symbol),
            HirElseBranch::If(expr) => self.expr_assigns_symbol(*expr, symbol),
        }
    }

    fn path_symbol(&self, expr: HirExprId) -> Option<SymbolId> {
        let HirExpr::Path(path) = &self.hir.exprs[expr] else {
            return None;
        };
        match path.resolution {
            ResolveResult::Resolved(symbol) => Some(symbol),
            _ => None,
        }
    }
}
