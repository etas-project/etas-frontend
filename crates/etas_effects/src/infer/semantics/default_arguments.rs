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
        let mut arguments = self.argument_slots_at_call(call, default.arity)?;
        // Default expressions may capture earlier calling slots. The omitted
        // slot itself is not an expression in the consuming HIR.
        arguments.truncate(position);
        let (mut types, rows) = self.checked_call_generic_values(call, state)?;
        if let Some(method) = self.types.facts.checked_method_calls.get(&call) {
            for (parameter, argument) in &method.owner_bindings {
                if let Some(etas_types::Type::Named(parameter)) = self.types.store.get(*parameter) {
                    if !types.iter().any(|(name, _)| name == &parameter.name) {
                        types.push((parameter.name.clone(), *argument));
                    }
                }
            }
        }
        let context = MetadataCallBindings {
            key: Some(default.graph_key),
            scope: CallableValueScope::Root,
            environment: None,
            arguments: &arguments,
            generics: &default.generics,
            type_bindings: &types,
            effect_row_bindings: &rows,
        };
        let sources = self.metadata_value_sources(context, &default.graph.sources, state, call)?;
        let projections = default
            .graph
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
