use std::collections::BTreeMap;

use super::{AliasConstraint, AliasSolution, AliasSolver, AliasVar};
use crate::alias::{AliasPrecisionConfig, domain::AliasValue};

pub struct InclusionAliasSolver {
    config: AliasPrecisionConfig,
    constraints: Vec<AliasConstraint>,
    values: BTreeMap<AliasVar, AliasValue>,
    incomplete: bool,
}

impl InclusionAliasSolver {
    pub fn new(config: AliasPrecisionConfig) -> Self {
        Self {
            config,
            constraints: Vec::new(),
            values: BTreeMap::new(),
            incomplete: false,
        }
    }

    fn apply_constraint(&mut self, constraint: &AliasConstraint) -> bool {
        match constraint {
            AliasConstraint::AddrOf { dst, target } => {
                self.join_var(dst.clone(), AliasValue::from_place(target.clone()))
            }
            AliasConstraint::Copy { dst, src }
            | AliasConstraint::Load { dst, src }
            | AliasConstraint::Store { dst, src } => {
                let value = self.value(src);
                self.join_var(dst.clone(), value)
            }
            AliasConstraint::Project {
                dst,
                base,
                projection,
            } => {
                let value = self.value(base).project(projection.clone(), self.config);
                self.join_var(dst.clone(), value)
            }
            AliasConstraint::Call { dst, .. } => {
                self.incomplete = true;
                dst.as_ref()
                    .map(|dst| self.join_var(dst.clone(), AliasValue::unknown()))
                    .unwrap_or(false)
            }
        }
    }

    fn value(&self, var: &AliasVar) -> AliasValue {
        self.values
            .get(var)
            .cloned()
            .unwrap_or_else(AliasValue::bottom)
    }

    fn join_var(&mut self, var: AliasVar, value: AliasValue) -> bool {
        self.values
            .entry(var)
            .or_default()
            .join_with_limit(&value, self.config.max_alias_set_size)
    }
}

impl AliasSolver for InclusionAliasSolver {
    fn seed(&mut self, var: AliasVar, value: AliasValue) {
        self.join_var(var, value);
    }

    fn add_constraint(&mut self, constraint: AliasConstraint) {
        self.constraints.push(constraint);
    }

    fn solve(&mut self) -> AliasSolution {
        let mut changed = true;
        let mut iterations = 0;
        while changed && iterations < self.config.max_fixpoint_iterations {
            iterations += 1;
            changed = false;
            for constraint in self.constraints.clone() {
                changed |= self.apply_constraint(&constraint);
            }
        }
        if iterations >= self.config.max_fixpoint_iterations {
            self.incomplete = true;
        }
        AliasSolution {
            values: self.values.clone(),
            incomplete: self.incomplete,
        }
    }
}
