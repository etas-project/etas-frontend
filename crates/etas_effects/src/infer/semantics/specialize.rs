use super::engine::EffectSemantics;
use super::shared::*;
use crate::ParameterCallTarget;
use std::collections::BTreeMap;

mod completion;
mod handler_scopes;
mod parameter_calls;
mod summary_mapper;
mod trace;
pub(crate) use parameter_calls::{TraceArgumentBinding, TraceArgumentBindings};
pub(crate) use trace::{ParameterCallSubstitution, substitute_parameter_calls};

#[derive(Debug)]
pub(crate) enum EffectSpecializationError {
    Interrupted(etas_utils::PipelineInterruption),
    StaticString(etas_hir_analysis::static_string::StaticStringEvaluationError),
    MemoryProvenance(String),
    InvalidActionSignature(crate::ActionRef),
    InvalidDefaultHandlerAction,
    InvalidCallableReturnSources,
    InvalidCallableParameterInterface,
    InvalidCallableEnvironment { reason: String },
    Type(etas_types::TypeSubstitutionError),
    MissingTypeBinding { param: String },
    MissingEffectRowBinding { param: String },
    MissingLatentEffectBinding { param: String },
    IncompatibleLatentEffectBindings { param: String },
}

impl std::fmt::Display for EffectSpecializationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Interrupted(error) => f.write_str(&error.reason),
            Self::InvalidCallableEnvironment { reason } => write!(f, "invalid callable environment: {reason}"),
            Self::StaticString(error) => error.fmt(f),
            Self::MemoryProvenance(error) => f.write_str(error),
            Self::InvalidActionSignature(action) => write!(
                f,
                "action specialization requires a matching checked selector signature for {action:?}"
            ),
            Self::Type(error) => error.fmt(f),
            Self::InvalidDefaultHandlerAction => f.write_str(
                "default handler specialization must preserve one closed checked action",
            ),
            Self::InvalidCallableReturnSources => f.write_str(
                "returned callable sources require a matching checked callee parameter interface",
            ),
            Self::InvalidCallableParameterInterface => f.write_str(
                "callback effect specialization requires a matching checked callable parameter interface",
            ),
            Self::MissingTypeBinding { param } => {
                write!(f, "checked generic instantiation is missing `{param}`")
            }
            Self::MissingEffectRowBinding { param } => {
                write!(
                    f,
                    "checked generic instantiation is missing `effect {param}`"
                )
            }
            Self::MissingLatentEffectBinding { param } => write!(
                f,
                "checked latent flow summary is missing for parameter `{param}`"
            ),
            Self::IncompatibleLatentEffectBindings { param } => write!(
                f,
                "latent flow sources do not agree on the row bound to `effect {param}`"
            ),
        }
    }
}

impl std::error::Error for EffectSpecializationError {}

impl From<etas_hir_analysis::static_string::StaticStringEvaluationError>
    for EffectSpecializationError
{
    fn from(error: etas_hir_analysis::static_string::StaticStringEvaluationError) -> Self {
        Self::StaticString(error)
    }
}

impl From<etas_types::TypeSubstitutionError> for EffectSpecializationError {
    fn from(error: etas_types::TypeSubstitutionError) -> Self {
        Self::Type(error)
    }
}

#[derive(Default)]
pub(crate) struct CheckedGenericBindings {
    pub(crate) types: Vec<(String, TypeId)>,
    pub(crate) effect_rows: Vec<(String, etas_types::EffectRowRef)>,
    pub(crate) deferred_effect_rows: Vec<etas_types::DeferredEffectRowObligation>,
}

