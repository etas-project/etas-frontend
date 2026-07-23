use etas_hir::{HirExprId, SymbolId};

use crate::unit::HirSemanticUnit;

use super::{
    place::AliasTarget,
    summary::{AliasSummary, AliasValueExpr},
};

pub trait AliasOracle {
    fn symbol_target(&self, symbol: SymbolId) -> Option<AliasTarget>;

    fn allocation_site(&self, expr: HirExprId, kind: AllocationKind) -> Option<AliasTarget>;

    fn intrinsic_call_summary(
        &self,
        call: HirExprId,
        callee: HirExprId,
    ) -> Option<AliasIntrinsicSummary>;

    fn resolved_call_target(&self, call: HirExprId, callee: HirExprId) -> AliasCallTarget;

    fn resource_constructor(&self, call: HirExprId) -> Option<AliasTarget>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocationKind {
    Record,
    Tuple,
    Array,
    List,
    Map,
    Set,
    Range,
    Lambda,
    Handler,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasIntrinsicSummary {
    pub summary: AliasSummary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AliasCallTarget {
    Direct(HirSemanticUnit),
    Dynamic,
    External,
    Incomplete,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoAliasOracle;

impl AliasOracle for NoAliasOracle {
    fn symbol_target(&self, symbol: SymbolId) -> Option<AliasTarget> {
        Some(AliasTarget::Symbol(symbol))
    }

    fn allocation_site(&self, _expr: HirExprId, _kind: AllocationKind) -> Option<AliasTarget> {
        None
    }

    fn intrinsic_call_summary(
        &self,
        _call: HirExprId,
        _callee: HirExprId,
    ) -> Option<AliasIntrinsicSummary> {
        None
    }

    fn resolved_call_target(&self, _call: HirExprId, _callee: HirExprId) -> AliasCallTarget {
        AliasCallTarget::External
    }

    fn resource_constructor(&self, _call: HirExprId) -> Option<AliasTarget> {
        None
    }
}

impl AliasIntrinsicSummary {
    pub fn returns(value: AliasValueExpr) -> Self {
        Self {
            summary: AliasSummary {
                return_alias: value,
                ..AliasSummary::default()
            },
        }
    }
}
