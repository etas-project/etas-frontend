use etas_hir::{HirExprId, HirItemId, SymbolId};

use crate::unit::HirSemanticUnit;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AliasTarget {
    Symbol(SymbolId),
    Param {
        unit: HirSemanticUnit,
        index: usize,
    },
    Return {
        unit: HirSemanticUnit,
    },
    Allocation {
        unit: Option<HirSemanticUnit>,
        expr: HirExprId,
    },
    AllocationKind {
        unit: Option<HirSemanticUnit>,
        kind: AllocationSiteKind,
    },
    Captured {
        unit: HirSemanticUnit,
        symbol: SymbolId,
    },
    TopLevel(HirItemId),
    ResourceHandle(SymbolId),
    MemoryPlace(Vec<String>),
    External,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AllocationSiteKind {
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Projection {
    Field(String),
    CollapsedField,
    Index,
    ConstIndex(String),
    MapKey(String),
    Deref,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Place {
    pub root: AliasTarget,
    pub projections: Vec<Projection>,
}

impl Place {
    pub fn new(root: AliasTarget) -> Self {
        Self {
            root,
            projections: Vec::new(),
        }
    }

    pub fn project(&self, projection: Projection, max_depth: usize) -> Option<Self> {
        if self.projections.len() >= max_depth {
            return None;
        }
        let mut next = self.clone();
        next.projections.push(projection);
        Some(next)
    }
}
