use std::collections::{BTreeMap, BTreeSet};

use etas_hir::{HirExprId, HirItemId, SymbolId};
use etas_utils::{JoinSemiLattice, PartialOrder};

use crate::unit::HirSemanticUnit;

use super::{
    AliasPrecisionConfig,
    domain::{AliasSet, AliasValue},
    place::{AliasTarget, AllocationSiteKind, Place, Projection},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AliasParamSummary {
    pub escaped: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasSummary {
    pub unit: Option<HirSemanticUnit>,
    pub params: Vec<AliasParamSummary>,
    pub return_alias: AliasValueExpr,
    pub escaped: BTreeSet<AliasValueExpr>,
    pub writes: Vec<AliasWriteEffect>,
    pub captures: BTreeMap<SymbolId, AliasValueExpr>,
    pub incomplete: bool,
    pub max_alias_set_size: usize,
}

impl AliasSummary {
    pub fn new(unit: HirSemanticUnit, config: AliasPrecisionConfig) -> Self {
        Self {
            unit: Some(unit),
            params: Vec::new(),
            return_alias: AliasValueExpr::Bottom,
            escaped: BTreeSet::new(),
            writes: Vec::new(),
            captures: BTreeMap::new(),
            incomplete: false,
            max_alias_set_size: config.max_alias_set_size,
        }
    }

    pub fn unknown(unit: HirSemanticUnit, config: AliasPrecisionConfig) -> Self {
        let mut summary = Self::new(unit, config);
        summary.return_alias = AliasValueExpr::Unknown;
        summary.incomplete = true;
        summary
    }

    pub fn value_to_expr(value: &AliasValue, current_unit: HirSemanticUnit) -> AliasValueExpr {
        match &value.may {
            AliasSet::Bottom => AliasValueExpr::Bottom,
            AliasSet::Unknown => AliasValueExpr::Unknown,
            AliasSet::Known(places) => {
                let mut values = places
                    .iter()
                    .map(|place| place_to_expr(place, current_unit))
                    .collect::<Vec<_>>();
                values.sort();
                values.dedup();
                match values.as_slice() {
                    [] => AliasValueExpr::Bottom,
                    [single] => single.clone(),
                    _ => AliasValueExpr::Union(values),
                }
            }
        }
    }
}

impl Default for AliasSummary {
    fn default() -> Self {
        Self {
            unit: None,
            params: Vec::new(),
            return_alias: AliasValueExpr::Bottom,
            escaped: BTreeSet::new(),
            writes: Vec::new(),
            captures: BTreeMap::new(),
            incomplete: false,
            max_alias_set_size: AliasPrecisionConfig::balanced().max_alias_set_size,
        }
    }
}

impl PartialOrder for AliasSummary {
    fn less_equal(&self, other: &Self) -> bool {
        self.return_alias.less_equal(&other.return_alias)
            && self.escaped.is_subset(&other.escaped)
            && self.writes.iter().all(|write| other.writes.contains(write))
            && self.captures.iter().all(|(symbol, value)| {
                other
                    .captures
                    .get(symbol)
                    .is_some_and(|o| value.less_equal(o))
            })
            && (!self.incomplete || other.incomplete)
    }
}

impl JoinSemiLattice for AliasSummary {
    fn bottom() -> Self {
        Self::default()
    }

    fn join_assign(&mut self, other: &Self) -> bool {
        let old = self.clone();
        self.unit = self.unit.or(other.unit);
        if other.params.len() > self.params.len() {
            self.params
                .resize(other.params.len(), AliasParamSummary::default());
        }
        for (index, param) in other.params.iter().enumerate() {
            self.params[index].escaped |= param.escaped;
        }
        self.return_alias = self
            .return_alias
            .join(&other.return_alias, self.max_alias_set_size);
        self.escaped.extend(other.escaped.iter().cloned());
        for write in &other.writes {
            if !self.writes.contains(write) {
                self.writes.push(write.clone());
            }
        }
        for (symbol, value) in &other.captures {
            self.captures
                .entry(*symbol)
                .and_modify(|current| *current = current.join(value, self.max_alias_set_size))
                .or_insert_with(|| value.clone());
        }
        self.incomplete |= other.incomplete;
        *self != old
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AliasValueExpr {
    Bottom,
    FormalParam(usize),
    Allocation {
        unit: Option<HirSemanticUnit>,
        expr: HirExprId,
    },
    AllocationKind {
        unit: Option<HirSemanticUnit>,
        kind: AllocationSiteKind,
    },
    Captured(SymbolId),
    TopLevel(HirItemId),
    ResourceHandle(SymbolId),
    MemoryPlace(Vec<String>),
    Unknown,
    Project {
        base: Box<AliasValueExpr>,
        projection: Projection,
    },
    Union(Vec<AliasValueExpr>),
}

impl AliasValueExpr {
    pub fn join(&self, other: &Self, limit: usize) -> Self {
        match (self, other) {
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (Self::Bottom, next) => next.clone(),
            (current, Self::Bottom) => current.clone(),
            (left, right) if left == right => left.clone(),
            _ => {
                let mut values = Vec::new();
                flatten_union(self, &mut values);
                flatten_union(other, &mut values);
                values.sort();
                values.dedup();
                if values.len() > limit {
                    Self::Unknown
                } else {
                    Self::Union(values)
                }
            }
        }
    }

    pub fn less_equal(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Bottom, _) | (_, Self::Unknown) => true,
            (Self::Unknown, _) => matches!(other, Self::Unknown),
            (_, Self::Bottom) => self == other,
            (Self::Union(left), Self::Union(right)) => left.iter().all(|v| right.contains(v)),
            (left, Self::Union(right)) => right.contains(left),
            (Self::Union(left), right) => left.iter().all(|v| v == right),
            (left, right) => left == right,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasWriteEffect {
    pub target: AliasValueExpr,
    pub value: AliasValueExpr,
}

fn flatten_union(value: &AliasValueExpr, out: &mut Vec<AliasValueExpr>) {
    match value {
        AliasValueExpr::Union(values) => {
            for value in values {
                flatten_union(value, out);
            }
        }
        AliasValueExpr::Bottom => {}
        other => out.push(other.clone()),
    }
}

fn place_to_expr(place: &Place, current_unit: HirSemanticUnit) -> AliasValueExpr {
    let mut value = match &place.root {
        AliasTarget::Param { unit, index } if *unit == current_unit => {
            AliasValueExpr::FormalParam(*index)
        }
        AliasTarget::Allocation { unit, expr } => AliasValueExpr::Allocation {
            unit: *unit,
            expr: *expr,
        },
        AliasTarget::AllocationKind { unit, kind } => AliasValueExpr::AllocationKind {
            unit: *unit,
            kind: *kind,
        },
        AliasTarget::Captured { unit, symbol } if *unit == current_unit => {
            AliasValueExpr::Captured(*symbol)
        }
        AliasTarget::TopLevel(item) => AliasValueExpr::TopLevel(*item),
        AliasTarget::ResourceHandle(symbol) => AliasValueExpr::ResourceHandle(*symbol),
        AliasTarget::MemoryPlace(path) => AliasValueExpr::MemoryPlace(path.clone()),
        AliasTarget::External | AliasTarget::Unknown => AliasValueExpr::Unknown,
        AliasTarget::Symbol(_)
        | AliasTarget::Param { .. }
        | AliasTarget::Return { .. }
        | AliasTarget::Captured { .. } => AliasValueExpr::Unknown,
    };
    for projection in &place.projections {
        value = AliasValueExpr::Project {
            base: Box::new(value),
            projection: projection.clone(),
        };
    }
    value
}
