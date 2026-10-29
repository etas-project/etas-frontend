use std::collections::{HashMap, HashSet};

use super::{
    EffectArgRef, EffectRowRef, SubstitutionTarget, TypeSubstitutionEngine, TypeSubstitutionError,
};

pub(super) enum EffectRowSubstitution<'a> {
    Arguments(&'a HashMap<String, EffectRowRef>),
    Equations(&'a HashMap<String, EffectRowRef>),
}

pub(super) struct ExpandedRow {
    pub row: EffectRowRef,
    pub argument_start: Option<usize>,
}

impl<'a> TypeSubstitutionEngine<'a> {
    /// Actual arguments are already expressed in the caller's binder namespace.
    pub fn with_effect_arguments(mut self, arguments: &'a HashMap<String, EffectRowRef>) -> Self {
        self.effect_rows = Some(EffectRowSubstitution::Arguments(arguments));
        self.memo.clear();
        self.memo_depths.clear();
        self
    }

    /// Solver equations compose transitively and reject non-identity cycles.
    pub fn with_row_equations(mut self, equations: &'a HashMap<String, EffectRowRef>) -> Self {
        self.effect_rows = Some(EffectRowSubstitution::Equations(equations));
        self.memo.clear();
        self.memo_depths.clear();
        self
    }

    pub fn substitute_effect_row(
        &mut self,
        row: EffectRowRef,
    ) -> Result<EffectRowRef, TypeSubstitutionError> {
        self.checkpoint()?;
        let original_members = row.effects.len();
        let ExpandedRow {
            mut row,
            argument_start,
        } = self.expanded_row(row)?;
        for (index, effect) in row.effects.iter_mut().enumerate() {
            self.checkpoint()?;
            if let Some(budget) = self.budget {
                budget.node()?;
                budget.edges(effect.args.len())?;
            }
            let names = match &self.target {
                SubstitutionTarget::Relocate(input) => Some(input.effect_names),
                SubstitutionTarget::RelocateMaterialize(input) => Some(input.effect_names),
                _ => None,
            };
            if let Some(names) = names {
                effect.name = names.get(&effect.name).cloned().ok_or_else(|| {
                    TypeSubstitutionError::MissingEffectIdentity(effect.name.clone())
                })?;
            }
            for arg in &mut effect.args {
                self.checkpoint()?;
                if let EffectArgRef::Type(ty) = arg {
                    *ty = if argument_start.is_some_and(|start| index >= start) {
                        self.substitute_argument(*ty)?
                    } else {
                        self.substitute(*ty)?
                    };
                }
            }
        }
        let mut unique = Vec::new();
        let mut seen = HashSet::new();
        for (index, effect) in row.effects.into_iter().enumerate() {
            self.checkpoint()?;
            let first = seen.insert(effect.clone());
            if index < original_members || first {
                unique.push(effect);
            }
        }
        row.effects = unique;
        Ok(row)
    }

    pub(super) fn expanded_row(
        &self,
        mut row: EffectRowRef,
    ) -> Result<ExpandedRow, TypeSubstitutionError> {
        if self.is_relocation()
            && let Some(tail) = &row.tail
        {
            return Err(TypeSubstitutionError::OpenEffectRow(tail.clone()));
        }
        let expanded = match &self.effect_rows {
            Some(EffectRowSubstitution::Arguments(arguments)) => {
                if let Some(binding) = row.tail.as_ref().and_then(|tail| arguments.get(tail))
                    && let Some(budget) = self.budget
                {
                    budget.edges(binding.effects.len())?;
                }
                let argument_start =
                    row.tail
                        .as_ref()
                        .and_then(|tail| arguments.get(tail))
                        .map(|argument| {
                            let start = row.effects.len();
                            row.effects.extend(argument.effects.iter().cloned());
                            row.tail = argument.tail.clone();
                            start
                        });
                Ok(ExpandedRow {
                    row,
                    argument_start,
                })
            }
            Some(EffectRowSubstitution::Equations(equations)) => {
                let mut visited = HashSet::new();
                while let Some((tail, binding)) = row.tail.as_ref().and_then(|tail| {
                    equations
                        .get(tail)
                        .filter(|binding| {
                            !binding.effects.is_empty() || binding.tail.as_ref() != Some(tail)
                        })
                        .map(|binding| (tail.clone(), binding))
                }) {
                    self.checkpoint()?;
                    if let Some(budget) = self.budget {
                        budget.node()?;
                        budget.edges(binding.effects.len())?;
                    }
                    if !visited.insert(tail.clone()) {
                        return Err(TypeSubstitutionError::CyclicEffectRow(tail));
                    }
                    if let Some(limit) = self.expansion_depth
                        && self.active.len() + visited.len() > limit
                    {
                        return Err(TypeSubstitutionError::ExpansionDepth(limit));
                    }
                    row.effects.extend(binding.effects.iter().cloned());
                    row.tail = binding.tail.clone();
                }
                Ok(ExpandedRow {
                    row,
                    argument_start: None,
                })
            }
            None => Ok(ExpandedRow {
                row,
                argument_start: None,
            }),
        }?;
        if self.closed_named_types.is_some()
            && let Some(tail) = &expanded.row.tail
        {
            return Err(TypeSubstitutionError::OpenEffectRow(tail.clone()));
        }
        Ok(expanded)
    }
}
