use std::collections::BTreeMap;

use etas_hir::{HirExprId, HirItemId};
use etas_types::{CallableGenericParam, ItemSignature, TypeId};

use super::callable_summary::CallableSummaryMode;
use super::{engine::EffectSemantics, specialize::EffectSpecializationError, state::EffectState};
use crate::{
    EffectRow,
    facts::{CallableGenericParameter as Parameter, CallableGenericValue as Value},
};

pub(super) type GenericValues = BTreeMap<Parameter, Value>;
pub(super) type GenericSubstitutions = (Vec<(String, TypeId)>, Vec<(String, EffectRow)>);

impl EffectSemantics<'_> {
    pub(super) fn metadata_environment_generics(
        &self,
        source: crate::MetadataCallableSource,
    ) -> Result<GenericSubstitutions, EffectSpecializationError> {
        self.metadata_environment_generics_in_mode(source, CallableSummaryMode::Invocation)
    }

    pub(super) fn metadata_environment_generics_in_mode(
        &self,
        source: crate::MetadataCallableSource,
        mode: CallableSummaryMode,
    ) -> Result<GenericSubstitutions, EffectSpecializationError> {
        let (_, node, _) = self
            .metadata_callable(source)
            .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
        if node.generic_bindings.is_empty() {
            return Ok((vec![], vec![]));
        }
        let environment = source
            .environment
            .map(|id| self.callable_environment(id))
            .transpose()?;
        let mut types = vec![];
        let mut rows = vec![];
        for binding in &node.generic_bindings {
            let key = Parameter::Graph {
                graph: source.graph,
                scope: binding.scope,
                position: binding.position,
            };
            let value = environment.as_ref().and_then(|env| env.generics.get(&key));
            if value.is_none()
                && mode == CallableSummaryMode::Descriptor
                && matches!(
                    binding.scope,
                    etas_package_metadata::CallableValueScope::Value(_)
                )
                && self
                    .metadata_callable_scopes(source)?
                    .contains(&binding.scope)
            {
                continue;
            }
            let value =
                value.ok_or_else(|| EffectSpecializationError::InvalidCallableEnvironment {
                    reason: "metadata callable has no checked generic capture binding".into(),
                })?;
            match value {
                Value::Type(ty) => types.push((binding.label.clone(), *ty)),
                Value::EffectRow(row) => rows.push((binding.label.clone(), row.clone())),
            }
        }
        Ok((types, rows))
    }

    pub(super) fn native_generic_params(
        &self,
        owner: HirItemId,
    ) -> Option<&[CallableGenericParam]> {
        match self.types.facts.item_signatures.get(&owner)? {
            ItemSignature::Flow(signature) | ItemSignature::Tool(signature) => {
                Some(&signature.generic_params)
            }
            ItemSignature::Agent(signature) => Some(&signature.callable()?.generic_params),
            ItemSignature::TopLevelLet(_) => Some(&[]),
        }
    }

    pub(super) fn checked_call_generic_values(
        &self,
        call: HirExprId,
        state: &EffectState,
    ) -> Result<GenericSubstitutions, EffectSpecializationError> {
        let checked = self.call_generic_bindings(call);
        let mut rows = checked
            .effect_rows
            .iter()
            .map(|(name, row)| (name.clone(), self.row_from_type_ref(row)))
            .collect::<BTreeMap<_, _>>();
        for (name, row) in
            self.solve_deferred_effect_row_obligations(state, &checked.deferred_effect_rows)?
        {
            if rows.get(&name).is_some_and(|previous| previous != &row) {
                return Err(
                    EffectSpecializationError::IncompatibleLatentEffectBindings { param: name },
                );
            }
            rows.insert(name, row);
        }
        let rows = rows.into_iter().collect::<Vec<_>>();
        self.generic_effect_rows
            .borrow_mut()
            .insert(call, rows.clone());
        Ok((checked.types, rows))
    }

    pub(super) fn native_creation_generics(
        &self,
        owner: HirItemId,
        call: HirExprId,
        state: &EffectState,
        environment: Option<crate::facts::CallableEnvironmentId>,
    ) -> Result<GenericValues, EffectSpecializationError> {
        let params = self
            .native_generic_params(owner)
            .ok_or(EffectSpecializationError::InvalidCallableParameterInterface)?;
        if params.is_empty() {
            return Ok(BTreeMap::new());
        }
        if let Some(environment) = environment {
            let snapshot = self.callable_environment(environment)?;
            // An invoked closure already carries its lexical owner's arguments.
            // They are not generic arguments of the monomorphic closure call.
            if params.iter().enumerate().all(|(position, _)| {
                snapshot.generics.contains_key(&Parameter::Native {
                    owner,
                    position: position as u32,
                })
            }) {
                return Ok(snapshot.generics.into_iter().filter(|(parameter, _)| matches!(parameter, Parameter::Native { owner: actual, .. } if *actual == owner)).collect());
            }
        }
        let (types, rows) = self.checked_call_generic_values(call, state)?;
        let mut values = BTreeMap::new();
        for (position, parameter) in params.iter().enumerate() {
            let value = match parameter.kind {
                etas_types::CallableGenericParamKind::Type => Value::Type(
                    types
                        .iter()
                        .find(|(name, _)| name == &parameter.name)
                        .map(|(_, ty)| *ty)
                        .ok_or_else(|| EffectSpecializationError::MissingTypeBinding {
                            param: parameter.name.clone(),
                        })?,
                ),
                etas_types::CallableGenericParamKind::Effect => Value::EffectRow(
                    rows.iter()
                        .find(|(name, _)| name == &parameter.name)
                        .map(|(_, row)| row.clone())
                        .ok_or_else(|| EffectSpecializationError::MissingEffectRowBinding {
                            param: parameter.name.clone(),
                        })?,
                ),
            };
            values.insert(
                Parameter::Native {
                    owner,
                    position: position as u32,
                },
                value,
            );
        }
        Ok(values)
    }

    pub(super) fn native_environment_generics(
        &self,
        owner: HirItemId,
        values: &GenericValues,
    ) -> Result<GenericSubstitutions, EffectSpecializationError> {
        let params = self
            .native_generic_params(owner)
            .ok_or(EffectSpecializationError::InvalidCallableParameterInterface)?;
        let mut types = vec![];
        let mut rows = vec![];
        for (position, parameter) in params.iter().enumerate() {
            let key = Parameter::Native {
                owner,
                position: position as u32,
            };
            match (parameter.kind, values.get(&key)) {
                (etas_types::CallableGenericParamKind::Type, Some(Value::Type(ty))) => {
                    types.push((parameter.name.clone(), *ty))
                }
                (etas_types::CallableGenericParamKind::Effect, Some(Value::EffectRow(row))) => {
                    rows.push((parameter.name.clone(), row.clone()))
                }
                _ => {
                    return Err(EffectSpecializationError::InvalidCallableEnvironment {
                        reason: "captured generic binding is missing or has the wrong kind".into(),
                    });
                }
            }
        }
        Ok((types, rows))
    }

    pub(super) fn instantiate_generic_values(
        &self,
        values: GenericValues,
        call: HirExprId,
        state: &EffectState,
    ) -> Result<GenericValues, EffectSpecializationError> {
        let (types, rows) = self.checked_call_generic_values(call, state)?;
        let substitution = types.iter().cloned().collect();
        let row_arguments = self.type_row_arguments(&rows, state.owner)?;
        values
            .into_iter()
            .map(|(parameter, value)| {
                let value = match value {
                    Value::Type(ty) => Value::Type(
                        etas_types::TypeSubstitutionEngine::existing(
                            &self.types.store,
                            &substitution,
                        )
                        .with_effect_arguments(&row_arguments)
                        .substitute(ty)?,
                    ),
                    Value::EffectRow(row) => {
                        Value::EffectRow(self.specialize_effect_row(&row, &[], &types, &rows)?)
                    }
                };
                Ok((parameter, value))
            })
            .collect()
    }

    pub(super) fn type_row_arguments(
        &self,
        rows: &[(String, EffectRow)],
        owner: Option<HirItemId>,
    ) -> Result<
        std::collections::HashMap<String, etas_types::EffectRowRef>,
        EffectSpecializationError,
    > {
        let parameters = owner
            .and_then(|owner| self.native_generic_params(owner))
            .unwrap_or(&[])
            .iter()
            .filter(|param| param.kind == etas_types::CallableGenericParamKind::Effect)
            .map(|param| param.name.clone())
            .collect::<Vec<_>>();
        rows.iter()
            .map(|(name, row)| {
                crate::facts::checked_type_effect_row(row, self.registry, &parameters)
                    .map(|row| (name.clone(), row))
                    .map_err(
                        |reason| EffectSpecializationError::InvalidCallableEnvironment { reason },
                    )
            })
            .collect()
    }
}
