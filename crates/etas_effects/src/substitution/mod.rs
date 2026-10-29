mod requests;
mod summary;
#[cfg(test)]
mod tests;
mod trace;

use etas_types::{EffectArgRef, TypeId, TypeSubstitutionEngine};

use crate::{Effect, EffectRegistry, EffectRow, EffectSet, EffectSummary};

pub(crate) use summary::map_summary;

#[derive(Debug)]
pub enum SummarySubstitutionError {
    Type(etas_types::TypeSubstitutionError),
    InvalidAction(crate::ActionRef),
    InvalidStructure,
    UnboundRow(crate::EffectVarId),
    AmbiguousRow(crate::EffectVarId),
    ResourceLimit,
    Interrupted(etas_utils::PipelineInterruption),
}

impl std::fmt::Display for SummarySubstitutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Type(error) => error.fmt(f),
            Self::InvalidAction(action) => write!(
                f,
                "summary action has no matching checked signature: {action:?}"
            ),
            Self::InvalidStructure => f.write_str("invalid effect summary structure"),
            Self::UnboundRow(row) => write!(f, "effect summary retains an unbound row: {row:?}"),
            Self::AmbiguousRow(row) => {
                write!(f, "effect summary has ambiguous row arguments: {row:?}")
            }
            Self::ResourceLimit => {
                f.write_str("effect summary substitution exceeds its node budget")
            }
            Self::Interrupted(interruption) => f.write_str(&interruption.reason),
        }
    }
}
impl std::error::Error for SummarySubstitutionError {}
impl From<etas_types::TypeSubstitutionError> for SummarySubstitutionError {
    fn from(error: etas_types::TypeSubstitutionError) -> Self {
        Self::Type(error)
    }
}

pub(crate) trait SummaryMapper {
    type Error;
    fn checkpoint(&mut self) -> Result<(), Self::Error>;
    fn invalid_structure(&self) -> Self::Error;
    fn ty(&mut self, ty: TypeId) -> Result<TypeId, Self::Error>;
    fn effect(&mut self, effect: Effect) -> Result<Effect, Self::Error>;
    fn row(&mut self, row: &EffectRow) -> Result<EffectRow, Self::Error>;
}

/// Applies checked type/row arguments without resolving names, evaluating HIR,
/// rebuilding registries, or deriving a new body analysis. Source provenance
/// stays in its original checked universe and must retain its application owner.
pub struct EffectSummarySubstitution<'a> {
    types: TypeSubstitutionEngine<'a>,
    registry: &'a EffectRegistry,
    rows: &'a [(String, EffectRow)],
    by_variable: std::collections::HashMap<crate::EffectVarId, &'a EffectRow>,
    remaining: usize,
    control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
}

impl<'a> EffectSummarySubstitution<'a> {
    pub fn new(
        types: TypeSubstitutionEngine<'a>,
        registry: &'a EffectRegistry,
        rows: &'a [(String, EffectRow)],
        max_nodes: usize,
        control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
    ) -> Self {
        Self {
            types,
            registry,
            rows,
            by_variable: std::collections::HashMap::new(),
            remaining: max_nodes,
            control,
        }
    }

    pub fn apply(
        mut self,
        summary: &EffectSummary,
    ) -> Result<EffectSummary, SummarySubstitutionError> {
        for (name, row) in self.rows {
            self.checkpoint()?;
            let variable = crate::effect_var_id_from_name(name);
            if self.by_variable.insert(variable, row).is_some() {
                return Err(SummarySubstitutionError::AmbiguousRow(variable));
            }
            if let Some(open) = row.open {
                return Err(SummarySubstitutionError::UnboundRow(open));
            }
        }
        map_summary(&mut self, summary)
    }

    fn action_arity(
        &self,
        action: &crate::ActionRef,
        count: usize,
    ) -> Result<(), SummarySubstitutionError> {
        if self
            .registry
            .action_signature(action)
            .is_some_and(|signature| signature.effect_args.len() == count)
        {
            Ok(())
        } else {
            Err(SummarySubstitutionError::InvalidAction(action.clone()))
        }
    }
}

impl SummaryMapper for EffectSummarySubstitution<'_> {
    type Error = SummarySubstitutionError;
    fn checkpoint(&mut self) -> Result<(), Self::Error> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or(SummarySubstitutionError::ResourceLimit)?;
        if let Some(control) = self.control {
            control
                .checkpoint()
                .map_err(SummarySubstitutionError::Interrupted)?;
        }
        Ok(())
    }
    fn invalid_structure(&self) -> Self::Error {
        SummarySubstitutionError::InvalidStructure
    }
    fn ty(&mut self, ty: TypeId) -> Result<TypeId, Self::Error> {
        self.checkpoint()?;
        let ty = self.types.substitute(ty)?;
        self.types.validate_closed_argument(ty)?;
        Ok(ty)
    }
    fn effect(&mut self, effect: Effect) -> Result<Effect, Self::Error> {
        self.checkpoint()?;
        Ok(match effect {
            Effect::Action(action) => {
                self.action_arity(&action, 0)?;
                Effect::Action(action)
            }
            Effect::AppliedAction(mut action) => {
                self.action_arity(&action.action, action.args.len())?;
                for argument in &mut action.args {
                    if let EffectArgRef::Type(ty) = argument {
                        *ty = self.ty(*ty)?;
                    }
                }
                Effect::AppliedAction(action)
            }
            Effect::Applied { tag, args } => Effect::Applied {
                tag,
                args: args
                    .into_iter()
                    .map(|ty| self.ty(ty))
                    .collect::<Result<_, _>>()?,
            },
            Effect::Error(ty) => Effect::Error(self.ty(ty)?),
            Effect::Var(row) => return Err(SummarySubstitutionError::UnboundRow(row)),
            other => other,
        })
    }
    fn row(&mut self, row: &EffectRow) -> Result<EffectRow, Self::Error> {
        self.checkpoint()?;
        let mut effects = EffectSet::from_iter(
            row.effects
                .iter()
                .cloned()
                .map(|effect| self.effect(effect))
                .collect::<Result<Vec<_>, _>>()?,
        );
        if let Some(open) = row.open {
            let binding = self
                .by_variable
                .get(&open)
                .copied()
                .ok_or(SummarySubstitutionError::UnboundRow(open))?;
            for effect in binding.effects.iter().cloned() {
                effects.insert(self.effect(effect)?);
            }
        }
        Ok(EffectRow::closed(effects))
    }
}
