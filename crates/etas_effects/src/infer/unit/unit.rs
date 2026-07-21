use etas_hir::{HirBlockId, HirExprId, HirHandlerArmId, HirItemId};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum EffectUnit {
    Item(HirItemId),
    HandlerArm {
        owner: HirItemId,
        arm: HirHandlerArmId,
    },
    AnonymousFlow {
        owner: HirItemId,
        value: HirExprId,
        body: EffectAnonymousFlowBody,
    },
    FirstClassFlowCall {
        owner: HirItemId,
        call: HirExprId,
    },
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum EffectAnonymousFlowBody {
    Expr(HirExprId),
    Block(HirBlockId),
}
