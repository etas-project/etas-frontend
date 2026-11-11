use super::*;

impl Relocation<'_> {
    pub(in crate::code::definition::frozen_calls) fn graph_objects(
        &self,
        objects: Vec<ExternalExistentialValueMetadata>,
        scope: &GenericOwner,
    ) -> Result<Vec<ExternalExistentialValueMetadata>, ProjectionError> {
        objects
            .into_iter()
            .map(|mut object| {
                object.object_type = self.ty(scope, object.object_type)?;
                object.source_type = self.ty(scope, object.source_type)?;
                for witness in &mut object.witnesses {
                    let etas_types::CheckedSpecRef::External(source) = &witness.bound.spec else {
                        return Err(invalid(
                            "frozen metadata object has no external spec identity",
                        ));
                    };
                    let identity =
                        self.from.identities.path(&source.path).ok_or_else(|| {
                            invalid("frozen object spec has no canonical identity")
                        })?;
                    let target = self
                        .target_output
                        .facts
                        .external_specs
                        .signatures
                        .keys()
                        .find(|target| self.to.identities.path(&target.path) == Some(identity))
                        .ok_or_else(|| {
                            invalid("frozen object spec has no checked target declaration")
                        })?;
                    let signature = &self.target_output.facts.external_specs.signatures[target];
                    let etas_types::SpecMethodIdentity::External { path, .. } = &witness.member
                    else {
                        return Err(invalid(
                            "frozen metadata object has no external member identity",
                        ));
                    };
                    let name = path
                        .last()
                        .ok_or_else(|| invalid("frozen object member identity is empty"))?;
                    let member = signature
                        .methods
                        .iter()
                        .find(|member| &member.name == name)
                        .ok_or_else(|| invalid("frozen object has no checked target member"))?;
                    witness.member = member.identity.clone();
                    witness.bound.spec = etas_types::CheckedSpecRef::External(target.clone());
                    for arg in &mut witness.bound.args {
                        *arg = self.ty(scope, *arg)?;
                    }
                }
                Ok(object)
            })
            .collect()
    }

    pub(in crate::code::definition::frozen_calls) fn graph_exits(
        &self,
        origins: &[crate::package_metadata::CallableGraphOrigin],
        scope: &GenericOwner,
    ) -> Result<Vec<Option<CallableExitSummary>>, ProjectionError> {
        use crate::package_metadata::CallableGraphOrigin as O;
        use etas_effects::facts::{
            CallableReferenceAnalysis as R, CapturedCallableAnalysis as C,
            MetadataCallableAnalysis as M,
        };
        origins
            .iter()
            .map(|origin| {
                let summary = match origin {
                    O::Native {
                        unit,
                        environment: None,
                    } => self.original.effects.unit_effects.get(unit),
                    O::Native {
                        unit,
                        environment: Some(environment),
                    } => match self.original.effects.captured_callables.get(
                        &etas_effects::facts::CapturedCallableId {
                            unit: *unit,
                            environment: *environment,
                        },
                    ) {
                        Some(C::Solved(summary)) => Some(summary.as_ref()),
                        _ => None,
                    },
                    O::Reference(reference) => {
                        match self.original.effects.callable_references.get(reference) {
                            Some(R::Solved { summary, .. }) => Some(summary.as_ref()),
                            _ => None,
                        }
                    }
                    O::Metadata(source) => {
                        match self.original.effects.metadata_callables.get(source) {
                            Some(M::Solved { summary, .. }) => Some(summary.as_ref()),
                            _ => None,
                        }
                    }
                }
                .ok_or_else(|| invalid("frozen callable graph has no checked source analysis"))?;
                let Some(exits) = &summary.exits else {
                    return Ok(None);
                };
                if !exits.gaps.is_empty() || !exits.parameter_exits.is_empty() {
                    return Ok(None);
                }
                let mut relocated = CallableExitSummary::default();
                for exit in &exits.exits {
                    let outcome = match &exit.outcome {
                        CallableOutcome::Returned { ty, observation } => {
                            CallableOutcome::Returned {
                                ty: self.ty(scope, *ty)?,
                                observation: match observation {
                                    ReturnObservation::Variant(symbol) => {
                                        ReturnObservation::Variant(self.variant(*symbol)?)
                                    }
                                    // Graph return edges carry callable identity; the control
                                    // proof must not retain source expression IDs.
                                    ReturnObservation::CallableSources(_) => ReturnObservation::Any,
                                    other => other.clone(),
                                },
                            }
                        }
                        CallableOutcome::Raised { error } => CallableOutcome::Raised {
                            error: self.ty(scope, *error)?,
                        },
                        CallableOutcome::NonReturn => CallableOutcome::NonReturn,
                        CallableOutcome::HandlerFinished { .. }
                        | CallableOutcome::HandlerResumed { .. } => {
                            return Err(invalid(
                                "handler-local control cannot escape into a frozen callable graph",
                            ));
                        }
                    };
                    relocated.exits.push(ExitTraceFact {
                        outcome,
                        trace: self.trace(&exit.trace, scope)?,
                    });
                }
                Ok(Some(relocated))
            })
            .collect()
    }

    pub(in crate::code::definition::frozen_calls) fn graph_values(
        &self,
        values: Vec<ExternalCallableValueMetadata>,
        scope: &GenericOwner,
    ) -> Result<Vec<ExternalCallableValueMetadata>, ProjectionError> {
        values
            .into_iter()
            .map(|mut value| {
                value.params = value
                    .params
                    .into_iter()
                    .map(|ty| self.ty(scope, ty))
                    .collect::<Result<_, _>>()?;
                value.output = self.ty(scope, value.output)?;
                if let Some(behavior) = &mut value.default_handler_effects.behavior {
                    self.graph_footprint(&mut behavior.direct, scope)?;
                    for (guards, footprint) in &mut behavior.guarded {
                        for guard in guards {
                            self.graph_effect(guard, scope)?;
                        }
                        self.graph_footprint(footprint, scope)?;
                    }
                }
                for row in [
                    &mut value.public_effects,
                    &mut value.requested_actions,
                    &mut value.handled_requested_actions,
                    &mut value.default_handler_effects.direct_effects,
                    &mut value.default_handler_effects.default_actions,
                ] {
                    self.graph_row(row, scope)?;
                }
                for handler in &mut value.default_handler_effects.handlers {
                    self.graph_effect(&mut handler.action, scope)?;
                    self.graph_row(&mut handler.produced, scope)?;
                }
                let mut pending = vec![&mut value.action_trace];
                if let Some(requests) = &mut value.handler_requests {
                    for action in &mut requests.actions {
                        self.graph_effect(&mut action.action, scope)?;
                        if let Some(signature) = &mut action.signature {
                            for ty in signature
                                .params
                                .iter_mut()
                                .chain(std::iter::once(&mut signature.output))
                            {
                                *ty = self.ty(scope, *ty)?;
                            }
                        }
                        for (_, ty) in action.type_bindings.iter_mut().flatten() {
                            *ty = self.ty(scope, *ty)?;
                        }
                    }
                    for parameter in &mut requests.parameters {
                        self.graph_target(&mut parameter.parameter, scope)?;
                        for effect in &mut parameter.excluded {
                            self.graph_effect(effect, scope)?;
                        }
                    }
                }
                while let Some(trace) = pending.pop() {
                    match trace {
                        ExternalActionTraceMetadata::Event { action, .. } => {
                            self.graph_effect(action, scope)?
                        }
                        ExternalActionTraceMetadata::DefaultHandler { guards, body } => {
                            for guard in guards {
                                self.graph_effect(guard, scope)?;
                            }
                            pending.push(body);
                        }
                        ExternalActionTraceMetadata::Seq(parts)
                        | ExternalActionTraceMetadata::Choice(parts) => pending.extend(parts),
                        ExternalActionTraceMetadata::Repeat(part) => pending.push(part),
                        ExternalActionTraceMetadata::UnknownOrder(actions) => {
                            for action in actions {
                                self.graph_effect(action, scope)?;
                            }
                        }
                        ExternalActionTraceMetadata::Widened {
                            actions,
                            parameter_calls,
                        } => {
                            for action in actions {
                                self.graph_effect(action, scope)?;
                            }
                            for target in parameter_calls {
                                self.graph_target(target, scope)?;
                            }
                        }
                        ExternalActionTraceMetadata::Empty => {}
                        ExternalActionTraceMetadata::ParameterCall { parameter } => {
                            self.graph_target(parameter, scope)?
                        }
                    }
                }
                Ok(value)
            })
            .collect()
    }

    fn graph_target(
        &self,
        target: &mut ExternalCallableRequestTarget,
        scope: &GenericOwner,
    ) -> Result<(), ProjectionError> {
        let ExternalCallableRequestTarget::SpecMethod(method) = target else {
            return Ok(());
        };
        let method = std::sync::Arc::make_mut(method);
        if !self
            .target_output
            .facts
            .external_specs
            .signatures
            .contains_key(&method.spec)
        {
            return Err(invalid(
                "frozen graph method has no checked target spec identity",
            ));
        }
        method.receiver = self.ty(scope, method.receiver)?;
        for arg in &mut method.spec_args {
            *arg = self.ty(scope, *arg)?;
        }
        for arg in &mut method.generic_arguments {
            match arg {
                ExternalSpecMethodGenericArgument::Type(ty) => *ty = self.ty(scope, *ty)?,
                ExternalSpecMethodGenericArgument::EffectRow(row) => {
                    *row = self.reference_row(row, scope)?
                }
            }
        }
        for argument in &mut method.callable_arguments {
            argument.values = self.graph_values(std::mem::take(&mut argument.values), scope)?;
            argument.objects = self.graph_objects(std::mem::take(&mut argument.objects), scope)?;
        }
        Ok(())
    }

    fn graph_footprint(
        &self,
        value: &mut etas_effects::ExternalEffectFootprintMetadata,
        scope: &GenericOwner,
    ) -> Result<(), ProjectionError> {
        for row in [
            &mut value.escaping_effects,
            &mut value.requested_actions,
            &mut value.default_actions,
            &mut value.handled_actions,
        ] {
            self.graph_row(row, scope)?;
        }
        if let Some(requests) = &mut value.handler_requests {
            for action in &mut requests.actions {
                self.graph_effect(&mut action.action, scope)?;
                for (_, ty) in action.type_bindings.iter_mut().flatten() {
                    *ty = self.ty(scope, *ty)?;
                }
                if let Some(signature) = &mut action.signature {
                    for ty in signature
                        .params
                        .iter_mut()
                        .chain(std::iter::once(&mut signature.output))
                    {
                        *ty = self.ty(scope, *ty)?;
                    }
                }
            }
            for parameter in &mut requests.parameters {
                self.graph_target(&mut parameter.parameter, scope)?;
                for effect in &mut parameter.excluded {
                    self.graph_effect(effect, scope)?;
                }
            }
        }
        for requirement in value
            .requirements
            .trace_requirements
            .iter_mut()
            .chain(&mut value.requirements.trace_obligations)
        {
            if let etas_effects::ExternalMethodTraceRequirement::Clause(clause) = requirement {
                for row in [
                    &mut clause.pattern,
                    &mut clause.guard,
                    &mut clause.target,
                    &mut clause.obligation,
                ]
                .into_iter()
                .flatten()
                {
                    for action in &mut row.effects {
                        let mut effect = ExternalEffectMetadata {
                            path: std::mem::take(&mut action.path),
                            args: std::mem::take(&mut action.args),
                        };
                        self.graph_effect(&mut effect, scope)?;
                        action.path = effect.path;
                        action.args = effect.args;
                    }
                }
            }
        }
        Ok(())
    }

    fn graph_row(
        &self,
        row: &mut ExternalEffectRowMetadata,
        scope: &GenericOwner,
    ) -> Result<(), ProjectionError> {
        for effect in &mut row.effects {
            self.graph_effect(effect, scope)?;
        }
        Ok(())
    }

    fn graph_effect(
        &self,
        effect: &mut ExternalEffectMetadata,
        scope: &GenericOwner,
    ) -> Result<(), ProjectionError> {
        let name = self
            .effect_names
            .get(&effect.path.join("."))
            .ok_or_else(|| {
                invalid("frozen callable graph effect has no canonical target identity")
            })?;
        effect.path = name.split('.').map(str::to_owned).collect();
        for arg in &mut effect.args {
            if let EffectArgRef::Type(ty) = arg {
                *ty = self.ty(scope, *ty)?;
            }
        }
        Ok(())
    }

    pub(in crate::code::definition::frozen_calls) fn graph_roots(
        &self,
        graph: &etas_package_metadata::CallableValueGraph,
        key: CodeFingerprint,
        owner: HirItemId,
    ) -> Result<std::collections::BTreeSet<CallableReturnSource>, ProjectionError> {
        graph
            .sources
            .iter()
            .map(|source| match source {
                etas_package_metadata::CallableValueSource::Existential(object) => {
                    graph.objects.get(*object as usize).ok_or_else(|| {
                        invalid("frozen object root has no checked evidence node")
                    })?;
                    Ok(CallableReturnSource::MetadataObject(
                        etas_effects::MetadataObjectSource {
                            graph: key,
                            object: *object,
                            environment: None,
                        },
                    ))
                }
                etas_package_metadata::CallableValueSource::Value(value)
                | etas_package_metadata::CallableValueSource::FrozenContract(value) => {
                    Ok(CallableReturnSource::Metadata(MetadataCallableSource {
                        graph: key,
                        value: *value,
                        environment: None,
                        analysis: if matches!(
                            source,
                            etas_package_metadata::CallableValueSource::FrozenContract(_)
                        ) {
                            etas_effects::CallableAnalysisKind::FrozenContract
                        } else {
                            etas_effects::CallableAnalysisKind::CheckedRevision
                        },
                    }))
                }
                etas_package_metadata::CallableValueSource::Parameter(parameter) => {
                    if parameter.scope != etas_package_metadata::CallableValueScope::Root {
                        return Err(invalid(
                            "frozen return graph has a non-root parameter source",
                        ));
                    }
                    let symbol = unit_parameters(&self.original.hir, EffectUnit::Item(owner))
                        .get(parameter.position as usize)
                        .ok_or_else(|| {
                            invalid("frozen graph references a missing root parameter")
                        })?;
                    Ok(CallableReturnSource::Parameter(
                        *self.parameters.get(symbol).ok_or_else(|| {
                            invalid("frozen graph root parameter has no candidate identity")
                        })?,
                    ))
                }
            })
            .collect()
    }
}
