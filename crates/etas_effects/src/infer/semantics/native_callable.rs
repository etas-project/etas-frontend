use etas_hir::{HirExpr, HirExprId, SymbolDef};
use std::collections::{BTreeMap, BTreeSet};

use super::{
    callable_summary::{CallableSummaryInstantiation, CallableSummaryMode, CallableSummaryPlan},
    engine::EffectSemantics,
    specialize::{EffectSpecializationError, TraceArgumentBinding, TraceArgumentBindings},
    state::{EffectState, LatentSource},
};
use crate::{
    EffectUnit, ParameterCallTarget,
    facts::{CallableEnvironment, CallableEnvironmentId},
};

fn invalid(reason: &str) -> EffectSpecializationError {
    EffectSpecializationError::InvalidCallableEnvironment {
        reason: reason.into(),
    }
}

impl EffectSemantics<'_> {
    pub(crate) fn capture_native_callable(
        &self,
        unit: EffectUnit,
        application: HirExprId,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
        environment: Option<CallableEnvironmentId>,
    ) -> Result<LatentSource, EffectSpecializationError> {
        let EffectUnit::AnonymousFlow { value, .. } = unit else {
            return Ok(LatentSource::Unit(unit));
        };
        let Some(HirExpr::Lambda { scope, .. }) = self.hir.exprs.get(value) else {
            return Ok(LatentSource::Unit(unit));
        };
        let mut ancestor_scopes = BTreeSet::new();
        let mut parent = self
            .hir
            .scopes
            .get(*scope)
            .ok_or_else(|| invalid("lambda lexical scope is missing"))?
            .parent;
        while let Some(scope) = parent {
            if !ancestor_scopes.insert(scope) {
                return Err(invalid("lambda lexical scope graph is cyclic"));
            }
            parent = self
                .hir
                .scopes
                .get(scope)
                .ok_or_else(|| invalid("lambda parent lexical scope is missing"))?
                .parent;
        }
        let mut pending = Vec::new();
        let symbols = self.lambda_referenced_symbols(value).map_err(invalid)?;
        for symbol in symbols {
            let data = self
                .hir
                .symbols
                .get(symbol)
                .ok_or_else(|| invalid("captured symbol is missing"))?;
            if !matches!(
                data.def,
                SymbolDef::Param { .. }
                    | SymbolDef::Local { .. }
                    | SymbolDef::PatternBinding { .. }
            ) {
                continue;
            }
            if !self.symbol_type_contains_flow(symbol) {
                continue;
            }
            let scope = self
                .symbol_lexical_scopes
                .get(&symbol)
                .copied()
                .flatten()
                .ok_or_else(|| {
                    invalid("captured symbol has no unique lexical declaration scope")
                })?;
            if !ancestor_scopes.contains(&scope) {
                continue;
            }
            match data.def {
                SymbolDef::Param { owner, .. } => {
                    pending.push(LatentSource::Parameter { owner, symbol })
                }
                SymbolDef::Local { .. } | SymbolDef::PatternBinding { .. } => {
                    let sources = self
                        .captured_values
                        .get(&unit)
                        .and_then(|capture| capture.symbol_sources(symbol))
                        .or_else(|| {
                            self.discovered_value_captures
                                .get(&unit)
                                .and_then(|capture| capture.symbol_sources(symbol))
                        })
                        .ok_or_else(|| {
                            invalid("lambda's creation-time callable source is missing")
                        })?;
                    pending.extend(sources);
                }
                _ => {}
            }
        }
        let mut visited = BTreeSet::new();
        let mut parameters = BTreeMap::new();
        while let Some(source) = pending.pop() {
            if !visited.insert(source) {
                continue;
            }
            match source {
                LatentSource::Parameter { owner, symbol } => {
                    let target = ParameterCallTarget::Native(symbol);
                    let origins = match bindings.get(&target) {
                        Some(binding) => self.trace_argument_origins(binding, state)?,
                        None if state.owner == Some(owner) => [source].into(),
                        None => {
                            return Err(invalid(
                                "captured callback parameter has no checked invocation binding",
                            ));
                        }
                    };
                    parameters.insert(target, origins);
                }
                LatentSource::Metadata(source) => {
                    if let Some(id) = source.environment {
                        for origins in self.callable_environment(id)?.parameters.into_values() {
                            pending.extend(origins);
                        }
                    }
                }
                LatentSource::Captured { environment, .. } => {
                    for origins in self
                        .callable_environment(environment)?
                        .parameters
                        .into_values()
                    {
                        pending.extend(origins);
                    }
                }
                _ => {}
            }
        }
        let generics = self.native_creation_generics(
            unit.owner()
                .ok_or_else(|| invalid("captured lambda has no owner"))?,
            application,
            state,
            environment,
        )?;
        if parameters.is_empty() && generics.is_empty() {
            return Ok(LatentSource::Unit(unit));
        }
        let id = CallableEnvironmentId {
            origin: self.callable_environment_site(value)?,
            application: self.callable_environment_site(application)?,
        };
        let owner = state
            .owner
            .ok_or_else(|| invalid("captured callable has no current checked owner"))?;
        self.record_callable_environment(
            id,
            CallableEnvironment {
                owner,
                parameters,
                generics,
            },
        )?;
        Ok(LatentSource::Captured {
            unit,
            environment: id,
        })
    }

    pub(super) fn prepare_native_value_summary(
        &self,
        unit: EffectUnit,
        environment: CallableEnvironmentId,
        span: etas_core::Span,
        mode: CallableSummaryMode,
    ) -> Result<CallableSummaryPlan, EffectSpecializationError> {
        let snapshot = self.callable_environment(environment)?;
        let (types, rows) = self.native_environment_generics(
            unit.owner()
                .ok_or_else(|| invalid("captured callable has no checked owner"))?,
            &snapshot.generics,
        )?;
        let template = self
            .latent_leaf_summary(LatentSource::Unit(unit), span)
            .ok_or_else(|| invalid("captured callable has no solved template summary"))?;
        let template = self.specialize_summary_with_bindings(&template, &[], &types, &rows)?;
        let mut bindings = snapshot
            .parameters
            .into_iter()
            .map(|(target, sources)| (target, TraceArgumentBinding::Origins(sources)))
            .collect::<TraceArgumentBindings>();
        let mut invocation_state = EffectState::for_unit(unit);
        invocation_state.owner = Some(snapshot.owner);
        self.bind_spec_method_arguments(
            &template,
            &mut bindings,
            environment.application,
            &invocation_state,
            &types,
            &rows,
        )?;
        let bindings = bindings
            .into_iter()
            .map(|(target, binding)| match binding {
                TraceArgumentBinding::Origins(sources) => Ok((target, sources)),
                _ => Err(invalid(
                    "captured invocation contains an unbound expression",
                )),
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let own_parameters = self
            .unit_params(unit)
            .ok_or_else(|| invalid("captured callable has no checked parameter interface"))?;
        let mut symbolic_parameters = own_parameters
            .into_iter()
            .map(ParameterCallTarget::Native)
            .collect::<BTreeSet<_>>();
        if mode == CallableSummaryMode::Descriptor {
            symbolic_parameters.extend(self.lexical_callable_parameters(unit)?);
        }
        Ok(CallableSummaryPlan::Captured(
            CallableSummaryInstantiation {
                template,
                bindings,
                own_parameters: symbolic_parameters,
                type_bindings: types,
            },
        ))
    }

    fn lexical_callable_parameters(
        &self,
        unit: EffectUnit,
    ) -> Result<BTreeSet<ParameterCallTarget>, EffectSpecializationError> {
        let EffectUnit::AnonymousFlow { value, .. } = unit else {
            return Err(invalid(
                "callable descriptor has no anonymous lexical scope",
            ));
        };
        let Some(HirExpr::Lambda { scope, .. }) = self.hir.exprs.get(value) else {
            return Err(invalid("callable descriptor expression is not a lambda"));
        };
        let mut ancestors = BTreeSet::new();
        let mut parent = Some(*scope);
        while let Some(scope) = parent {
            if !ancestors.insert(scope) {
                return Err(invalid("callable descriptor lexical scope graph is cyclic"));
            }
            parent = self
                .hir
                .scopes
                .get(scope)
                .ok_or_else(|| invalid("callable descriptor lexical scope is missing"))?
                .parent;
        }
        let mut parameters = BTreeSet::new();
        for symbol in self.lambda_referenced_symbols(value).map_err(invalid)? {
            if self.checked_function_parameter(symbol).is_none() {
                continue;
            }
            let scope = self
                .symbol_lexical_scopes
                .get(&symbol)
                .copied()
                .flatten()
                .ok_or_else(|| {
                    invalid("callable descriptor parameter has no unique lexical scope")
                })?;
            if ancestors.contains(&scope) {
                parameters.insert(ParameterCallTarget::Native(symbol));
            }
        }
        Ok(parameters)
    }

    pub(crate) fn native_environment_bindings(
        &self,
        id: CallableEnvironmentId,
    ) -> Result<TraceArgumentBindings, EffectSpecializationError> {
        Ok(self
            .callable_environment(id)?
            .parameters
            .into_iter()
            .map(|(parameter, sources)| (parameter, TraceArgumentBinding::Origins(sources)))
            .collect())
    }
}
