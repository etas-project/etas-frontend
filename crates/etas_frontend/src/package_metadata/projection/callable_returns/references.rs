use super::*;
use etas_effects::{CallableReturnReference, facts::CallableReferenceAnalysis};

impl Builder<'_, '_> {
    pub(super) fn reserve_reference(
        &mut self,
        reference: CallableReturnReference,
        scope: CallableValueScope,
    ) -> Result<u32, PackageMetadataError> {
        if let Some(index) = self.references.get(&reference) {
            return Ok(*index);
        }
        if self.values.len() >= etas_package_metadata::MAX_METADATA_GRAPH_NODES {
            return Err(invalid(
                "callable references exceed the metadata node budget",
            ));
        }
        let index = u32::try_from(self.values.len())
            .map_err(|_| invalid("callable reference index exceeds u32"))?;
        self.references.insert(reference.clone(), index);
        self.parents.insert(index, scope);
        self.values.push(None);
        self.origins
            .push(crate::package_metadata::CallableGraphOrigin::Reference(
                reference.clone(),
            ));
        self.pending.push(Pending::Reference(index, reference));
        Ok(index)
    }

    pub(super) fn reference_node(
        &mut self,
        index: u32,
        reference: CallableReturnReference,
    ) -> Result<Option<CallableValueNode>, PackageMetadataError> {
        let checked = self.projection.checked;
        let analysis = checked
            .effects
            .callable_references
            .get(&reference)
            .ok_or_else(|| {
                invalid("returned function reference has no materialized checked analysis")
            })?;
        let CallableReferenceAnalysis::Solved {
            summary,
            type_bindings,
            effect_row_bindings,
            invocation_environment,
            param_names,
            param_symbols,
        } = analysis
        else {
            return Ok(None);
        };
        let Type::Function(signature) = checked
            .type_store
            .get(reference.signature)
            .ok_or_else(|| invalid("returned reference has no checked signature type"))?
        else {
            return Err(invalid("returned reference signature is not callable"));
        };
        if param_names.len() != signature.input.len()
            || (!param_symbols.is_empty() && param_symbols.len() != signature.input.len())
        {
            return Err(invalid(
                "returned reference parameter interface is inconsistent",
            ));
        }
        let owner = reference
            .environment
            .map(|id| {
                checked
                    .effects
                    .callable_environments
                    .get(&id)
                    .map(|env| env.owner)
                    .ok_or_else(|| invalid("reference creation environment is missing"))
            })
            .transpose()?
            .unwrap_or(reference.owner);
        let mut projection = self.generic_context(Some(owner))?;
        for (name, ty) in type_bindings {
            projection.owner_type_names.insert(name.clone());
            let mut ty = self.projection.metadata_type(*ty)?;
            normalize_generic_type(&mut ty, &projection.type_names);
            projection.types.insert(name.clone(), ty);
        }
        for (name, row) in effect_row_bindings {
            projection.rows.insert(
                name.clone(),
                self.projection.effect_row_metadata(
                    row,
                    &projection.type_names,
                    &projection.effect_names,
                )?,
            );
        }
        self.parameters.insert(
            CallableValueScope::Value(index),
            param_symbols
                .iter()
                .enumerate()
                .map(|(position, symbol)| (*symbol, position as u32))
                .collect(),
        );
        let returned_sources = match sources(summary) {
            Some(returned) => match self.sources(
                &returned,
                CallableValueScope::Value(index),
                *invocation_environment,
            )? {
                Some(sources) => Some(sources),
                None => return Ok(None),
            },
            None => None,
        };
        let mut trace_labels = BTreeMap::new();
        let mut parameter_bindings = vec![];
        let mut bound = BTreeSet::new();
        for target in trace_parameters(&summary.action_trace) {
            let etas_effects::ParameterCallTarget::Native(symbol) = target else {
                return Ok(None);
            };
            let Some(parameter) = self.parameter(symbol, CallableValueScope::Value(index))? else {
                return Ok(None);
            };
            let label = match parameter.scope {
                CallableValueScope::Root => format!("rp{}", parameter.position),
                CallableValueScope::Value(value) => format!("v{value}p{}", parameter.position),
            };
            trace_labels.insert(target, label.clone());
            if bound.insert(parameter) {
                parameter_bindings.push(etas_package_metadata::CallableValueParameterBinding {
                    label,
                    parameter,
                });
            }
        }
        let Some(projected) = summary::project(
            self.projection,
            summary,
            &projection.type_names,
            &projection.effect_names,
            &trace_labels,
        )?
        else {
            return Ok(None);
        };
        Ok(Some(CallableValueNode {
            handler_requests: projected.handler_requests,
            parent_scope: *self
                .parents
                .get(&index)
                .ok_or_else(|| invalid("reference node has no checked lexical parent"))?,
            generic_params: vec![],
            generic_bindings: projection.bindings.clone(),
            param_names: param_names.clone(),
            params: signature
                .input
                .iter()
                .map(|ty| projection.metadata_type(self.projection, *ty))
                .collect::<Result<_, _>>()?,
            output: projection.metadata_type(self.projection, signature.output)?,
            behavior: projected.behavior,
            public_effects: projected.public_effects,
            requested_actions: projected.requested_actions,
            handled_requested_actions: projected.handled_requested_actions,
            default_handler_effects: projected.default_handler_effects,
            parameter_bindings,
            action_trace: projected.action_trace,
            returned_sources,
        }))
    }
}
