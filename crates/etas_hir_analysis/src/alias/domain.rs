use std::collections::{BTreeMap, BTreeSet};

use etas_hir::{HirExprId, SymbolId};
use etas_utils::{JoinSemiLattice, PartialOrder};

use crate::unit::HirSemanticUnit;

use super::{
    AliasContext, AliasPrecisionConfig,
    place::{AliasTarget, Place, Projection},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasValue {
    pub may: AliasSet,
    pub must: Option<Place>,
}

impl AliasValue {
    pub fn bottom() -> Self {
        Self {
            may: AliasSet::Bottom,
            must: None,
        }
    }

    pub fn unknown() -> Self {
        Self {
            may: AliasSet::Unknown,
            must: None,
        }
    }

    pub fn from_place(place: Place) -> Self {
        let mut places = BTreeSet::new();
        places.insert(place.clone());
        Self {
            may: AliasSet::Known(places),
            must: Some(place),
        }
    }

    pub fn from_target(target: AliasTarget) -> Self {
        Self::from_place(Place::new(target))
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self.may, AliasSet::Unknown)
    }

    pub fn project(&self, projection: Projection, config: AliasPrecisionConfig) -> Self {
        match &self.may {
            AliasSet::Bottom => Self::bottom(),
            AliasSet::Unknown => Self::unknown(),
            AliasSet::Known(places) => {
                let mut projected = BTreeSet::new();
                for place in places {
                    let Some(place) =
                        place.project(projection.clone(), config.max_projection_depth)
                    else {
                        return Self::unknown();
                    };
                    projected.insert(place);
                    if projected.len() > config.max_alias_set_size {
                        return Self::unknown();
                    }
                }
                let must = self
                    .must
                    .as_ref()
                    .and_then(|place| place.project(projection, config.max_projection_depth));
                Self {
                    may: AliasSet::Known(projected),
                    must,
                }
            }
        }
    }

    pub fn join_with_limit(&mut self, other: &Self, limit: usize) -> bool {
        let old = self.clone();
        self.may.join_with_limit(&other.may, limit);
        self.must = match (&old.may, &other.may, &old.must, &other.must) {
            (AliasSet::Bottom, _, _, right) => right.clone(),
            (_, AliasSet::Bottom, left, _) => left.clone(),
            (_, AliasSet::Unknown, _, _) | (AliasSet::Unknown, _, _, _) => None,
            (_, _, Some(left), Some(right)) if left == right => Some(left.clone()),
            _ => None,
        };
        *self != old
    }
}

impl Default for AliasValue {
    fn default() -> Self {
        Self::bottom()
    }
}

impl PartialOrder for AliasValue {
    fn less_equal(&self, other: &Self) -> bool {
        self.may.less_equal(&other.may)
            && match (&self.must, &other.must) {
                (_, None) => true,
                (Some(left), Some(right)) => left == right,
                (None, Some(_)) => false,
            }
    }
}

impl JoinSemiLattice for AliasValue {
    fn bottom() -> Self {
        Self::bottom()
    }

    fn join_assign(&mut self, other: &Self) -> bool {
        self.join_with_limit(other, AliasPrecisionConfig::balanced().max_alias_set_size)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AliasSet {
    Bottom,
    Known(BTreeSet<Place>),
    Unknown,
}

impl AliasSet {
    pub fn join_with_limit(&mut self, other: &Self, limit: usize) -> bool {
        let old = self.clone();
        let current = self.clone();
        *self = match (current, other) {
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (Self::Bottom, next) => next.clone(),
            (current, Self::Bottom) => current.clone(),
            (Self::Known(left), Self::Known(right)) => {
                let mut joined = left.clone();
                joined.extend(right.iter().cloned());
                if joined.len() > limit {
                    Self::Unknown
                } else {
                    Self::Known(joined)
                }
            }
        };
        *self != old
    }

    pub fn places(&self) -> Option<&BTreeSet<Place>> {
        match self {
            Self::Known(places) => Some(places),
            Self::Bottom | Self::Unknown => None,
        }
    }
}

impl Default for AliasSet {
    fn default() -> Self {
        Self::Bottom
    }
}

impl PartialOrder for AliasSet {
    fn less_equal(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Bottom, _) | (_, Self::Unknown) => true,
            (Self::Unknown, _) => matches!(other, Self::Unknown),
            (Self::Known(_), Self::Bottom) => false,
            (Self::Known(left), Self::Known(right)) => left.is_subset(right),
        }
    }
}

impl JoinSemiLattice for AliasSet {
    fn bottom() -> Self {
        Self::Bottom
    }

