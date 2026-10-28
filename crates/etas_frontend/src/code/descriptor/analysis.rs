use etas_code::{CallableId, descriptor::*};
use etas_effects::{ActionTraceDomain, Determinism, Effect, EffectRow};

use super::{DefinitionDescriptorProjector, ProjectedSignature, ProjectionError};

impl DefinitionDescriptorProjector<'_> {
    pub fn project_analysis(
        &mut self,
        callable: &CallableId,
        item: etas_hir::HirItemId,
        signature: ProjectedSignature,
    ) -> Result<CallableAnalysisInfo, ProjectionError> {
        let Some(Some(TypeInfo::Function(header))) = self.types.get(signature.ty.0 as usize) else {
            return Err(ProjectionError::Invalid(
                "analysis requires a projected callable signature".into(),
            ));
        };
        let declared_bound = header.effects;
        let parameters = match self.checked.items.get(item) {
            Some(etas_hir::HirItem::Flow(flow)) => &flow.params,
            Some(etas_hir::HirItem::Tool(tool)) => &tool.params,
            Some(etas_hir::HirItem::Agent(agent)) => &agent.params,
            _ => {
                return Err(ProjectionError::Invalid(
                    "analysis requires a checked callable item".into(),
                ));
            }
        }
        .clone();
        if parameters.len() != header.parameters.len() {
            return Err(ProjectionError::Invalid(
                "analysis parameter identities differ from signature arity".into(),
            ));
        }
        let owner = match (callable.as_declaration(), callable.as_method()) {
            (Some(declaration), None) => GenericOwner::Declaration(declaration.clone()),
            (None, Some(method)) => GenericOwner::Method(method.clone()),
            _ => return Err(ProjectionError::Invalid("invalid callable identity".into())),
        };
        let scope = self
            .scopes
            .iter()
            .position(|scope| scope.owner == owner)
            .ok_or_else(|| {
                ProjectionError::Invalid("callable analysis has no checked generic scope".into())
            })?;
        let Some(summary) = self
            .checked
            .effects
            .and_then(|effects| effects.item_effects.get(&item))
        else {
            return Ok(CallableAnalysisInfo {
                effects: EffectSummaryInfo {
                    declared_bound,
                    ..Default::default()
                },
                determinism: None,
                trace: None,
                outcomes: None,
            });
        };
        let escaping = self.analysis_row(&summary.escaping_effects, scope)?;
        let requested = self.analysis_row(&summary.requested_actions, scope)?;
        let default_handled = self.analysis_row(&summary.default_actions, scope)?;
        let trace = self.analysis_trace(
            &summary.action_trace,
            callable,
            signature.ty,
            &parameters,
            scope,
        )?;
        let outcomes = summary
            .exits
            .as_ref()
            .filter(|exits| exits.gaps.is_empty())
            .map(|exits| self.project_outcomes(exits, callable, signature.ty, &parameters, scope))
            .transpose()?;
        Ok(CallableAnalysisInfo {
            effects: EffectSummaryInfo {
                declared_bound,
                escaping: Some(escaping),
                requested: Some(requested),
                default_handled: Some(default_handled),
            },
            determinism: Some(match summary.determinism {
                Determinism::Deterministic => DeterminismInfo::Deterministic,
                Determinism::RuntimeMediated => DeterminismInfo::RuntimeMediated,
                Determinism::NonDeterministic => DeterminismInfo::NonDeterministic,
            }),
            trace: Some(trace),
            outcomes,
        })
    }

    pub(super) fn analysis_row(
        &mut self,
        row: &EffectRow,
        scope: usize,
    ) -> Result<EffectRowInfoRef, ProjectionError> {
        self.node()?;
        self.edge()?;
        let mut effects = Vec::new();
        let mut open = row.open;
        for effect in row.effects.iter() {
            if let Effect::Var(variable) = effect {
                if open.is_some_and(|open| open != *variable) {
                    return Err(ProjectionError::Invalid(
                        "one effect row contains distinct open variables".into(),
                    ));
                }
                open = Some(*variable);
            } else {
                effects.push(self.analysis_effect(effect, scope)?);
            }
        }
        let tail = open
            .map(|open| {
                let mut matching = self.scopes[scope]
                    .binders
                    .iter()
                    .filter(|(name, reference)| {
                        self.binders[reference.0 as usize].kind == GenericParameterKind::EffectRow
                            && etas_effects::effect_var_id_from_name(name) == open
                    });
                match (matching.next(), matching.next()) {
                    (Some((_, reference)), None) => Ok(*reference),
                    _ => Err(ProjectionError::Invalid(
                        "open summary row has no unique declared effect parameter".into(),
                    )),
                }
            })
            .transpose()?;
        let target = EffectRowInfoRef(self.rows.len() as u32);
        self.rows.push(Some(EffectRowInfo { effects, tail }));
        Ok(target)
    }

    pub(super) fn analysis_effect(
        &mut self,
        effect: &Effect,
        scope: usize,
    ) -> Result<EffectInfo, ProjectionError> {
        self.edge()?;
        let (tag, action, args) = match effect {
            Effect::Tag(tag) => (*tag, None, Vec::new()),
            Effect::Applied { tag, args } => (
                *tag,
                None,
                args.iter()
                    .map(|ty| etas_types::EffectArgRef::Type(*ty))
                    .collect(),
            ),
            Effect::Action(action) => (action.tag, Some(action), Vec::new()),
            Effect::AppliedAction(instance) => (
                instance.action.tag,
                Some(&instance.action),
                instance.args.clone(),
            ),
            Effect::Error(ty) => (
                self.checked
                    .effect_registry
                    .tag_by_core(etas_effects::CoreEffect::Error)
                    .ok_or_else(|| ProjectionError::MissingIdentity("core Error effect".into()))?,
                None,
                vec![etas_types::EffectArgRef::Type(*ty)],
            ),
            Effect::Var(_) => {
                return Err(ProjectionError::Invalid(
                    "row variable used as an action".into(),
                ));
            }
        };
        let owner = self.identities.effect_tag(tag)?.clone();
        let target = if let Some(action) = action {
            let signature = self
                .checked
                .effect_registry
                .action_signature(action)
                .ok_or_else(|| {
                    ProjectionError::Invalid("checked action has no registry descriptor".into())
                })?;
            self.text(&signature.name)?;
            EffectTargetInfo::Action {
                effect: owner,
                name: signature.name.clone(),
            }
        } else {
            EffectTargetInfo::Effect(owner)
        };
        let arguments = args
            .iter()
            .map(|argument| self.project_static_arg(argument, scope))
            .collect::<Result<_, _>>()?;
        Ok(EffectInfo { target, arguments })
    }

    pub(super) fn analysis_trace(
        &mut self,
        trace: &ActionTraceDomain,
        callable: &CallableId,
        signature: TypeInfoRef,
        parameters: &[etas_hir::SymbolId],
        scope: usize,
    ) -> Result<TraceInfoRef, ProjectionError> {
        let root = self.reserve_trace()?;
        let mut work = vec![(trace, root)];
        while let Some((trace, target)) = work.pop() {
            let node = match trace {
                ActionTraceDomain::Empty => TraceInfo::Empty,
                ActionTraceDomain::Event(event) => {
                    TraceInfo::Event(self.analysis_effect(&event.action, scope)?)
                }
                ActionTraceDomain::ParameterCall { target, .. } => {
                    self.edge()?;
                    TraceInfo::ParameterCall(parameter_call(
                        callable, signature, parameters, *target,
                    )?)
                }
                ActionTraceDomain::Seq(parts) | ActionTraceDomain::Choice(parts) => {
                    self.ensure_edges(parts.len())?;
                    self.ensure_nodes(parts.len())?;
                    let mut children = Vec::with_capacity(parts.len());
                    for part in parts {
                        let child = self.reserve_trace()?;
                        children.push(child);
                        work.push((part, child));
                    }
                    if matches!(trace, ActionTraceDomain::Seq(_)) {
                        TraceInfo::Seq(children)
                    } else {
                        TraceInfo::Choice(children)
                    }
                }
                ActionTraceDomain::Repeat(inner) => {
                    let child = self.reserve_trace()?;
                    work.push((inner, child));
                    TraceInfo::Repeat(child)
                }
                ActionTraceDomain::UnknownOrder(actions)
                | ActionTraceDomain::Widened { actions, .. } => {
                    let actions = self.analysis_row(&EffectRow::closed(actions.clone()), scope)?;
                    if let ActionTraceDomain::Widened {
                        parameter_calls, ..
                    } = trace
                    {
                        self.ensure_edges(parameter_calls.len())?;
                        let mut calls = Vec::with_capacity(parameter_calls.len());
                        for call in parameter_calls {
                            self.edge()?;
                            calls.push(parameter_call(
                                callable,
                                signature,
                                parameters,
                                call.target,
                            )?);
                        }
                        TraceInfo::Widened {
                            actions,
                            parameter_calls: calls,
                        }
                    } else {
                        TraceInfo::UnknownOrder(actions)
                    }
                }
            };
            self.traces[target.0 as usize] = Some(node);
        }
        Ok(root)
    }

    pub(super) fn reserve_trace(&mut self) -> Result<TraceInfoRef, ProjectionError> {
        self.node()?;
        self.edge()?;
        let target = TraceInfoRef(self.traces.len() as u32);
        self.traces.push(None);
        Ok(target)
    }
}

fn parameter_call(
    callable: &CallableId,
    signature: TypeInfoRef,
    parameters: &[etas_hir::SymbolId],
    target: etas_effects::ParameterCallTarget,
) -> Result<ParameterCallInfo, ProjectionError> {
    let mut positions = parameters
        .iter()
        .enumerate()
        .filter(|(_, parameter)| target == etas_effects::ParameterCallTarget::Native(**parameter));
    match (positions.next(), positions.next()) {
        (Some((position, _)), None) => Ok(ParameterCallInfo {
            callable: callable.clone(),
            signature,
            position: position as u32,
        }),
        _ => Err(ProjectionError::Invalid(
            "trace parameter call has no unique signature parameter".into(),
        )),
    }
}
