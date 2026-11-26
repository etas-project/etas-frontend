use std::sync::Arc;

use etas_hir::HirExprId;
use etas_package_metadata::CallableValueScope;

use super::{
    engine::EffectSemantics,
    metadata_callable::MetadataCallBindings,
    specialize::{EffectSpecializationError as Error, TraceArgumentBinding},
    state::EffectState,
};

impl EffectSemantics<'_> {
    pub(super) fn omitted_argument_units(&self, call: HirExprId) -> Vec<crate::EffectUnit> {
        self.types
            .facts
            .checked_method_calls
            .get(&call)
            .map(|method| method.defaults.as_slice())
            .or_else(|| {
                self.types
                    .facts
                    .declared_call_arguments
                    .get(&call)
                    .map(|binding| binding.defaults.as_slice())
            })
            .into_iter()
            .flatten()
            .filter_map(|default| {
                let etas_types::ParameterDefaultRef::Source { parameter } = default.source else {
                    return None;
                };
                let etas_hir::SymbolDef::Param { owner, .. } = self.hir.symbols.get(parameter)?.def
                else {
                    return None;
                };
                Some(crate::EffectUnit::ParameterDefault {
                    owner,
                    parameter,
                    expr: self.hir.parameter_defaults[&parameter],
                })
            })
            .collect()
    }

    pub(super) fn bind_parameter_default_generics(
        &self,
        call: HirExprId,
        parameter: Option<etas_hir::SymbolId>,
        types: &mut Vec<(String, etas_types::TypeId)>,
        parameters: &mut std::collections::HashMap<etas_types::TypeId, etas_types::TypeId>,
    ) -> Result<(), Error> {
        let Some(method) = self.types.facts.checked_method_calls.get(&call) else {
            return Ok(());
        };
        for (parameter, argument) in &method.owner_bindings {
            parameters.insert(*parameter, *argument);
            if let Some(etas_types::Type::Named(parameter)) = self.types.store.get(*parameter) {
                if !types.iter().any(|(name, _)| name == &parameter.name) {
                    types.push((parameter.name.clone(), *argument));
                }
            }
        }
        let Some(parameter) = parameter else {
            return Ok(());
        };
        let Some(etas_hir::SymbolDef::Param { owner, .. }) =
            self.hir.symbols.get(parameter).map(|symbol| &symbol.def)
        else {
            return Err(Error::InvalidCallableParameterInterface);
        };
        let etas_hir::HirItem::Spec(spec) = &self.hir.items[*owner] else {
            return Ok(());
        };
        let (subject, arguments) = match &method.evidence {
            etas_types::method::MethodEvidence::Implementation { subject, args, .. }
            | etas_types::method::MethodEvidence::Bound { subject, args, .. } => (*subject, args),
            etas_types::method::MethodEvidence::Inherent => {
                return Err(Error::InvalidCallableParameterInterface);
            }
        };
        let mut bind = |symbol, argument| {
            let declared = self
                .hir
                .symbols
                .get(symbol)
                .expect("checked spec generic parameter");
            if let Some((_, value)) = types.iter_mut().find(|(name, _)| name == &declared.name) {
                *value = argument;
            } else {
                types.push((declared.name.clone(), argument));
            }
            let Some(etas_types::SymbolTypeFact::Param { ty }) =
                self.types.facts.symbol_types.get(&symbol)
            else {
                return Err(Error::InvalidCallableParameterInterface);
            };
            parameters.insert(*ty, argument);
            Ok(())
        };
        for (symbol, argument) in spec.type_params.iter().zip(arguments) {
            bind(*symbol, *argument)?;
        }
        if let Some(signature) = spec.items.iter().find_map(|member| match member {
            etas_hir::HirSpecItem::FlowSignature(signature)
                if signature.params.contains(&parameter) =>
            {
                Some(signature)
            }
            _ => None,
        }) {
            if let Some(receiver) = signature.receiver_type_param {
                bind(receiver, subject)?;
            }
            for (symbol, actual) in signature
                .type_params
                .iter()
                .zip(&method.signature.generic_params)
            {
                if let Some((_, argument)) = method
                    .instantiation
                    .type_bindings
                    .iter()
                    .find(|(name, _)| name == &actual.name)
                {
                    bind(*symbol, *argument)?;
                }
            }
        }
        Ok(())
    }

    fn bind_external_default_generics(
        &self,
        call: HirExprId,
        metadata: &crate::ExternalParameterDefaultMetadata,
        types: &mut Vec<(String, etas_types::TypeId)>,
        parameters: &mut std::collections::HashMap<etas_types::TypeId, etas_types::TypeId>,
    ) -> Result<(), Error> {
        self.bind_parameter_default_generics(call, None, types, parameters)?;
        if matches!(
            metadata.reference.owner,
            etas_code::descriptor::ParameterDefaultOwner::SpecMethod { .. }
        ) {
            let method = self
                .types
                .facts
                .checked_method_calls
                .get(&call)
                .ok_or(Error::InvalidCallableParameterInterface)?;
            let arguments = match &method.evidence {
                etas_types::method::MethodEvidence::Implementation { args, .. }
                | etas_types::method::MethodEvidence::Bound { args, .. } => args,
                etas_types::method::MethodEvidence::Inherent => {
                    return Err(Error::InvalidCallableParameterInterface);
                }
            };
            for (parameter, argument) in metadata.owner_params.iter().zip(arguments) {
                types.push((parameter.name.clone(), *argument));
                parameters.insert(parameter.subject, *argument);
            }
        }
        Ok(())
    }

    fn external_binding_summary(
        &self,
        call: HirExprId,
        reference: &etas_code::descriptor::ParameterDefaultInfo,
        state: &EffectState,
    ) -> Result<crate::EffectSummary, Error> {
        let metadata = self
            .external_defaults
            .iter()
            .find(|value| &value.reference == reference)
            .ok_or(Error::InvalidCallableParameterInterface)?;
        let binding = metadata
            .binding
            .as_ref()
            .ok_or(Error::InvalidCallableParameterInterface)?;
        let span = self.hir.exprs[call].span(&self.hir.blocks);
        let summary = self
            .external_summary_from_metadata(binding, span)
            .ok_or(Error::InvalidCallableParameterInterface)?;
        let (mut types, rows) = self.checked_call_generic_values(call, state)?;
        let mut parameters = std::collections::HashMap::new();
        self.bind_external_default_generics(call, metadata, &mut types, &mut parameters)?;
        self.specialize_summary_with_residual_bindings(
            &summary,
            &[],
            &types,
            &parameters,
            &rows,
            None,
            state.owner,
        )
    }

    pub(super) fn apply_omitted_argument_effects(
        &mut self,
        call: HirExprId,
        mut state: EffectState,
    ) -> EffectState {
        let defaults = self
            .types
            .facts
            .checked_method_calls
            .get(&call)
            .map(|method| &method.defaults)
            .or_else(|| {
                self.types
                    .facts
                    .declared_call_arguments
                    .get(&call)
                    .map(|binding| &binding.defaults)
            })
            .cloned()
            .unwrap_or_default();
        let span = self.hir.exprs[call].span(&self.hir.blocks);
        for default in defaults {
            let summary = match default.source {
                etas_types::ParameterDefaultRef::Source { parameter } => {
                    if !self.hir.parameter_injections.contains(&parameter) {
                        continue;
                    }
                    let etas_hir::SymbolDef::Param { owner, .. } = self
                        .hir
                        .symbols
                        .get(parameter)
                        .expect("checked injected parameter")
                        .def
                    else {
                        unreachable!()
                    };
                    let unit = crate::EffectUnit::ParameterDefault {
                        owner,
                        parameter,
                        expr: self.hir.parameter_defaults[&parameter],
                    };
                    let Some(summary) = self.unit_summary(&unit).cloned() else {
                        continue;
                    };
                    let site =
                        etas_hir_analysis::interprocedural::CallSite::unresolved(call, call, span);
                    self.specialize_summary_for_call(unit, &site, &state, &summary, None)
                }
                etas_types::ParameterDefaultRef::External { reference } => {
                    if !reference.injected {
                        continue;
                    }
                    self.external_binding_summary(call, &reference, &state)
                }
            };
            match summary {
                Ok(summary) => {
                    self.note_call_outcomes(call, None, &summary, &mut state);
                    state.summary.seq_assign(&summary);
                }
                Err(error) => {
                    return self.incomplete_at(
                        span,
                        format!("omitted parameter binding: {error}"),
                        state,
                    );
                }
            }
        }
        state
    }

    pub(super) fn default_argument_binding(
        &self,
        call: HirExprId,
        position: usize,
        state: &EffectState,
    ) -> Result<Option<TraceArgumentBinding>, Error> {
        let defaults = self
            .types
            .facts
            .checked_method_calls
            .get(&call)
            .map(|method| method.defaults.as_slice())
            .or_else(|| {
                self.types
                    .facts
                    .declared_call_arguments
                    .get(&call)
                    .map(|binding| binding.defaults.as_slice())
            });
        let Some(etas_types::DefaultArgument {
            source: etas_types::ParameterDefaultRef::External { reference },
            ..
        }) = defaults
            .into_iter()
            .flatten()
            .find(|default| default.slot as usize == position)
        else {
            return Ok(None);
        };
        let Some(default) = self
            .external_defaults
            .iter()
            .find(|default| &default.reference == reference)
        else {
            return Ok(None);
        };
        let Some(graph) = default.graph.as_ref() else {
            return Ok(None);
        };
        let mut arguments = self.argument_slots_at_call(call, default.arity)?;
        // Default expressions may capture earlier calling slots. The omitted
        // slot itself is not an expression in the consuming HIR.
        arguments.truncate(position);
        let (mut types, rows) = self.checked_call_generic_values(call, state)?;
        self.bind_external_default_generics(
            call,
            default,
            &mut types,
            &mut std::collections::HashMap::new(),
        )?;
        let context = MetadataCallBindings {
            key: Some(default.graph_key),
            scope: CallableValueScope::Root,
            environment: None,
            arguments: &arguments,
            generics: &default.generics,
            type_bindings: &types,
            effect_row_bindings: &rows,
        };
        let sources = self.metadata_value_sources(context, &graph.sources, state, call)?;
        let projections = graph
            .source_projections
            .iter()
            .map(|projection| {
                let sources = match &projection.sources {
                    Some(sources) => self.metadata_value_sources(context, sources, state, call)?,
                    None => [super::state::LatentSource::Unavailable { expr: call }].into(),
                };
                Ok(crate::facts::NativeArgumentProjection {
                    path: projection.path.clone(),
                    sources,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(Some(TraceArgumentBinding::InputData {
            argument: Arc::new(crate::facts::NativeCallArgument {
                name: None,
                sources,
                projections,
            }),
            path: Vec::new(),
            call,
        }))
    }
}