    fn join_assign(&mut self, other: &Self) -> bool {
        self.join_with_limit(other, AliasPrecisionConfig::balanced().max_alias_set_size)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasDomain {
    pub unit: Option<HirSemanticUnit>,
    pub context: AliasContext,
    pub symbols: BTreeMap<SymbolId, AliasValue>,
    pub exprs: BTreeMap<HirExprId, AliasValue>,
    pub heap: BTreeMap<Place, AliasValue>,
    pub escaped: BTreeSet<Place>,
    pub return_alias: AliasValue,
    pub last_expr_alias: AliasValue,
    pub incomplete: bool,
    pub config: AliasPrecisionConfig,
}

impl AliasDomain {
    pub fn new(config: AliasPrecisionConfig) -> Self {
        Self {
            unit: None,
            context: AliasContext::empty(),
            symbols: BTreeMap::new(),
            exprs: BTreeMap::new(),
            heap: BTreeMap::new(),
            escaped: BTreeSet::new(),
            return_alias: AliasValue::bottom(),
            last_expr_alias: AliasValue::bottom(),
            incomplete: false,
            config,
        }
    }

    pub fn mark_incomplete(&mut self) {
        self.incomplete = true;
    }

    pub fn expr_alias(&self, expr: HirExprId) -> AliasValue {
        self.exprs
            .get(&expr)
            .cloned()
            .unwrap_or_else(AliasValue::bottom)
    }

    pub fn set_expr_alias(&mut self, expr: HirExprId, value: AliasValue) {
        self.last_expr_alias = value.clone();
        self.exprs.insert(expr, value);
    }

    pub fn assign_symbol(&mut self, symbol: SymbolId, value: AliasValue) {
        self.symbols.insert(symbol, value);
    }

    pub fn weak_assign_symbol(&mut self, symbol: SymbolId, value: AliasValue) {
        self.symbols
            .entry(symbol)
            .or_default()
            .join_with_limit(&value, self.config.max_alias_set_size);
    }

    pub fn assign_place(&mut self, place: Place, value: AliasValue) {
        self.heap.insert(place, value);
    }

    pub fn weak_assign_place(&mut self, place: Place, value: AliasValue) {
        self.heap
            .entry(place)
            .or_default()
            .join_with_limit(&value, self.config.max_alias_set_size);
    }

    pub fn mark_escaped(&mut self, value: &AliasValue) {
        match &value.may {
            AliasSet::Known(places) => {
                self.escaped.extend(places.iter().cloned());
            }
            AliasSet::Unknown => self.incomplete = true,
            AliasSet::Bottom => {}
        }
    }
}

impl Default for AliasDomain {
    fn default() -> Self {
        Self::new(AliasPrecisionConfig::balanced())
    }
}

impl PartialOrder for AliasDomain {
    fn less_equal(&self, other: &Self) -> bool {
        map_less_equal(&self.symbols, &other.symbols)
            && map_less_equal(&self.exprs, &other.exprs)
            && map_less_equal(&self.heap, &other.heap)
            && self.escaped.is_subset(&other.escaped)
            && self.return_alias.less_equal(&other.return_alias)
            && self.last_expr_alias.less_equal(&other.last_expr_alias)
            && (!self.incomplete || other.incomplete)
    }
}

impl JoinSemiLattice for AliasDomain {
    fn bottom() -> Self {
        Self::default()
    }

    fn join_assign(&mut self, other: &Self) -> bool {
        let mut changed = false;
        let limit = self
            .config
            .max_alias_set_size
            .max(other.config.max_alias_set_size);
        changed |= join_map(&mut self.symbols, &other.symbols, limit);
        changed |= join_map(&mut self.exprs, &other.exprs, limit);
        changed |= join_map(&mut self.heap, &other.heap, limit);
        let old_escaped = self.escaped.len();
        self.escaped.extend(other.escaped.iter().cloned());
        changed |= self.escaped.len() != old_escaped;
        changed |= self
            .return_alias
            .join_with_limit(&other.return_alias, limit);
        changed |= self
            .last_expr_alias
            .join_with_limit(&other.last_expr_alias, limit);
        let old_incomplete = self.incomplete;
        self.incomplete |= other.incomplete;
        changed |= self.incomplete != old_incomplete;
        changed
    }
}

fn map_less_equal<K>(left: &BTreeMap<K, AliasValue>, right: &BTreeMap<K, AliasValue>) -> bool
where
    K: Ord,
{
    left.iter()
        .all(|(key, value)| right.get(key).is_some_and(|other| value.less_equal(other)))
}

fn join_map<K>(
    left: &mut BTreeMap<K, AliasValue>,
    right: &BTreeMap<K, AliasValue>,
    limit: usize,
) -> bool
where
    K: Clone + Ord,
{
    let mut changed = false;
    for (key, value) in right {
        changed |= left
            .entry(key.clone())
            .or_default()
            .join_with_limit(value, limit);
    }
    changed
}
