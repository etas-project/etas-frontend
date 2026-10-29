use etas_hir::HirExprId;
use etas_types::{TypeId, TypeSubstitutionEngine};

use super::EffectSpecializationError;
use crate::{
    Effect, EffectRow, infer::semantics::engine::EffectSemantics, substitution::SummaryMapper,
};

pub(super) struct CallSummaryMapper<'a, 'hir> {
    pub(super) semantics: &'a EffectSemantics<'hir>,
    pub(super) bindings: &'a [(String, HirExprId)],
    pub(super) type_bindings: &'a [(String, TypeId)],
    pub(super) rows: &'a [(String, EffectRow)],
    pub(super) types: TypeSubstitutionEngine<'a>,
}

impl SummaryMapper for CallSummaryMapper<'_, '_> {
    type Error = EffectSpecializationError;
    fn checkpoint(&mut self) -> Result<(), Self::Error> {
        if let Some(control) = self.semantics.query_control {
            control
                .checkpoint()
                .map_err(EffectSpecializationError::Interrupted)?;
        }
        Ok(())
    }
    fn invalid_structure(&self) -> Self::Error {
        EffectSpecializationError::InvalidDefaultHandlerAction
    }
    fn ty(&mut self, ty: TypeId) -> Result<TypeId, Self::Error> {
        self.types.substitute(ty).map_err(Into::into)
    }
    fn effect(&mut self, effect: Effect) -> Result<Effect, Self::Error> {
        self.semantics
            .specialize_effect(effect, self.bindings, self.type_bindings)
    }
    fn row(&mut self, row: &EffectRow) -> Result<EffectRow, Self::Error> {
        self.semantics
            .specialize_effect_row(row, self.bindings, self.type_bindings, self.rows)
    }
}
