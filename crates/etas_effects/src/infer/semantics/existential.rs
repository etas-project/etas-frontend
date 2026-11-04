use std::collections::BTreeMap;

use super::{
    engine::EffectSemantics,
    specialize::EffectSpecializationError,
    state::{EffectState, LatentSource},
};
use crate::{
    ParameterCallTarget,
    facts::{
        CallableEnvironment, CallableEnvironmentId, CallableGenericParameter, CallableGenericValue,
    },
};

impl EffectSemantics<'_> {
    pub(super) fn lexical_generic_values(
        &self,
        owner: etas_hir::HirItemId,
    ) -> Result<BTreeMap<CallableGenericParameter, CallableGenericValue>, EffectSpecializationError>
    {
        self.native_generic_params(owner)
            .ok_or(EffectSpecializationError::InvalidCallableParameterInterface)?
            .iter()
            .enumerate()
            .map(|(position, parameter)| {
                let value = match parameter.kind {
                    etas_types::CallableGenericParamKind::Type => {
                        CallableGenericValue::Type(parameter.subject)
                    }
                    etas_types::CallableGenericParamKind::Effect => {
                        CallableGenericValue::EffectRow(crate::EffectRow {
                            effects: crate::EffectSet::new(),
                            open: Some(crate::effect_var_id_from_name(&parameter.name)),
                        })
                    }
                };
                Ok((
                    CallableGenericParameter::Native {
                        owner,
                        position: position as u32,
                    },
                    value,
                ))
            })
            .collect()
    }

    pub(super) fn existential_pack_origin(
        &self,
        state: &EffectState,
        pack: etas_hir::HirExprId,
    ) -> Result<LatentSource, EffectSpecializationError> {
        let owner = state
            .owner
            .ok_or(EffectSpecializationError::InvalidCallableParameterInterface)?;
        let site = self.callable_environment_site(pack)?;
        let environment = CallableEnvironmentId {
            origin: site,
            application: site,
        };
        let fact = &self.types.facts.existential_packs[&pack];
        let sources = match self.latent_producer_sources(state, pack) {
            Some(sources) => sources,
            None if !self.type_contains_flow(fact.source_type) => Vec::new(),
            None => vec![LatentSource::Unavailable { expr: pack }],
        }
        .into_iter()
        .collect();
        self.record_callable_environment(
            environment,
            CallableEnvironment {
                owner,
                generics: self.lexical_generic_values(owner)?,
                parameters: BTreeMap::from([(ParameterCallTarget::Interface(0), sources)]),
            },
        )?;
        Ok(LatentSource::Existential { pack, environment })
    }
}
