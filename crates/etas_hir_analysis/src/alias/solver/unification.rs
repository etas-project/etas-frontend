use std::collections::{BTreeMap, BTreeSet};

use super::{AliasConstraint, AliasSolution, AliasSolver, AliasVar};
use crate::alias::{AliasPrecisionConfig, domain::AliasValue};

pub struct UnificationAliasSolver {
    config: AliasPrecisionConfig,
    constraints: Vec<AliasConstraint>,
    parent: BTreeMap<AliasVar, AliasVar>,
    values: BTreeMap<AliasVar, AliasValue>,
    incomplete: bool,
}

impl UnificationAliasSolver {
    pub fn new(config: AliasPrecisionConfig) -> Self {
        Self {
            config,
            constraints: Vec::new(),
            parent: BTreeMap::new(),
            values: BTreeMap::new(),
            incomplete: false,
        }
    }

    fn apply_constraint(&mut self, constraint: &AliasConstraint) {
        match constraint {
            AliasConstraint::AddrOf { dst, target } => {
                self.seed(dst.clone(), AliasValue::from_place(target.clone()));
            }
            AliasConstraint::Copy { dst, src }
            | AliasConstraint::Load { dst, src }
            | AliasConstraint::Store { dst, src } => self.union(dst.clone(), src.clone()),
            AliasConstraint::Project {
                dst,
                base,
                projection,
            } => {
                let value = self.value(base).project(projection.clone(), self.config);
                self.seed(dst.clone(), value);
            }
            AliasConstraint::Call { dst, .. } => {
                self.incomplete = true;
                if let Some(dst) = dst {
                    self.seed(dst.clone(), AliasValue::unknown());
                }
            }
        }
    }

    fn value(&mut self, var: &AliasVar) -> AliasValue {
        let root = self.find(var.clone());
        self.values
            .get(&root)
            .cloned()
            .unwrap_or_else(AliasValue::bottom)
    }

    fn find(&mut self, var: AliasVar) -> AliasVar {
        let parent = self.parent.get(&var).cloned();
        match parent {
            Some(parent) if parent != var => {
                let root = self.find(parent);
                self.parent.insert(var.clone(), root.clone());
                root
            }
            Some(parent) => parent,
            None => {
                self.parent.insert(var.clone(), var.clone());
                var
            }
        }
    }

    fn union(&mut self, left: AliasVar, right: AliasVar) {
        let left_root = self.find(left);
        let right_root = self.find(right);
        if left_root == right_root {
            return;
        }
        let mut value = self
            .values
            .remove(&left_root)
            .unwrap_or_else(AliasValue::bottom);
        if let Some(right_value) = self.values.remove(&right_root) {
            value.join_with_limit(&right_value, self.config.max_alias_set_size);
        }
        let (root, other) = if left_root <= right_root {
            (left_root, right_root)
        } else {
            (right_root, left_root)
        };
        self.parent.insert(other, root.clone());
        self.values.insert(root, value);
    }
}

impl AliasSolver for UnificationAliasSolver {
    fn seed(&mut self, var: AliasVar, value: AliasValue) {
        let root = self.find(var);
        self.values
            .entry(root)
            .or_default()
            .join_with_limit(&value, self.config.max_alias_set_size);
    }

    fn add_constraint(&mut self, constraint: AliasConstraint) {
        self.constraints.push(constraint);
    }

    fn solve(&mut self) -> AliasSolution {
        for constraint in self.constraints.clone() {
            self.apply_constraint(&constraint);
        }
        let mut values = BTreeMap::new();
        let vars = self
            .parent
            .keys()
            .chain(self.values.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        for var in vars {
            let root = self.find(var.clone());
            let value = self
                .values
                .get(&root)
                .cloned()
                .unwrap_or_else(AliasValue::bottom);
            values.insert(var, value);
        }
        AliasSolution {
            values,
            incomplete: self.incomplete,
        }
    }
}
