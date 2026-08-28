use super::engine::EffectSemantics;
use super::shared::*;

#[derive(Debug)]
pub(crate) enum EffectSpecializationError {
    StaticString(etas_hir_analysis::static_string::StaticStringEvaluationError),
    Type(etas_types::TypeSubstitutionError),
    MissingTypeBinding { param: String },
    MissingEffectRowBinding { param: String },
    MissingLatentEffectBinding { param: String },
}

impl std::fmt::Display for EffectSpecializationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaticString(error) => error.fmt(f),
            Self::Type(error) => error.fmt(f),
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
                "checked latent flow summary is missing for `effect {param}`"
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
    pub(crate) deferred_effect_rows: Vec<(String, HirExprId)>,
}

impl EffectSemantics<'_> {
    pub(crate) fn specialize_summary_for_call(
        &self,
        callee: EffectUnit,
        site: &CallSite<EffectUnit>,
        state: &EffectState,
        summary: &EffectSummary,
    ) -> Result<EffectSummary, EffectSpecializationError> {
        let Some(params) = self.unit_params(callee) else {
            return Ok(summary.clone());
        };
        let Some(HirExpr::Call { args, .. }) = self.hir.exprs.get(site.call) else {
            return Ok(summary.clone());
        };
        let mut bindings = Vec::new();
        for (index, param) in params.iter().enumerate() {
            let Some(arg) = args.get(index).and_then(arg_expr) else {
                continue;
            };
            let Some(symbol) = self.hir.symbols.get(*param) else {
                continue;
            };
            bindings.push((symbol.name.clone(), arg));
        }
        let generic_bindings = self.call_generic_bindings(site.call);
        let mut effect_row_bindings = generic_bindings.effect_rows.clone();
        effect_row_bindings.extend(generic_bindings.deferred_effect_rows.iter().map(
            |(name, _)| {
                (
                    name.clone(),
                    etas_types::EffectRowRef {
                        effects: Vec::new(),
                        tail: None,
                    },
                )
            },
        ));
        self.require_effect_type_bindings(callee, summary, &generic_bindings.types)?;
        self.require_effect_row_bindings(callee, summary, &effect_row_bindings)?;
        if bindings.is_empty()
            && generic_bindings.types.is_empty()
            && effect_row_bindings.is_empty()
        {
            return Ok(summary.clone());
        }

        let mut specialized = self.specialize_summary_with_bindings(
            summary,
            &bindings,
            &generic_bindings.types,
            &effect_row_bindings,
        )?;
        for (param, expr) in &generic_bindings.deferred_effect_rows {
            let Some(sources) = self.latent_sources_for_expr(state, *expr) else {
                return Err(EffectSpecializationError::MissingLatentEffectBinding {
                    param: param.clone(),
                });
            };
            for source in sources {
                let Some(source_summary) = self.inputs.unit_effects.get(&source) else {
                    return Err(EffectSpecializationError::MissingLatentEffectBinding {
                        param: param.clone(),
                    });
                };
                specialized.seq_assign(source_summary);
            }
        }
        Ok(specialized)
    }

    pub(crate) fn call_bindings_from_param_names(
        &self,
        site: &CallSite<EffectUnit>,
        param_names: &[String],
    ) -> Vec<(String, HirExprId)> {
        let Some(HirExpr::Call { args, .. }) = self.hir.exprs.get(site.call) else {
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
                deferred_effect_rows: fact.deferred_effect_row_bindings.clone(),
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
        effect_row_bindings: &[(String, etas_types::EffectRowRef)],
    ) -> Result<EffectSummary, EffectSpecializationError> {
        if bindings.is_empty() && type_bindings.is_empty() && effect_row_bindings.is_empty() {
            return Ok(summary.clone());
        }
        let mut specialized = summary.clone();
        specialized.escaping_effects = self.specialize_effect_row(
            &summary.escaping_effects,
            bindings,
            type_bindings,
            effect_row_bindings,
        )?;
        specialized.requested_actions = self.specialize_effect_row(
            &summary.requested_actions,
            bindings,
            type_bindings,
            effect_row_bindings,
        )?;
        specialized.default_actions = self.specialize_effect_row(
            &summary.default_actions,
            bindings,
            type_bindings,
            effect_row_bindings,
        )?;
        specialized.handled_actions = self.specialize_effect_row(
            &summary.handled_actions,
            bindings,
            type_bindings,
            effect_row_bindings,
        )?;
        specialized.action_trace =
            self.specialize_action_trace(&summary.action_trace, bindings, type_bindings)?;
        Ok(specialized)
    }

    pub(crate) fn unit_params(&self, unit: EffectUnit) -> Option<Vec<etas_hir::SymbolId>> {
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
        effect_row_bindings: &[(String, etas_types::EffectRowRef)],
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
                let binding = self.row_from_type_ref(binding);
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
                action.args = action
                    .args
                    .into_iter()
                    .map(|arg| self.specialize_effect_arg(arg, bindings, type_bindings))
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

    pub(crate) fn specialize_action_trace(
        &self,
        trace: &ActionTraceDomain,
        bindings: &[(String, HirExprId)],
        type_bindings: &[(String, TypeId)],
    ) -> Result<ActionTraceDomain, EffectSpecializationError> {
        match trace {
            ActionTraceDomain::Empty => Ok(ActionTraceDomain::Empty),
            ActionTraceDomain::Event(event) => {
                let mut event = event.clone();
                event.action = self.specialize_effect(event.action, bindings, type_bindings)?;
                Ok(ActionTraceDomain::Event(event))
            }
            ActionTraceDomain::Seq(items) => Ok(ActionTraceDomain::Seq(
                items
                    .iter()
                    .map(|item| self.specialize_action_trace(item, bindings, type_bindings))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            ActionTraceDomain::Choice(items) => Ok(ActionTraceDomain::Choice(
                items
                    .iter()
                    .map(|item| self.specialize_action_trace(item, bindings, type_bindings))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            ActionTraceDomain::Repeat(item) => Ok(ActionTraceDomain::Repeat(Box::new(
                self.specialize_action_trace(item, bindings, type_bindings)?,
            ))),
            ActionTraceDomain::UnknownOrder(actions) => {
                Ok(ActionTraceDomain::UnknownOrder(EffectSet::from_iter(
                    actions
                        .iter()
                        .cloned()
                        .map(|effect| self.specialize_effect(effect, bindings, type_bindings))
                        .collect::<Result<Vec<_>, _>>()?,
                )))
            }
        }
    }

    pub(crate) fn specialize_effect_arg(
        &self,
        arg: EffectArgRef,
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
        Ok(self.effect_arg_from_expr_path(*expr, tail)?)
    }

    pub(crate) fn effect_arg_from_expr_path(
        &self,
        expr: HirExprId,
        path: &[String],
    ) -> Result<EffectArgRef, etas_hir_analysis::static_string::StaticStringEvaluationError> {
        evaluate_static_string(self, expr, path).map(EffectArgRef::String)
    }

    fn require_effect_type_bindings(
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

    fn require_effect_row_bindings(
        &self,
        callee: EffectUnit,
        summary: &EffectSummary,
        effect_row_bindings: &[(String, etas_types::EffectRowRef)],
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
        effect_row_bindings: &[(String, etas_types::EffectRowRef)],
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
