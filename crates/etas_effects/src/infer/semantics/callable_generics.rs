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

#[derive(Clone, Copy)]
pub(super) struct ScopedGenericArguments<'a> {
    pub types: &'a [(String, TypeId)],
    pub parameters: Option<&'a std::collections::HashMap<TypeId, TypeId>>,
    pub rows: &'a [(String, EffectRow)],
}

impl<'a> ScopedGenericArguments<'a> {
    pub fn named(types: &'a [(String, TypeId)], rows: &'a [(String, EffectRow)]) -> Self {
        Self {
            types,
            parameters: None,
            rows,
        }
    }
}

impl EffectSemantics<'_> {
    pub(super) fn metadata_environment_type_parameters(
        &self,
        source: crate::MetadataCallableSource,
    ) -> Result<std::collections::HashMap<TypeId, TypeId>, EffectSpecializationError> {
        let Some(method) = self
            .implementation_graphs
            .get(&source.graph)
            .and_then(|index| index.and_then(|index| self.external_implementations.get(index)))
        else {
            return Ok(Default::default());
        };
        if method.owner_params.is_empty() {
            return Ok(Default::default());
        }
        let snapshot = source
            .environment
            .map(|id| self.callable_environment(id))
            .transpose()?
            .ok_or_else(|| EffectSpecializationError::InvalidCallableEnvironment {
                reason: "metadata method owner has no captured type arguments".into(),
            })?;
        method
            .owner_params
            .iter()
            .enumerate()
            .map(|(position, parameter)| {
                let key = Parameter::Graph {
                    graph: source.graph,
                    scope: etas_package_metadata::CallableValueScope::Root,
                    position: position as u32,
                };
                match snapshot.generics.get(&key) {
                    Some(Value::Type(argument)) => Ok((parameter.subject, *argument)),
                    _ => Err(EffectSpecializationError::InvalidCallableEnvironment {
                        reason: "metadata method owner has an incomplete type argument binding"
                            .into(),
                    }),
                }
            })
            .collect()
    }

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
                Value::EffectRow(row) | Value::EffectRowFromCall { row, .. } => {
                    rows.push((binding.label.clone(), row.clone()))
                }
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

    pub(super) fn native_impl_method(
        &self,
        owner: HirItemId,
    ) -> Option<&etas_types::facts::TypeImplMethodFact> {
        let symbol = self.hir.items.get(owner)?.symbol()?;
        self.types.facts.type_impl_methods.get(&symbol)
    }

    pub(super) fn native_generic_interface(
        &self,
        owner: HirItemId,
    ) -> Option<Vec<(Parameter, &CallableGenericParam)>> {
        let local = self.native_generic_params(owner)?;
        let mut parameters = Vec::new();
        if let Some(method) = self.native_impl_method(owner) {
            parameters.extend(
                method
                    .owner_params
                    .iter()
                    .enumerate()
                    .map(|(position, param)| {
                        (
                            Parameter::Native {
                                owner: method.implementation,
                                position: position as u32,
                            },
                            param,
                        )
                    }),
            );
        }
        parameters.extend(local.iter().enumerate().map(|(position, param)| {
            (
                Parameter::Native {
                    owner,
                    position: position as u32,
                },
                param,
            )
        }));
        Some(parameters)
    }

    pub(super) fn native_environment_type_parameters(
        &self,
        owner: HirItemId,
        values: &GenericValues,
    ) -> Result<std::collections::HashMap<TypeId, TypeId>, EffectSpecializationError> {
        self.native_generic_interface(owner)
            .ok_or(EffectSpecializationError::InvalidCallableParameterInterface)?
            .into_iter()
            .filter(|(_, parameter)| parameter.kind == etas_types::CallableGenericParamKind::Type)
            .map(|(key, parameter)| match values.get(&key) {
                Some(Value::Type(ty)) => Ok((parameter.subject, *ty)),
                _ => Err(EffectSpecializationError::InvalidCallableEnvironment {
                    reason: "captured generic has no checked type argument".into(),
                }),
            })
            .collect()
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
            self.solve_deferred_effect_row_obligations(state, &checked.deferred_effect_rows, &rows)?
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
            .native_generic_interface(owner)
            .ok_or(EffectSpecializationError::InvalidCallableParameterInterface)?;
        if params.is_empty() {
            return Ok(BTreeMap::new());
        }
        let snapshot = environment
            .map(|environment| self.callable_environment(environment))
            .transpose()?;
        if let Some(snapshot) = &snapshot {
            // An invoked closure already carries its lexical owner's arguments.
            // They are not generic arguments of the monomorphic closure call.
            if params
                .iter()
                .all(|(key, _)| snapshot.generics.contains_key(key))
            {
                return Ok(params
                    .iter()
                    .map(|(key, _)| (*key, snapshot.generics[key].clone()))
                    .collect());
            }
        }
        let (types, rows) = self.checked_call_generic_values(call, state)?;
        let method_arguments = self.types.facts.checked_method_calls.get(&call);
        let mut values = BTreeMap::new();
        for (key, parameter) in params {
            if let Some(value) = snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.generics.get(&key))
            {
                values.insert(key, value.clone());
                continue;
            }
            if matches!(key, Parameter::Native { owner: declaration, .. } if declaration != owner) {
                let value = method_arguments
                    .and_then(|fact| {
                        fact.owner_bindings.iter().find_map(|(subject, argument)| {
                            (*subject == parameter.subject).then_some(Value::Type(*argument))
                        })
                    })
                    .ok_or_else(|| EffectSpecializationError::MissingTypeBinding {
                        param: format!("impl {}", parameter.name),
                    })?;
                values.insert(key, value);
                continue;
            }
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
                etas_types::CallableGenericParamKind::Effect => {
                    let row = rows
                        .iter()
                        .find(|(name, _)| name == &parameter.name)
                        .map(|(_, row)| row.clone())
                        .ok_or_else(|| EffectSpecializationError::MissingEffectRowBinding {
                            param: parameter.name.clone(),
                        })?;
                    if row.open.is_some() {
                        Value::EffectRowFromCall { row, source: call }
                    } else {
                        Value::EffectRow(row)
                    }
                }
            };
            values.insert(key, value);
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
                (
                    etas_types::CallableGenericParamKind::Effect,
                    Some(Value::EffectRow(row) | Value::EffectRowFromCall { row, .. }),
                ) => rows.push((parameter.name.clone(), row.clone())),
                _ => {
                    return Err(EffectSpecializationError::InvalidCallableEnvironment {
                        reason: "captured generic binding is missing or has the wrong kind".into(),
                    });
                }
            }
        }
        Ok((types, rows))
    }

    pub(super) fn substitute_generic_values(
        &self,
        values: GenericValues,
        arguments: ScopedGenericArguments<'_>,
        owner: Option<HirItemId>,
    ) -> Result<GenericValues, EffectSpecializationError> {
        let ScopedGenericArguments {
            types,
            parameters,
            rows,
        } = arguments;
        let substitution = types.iter().cloned().collect();
        let row_arguments = self.type_row_arguments(rows, owner)?;
        let mut engine =
            etas_types::TypeSubstitutionEngine::existing(&self.types.store, &substitution)
                .with_effect_arguments(&row_arguments);
        if let Some(parameters) = parameters {
            engine = engine.with_type_parameters(parameters);
        }
        values
            .into_iter()
            .map(|(parameter, value)| {
                let value = match value {
                    Value::Type(ty) => Value::Type(engine.substitute(ty)?),
                    Value::EffectRow(row) => {
                        Value::EffectRow(self.specialize_effect_row(&row, &[], types, rows)?)
                    }
                    Value::EffectRowFromCall { row, source } => {
                        let specialized = self.specialize_effect_row(&row, &[], types, rows)?;
                        if specialized == row {
                            Value::EffectRowFromCall { row, source }
                        } else {
                            // The old call site cannot establish a changed row's identity.
                            Value::EffectRow(specialized)
                        }
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
