use etas_hir::{HirBlockId, HirExprId, HirHandlerArmId, HirItemId};

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
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
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum EffectAnonymousFlowBody {
    Expr(HirExprId),
    Block(HirBlockId),
}
