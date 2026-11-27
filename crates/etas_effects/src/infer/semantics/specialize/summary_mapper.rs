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
    pub(super) type_parameters: &'a std::collections::HashMap<TypeId, TypeId>,
    pub(super) rows: &'a [(String, EffectRow)],
    pub(super) residual_bindings: Option<
        &'a std::collections::BTreeMap<
            etas_code::residual::identity::GenericParameterId,
            etas_code::residual::identity::GenericArgumentIdentity,
        >,
    >,
    pub(super) residual_work: usize,
    pub(super) owner: Option<etas_hir::HirItemId>,
    pub(super) types: TypeSubstitutionEngine<'a>,
    pub(super) source_memory_place: Option<&'a etas_types::EffectArgRef>,
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
            && (!self.type_bindings.is_empty()
                || !self.type_parameters.is_empty()
                || !self.rows.is_empty())
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
        self.types.substitute(ty).map_err(|error| {
            if let etas_types::TypeSubstitutionError::UnmaterializedType(ty) = &error {
                self.semantics.requested_types.borrow_mut().push(ty.clone());
            }
            error.into()
        })
    }
    fn request_type(&mut self, ty: TypeId) -> Result<TypeId, Self::Error> {
        // Captured callable sources retain their own row environment. A request
        // ABI applies the invocation's row arguments before crossing a handler.
        if self.rows.is_empty() {
            return self.ty(ty);
        }
        let rows = self.semantics.type_row_arguments(self.rows, self.owner)?;
        let names = self.type_bindings.iter().cloned().collect();
        self.semantics
            .type_substitution(&names)
            .with_type_parameters(self.type_parameters)
            .with_effect_arguments(&rows)
            .substitute(ty)
            .map_err(|error| {
                if let etas_types::TypeSubstitutionError::UnmaterializedType(ty) = &error {
                    self.semantics.requested_types.borrow_mut().push(ty.clone());
                }
                error.into()
            })
    }
    fn effect(&mut self, effect: Effect) -> Result<Effect, Self::Error> {
        let mut effect = match effect {
            Effect::AppliedAction(_) => {
                self.semantics
                    .specialize_effect(effect, self.bindings, &[])?
            }
            other => other,
        };
        match &mut effect {
            Effect::AppliedAction(action) => {
                if let Some(place) = self.source_memory_place
                    && action.action.tag == crate::MEMORY_TAG
                    && action.args == [etas_types::EffectArgRef::Wildcard]
                {
                    action.args[0] = place.clone();
                }
                for arg in &mut action.args {
                    if let etas_types::EffectArgRef::Type(ty) = arg {
                        *ty = self.ty(*ty)?;
                    }
                }
            }
            Effect::Applied { args, .. } => {
                for ty in args {
                    *ty = self.ty(*ty)?;
                }
            }
            Effect::Error(ty) => *ty = self.ty(*ty)?,
            _ => {}
        }
        Ok(effect)
    }
    fn row(&mut self, row: &EffectRow) -> Result<EffectRow, Self::Error> {
        let effects = row
            .effects
            .iter()
            .cloned()
            .map(|effect| self.effect(effect))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(super::bind_effect_row_tail(
            EffectRow {
                effects: effects.into_iter().collect(),
                open: row.open,
            },
            self.rows,
        ))
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
            .specialize_summary_with_bindings(&summary, &args, &[], &[], None)
            .unwrap();
        assert_eq!(mapped.residual_checks, summary.residual_checks);
        assert_eq!(mapped.residual_checks.iter().count(), 2);
        assert!(matches!(
            semantics.specialize_summary_with_bindings(
                &summary,
                &args,
                &[("T".into(), TypeId(0))],
                &[],
                None,
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
                &[],
                None,
            ),
            Err(EffectSpecializationError::UnsupportedResidualSubstitution)
        ));
        assert!(matches!(
            semantics.specialize_summary_with_bindings(
                &summary,
                &args,
                &[],
                &[("E".into(), EffectRow::empty())],
                None,
            ),
            Err(EffectSpecializationError::UnsupportedResidualSubstitution)
        ));
    }
}
