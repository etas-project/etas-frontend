use etas_hir::{HirExpr, HirExprId, HirItemId, HirLiteral, ResolveResult, SymbolDef};
use etas_hir_analysis::{interprocedural::HirAnalysisBody, intraprocedural::Control};
use etas_types::{ItemSignature, PrimitiveType, Type, TypeId};

use super::{engine::EffectSemantics, state::EffectState};
use crate::{
    CallableExitSummary, CallableOutcome, Effect, EffectUnit, OutcomeAnalysisGap as Gap,
    ReturnObservation,
};

mod completion;

impl EffectSemantics<'_> {
    pub(crate) fn record_return(&self, value: Option<HirExprId>, state: &mut EffectState) {
        if let Some(value) = value
            && let Some(sources) = self.latent_sources_for_expr(state, value)
            && let Some(sources) = self.returned_callable_sources(state, &sources)
            && self.expr_type_contains_flow(value)
        {
            let Some(ty) = state.unit.and_then(|unit| self.unit_output_type(unit)) else {
                state.exits.gaps.insert(Gap::MissingReturnType);
                return;
            };
            state.exits.record(
                CallableOutcome::Returned {
                    ty,
                    observation: ReturnObservation::CallableSources(sources),
                },
                state.summary.action_trace.clone(),
            );
            return;
        }
        if let Some(value) = value
            && let Some(ty) = self.types.facts.expr_types.get(&value)
            && let Some(exits) = self.inputs.expr_effects.get(&value).and_then(|summary| summary.exits.as_ref())
            && exits.returns_only()
            && exits.exits.iter().all(|exit| matches!(exit.outcome, CallableOutcome::Returned { ty: returned, .. } if returned == *ty))
        {
            state.exits.join_assign(exits);
            return;
        }
        let outcome = match value {
            Some(value) => self.return_observation(value),
            None => state
                .unit
                .and_then(|unit| self.unit_output_type(unit))
                .and_then(|ty| {
                    matches!(
                        self.types.store.get(ty),
                        Some(Type::Primitive(PrimitiveType::Unit))
                    )
                    .then_some(CallableOutcome::Returned {
                        ty,
                        observation: ReturnObservation::Unit,
                    })
                }),
        };
        match outcome {
            Some(outcome) => {
                state
                    .exits
                    .record(outcome, state.summary.action_trace.clone());
            }
            None => {
                state.exits.gaps.insert(Gap::MissingReturnObservation);
            }
        }
    }

    pub(crate) fn returned_callable_sources(
        &self,
        _state: &EffectState,
        sources: &[super::state::LatentSource],
    ) -> Option<std::collections::BTreeSet<crate::CallableReturnSource>> {
        let mut returned = std::collections::BTreeSet::new();
        for source in sources {
            let source = match source {
                super::state::LatentSource::Captured { unit, environment } => {
                    crate::CallableReturnSource::Captured {
                        unit: *unit,
                        environment: *environment,
                    }
                }
                super::state::LatentSource::Metadata(source) => {
                    crate::CallableReturnSource::Metadata(*source)
                }
                super::state::LatentSource::Parameter { owner, symbol } => {
                    if !matches!(self.hir.symbols.get(*symbol)?.def, SymbolDef::Param { owner: actual, .. } if actual == *owner)
                    {
                        return None;
                    }
                    crate::CallableReturnSource::Parameter(*symbol)
                }
                super::state::LatentSource::StdReference {
                    value: expr,
                    environment,
                } => {
                    let mut reference = self.callable_reference_key(
                        crate::CallableReferenceTarget::StdIntrinsic(
                            self.inputs.std_callable_values.get(expr)?.intrinsic,
                        ),
                        *expr,
                    )?;
                    reference.environment = *environment;
                    crate::CallableReturnSource::Reference(reference)
                }
                super::state::LatentSource::Reference {
                    unit: EffectUnit::Item(item),
                    value,
                    environment,
                } => {
                    let mut reference = self.callable_reference_key(
                        crate::CallableReferenceTarget::Source(*item),
                        *value,
                    )?;
                    reference.environment = *environment;
                    crate::CallableReturnSource::Reference(reference)
                }
                super::state::LatentSource::Unit(EffectUnit::Item(item)) => {
                    crate::CallableReturnSource::SourceItem(*item)
                }
                super::state::LatentSource::Unit(
                    unit @ EffectUnit::AnonymousFlow { owner, .. },
                ) if self.top_level_anonymous_flow_unit(*owner) == Some(*unit) => {
                    crate::CallableReturnSource::SourceItem(*owner)
                }
                super::state::LatentSource::Unit(unit @ EffectUnit::AnonymousFlow { .. }) => {
                    crate::CallableReturnSource::AnonymousFlow(*unit)
                }
                super::state::LatentSource::Unit(_)
                | super::state::LatentSource::Reference { .. }
                | super::state::LatentSource::EnumConstructor(_) => return None,
                super::state::LatentSource::Unavailable { .. } => return None,
            };
            returned.insert(source);
        }
        (!returned.is_empty()).then_some(returned)
    }

    pub(crate) fn expr_is_never(&self, expr: HirExprId) -> bool {
        self.types
            .facts
            .expr_types
            .get(&expr)
            .and_then(|ty| self.types.store.get(*ty))
            .is_some_and(|ty| matches!(ty, Type::Primitive(PrimitiveType::Never)))
    }

    pub(crate) fn record_terminal_expr(&self, expr: HirExprId, state: &mut EffectState) {
        let outcome = match self.inputs.performed_actions.get(&expr) {
            Some(fact) => {
                let mut errors = fact.summary.escaping_effects.effects.iter().filter_map(
                    |effect| match effect {
                        Effect::Error(error) => Some(*error),
                        _ => None,
                    },
                );
                match (errors.next(), errors.next()) {
                    (Some(error), None) => CallableOutcome::Raised { error },
                    (None, None) => CallableOutcome::NonReturn,
                    _ => {
                        state.exits.gaps.insert(Gap::InvalidPerformedAction(expr));
                        return;
                    }
                }
            }
            None if matches!(
                self.hir.exprs.get(expr),
                Some(HirExpr::Perform { .. } | HirExpr::Infer { .. })
            ) =>
            {
                state.exits.gaps.insert(Gap::MissingPerformedAction(expr));
                return;
            }
            // A checked never type proves the absence of normal completion;
            // it does not invent a particular error or abort classification.
            None => CallableOutcome::NonReturn,
        };
        state
            .exits
            .record(outcome, state.summary.action_trace.clone());
    }

    fn return_observation(&self, expr: HirExprId) -> Option<CallableOutcome> {
        let ty = *self.types.facts.expr_types.get(&expr)?;
        if matches!(
            self.types.store.get(ty),
            Some(Type::Primitive(PrimitiveType::Never))
        ) {
            return None;
        }
        let observation = if matches!(
            self.types.store.get(ty),
            Some(Type::Primitive(PrimitiveType::Unit))
        ) {
            ReturnObservation::Unit
        } else {
            match self.hir.exprs.get(expr)? {
                HirExpr::Literal(HirLiteral::Bool { value, .. }) => ReturnObservation::Bool(*value),
                HirExpr::Path(path) => self.path_value_observation(expr, path)?,
                HirExpr::Call { callee, .. } => match self.hir.exprs.get(*callee)? {
                    HirExpr::Path(path) => self.path_observation(path)?,
                    _ => ReturnObservation::Any,
                },
                HirExpr::Record(record) => match &record.path {
                    Some(path) => self.path_observation(path)?,
                    None => ReturnObservation::Any,
                },
                _ => ReturnObservation::Any,
            }
        };
        Some(CallableOutcome::Returned { ty, observation })
    }

    fn path_value_observation(
        &self,
        expr: HirExprId,
        path: &etas_hir::ResolvedPath,
    ) -> Option<ReturnObservation> {
        if let ResolveResult::PartiallyResolved(partial) = &path.resolution
            && partial.reason == etas_hir::PartialResolutionReason::MemberRequiresTypeChecking
        {
            let projections = self.types.facts.field_projections.get(&expr)?;
            let last = projections.last()?;
            return (partial.resolved_prefix.is_some()
                && projections.len() == partial.remaining.len()
                && projections
                    .iter()
                    .zip(&partial.remaining)
                    .all(|(fact, field)| fact.field == *field)
                && self.types.facts.expr_types.get(&expr) == Some(&last.output))
            .then_some(ReturnObservation::Any);
        }
        self.path_observation(path)
    }

    fn path_observation(&self, path: &etas_hir::ResolvedPath) -> Option<ReturnObservation> {
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        let symbol = self.type_symbols.canonical_symbol(self.hir, symbol)?;
        Some(
            if matches!(
                self.hir.symbols.get(symbol)?.def,
                SymbolDef::EnumVariant { .. }
            ) {
                ReturnObservation::Variant(symbol)
            } else {
                ReturnObservation::Any
            },
        )
    }

    pub(crate) fn capture_exits(
        &self,
        unit: EffectUnit,
        control: &Control<EffectState>,
    ) -> CallableExitSummary {
        let mut result = CallableExitSummary::default();
        if let Some(state) = control.return_state() {
            result.join_assign(&state.exits);
            if state.exits.exits.is_empty() {
                result.gaps.insert(Gap::MissingReturnObservation);
            }
            self.record_escaping_error_gap(state, &mut result);
        }
        if let Some(state) = control.normal_state() {
            result.gaps.extend(state.exits.gaps.iter().cloned());
            let value = match unit.analysis_body_with_context(&self.context) {
                HirAnalysisBody::Expr(expr) => self.return_observation(expr),
                HirAnalysisBody::Block(block) => match self
                    .hir
                    .blocks
                    .get(block)
                    .and_then(|block| block.final_expr)
                {
                    Some(expr) => self.return_observation(expr),
                    None => self.unit_output_type(unit).and_then(|ty| {
                        matches!(
                            self.types.store.get(ty),
                            Some(Type::Primitive(PrimitiveType::Unit))
                        )
                        .then_some(CallableOutcome::Returned {
                            ty,
                            observation: ReturnObservation::Unit,
                        })
                    }),
                },
                _ => None,
            };
            if let Some(outcome) = value {
                result.record(outcome, state.summary.action_trace.clone());
            } else {
                result.gaps.insert(Gap::MissingReturnType);
            }
            self.record_escaping_error_gap(state, &mut result);
        }
        if let Some(state) = control.error_state() {
            result.join_assign(&state.exits);
            if state.exits.exits.is_empty() {
                result.gaps.insert(Gap::MissingReturnObservation);
            }
            if state.incomplete {
                result.gaps.insert(Gap::InvalidControlExit);
            }
        }
        for state in [
            control.break_state(),
            control.continue_state(),
            control.resume_state(),
            control.finish_state(),
        ]
        .into_iter()
        .flatten()
        {
            result.gaps.extend(state.exits.gaps.iter().cloned());
            result.gaps.insert(Gap::InvalidControlExit);
        }
        if result.exits.is_empty() && result.gaps.is_empty() {
            result.gaps.insert(Gap::InvalidControlExit);
        }
        result
    }

    fn record_escaping_error_gap(&self, state: &EffectState, result: &mut CallableExitSummary) {
        if state
            .summary
            .escaping_effects
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::Error(_)))
        {
            result.gaps.insert(Gap::EscapingError);
        }
        if state.incomplete {
            result.gaps.insert(Gap::InvalidControlExit);
        }
    }

    pub(crate) fn unit_output_type(&self, unit: EffectUnit) -> Option<TypeId> {
        match unit {
            EffectUnit::Item(item) => self.item_output_type(item),
            EffectUnit::AnonymousFlow { value, .. } => {
                let ty = self.types.facts.expr_types.get(&value)?;
                let Type::Function(function) = self.types.store.get(*ty)? else {
                    return None;
                };
                Some(function.output)
            }
            EffectUnit::HandlerArm { .. } => None,
        }
    }

    fn item_output_type(&self, item: HirItemId) -> Option<TypeId> {
        match self.types.facts.item_signatures.get(&item)? {
            ItemSignature::Flow(signature) | ItemSignature::Tool(signature) => {
                Some(signature.output)
            }
            ItemSignature::Agent(signature) => Some(signature.callable()?.output),
            ItemSignature::TopLevelLet(_) => None,
        }
    }

    pub(crate) fn note_call_outcomes(
        &mut self,
        call: HirExprId,
        callee: EffectUnit,
        summary: &crate::EffectSummary,
        state: &mut EffectState,
    ) {
        if let Some(unit) = state.unit
            && let Some(exits) = &summary.exits
            && (exits.has_only_terminal_exits()
                || (!self.expr_is_never(call) && exits.returns_only()))
        {
            let mut completion = CallableExitSummary::default();
            for exit in &exits.exits {
                let mut trace = state.summary.action_trace.clone();
                trace.seq_assign(exit.trace.clone());
                completion.record(exit.outcome.clone(), trace);
            }
            self.call_completion_exits.insert((unit, call), completion);
            return;
        }
        if self.expr_is_never(call)
            || !summary
                .exits
                .as_ref()
                .is_some_and(CallableExitSummary::returns_only)
        {
            state.exits.gaps.insert(Gap::Callee(callee));
        }
    }

    pub(crate) fn note_operation_outcomes(&self, expr: HirExprId, state: &mut EffectState) {
        if self.checked_total_operation(expr) {
            return;
        }
        match self.hir.exprs.get(expr) {
            Some(
                HirExpr::Binary { .. }
                | HirExpr::Unary { .. }
                | HirExpr::Index { .. }
                | HirExpr::Slice { .. },
            ) => {
                state.exits.gaps.insert(Gap::Operation(expr));
            }
            Some(HirExpr::Infer { .. } | HirExpr::Perform { .. }) => {
                if !self.expr_is_never(expr) {
                    self.note_external_outcomes(expr, state);
                }
            }
            Some(HirExpr::MethodCall { .. })
                if !self.types.facts.agent_method_calls.contains_key(&expr) =>
            {
                self.note_external_outcomes(expr, state)
            }
            _ => {}
        }
    }
}
