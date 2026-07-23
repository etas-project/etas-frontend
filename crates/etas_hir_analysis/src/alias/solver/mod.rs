mod hybrid;
mod inclusion;
mod unification;

use std::collections::BTreeMap;

use etas_hir::{HirExprId, SymbolId};

use crate::unit::HirSemanticUnit;

use super::{
    AliasPrecisionConfig,
    domain::AliasValue,
    place::{Place, Projection},
};

pub use hybrid::HybridAliasSolver;
pub use inclusion::InclusionAliasSolver;
pub use unification::UnificationAliasSolver;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AliasVar {
    Symbol(SymbolId),
    Expr(HirExprId),
    Place(Place),
    Return(HirSemanticUnit),
    Synthetic(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AliasConstraint {
    AddrOf {
        dst: AliasVar,
        target: Place,
    },
    Copy {
        dst: AliasVar,
        src: AliasVar,
    },
    Load {
        dst: AliasVar,
        src: AliasVar,
    },
    Store {
        dst: AliasVar,
        src: AliasVar,
    },
    Project {
        dst: AliasVar,
        base: AliasVar,
        projection: Projection,
    },
    Call {
        dst: Option<AliasVar>,
        callee: AliasVar,
        args: Vec<AliasVar>,
        site: HirExprId,
    },
}

pub trait AliasSolver {
    fn seed(&mut self, var: AliasVar, value: AliasValue);
    fn add_constraint(&mut self, constraint: AliasConstraint);
    fn solve(&mut self) -> AliasSolution;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AliasSolution {
    pub(crate) values: BTreeMap<AliasVar, AliasValue>,
    pub incomplete: bool,
}

impl AliasSolution {
    pub fn value(&self, var: &AliasVar) -> AliasValue {
        self.values
            .get(var)
            .cloned()
            .unwrap_or_else(AliasValue::bottom)
    }
}

pub struct AliasConstraintFrame {
    config: AliasPrecisionConfig,
    seeds: Vec<(AliasVar, AliasValue)>,
    constraints: Vec<AliasConstraint>,
}

impl AliasConstraintFrame {
    pub fn new(config: AliasPrecisionConfig) -> Self {
        Self {
            config,
            seeds: Vec::new(),
            constraints: Vec::new(),
        }
    }

    pub fn seed(&mut self, var: AliasVar, value: AliasValue) {
        self.seeds.push((var, value));
    }

    pub fn add_constraint(&mut self, constraint: AliasConstraint) {
        self.constraints.push(constraint);
    }

    pub fn solve(self) -> AliasSolution {
        let mut solver = ConfiguredAliasSolver::new(self.config);
        for (var, value) in self.seeds {
            solver.seed(var, value);
        }
        for constraint in self.constraints {
            solver.add_constraint(constraint);
        }
        solver.solve()
    }
}

enum ConfiguredAliasSolver {
    Inclusion(InclusionAliasSolver),
    Unification(UnificationAliasSolver),
    Hybrid(Box<HybridAliasSolver>),
}

impl ConfiguredAliasSolver {
    fn new(config: AliasPrecisionConfig) -> Self {
        match config.constraint_model {
            super::config::AliasConstraintModel::InclusionBased => {
                Self::Inclusion(InclusionAliasSolver::new(config))
            }
            super::config::AliasConstraintModel::UnificationBased => {
                Self::Unification(UnificationAliasSolver::new(config))
            }
            super::config::AliasConstraintModel::Hybrid { .. } => {
                Self::Hybrid(Box::new(HybridAliasSolver::new(config)))
            }
        }
    }
}

impl AliasSolver for ConfiguredAliasSolver {
    fn seed(&mut self, var: AliasVar, value: AliasValue) {
        match self {
            Self::Inclusion(solver) => solver.seed(var, value),
            Self::Unification(solver) => solver.seed(var, value),
            Self::Hybrid(solver) => solver.seed(var, value),
        }
    }

    fn add_constraint(&mut self, constraint: AliasConstraint) {
        match self {
            Self::Inclusion(solver) => solver.add_constraint(constraint),
            Self::Unification(solver) => solver.add_constraint(constraint),
            Self::Hybrid(solver) => solver.add_constraint(constraint),
        }
    }

    fn solve(&mut self) -> AliasSolution {
        match self {
            Self::Inclusion(solver) => solver.solve(),
            Self::Unification(solver) => solver.solve(),
            Self::Hybrid(solver) => solver.solve(),
        }
    }
}
