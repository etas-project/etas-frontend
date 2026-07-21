use std::collections::BTreeSet;

use super::{
    AliasConstraint, AliasSolution, AliasSolver, AliasVar, InclusionAliasSolver,
    UnificationAliasSolver,
};
use crate::alias::{
    AliasConstraintModel, AliasPrecisionConfig,
    domain::{AliasSet, AliasValue},
    place::{AliasTarget, Place},
};

pub struct HybridAliasSolver {
    config: AliasPrecisionConfig,
    inclusion: InclusionAliasSolver,
    unification: UnificationAliasSolver,
    inclusion_vars: BTreeSet<AliasVar>,
    constraints: Vec<AliasConstraint>,
}

impl HybridAliasSolver {
    pub fn new(config: AliasPrecisionConfig) -> Self {
        Self {
            config,
            inclusion: InclusionAliasSolver::new(config),
            unification: UnificationAliasSolver::new(config),
            inclusion_vars: BTreeSet::new(),
            constraints: Vec::new(),
        }
    }

    fn propagate_inclusion_vars(&mut self) {
        let mut changed = true;
        while changed {
            changed = false;
            for constraint in &self.constraints {
                let requires_inclusion = constraint_places(constraint)
                    .any(|place| place_requires_inclusion_for_config(place, self.config))
                    || constraint_vars(constraint).any(|var| self.inclusion_vars.contains(var));
                if requires_inclusion {
                    for var in constraint_vars(constraint) {
                        changed |= self.inclusion_vars.insert(var.clone());
                    }
                }
            }
        }
    }

    fn uses_inclusion(&self, constraint: &AliasConstraint) -> bool {
        match self.config.constraint_model {
            AliasConstraintModel::Hybrid {
                pre_unify_locals, ..
            } => {
                if !pre_unify_locals {
                    return true;
                }
                constraint_places(constraint)
                    .any(|place| place_requires_inclusion_for_config(place, self.config))
                    || constraint_vars(constraint).any(|var| self.inclusion_vars.contains(var))
            }
            AliasConstraintModel::InclusionBased => true,
            AliasConstraintModel::UnificationBased => false,
        }
    }
}

impl AliasSolver for HybridAliasSolver {
    fn seed(&mut self, var: AliasVar, value: AliasValue) {
        if value_requires_inclusion(&value, self.config) {
            self.inclusion_vars.insert(var.clone());
        }
        self.inclusion.seed(var.clone(), value.clone());
        self.unification.seed(var, value);
    }

    fn add_constraint(&mut self, constraint: AliasConstraint) {
        self.constraints.push(constraint);
    }

    fn solve(&mut self) -> AliasSolution {
        self.propagate_inclusion_vars();
        for constraint in self.constraints.clone() {
            if self.uses_inclusion(&constraint) {
                self.inclusion.add_constraint(constraint);
            } else {
                self.unification.add_constraint(constraint);
            }
        }
        let inclusion = self.inclusion.solve();
        let unification = self.unification.solve();
        let mut values = inclusion.values;
        for (var, value) in unification.values {
            values
                .entry(var)
                .or_default()
                .join_with_limit(&value, self.config.max_alias_set_size);
        }
        AliasSolution {
            values,
            incomplete: inclusion.incomplete || unification.incomplete,
        }
    }
}

fn value_requires_inclusion(value: &AliasValue, config: AliasPrecisionConfig) -> bool {
    match &value.may {
        AliasSet::Bottom => false,
        AliasSet::Unknown => true,
        AliasSet::Known(places) => places
            .iter()
            .any(|place| place_requires_inclusion_for_config(place, config)),
    }
}

fn place_requires_inclusion_for_config(place: &Place, config: AliasPrecisionConfig) -> bool {
    match config.constraint_model {
        AliasConstraintModel::Hybrid {
            inclusion_for_resources,
            inclusion_for_public_api,
            ..
        } => place_requires_inclusion(place, inclusion_for_resources, inclusion_for_public_api),
        AliasConstraintModel::InclusionBased => true,
        AliasConstraintModel::UnificationBased => false,
    }
}

fn constraint_places(constraint: &AliasConstraint) -> impl Iterator<Item = &Place> {
    let mut places = Vec::new();
    if let AliasConstraint::AddrOf { target, .. } = constraint {
        places.push(target);
    }
    places.into_iter()
}

fn constraint_vars(constraint: &AliasConstraint) -> impl Iterator<Item = &AliasVar> {
    let mut vars = Vec::new();
    match constraint {
        AliasConstraint::AddrOf { dst, .. } => vars.push(dst),
        AliasConstraint::Copy { dst, src }
        | AliasConstraint::Load { dst, src }
        | AliasConstraint::Store { dst, src } => {
            vars.push(dst);
            vars.push(src);
        }
        AliasConstraint::Project { dst, base, .. } => {
            vars.push(dst);
            vars.push(base);
        }
        AliasConstraint::Call {
            dst, callee, args, ..
        } => {
            if let Some(dst) = dst {
                vars.push(dst);
            }
            vars.push(callee);
            vars.extend(args);
        }
    }
    vars.into_iter()
}

fn place_requires_inclusion(
    place: &Place,
    inclusion_for_resources: bool,
    inclusion_for_public_api: bool,
) -> bool {
    match &place.root {
        AliasTarget::ResourceHandle(_) | AliasTarget::MemoryPlace(_) => inclusion_for_resources,
        AliasTarget::Param { .. }
        | AliasTarget::Return { .. }
        | AliasTarget::TopLevel(_)
        | AliasTarget::AllocationKind { .. }
        | AliasTarget::Captured { .. } => inclusion_for_public_api,
        AliasTarget::Symbol(_)
        | AliasTarget::Allocation { .. }
        | AliasTarget::External
        | AliasTarget::Unknown => false,
    }
}
