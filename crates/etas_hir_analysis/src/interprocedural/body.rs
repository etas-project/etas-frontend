use etas_hir::{HirBlockId, HirExprId, HirHandlerArmId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HirAnalysisBody {
    Block(HirBlockId),
    Expr(HirExprId),
    HandlerArm(HirHandlerArmId),
    External,
    Missing,
}
