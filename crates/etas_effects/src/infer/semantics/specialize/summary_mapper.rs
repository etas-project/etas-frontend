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
    pub(super) residual_bindings: Option<
        &'a std::collections::BTreeMap<
            etas_code::residual::identity::GenericParameterId,
            etas_code::residual::identity::GenericArgumentIdentity,
        >,
    >,
    pub(super) residual_work: usize,
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
    fn unsupported_residual_substitution(&self) -> Self::Error {
        EffectSpecializationError::UnsupportedResidualSubstitution
    }
    fn residual_checks(
        &mut self,
        checks: &crate::ResidualCheckSet,
    ) -> Result<crate::ResidualCheckSet, Self::Error> {
        if !checks.is_empty()
            && self.residual_bindings.is_none()
            && (!self.type_bindings.is_empty() || !self.rows.is_empty())
        {
            return Err(self.unsupported_residual_substitution());
        }
        let mut result = crate::ResidualCheckSet::default();
        for reference in checks.iter() {
            self.checkpoint()?;
            let reference = if let Some(bindings) = self.residual_bindings {
                reference
                    .instantiate_with_budget(bindings, &mut self.residual_work)
                    .map_err(EffectSpecializationError::ResidualSubstitution)?
            } else {
                reference.clone()
            };
            result.insert(reference);
        }
        Ok(result)
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

#[cfg(test)]
mod tests {
    use super::*;
    use etas_code::{
        CodeFingerprint,
        residual::identity::{
            GenericArgumentIdentity, GenericParameterId, ObligationId, ResidualObligationRef,
        },
    };
    use etas_core::{SourceFile, SourceId};
    use std::collections::BTreeMap;

    #[test]
    fn call_specialization_preserves_residual_instances_without_guessing_generic_bindings() {
        let parsed = etas_syntax::parse_program(SourceFile::new(
            SourceId(0),
            None,
            "flow main(value: bool) -> unit { return; }",
        ));
        assert!(parsed.diagnostics.is_empty());
        let hir = etas_hir::HirRevision::try_new(etas_hir::lower_program(&parsed.value)).unwrap();
        let types = etas_types::TypeOutput::default();
        let std = etas_std::standard_registry();
        let registry = crate::EffectRegistry::with_standard_effects_from(&std);
        let semantics = EffectSemantics::new(&hir, &types, &std, &registry, &[], &[]);
        let id = ObligationId::from_canonical_fingerprint(CodeFingerprint::from_bytes([1; 32]));
        let parameter =
            GenericParameterId::from_canonical_fingerprint(CodeFingerprint::from_bytes([2; 32]));
        let reference = |byte| {
            ResidualObligationRef::new(
                id,
                BTreeMap::from([(
                    parameter,
                    GenericArgumentIdentity::Type(CodeFingerprint::from_bytes([byte; 32])),
                )]),
            )
            .unwrap()
        };
        let mut summary = crate::EffectSummary::local();
        assert!(summary.residual_checks.insert(reference(3)));
        assert!(summary.residual_checks.insert(reference(4)));
        let args = [("value".to_owned(), HirExprId(0))];
        let mapped = semantics
            .specialize_summary_with_bindings(&summary, &args, &[], &[])
            .unwrap();
        assert_eq!(mapped.residual_checks, summary.residual_checks);
        assert_eq!(mapped.residual_checks.iter().count(), 2);
        assert!(matches!(
            semantics.specialize_summary_with_bindings(
                &summary,
                &args,
                &[("T".into(), TypeId(0))],
                &[]
            ),
            Err(EffectSpecializationError::UnsupportedResidualSubstitution)
        ));
        summary.residual_checks.insert(
            ResidualObligationRef::new(
                id,
                BTreeMap::from([(parameter, GenericArgumentIdentity::TypeParameter(parameter))]),
            )
            .unwrap(),
        );
        assert!(matches!(
            semantics.specialize_summary_with_bindings(
                &summary,
                &args,
                &[("T".into(), TypeId(0))],
                &[]
            ),
            Err(EffectSpecializationError::UnsupportedResidualSubstitution)
        ));
        assert!(matches!(
            semantics.specialize_summary_with_bindings(
                &summary,
                &args,
                &[],
                &[("E".into(), EffectRow::empty())]
            ),
            Err(EffectSpecializationError::UnsupportedResidualSubstitution)
        ));
    }
}
