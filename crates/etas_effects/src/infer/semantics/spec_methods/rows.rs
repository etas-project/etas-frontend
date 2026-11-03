use etas_hir::HirExprId;

use crate::{
    ParameterCallTarget,
    facts::CallableEnvironment,
    infer::semantics::{
        engine::EffectSemantics,
        specialize::{CheckedGenericBindings, EffectSpecializationError},
        state::EffectState,
    },
};

impl EffectSemantics<'_> {
    pub(super) fn solve_spec_method_row_obligations(
        &self,
        call: HirExprId,
        snapshot: &CallableEnvironment,
        arguments: &mut CheckedGenericBindings,
    ) -> Result<(), EffectSpecializationError> {
        if arguments.deferred_effect_rows.is_empty() {
            return Ok(());
        }
        let invalid = || EffectSpecializationError::InvalidCallableEnvironment {
            reason: "method callback row has no checked argument capture".into(),
        };
        let mut state = EffectState::for_unit(crate::EffectUnit::Item(snapshot.owner));
        for (position, expr) in self.checked_method_arguments(call)?.into_iter().enumerate() {
            if let Some(sources) = snapshot
                .parameters
                .get(&ParameterCallTarget::Interface(position as u32))
            {
                state
                    .local_latent_values
                    .record_value_sources(expr, sources.iter().copied());
            }
        }
        for obligation in &arguments.deferred_effect_rows {
            if state
                .local_latent_values
                .value_sources(obligation.source)
                .is_none()
            {
                return Err(invalid());
            }
        }
        let existing = arguments
            .effect_rows
            .iter()
            .map(|(name, row)| (name.clone(), self.row_from_type_ref(row)))
            .collect();
        let solved = self
            .solve_deferred_effect_row_obligations(
                &state,
                &arguments.deferred_effect_rows,
                &existing,
            )?
            .into_iter()
            .collect::<Vec<_>>();
        let type_rows = self.type_row_arguments(&solved, Some(snapshot.owner))?;
        for (name, row) in &type_rows {
            if let Some((_, previous)) = arguments
                .effect_rows
                .iter()
                .find(|(param, _)| param == name)
            {
                if previous != row {
                    return Err(
                        EffectSpecializationError::IncompatibleLatentEffectBindings {
                            param: name.clone(),
                        },
                    );
                }
            } else {
                arguments.effect_rows.push((name.clone(), row.clone()));
            }
        }
        self.generic_effect_rows.borrow_mut().insert(call, solved);
        arguments.deferred_effect_rows.clear();
        Ok(())
    }
}
