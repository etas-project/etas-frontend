mod requests;
mod summary;
#[cfg(test)]
mod tests;
mod trace;

use etas_code::residual::identity::{
    GenericArgumentIdentity, GenericParameterId, ResidualSubstitutionError,
};
use etas_types::{EffectArgRef, TypeId, TypeSubstitutionEngine};
use std::collections::BTreeMap;

use crate::{Effect, EffectRegistry, EffectRow, EffectSet, EffectSummary};

pub(crate) use summary::map_summary;
pub(crate) use trace::map_trace;

#[derive(Debug)]
pub enum SummarySubstitutionError {
    Type(etas_types::TypeSubstitutionError),
    InvalidAction(crate::ActionRef),
    InvalidStructure,
    UnsupportedResidualSubstitution,
    Residual(ResidualSubstitutionError),
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
            Self::UnsupportedResidualSubstitution => {
                f.write_str("checked residual substitutions are not available")
            }
            Self::Residual(error) => write!(f, "invalid residual substitution: {error:?}"),
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
    fn unsupported_residual_substitution(&self) -> Self::Error;
    fn residual_checks(
        &mut self,
        checks: &crate::ResidualCheckSet,
    ) -> Result<crate::ResidualCheckSet, Self::Error> {
        if checks.is_empty() {
            Ok(crate::ResidualCheckSet::default())
        } else {
            Err(self.unsupported_residual_substitution())
        }
    }
    fn ty(&mut self, ty: TypeId) -> Result<TypeId, Self::Error>;
    fn parameter_target(
        &mut self,
        target: crate::ParameterCallTarget,
    ) -> Result<crate::ParameterCallTarget, Self::Error> {
        Ok(match target {
            crate::ParameterCallTarget::AgentTools(context) => {
                crate::ParameterCallTarget::AgentTools(self.ty(context)?)
            }
            other => other,
        })
    }
    fn effect(&mut self, effect: Effect) -> Result<Effect, Self::Error>;
    fn default_guards(&mut self, guards: &EffectSet) -> Result<Option<EffectSet>, Self::Error> {
        guards
            .iter()
            .cloned()
            .map(|effect| self.effect(effect))
            .collect::<Result<EffectSet, _>>()
            .map(Some)
    }
    fn event(
        &mut self,
        event: &crate::ActionEvent,
    ) -> Result<crate::ActionTraceDomain, Self::Error> {
        let mut event = event.clone();
        event.action = self.effect(event.action)?;
        if !event.action.is_action() {
            return Err(self.invalid_structure());
        }
        Ok(crate::ActionTraceDomain::Event(event))
    }
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
    residual_bindings: Option<&'a BTreeMap<GenericParameterId, GenericArgumentIdentity>>,
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
            residual_bindings: None,
        }
    }

    /// Supplies portable, checked binder identities; this is descriptive
    /// substitution, not plan verification or installation evidence.
    pub fn with_residual_bindings(
        mut self,
        bindings: &'a BTreeMap<GenericParameterId, GenericArgumentIdentity>,
    ) -> Self {
        self.residual_bindings = Some(bindings);
        self
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
    fn unsupported_residual_substitution(&self) -> Self::Error {
        SummarySubstitutionError::UnsupportedResidualSubstitution
    }
    fn residual_checks(
        &mut self,
        checks: &crate::ResidualCheckSet,
    ) -> Result<crate::ResidualCheckSet, Self::Error> {
        if !checks.is_empty()
            && self.residual_bindings.is_none()
            && (!self.types.is_identity_mapping() || !self.rows.is_empty())
        {
            return Err(self.unsupported_residual_substitution());
        }
        let mut result = crate::ResidualCheckSet::default();
        for reference in checks.iter() {
            self.checkpoint()?;
            let reference = if let Some(bindings) = self.residual_bindings {
                reference
                    .substitute_with_budget(bindings, &mut self.remaining)
                    .map_err(SummarySubstitutionError::Residual)?
            } else {
                reference.clone()
            };
            result.insert(reference);
        }
        Ok(result)
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
