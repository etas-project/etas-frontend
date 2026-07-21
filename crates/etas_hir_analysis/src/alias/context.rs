use etas_hir::HirExprId;

use crate::unit::HirSemanticUnit;

pub const MAX_CALL_STRING: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AliasContext {
    len: u8,
    calls: [Option<HirExprId>; MAX_CALL_STRING],
}

impl AliasContext {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_calls(calls: &[HirExprId]) -> Self {
        let mut context = Self::empty();
        let keep = calls.len().min(MAX_CALL_STRING);
        context.len = keep as u8;
        for (index, call) in calls[calls.len().saturating_sub(keep)..].iter().enumerate() {
            context.calls[index] = Some(*call);
        }
        context
    }

    pub fn push(self, call: HirExprId, k: usize) -> Self {
        let k = k.min(MAX_CALL_STRING);
        if k == 0 {
            return Self::empty();
        }
        let mut calls = self.calls();
        calls.push(call);
        Self::from_calls(&calls[calls.len().saturating_sub(k)..])
    }

    pub fn calls(self) -> Vec<HirExprId> {
        self.calls
            .into_iter()
            .take(self.len as usize)
            .flatten()
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContextualAliasUnit {
    pub semantic: HirSemanticUnit,
    pub context: AliasContext,
}

impl ContextualAliasUnit {
    pub fn new(semantic: HirSemanticUnit, context: AliasContext) -> Self {
        Self { semantic, context }
    }
}