impl EffectSemantics<'_> {
    pub(crate) fn solve_deferred_effect_row_obligations(
        &self,
        state: &EffectState,
        obligations: &[etas_types::DeferredEffectRowObligation],
    ) -> Result<BTreeMap<String, EffectRow>, EffectSpecializationError> {
        let mut solved = BTreeMap::<String, EffectRow>::new();
        for obligation in obligations {
            let source_summary =
                self.latent_summary_for_expr(state, obligation.source, &obligation.param)?;
            if let Some(existing) = solved.get(&obligation.param)
                && existing != &source_summary.escaping_effects
            {
                return Err(
                    EffectSpecializationError::IncompatibleLatentEffectBindings {
                        param: obligation.param.clone(),
                    },
                );
            }
            solved.insert(obligation.param.clone(), source_summary.escaping_effects);
        }
        Ok(solved)
    }

    pub(crate) fn latent_summary_for_expr(
        &self,
        state: &EffectState,
        expr: HirExprId,
        param: &str,
    ) -> Result<EffectSummary, EffectSpecializationError> {
        let Some(sources) = self.latent_sources_for_expr(state, expr) else {
            return Err(EffectSpecializationError::MissingLatentEffectBinding {
                param: param.to_owned(),
            });
        };
        let span = self
            .hir
            .exprs
            .get(expr)
            .ok_or_else(|| EffectSpecializationError::MissingLatentEffectBinding {
                param: param.to_owned(),
            })?
            .span(&self.hir.blocks);
        self.latent_summary_for_origins(sources, span, param)
    }

    pub(crate) fn latent_summary_for_origins(
        &self,
        sources: impl IntoIterator<Item = LatentSource>,
        span: Span,
        param: &str,
    ) -> Result<EffectSummary, EffectSpecializationError> {
        let mut source_summary: Option<EffectSummary> = None;
        for source in sources {
            let summary = self.latent_source_summary(source, span)?;
            match &mut source_summary {
                Some(combined) => {
                    combined.join_branch(&summary);
                }
                None => source_summary = Some(summary),
            }
        }
        source_summary.ok_or_else(|| EffectSpecializationError::MissingLatentEffectBinding {
            param: param.to_owned(),
        })
    }

    pub(crate) fn specialize_summary_for_call(
        &self,
        callee: EffectUnit,
        site: &CallSite<EffectUnit>,
        state: &EffectState,
        summary: &EffectSummary,
        environment: Option<crate::facts::CallableEnvironmentId>,
    ) -> Result<EffectSummary, EffectSpecializationError> {
        let Some(params) = self.unit_params(callee) else {
            return Ok(summary.clone());
        };
        let Some(HirExpr::Call { args, .. } | HirExpr::MethodCall { args, .. }) =
            self.hir.exprs.get(site.call)
        else {
            return Ok(summary.clone());
        };
        let mut bindings = Vec::new();
        let mut trace_bindings = environment
            .map(|id| self.native_environment_bindings(id))
            .transpose()?
            .unwrap_or_default();
        if args.len() != params.len() {
            return Err(EffectSpecializationError::InvalidCallableParameterInterface);
        }
        for (index, param) in params.iter().enumerate() {
            let arg = args
                .get(index)
                .and_then(arg_expr)
                .ok_or(EffectSpecializationError::InvalidCallableParameterInterface)?;
            let symbol = self
                .hir
                .symbols
                .get(*param)
                .ok_or(EffectSpecializationError::InvalidCallableParameterInterface)?;
            bindings.push((symbol.name.clone(), arg));
            trace_bindings.insert(
                ParameterCallTarget::Native(*param),
                TraceArgumentBinding::Expression(arg),
            );
        }
        let (mut type_bindings, mut effect_row_bindings) =
            self.checked_call_generic_values(site.call, state)?;
        if let Some(environment) = environment {
            let snapshot = self.callable_environment(environment)?;
            let owner = callee
                .owner()
                .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
            let (types, rows) = self.native_environment_generics(owner, &snapshot.generics)?;
            type_bindings.extend(types);
            effect_row_bindings.extend(rows);
        }
        self.require_effect_type_bindings(callee, summary, &type_bindings)?;
        self.require_effect_row_bindings(callee, summary, &effect_row_bindings)?;
        if bindings.is_empty()
            && trace_bindings.is_empty()
            && type_bindings.is_empty()
            && effect_row_bindings.is_empty()
        {
            return Ok(summary.clone());
        }

        let mut specialized = self.specialize_summary_with_bindings(
            summary,
            &bindings,
            &type_bindings,
            &effect_row_bindings,
        )?;
        let param_types = params
            .iter()
            .map(|param| {
                self.symbol_value_type(*param)
                    .ok_or(EffectSpecializationError::InvalidCallableParameterInterface)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let param_targets = params
            .iter()
            .map(|param| ParameterCallTarget::Native(*param))
            .collect::<Vec<_>>();
        let inferred_parameters =
            self.inferred_callback_parameters(&param_targets, &param_types)?;
        self.merge_parameter_call_summaries(
            &mut specialized,
            &summary.action_trace,
            &trace_bindings,
            state,
            &inferred_parameters,
        )?;
        specialized.action_trace = self.specialize_parameter_call_trace(
            &specialized.action_trace,
            &trace_bindings,
            state,
        )?;
        if let Some(exits) = &mut specialized.exits {
            for exit in &mut exits.exits {
                exit.trace =
                    self.specialize_parameter_call_trace(&exit.trace, &trace_bindings, state)?;
                if let crate::CallableOutcome::Returned { observation, .. }
                | crate::CallableOutcome::HandlerFinished { observation, .. }
                | crate::CallableOutcome::HandlerResumed { observation, .. } = &mut exit.outcome
                    && let crate::ReturnObservation::CallableSources(parameters) = observation
                {
                    let sources = self.returned_callable_bindings(
                        parameters,
                        &trace_bindings,
                        state,
                        site.call,
                        environment,
                    )?;
                    *observation = self.returned_callable_sources(state, &sources).map_or(
                        crate::ReturnObservation::Any,
                        crate::ReturnObservation::CallableSources,
                    );
                }
            }
        }
        self.solve_callback_completion(&mut specialized, &trace_bindings, &type_bindings, state)?;
        self.forward_callback_outcomes(&mut specialized, &trace_bindings, &params, state)?;
        Ok(specialized)
    }

    pub(crate) fn call_bindings_from_param_names(
        &self,
        site: &CallSite<EffectUnit>,
        param_names: &[String],
    ) -> Vec<(String, HirExprId)> {
        let Some(HirExpr::Call { args, .. } | HirExpr::MethodCall { args, .. }) =
            self.hir.exprs.get(site.call)
        else {
            return Vec::new();
        };
        param_names
            .iter()
            .enumerate()
            .filter_map(|(index, param)| {
                args.get(index)
                    .and_then(arg_expr)
                    .map(|arg| (param.clone(), arg))
            })
            .collect()
    }

    pub(crate) fn call_generic_bindings(&self, call: HirExprId) -> CheckedGenericBindings {
        self.types
            .facts
            .generic_instantiations
            .get(&call)
            .map(|fact| CheckedGenericBindings {
                types: fact.type_bindings.clone(),
                effect_rows: fact.effect_row_bindings.clone(),
                deferred_effect_rows: fact.deferred_effect_row_obligations.clone(),
            })
            .unwrap_or_default()
    }

    pub(crate) fn unit_type_params(&self, unit: EffectUnit) -> Vec<etas_hir::SymbolId> {
        let EffectUnit::Item(item) = unit else {
            return Vec::new();
        };
        match self.hir.items.get(item) {
            Some(etas_hir::HirItem::Flow(flow)) => flow.type_params.clone(),
            Some(etas_hir::HirItem::Tool(tool)) => tool.type_params.clone(),
            Some(etas_hir::HirItem::Type(decl)) => decl.type_params.clone(),
            Some(etas_hir::HirItem::TypeAlias(decl)) => decl.type_params.clone(),
            Some(etas_hir::HirItem::Enum(decl)) => decl.type_params.clone(),
            _ => Vec::new(),
        }
    }

    pub(crate) fn specialize_summary_with_bindings(
        &self,
        summary: &EffectSummary,
        bindings: &[(String, HirExprId)],
        type_bindings: &[(String, TypeId)],
        effect_row_bindings: &[(String, EffectRow)],
    ) -> Result<EffectSummary, EffectSpecializationError> {
        if bindings.is_empty() && type_bindings.is_empty() && effect_row_bindings.is_empty() {
            return Ok(summary.clone());
        }
        let names = type_bindings.iter().cloned().collect();
        crate::substitution::map_summary(
            &mut summary_mapper::CallSummaryMapper {
                semantics: self,
                bindings,
                type_bindings,
                rows: effect_row_bindings,
                types: etas_types::TypeSubstitutionEngine::existing(&self.types.store, &names),
            },
            summary,
        )
    }

    pub(crate) fn unit_params(&self, unit: EffectUnit) -> Option<Vec<etas_hir::SymbolId>> {
        if let EffectUnit::AnonymousFlow { value, .. } = unit {
            return match self.hir.exprs.get(value)? {
                HirExpr::Lambda { params, .. } => Some(params.clone()),
                _ => None,
            };
        }
        let EffectUnit::Item(item) = unit else {
            return None;
        };
        match self.hir.items.get(item)? {
            etas_hir::HirItem::Flow(flow) => Some(flow.params.clone()),
            etas_hir::HirItem::Agent(agent) => Some(agent.params.clone()),
            etas_hir::HirItem::Tool(tool) => Some(tool.params.clone()),
            _ => None,
        }
    }

    pub(crate) fn specialize_effect_row(
        &self,
        row: &EffectRow,
        bindings: &[(String, HirExprId)],
        type_bindings: &[(String, TypeId)],
        effect_row_bindings: &[(String, EffectRow)],
    ) -> Result<EffectRow, EffectSpecializationError> {
        let effects = row
            .effects
            .iter()
            .cloned()
            .map(|effect| self.specialize_effect(effect, bindings, type_bindings))
            .collect::<Result<Vec<_>, _>>()?;
        let mut specialized = EffectRow {
            effects: EffectSet::from_iter(effects),
            open: row.open,
        };
        if let Some(open) = row.open {
            for (name, binding) in effect_row_bindings {
                if effect_var_id_from_name(name) != open {
                    continue;
                }
                specialized.effects.union_assign(&binding.effects);
                specialized.open = binding.open;
                break;
            }
        }
        Ok(specialized)
    }

    pub(crate) fn specialize_effect(
        &self,
        effect: Effect,
        bindings: &[(String, HirExprId)],
        type_bindings: &[(String, TypeId)],
    ) -> Result<Effect, EffectSpecializationError> {
        match effect {
            Effect::AppliedAction(mut action) => {
                let signature = self
                    .registry
                    .action_signature(&action.action)
                    .filter(|signature| signature.effect_args.len() == action.args.len())
                    .ok_or_else(|| {
                        EffectSpecializationError::InvalidActionSignature(action.action.clone())
                    })?;
                action.args = action
                    .args
                    .into_iter()
                    .zip(&signature.effect_args)
                    .map(|(arg, kind)| {
                        self.specialize_effect_arg(arg, kind, bindings, type_bindings)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Effect::AppliedAction(action))
            }
            Effect::Applied { tag, args } => {
                let bindings = type_bindings.iter().cloned().collect();
                let args = args
                    .into_iter()
                    .map(|ty| {
                        etas_types::substitute_named_params_in_store(
                            &self.types.store,
                            ty,
                            &bindings,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Effect::Applied { tag, args })
            }
            Effect::Error(ty) => {
                let bindings = type_bindings.iter().cloned().collect();
                Ok(Effect::Error(etas_types::substitute_named_params_in_store(
                    &self.types.store,
                    ty,
                    &bindings,
                )?))
            }
            other => Ok(other),
        }
    }

    pub(crate) fn specialize_effect_arg(
        &self,
        arg: EffectArgRef,
        kind: &crate::EffectActionArgKind,
        bindings: &[(String, HirExprId)],
        type_bindings: &[(String, TypeId)],
    ) -> Result<EffectArgRef, EffectSpecializationError> {
        if let EffectArgRef::Type(ty) = arg {
            let bindings = type_bindings.iter().cloned().collect();
            return etas_types::substitute_named_params_in_store(&self.types.store, ty, &bindings)
                .map(EffectArgRef::Type)
                .map_err(Into::into);
        }
        let EffectArgRef::Path(path) = &arg else {
            return Ok(arg);
        };
        let Some((head, tail)) = path.split_first() else {
            return Ok(arg);
        };
        let Some((_, expr)) = bindings.iter().find(|(param, _)| param == head) else {
            return Ok(arg);
        };
        if matches!(kind, crate::EffectActionArgKind::MemoryPlace) {
            let arguments = self
                .memory_provenance
                .arguments(self.hir, self.types, *expr)
                .map_err(EffectSpecializationError::MemoryProvenance)?;
            let [EffectArgRef::Path(origin)] = arguments.as_slice() else {
                return Err(EffectSpecializationError::MemoryProvenance(
                    "call argument has multiple resource origins".into(),
                ));
            };
            let mut origin = origin.clone();
            origin.extend_from_slice(tail);
            return Ok(EffectArgRef::Path(origin));
        }
        Ok(self.effect_arg_from_expr_path(*expr, tail)?)
    }

    pub(crate) fn effect_arg_from_expr_path(
        &self,
        expr: HirExprId,
        path: &[String],
    ) -> Result<EffectArgRef, etas_hir_analysis::static_string::StaticStringEvaluationError> {
        evaluate_static_string(self, expr, path).map(EffectArgRef::String)
    }

    pub(crate) fn require_effect_type_bindings(
        &self,
        callee: EffectUnit,
        summary: &EffectSummary,
        type_bindings: &[(String, TypeId)],
    ) -> Result<(), EffectSpecializationError> {
        let params = self
            .unit_type_params(callee)
            .into_iter()
            .filter_map(|symbol| self.hir.symbols.get(symbol))
            .filter(|symbol| matches!(symbol.def, SymbolDef::TypeParam { .. }))
            .map(|symbol| symbol.name.clone())
            .collect::<Vec<_>>();
        self.require_named_effect_type_bindings(&params, summary, type_bindings)
    }

    pub(crate) fn require_effect_row_bindings(
        &self,
        callee: EffectUnit,
        summary: &EffectSummary,
        effect_row_bindings: &[(String, EffectRow)],
    ) -> Result<(), EffectSpecializationError> {
        let params = self
            .unit_type_params(callee)
            .into_iter()
            .filter_map(|symbol| self.hir.symbols.get(symbol))
            .filter(|symbol| matches!(symbol.def, SymbolDef::EffectParam { .. }))
            .map(|symbol| symbol.name.clone())
            .collect::<Vec<_>>();
        self.require_named_effect_row_bindings(&params, summary, effect_row_bindings)
    }

    pub(crate) fn require_named_effect_row_bindings(
        &self,
        params: &[String],
        summary: &EffectSummary,
        effect_row_bindings: &[(String, EffectRow)],
    ) -> Result<(), EffectSpecializationError> {
        let bound = effect_row_bindings
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<std::collections::HashSet<_>>();
        for param in params {
            if bound.contains(param.as_str()) || !summary_contains_effect_row(summary, param) {
                continue;
            }
            return Err(EffectSpecializationError::MissingEffectRowBinding {
                param: param.clone(),
            });
        }
        Ok(())
    }

    pub(crate) fn require_named_effect_type_bindings(
        &self,
        params: &[String],
        summary: &EffectSummary,
        type_bindings: &[(String, TypeId)],
    ) -> Result<(), EffectSpecializationError> {
        let bound = type_bindings
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<std::collections::HashSet<_>>();
        for param in params {
            if bound.contains(param.as_str())
                || !self.summary_contains_named_type(summary, param)?
            {
                continue;
            }
            return Err(EffectSpecializationError::MissingTypeBinding {
                param: param.clone(),
            });
        }
        Ok(())
    }

    fn summary_contains_named_type(
        &self,
        summary: &EffectSummary,
        name: &str,
    ) -> Result<bool, etas_types::TypeSubstitutionError> {
        for row in [
            &summary.escaping_effects,
            &summary.requested_actions,
            &summary.default_actions,
            &summary.handled_actions,
        ] {
            for effect in row.effects.iter() {
                if self.effect_contains_named_type(effect, name)? {
                    return Ok(true);
                }
            }
        }
        self.action_trace_contains_named_type(&summary.action_trace, name)
    }

    fn effect_contains_named_type(
        &self,
        effect: &Effect,
        name: &str,
    ) -> Result<bool, etas_types::TypeSubstitutionError> {
        match effect {
            Effect::AppliedAction(action) => {
                for arg in &action.args {
                    if let EffectArgRef::Type(ty) = arg
                        && etas_types::type_contains_named_param(&self.types.store, *ty, name)?
                    {
                        return Ok(true);
                    }
                }
            }
            Effect::Applied { args, .. } => {
                for ty in args {
                    if etas_types::type_contains_named_param(&self.types.store, *ty, name)? {
                        return Ok(true);
                    }
                }
            }
            Effect::Error(ty) => {
                if etas_types::type_contains_named_param(&self.types.store, *ty, name)? {
                    return Ok(true);
                }
            }
            _ => {}
        }
        Ok(false)
    }

    fn action_trace_contains_named_type(
        &self,
        trace: &ActionTraceDomain,
        name: &str,
    ) -> Result<bool, etas_types::TypeSubstitutionError> {
        match trace {
            ActionTraceDomain::Empty => Ok(false),
            ActionTraceDomain::Event(event) => self.effect_contains_named_type(&event.action, name),
            ActionTraceDomain::ParameterCall { .. } => Ok(false),
            ActionTraceDomain::Seq(items) | ActionTraceDomain::Choice(items) => {
                for item in items {
                    if self.action_trace_contains_named_type(item, name)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            ActionTraceDomain::Repeat(item) => self.action_trace_contains_named_type(item, name),
            ActionTraceDomain::UnknownOrder(actions) => {
                for effect in actions.iter() {
                    if self.effect_contains_named_type(effect, name)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            ActionTraceDomain::Widened { actions, .. } => {
                for effect in actions.iter() {
                    if self.effect_contains_named_type(effect, name)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }
}

fn summary_contains_effect_row(summary: &EffectSummary, name: &str) -> bool {
    let open = effect_var_id_from_name(name);
    [
        &summary.escaping_effects,
        &summary.requested_actions,
        &summary.default_actions,
        &summary.handled_actions,
    ]
    .into_iter()
    .any(|row| row.open == Some(open))
}
