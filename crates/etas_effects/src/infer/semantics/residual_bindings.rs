use std::collections::{BTreeMap, BTreeSet};

use etas_code::{
    descriptor::{DescriptorLimits, GenericParameterKind},
    residual::identity::{GenericArgumentIdentity, GenericParameterId, ResidualOwnerId},
};
use etas_hir::{
    HirExpr, HirExprId, HirGenericArg, HirItem, HirItemId, ResolveResult, SymbolDef, SymbolId,
};
use etas_types::{CallableGenericParamKind, Type, TypeId};

use super::{
    callable_generics::GenericValues, engine::EffectSemantics,
    specialize::EffectSpecializationError,
};
use crate::facts::{CallableGenericParameter, CallableGenericValue};

impl EffectSemantics<'_> {
    /// Projects only bindings backed by checked native declaration positions.
    /// This produces substitutions, not proof of plan coverage or installation.
    pub(super) fn native_residual_bindings(
        &self,
        owner: HirItemId,
        values: &GenericValues,
        checks: &crate::ResidualCheckSet,
        call: HirExprId,
        captured: Option<crate::facts::CallableEnvironmentId>,
    ) -> Result<BTreeMap<GenericParameterId, GenericArgumentIdentity>, EffectSpecializationError>
    {
        let unsupported = || EffectSpecializationError::UnsupportedResidualSubstitution;
        let params = self.native_generic_params(owner).ok_or_else(unsupported)?;
        let symbols = self.native_generic_symbols(owner)?;
        if params.len() != symbols.len() || values.len() != params.len() {
            return Err(unsupported());
        }
        let source_owner = self.native_residual_owner(owner)?;
        let mut needed = BTreeSet::new();
        let mut remaining = etas_package_metadata::MAX_METADATA_GRAPH_NODES;
        for reference in checks.iter() {
            self.residual_checkpoint()?;
            remaining =
                remaining
                    .checked_sub(1)
                    .ok_or(EffectSpecializationError::ResidualSubstitution(
                        etas_code::residual::identity::ResidualSubstitutionError::ResourceLimit,
                    ))?;
            for value in reference.substitutions().values() {
                remaining = remaining.checked_sub(1).ok_or(
                    EffectSpecializationError::ResidualSubstitution(
                        etas_code::residual::identity::ResidualSubstitutionError::ResourceLimit,
                    ),
                )?;
                if let GenericArgumentIdentity::TypeParameter(id)
                | GenericArgumentIdentity::EffectRowParameter(id) = value
                {
                    needed.insert(*id);
                }
            }
        }
        let mut bindings = BTreeMap::new();
        for (position, (param, symbol)) in params.iter().zip(symbols).enumerate() {
            self.residual_checkpoint()?;
            let position = u32::try_from(position).map_err(|_| unsupported())?;
            let kind = match param.kind {
                CallableGenericParamKind::Type => GenericParameterKind::Type,
                CallableGenericParamKind::Effect => GenericParameterKind::EffectRow,
            };
            let data = self.hir.symbols.get(*symbol).ok_or_else(unsupported)?;
            let valid = match (&data.def, param.kind) {
                (
                    SymbolDef::TypeParam {
                        owner: item,
                        param_index,
                        ..
                    },
                    CallableGenericParamKind::Type,
                )
                | (
                    SymbolDef::EffectParam {
                        owner: item,
                        param_index,
                    },
                    CallableGenericParamKind::Effect,
                ) => *item == owner && *param_index == position,
                _ => false,
            };
            if !valid
                || data.name != param.name
                || !matches!(self.types.store.get(param.subject), Some(Type::Named(named)) if named.source_parameter == Some(*symbol) && named.name == data.name)
            {
                return Err(unsupported());
            }
            let id = GenericParameterId::from_source_owner(source_owner, position, kind);
            if !needed.contains(&id) {
                continue;
            }
            let value = values
                .get(&CallableGenericParameter::Native { owner, position })
                .ok_or_else(unsupported)?;
            let argument = match (param.kind, value) {
                (CallableGenericParamKind::Type, CallableGenericValue::Type(ty)) => {
                    self.native_residual_type(owner, *ty)?
                }
                (
                    CallableGenericParamKind::Effect,
                    CallableGenericValue::EffectRowFromCall { row, source },
                ) if row.open.is_some() && row.effects.is_empty() => {
                    if let Some(environment) = captured {
                        let snapshot = self.callable_environment(environment)?;
                        if snapshot
                            .generics
                            .get(&CallableGenericParameter::Native { owner, position })
                            != Some(value)
                            || self.hir.index().expr_item.get(source) != Some(&snapshot.owner)
                            || self.callable_environment_site(*source)? != environment.application
                            || !self.residual_call_targets_owner(*source, owner)
                        {
                            return Err(unsupported());
                        }
                    } else if *source != call {
                        return Err(unsupported());
                    }
                    self.native_residual_effect_parameter(
                        *source,
                        position,
                        params.len(),
                        &param.name,
                        row,
                    )?
                }
                (CallableGenericParamKind::Effect, CallableGenericValue::EffectRow(row))
                    if row.open.is_some() && row.effects.is_empty() && captured.is_none() =>
                {
                    self.native_residual_effect_parameter(
                        call,
                        position,
                        params.len(),
                        &param.name,
                        row,
                    )?
                }
                (CallableGenericParamKind::Effect, CallableGenericValue::EffectRow(row))
                    if row.open.is_none() && self.residual_identity.is_some() =>
                {
                    let provider = self.residual_identity.ok_or_else(unsupported)?;
                    GenericArgumentIdentity::EffectRow(
                        provider
                            .closed_row(self.registry, owner, row)
                            .map_err(map_identity_error)?,
                    )
                }
                (CallableGenericParamKind::Effect, CallableGenericValue::EffectRow(row))
                    if row.effects.is_empty() && row.open.is_none() =>
                {
                    let graph = etas_code::descriptor::DescriptorGraph::new(
                        etas_code::descriptor::DescriptorGraphInput {
                            rows: vec![etas_code::descriptor::EffectRowInfo {
                                effects: vec![],
                                tail: None,
                            }],
                            ..Default::default()
                        },
                        DescriptorLimits::default(),
                    )
                    .map_err(|_| unsupported())?;
                    GenericArgumentIdentity::EffectRow(
                        graph
                            .row_fingerprint(etas_code::descriptor::EffectRowInfoRef(0))
                            .map_err(|_| unsupported())?,
                    )
                }
                _ => return Err(unsupported()),
            };
            bindings.insert(id, argument);
        }
        Ok(bindings)
    }

    fn residual_call_targets_owner(&self, call: HirExprId, owner: HirItemId) -> bool {
        let Some(HirExpr::Call { callee, .. }) = self.hir.exprs.get(call) else {
            return false;
        };
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(*callee) else {
            return false;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        let Some(symbol) = self.type_symbols.canonical_symbol(self.hir, symbol) else {
            return false;
        };
        matches!(self.hir.symbols.get(symbol).map(|symbol| &symbol.def), Some(SymbolDef::Item { item }) if *item == owner)
    }

    fn native_residual_type(
        &self,
        owner: HirItemId,
        ty: TypeId,
    ) -> Result<GenericArgumentIdentity, EffectSpecializationError> {
        let unsupported = || EffectSpecializationError::UnsupportedResidualSubstitution;
        if let Some(Type::Named(named)) = self.types.store.get(ty)
            && let Some(symbol) = named.source_parameter
        {
            let data = self.hir.symbols.get(symbol).ok_or_else(unsupported)?;
            let SymbolDef::TypeParam {
                owner, param_index, ..
            } = data.def
            else {
                return Err(unsupported());
            };
            let symbols = self.native_generic_symbols(owner)?;
            if symbols.get(param_index as usize) != Some(&symbol) || data.name != named.name {
                return Err(unsupported());
            }
            let id = GenericParameterId::from_source_owner(
                self.native_residual_owner(owner)?,
                param_index,
                GenericParameterKind::Type,
            );
            return Ok(GenericArgumentIdentity::TypeParameter(id));
        }
        if let Some(provider) = self.residual_identity {
            return provider
                .closed_type(self.registry, owner, ty)
                .map(GenericArgumentIdentity::Type)
                .map_err(map_identity_error);
        }
        let fingerprint = etas_types::ty::closed_structural_type_fingerprint(
            &self.types.store,
            ty,
            DescriptorLimits::default(),
        )
        .map_err(|_| unsupported())?
        .ok_or_else(unsupported)?;
        Ok(GenericArgumentIdentity::Type(fingerprint))
    }

    fn native_residual_effect_parameter(
        &self,
        call: HirExprId,
        position: u32,
        arity: usize,
        target_name: &str,
        row: &crate::EffectRow,
    ) -> Result<GenericArgumentIdentity, EffectSpecializationError> {
        let unsupported = || EffectSpecializationError::UnsupportedResidualSubstitution;
        let args = match self.hir.exprs.get(call) {
            Some(HirExpr::Call { generic_args, .. } | HirExpr::MethodCall { generic_args, .. }) => {
                generic_args
            }
            _ => return Err(unsupported()),
        };
        if args.len() != arity {
            return Err(unsupported());
        }
        let Some(HirGenericArg::EffectRow(source_row)) = args.get(position as usize) else {
            return Err(unsupported());
        };
        if !source_row.effects.is_empty() {
            return Err(unsupported());
        }
        let path = source_row.tail.as_ref().ok_or_else(unsupported)?;
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return Err(unsupported());
        };
        let data = self.hir.symbols.get(symbol).ok_or_else(unsupported)?;
        let SymbolDef::EffectParam { owner, param_index } = data.def else {
            return Err(unsupported());
        };
        if path.segments.len() != 1
            || path.segments[0].name != data.name
            || self.hir.index().expr_item.get(&call) != Some(&owner)
            || self
                .native_generic_symbols(owner)?
                .get(param_index as usize)
                != Some(&symbol)
            || row.open != Some(crate::effect_var_id_from_name(&data.name))
        {
            return Err(unsupported());
        }
        let fact = self
            .types
            .facts
            .generic_instantiations
            .get(&call)
            .ok_or_else(unsupported)?;
        if fact
            .deferred_effect_row_obligations
            .iter()
            .any(|obligation| obligation.param == target_name)
        {
            return Err(unsupported());
        }
        let mut bindings = fact
            .effect_row_bindings
            .iter()
            .filter(|(name, _)| name == target_name);
        let (_, checked_row) = bindings.next().ok_or_else(unsupported)?;
        if bindings.next().is_some()
            || !checked_row.effects.is_empty()
            || checked_row.tail.as_deref() != Some(&data.name)
        {
            return Err(unsupported());
        }
        Ok(GenericArgumentIdentity::EffectRowParameter(
            GenericParameterId::from_source_owner(
                self.native_residual_owner(owner)?,
                param_index,
                GenericParameterKind::EffectRow,
            ),
        ))
    }

    fn native_generic_symbols(
        &self,
        owner: HirItemId,
    ) -> Result<&[SymbolId], EffectSpecializationError> {
        match self.hir.items.get(owner) {
            Some(HirItem::Flow(flow)) => Ok(&flow.type_params),
            Some(HirItem::Tool(tool)) => Ok(&tool.type_params),
            _ => Err(EffectSpecializationError::UnsupportedResidualSubstitution),
        }
    }

    fn native_residual_owner(
        &self,
        owner: HirItemId,
    ) -> Result<ResidualOwnerId, EffectSpecializationError> {
        let unsupported = || EffectSpecializationError::UnsupportedResidualSubstitution;
        let item = self.hir.items.get(owner).ok_or_else(unsupported)?;
        let span = item.span();
        let snapshot = self
            .source_snapshots
            .get(span.source)
            .map_err(|_| unsupported())?
            .ok_or_else(unsupported)?;
        let location = snapshot
            .location(u64::from(span.range.start.0), u64::from(span.range.end.0))
            .map_err(|_| unsupported())?;
        let kind = crate::trace_spec::checked_callable_owner_kind(self.hir, owner)
            .map_err(|_| unsupported())?;
        ResidualOwnerId::from_checked_source_declaration(&snapshot, &location, kind)
            .map_err(|_| unsupported())
    }

    fn residual_checkpoint(&self) -> Result<(), EffectSpecializationError> {
        if let Some(control) = self.query_control {
            control
                .checkpoint()
                .map_err(EffectSpecializationError::Interrupted)?;
        }
        Ok(())
    }
}

fn map_identity_error(error: crate::ResidualIdentityError) -> EffectSpecializationError {
    match error {
        crate::ResidualIdentityError::Interrupted(interruption) => {
            EffectSpecializationError::Interrupted(interruption)
        }
        error => EffectSpecializationError::ResidualIdentity(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use etas_code::{
        CodeFingerprint, StableSourceKey, StableSourceOrigin,
        residual::identity::{ObligationId, ResidualObligationRef},
    };
    use etas_core::{SourceFile, SourceId};
    use etas_hir::{HirExpr, HirItem};
    use etas_hir_analysis::interprocedural::CallSite;
    use std::sync::Arc;

    #[test]
    fn residual_identity_cancellation_remains_a_pipeline_interruption() {
        let interruption = etas_utils::PipelineInterruption {
            kind: etas_utils::PipelineInterruptionKind::Cancelled,
            reason: "cancelled".into(),
        };
        assert!(matches!(
            map_identity_error(crate::ResidualIdentityError::Interrupted(interruption.clone())),
            EffectSpecializationError::Interrupted(actual) if actual == interruption
        ));
    }

    #[test]
    fn checked_native_calls_specialize_residual_type_binders() {
        let text = r#"
module app;
type Token = i32;
effect Gate { action tick() -> unit; }
flow callee<T>(value: T) -> unit ![] { return; }
flow caller<U>(value: U) -> unit ![] { callee<U>(value); return; }
flow swap<T, U>(left: T, right: U) -> unit ![] { swap<U, T>(right, left); return; }
flow swap_rows<effect E, effect F>(left: () -> unit ![E], right: () -> unit ![F]) -> unit ![] { swap_rows<![F], ![E]>(right, left); return; }
flow use_token(value: Token) -> unit ![] { callee<Token>(value); return; }
flow invoke<effect E>(f: () -> unit ![E]) -> unit ![E] { f(); return; }
flow alternate<effect E>(f: () -> unit ![E]) -> unit ![E] { f(); return; }
flow forward<effect X>(f: () -> unit ![X]) -> unit ![X] { invoke<![X]>(f); alternate<![X]>(f); return; }
flow noop() -> unit ![] { return; }
flow effectful() -> unit ![Gate.tick] { perform Gate.tick(); return; }
flow use_effect() -> unit ![Gate.tick] { invoke<![Gate.tick]>(effectful); return; }
flow main() -> unit ![] { callee<i32>(1); invoke<![]>(noop); forward<![]>(noop); return; }
"#;
        let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(0), None, text));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = etas_hir::HirRevision::try_new(etas_hir::lower_program(&parsed.value)).unwrap();
        let types = etas_types::check_program(&hir);
        assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
        let item = |name: &str| {
            hir.items
                .iter()
                .find_map(|(id, item)| match item {
                    HirItem::Flow(flow) if hir.symbols.get(flow.symbol).unwrap().name == name => {
                        Some(id)
                    }
                    _ => None,
                })
                .unwrap()
        };
        let callee = item("callee");
        let caller = item("caller");
        let swap = item("swap");
        let swap_rows = item("swap_rows");
        let use_token = item("use_token");
        let invoke = item("invoke");
        let forward = item("forward");
        let use_effect = item("use_effect");
        let main = item("main");
        let std = etas_std::standard_registry();
        let registry = crate::EffectRegistry::from_hir_and_types(&hir, &types).unwrap();
        let mut semantics = EffectSemantics::new(&hir, &types, &std, &registry, &[], &[]);
        let key = StableSourceKey::new(
            StableSourceOrigin::Project(CodeFingerprint::from_bytes([1; 32])),
            &["app.es".into()],
        )
        .unwrap();
        semantics.source_snapshots = Arc::new(
            crate::infer::analysis::EffectSourceSnapshots::new(
                &[crate::EffectSourceSnapshot {
                    source: SourceId(0),
                    key,
                    text,
                }],
                None,
            )
            .unwrap(),
        );
        let callee_parameter = GenericParameterId::from_source_owner(
            semantics.native_residual_owner(callee).unwrap(),
            0,
            GenericParameterKind::Type,
        );
        let caller_parameter = GenericParameterId::from_source_owner(
            semantics.native_residual_owner(caller).unwrap(),
            0,
            GenericParameterKind::Type,
        );
        let obligation =
            ObligationId::from_canonical_fingerprint(CodeFingerprint::from_bytes([2; 32]));
        let swap_owner = semantics.native_residual_owner(swap).unwrap();
        let left = GenericParameterId::from_source_owner(swap_owner, 0, GenericParameterKind::Type);
        let right =
            GenericParameterId::from_source_owner(swap_owner, 1, GenericParameterKind::Type);
        let mut recursive_summary = crate::EffectSummary::local();
        recursive_summary.residual_checks.insert(
            ResidualObligationRef::new(
                obligation,
                [
                    (left, GenericArgumentIdentity::TypeParameter(left)),
                    (right, GenericArgumentIdentity::TypeParameter(right)),
                ]
                .into_iter()
                .collect(),
            )
            .unwrap(),
        );
        let swap_call = hir.exprs.iter().find_map(|(id, expr)| match expr {
            HirExpr::Call { callee: target, .. }
                if hir.index().expr_item.get(&id) == Some(&swap)
                    && matches!(hir.exprs.get(*target), Some(HirExpr::Path(path)) if path.segments.last().is_some_and(|segment| segment.name == "swap")) =>
            {
                Some(CallSite::unresolved(id, *target, expr.span(&hir.blocks)))
            }
            _ => None,
        }).unwrap();
        let swapped = semantics
            .specialize_summary_for_call(
                crate::EffectUnit::Item(swap),
                &swap_call,
                &Default::default(),
                &recursive_summary,
                None,
            )
            .unwrap();
        let reference = swapped.residual_checks.iter().next().unwrap();
        assert_eq!(
            reference.substitutions()[&left],
            GenericArgumentIdentity::TypeParameter(right)
        );
        assert_eq!(
            reference.substitutions()[&right],
            GenericArgumentIdentity::TypeParameter(left)
        );
        let restored = semantics
            .specialize_summary_for_call(
                crate::EffectUnit::Item(swap),
                &swap_call,
                &Default::default(),
                &swapped,
                None,
            )
            .unwrap();
        assert_eq!(restored.residual_checks, recursive_summary.residual_checks);
        let row_owner = semantics.native_residual_owner(swap_rows).unwrap();
        let first_row =
            GenericParameterId::from_source_owner(row_owner, 0, GenericParameterKind::EffectRow);
        let second_row =
            GenericParameterId::from_source_owner(row_owner, 1, GenericParameterKind::EffectRow);
        let mut row_summary = crate::EffectSummary::local();
        row_summary.residual_checks.insert(
            ResidualObligationRef::new(
                obligation,
                [
                    (
                        first_row,
                        GenericArgumentIdentity::EffectRowParameter(first_row),
                    ),
                    (
                        second_row,
                        GenericArgumentIdentity::EffectRowParameter(second_row),
                    ),
                ]
                .into_iter()
                .collect(),
            )
            .unwrap(),
        );
        let row_call = hir.exprs.iter().find_map(|(id, expr)| match expr {
            HirExpr::Call { callee: target, .. }
                if hir.index().expr_item.get(&id) == Some(&swap_rows)
                    && matches!(hir.exprs.get(*target), Some(HirExpr::Path(path)) if path.segments.last().is_some_and(|segment| segment.name == "swap_rows")) =>
            {
                Some(CallSite::unresolved(id, *target, expr.span(&hir.blocks)))
            }
            _ => None,
        }).unwrap();
        let swapped_rows = semantics
            .specialize_summary_for_call(
                crate::EffectUnit::Item(swap_rows),
                &row_call,
                &Default::default(),
                &row_summary,
                None,
            )
            .unwrap();
        let reference = swapped_rows.residual_checks.iter().next().unwrap();
        assert_eq!(
            reference.substitutions()[&first_row],
            GenericArgumentIdentity::EffectRowParameter(second_row)
        );
        assert_eq!(
            reference.substitutions()[&second_row],
            GenericArgumentIdentity::EffectRowParameter(first_row)
        );
        let restored_rows = semantics
            .specialize_summary_for_call(
                crate::EffectUnit::Item(swap_rows),
                &row_call,
                &Default::default(),
                &swapped_rows,
                None,
            )
            .unwrap();
        assert_eq!(restored_rows.residual_checks, row_summary.residual_checks);
        let mut summary = crate::EffectSummary::local();
        summary.residual_checks.insert(
            ResidualObligationRef::new(
                obligation,
                [(
                    callee_parameter,
                    GenericArgumentIdentity::TypeParameter(callee_parameter),
                )]
                .into_iter()
                .collect(),
            )
            .unwrap(),
        );
        let index = hir.index();
        let call = |owner| {
            hir.exprs.iter().find_map(|(id, expr)| match expr {
                HirExpr::Call { callee: target, .. }
                    if index.expr_item.get(&id) == Some(&owner)
                        && matches!(hir.exprs.get(*target), Some(HirExpr::Path(path)) if path.segments.last().is_some_and(|segment| segment.name == "callee")) =>
                {
                    Some(CallSite::unresolved(id, *target, expr.span(&hir.blocks)))
                }
                _ => None,
            }).unwrap()
        };
        let specialize = |owner| {
            semantics
                .specialize_summary_for_call(
                    crate::EffectUnit::Item(callee),
                    &call(owner),
                    &Default::default(),
                    &summary,
                    None,
                )
                .unwrap()
        };
        let symbolic = specialize(caller);
        let reference = symbolic.residual_checks.iter().next().unwrap();
        assert_eq!(reference.obligation(), obligation);
        assert_eq!(
            reference.substitutions()[&callee_parameter],
            GenericArgumentIdentity::TypeParameter(caller_parameter)
        );

        let concrete = specialize(main);
        let fact = &types.facts.generic_instantiations[&call(main).call];
        let ty = fact.type_bindings[0].1;
        let fingerprint = etas_types::ty::closed_structural_type_fingerprint(
            &types.store,
            ty,
            DescriptorLimits::default(),
        )
        .unwrap()
        .unwrap();
        let reference = concrete.residual_checks.iter().next().unwrap();
        assert_eq!(reference.obligation(), obligation);
        assert_eq!(
            reference.substitutions()[&callee_parameter],
            GenericArgumentIdentity::Type(fingerprint)
        );
        assert!(matches!(
            semantics.specialize_summary_for_call(
                crate::EffectUnit::Item(callee),
                &call(use_token),
                &Default::default(),
                &summary,
                None,
            ),
            Err(EffectSpecializationError::UnsupportedResidualSubstitution)
        ));
        let effect_parameter = GenericParameterId::from_source_owner(
            semantics.native_residual_owner(invoke).unwrap(),
            0,
            GenericParameterKind::EffectRow,
        );
        let mut effect_summary = crate::EffectSummary::local();
        effect_summary.residual_checks.insert(
            ResidualObligationRef::new(
                obligation,
                [(
                    effect_parameter,
                    GenericArgumentIdentity::EffectRowParameter(effect_parameter),
                )]
                .into_iter()
                .collect(),
            )
            .unwrap(),
        );
        let invoke_site = hir.exprs.iter().find_map(|(id, expr)| match expr {
            HirExpr::Call { callee: target, .. }
                if index.expr_item.get(&id) == Some(&main)
                    && matches!(hir.exprs.get(*target), Some(HirExpr::Path(path)) if path.segments.last().is_some_and(|segment| segment.name == "invoke")) =>
            {
                Some(CallSite::unresolved(id, *target, expr.span(&hir.blocks)))
            }
            _ => None,
        }).unwrap();
        let specialized = semantics
            .specialize_summary_for_call(
                crate::EffectUnit::Item(invoke),
                &invoke_site,
                &Default::default(),
                &effect_summary,
                None,
            )
            .unwrap();
        let graph = etas_code::descriptor::DescriptorGraph::new(
            etas_code::descriptor::DescriptorGraphInput {
                rows: vec![etas_code::descriptor::EffectRowInfo {
                    effects: vec![],
                    tail: None,
                }],
                ..Default::default()
            },
            DescriptorLimits::default(),
        )
        .unwrap();
        assert_eq!(
            specialized
                .residual_checks
                .iter()
                .next()
                .unwrap()
                .substitutions()[&effect_parameter],
            GenericArgumentIdentity::EffectRow(
                graph
                    .row_fingerprint(etas_code::descriptor::EffectRowInfoRef(0))
                    .unwrap()
            ),
        );
        let forward_site = hir.exprs.iter().find_map(|(id, expr)| match expr {
            HirExpr::Call { callee: target, .. }
                if index.expr_item.get(&id) == Some(&forward)
                    && matches!(hir.exprs.get(*target), Some(HirExpr::Path(path)) if path.segments.last().is_some_and(|segment| segment.name == "invoke")) =>
            {
                Some(CallSite::unresolved(id, *target, expr.span(&hir.blocks)))
            }
            _ => None,
        }).unwrap();
        let forwarded = semantics
            .specialize_summary_for_call(
                crate::EffectUnit::Item(invoke),
                &forward_site,
                &Default::default(),
                &effect_summary,
                None,
            )
            .unwrap();
        let forward_parameter = GenericParameterId::from_source_owner(
            semantics.native_residual_owner(forward).unwrap(),
            0,
            GenericParameterKind::EffectRow,
        );
        assert_eq!(
            forwarded
                .residual_checks
                .iter()
                .next()
                .unwrap()
                .substitutions()[&effect_parameter],
            GenericArgumentIdentity::EffectRowParameter(forward_parameter),
        );
        let captured_values = semantics
            .native_creation_generics(invoke, forward_site.call, &Default::default(), None)
            .unwrap();
        let captured_key = CallableGenericParameter::Native {
            owner: invoke,
            position: 0,
        };
        assert!(matches!(
            captured_values.get(&captured_key),
            Some(CallableGenericValue::EffectRowFromCall { source, .. }) if *source == forward_site.call
        ));
        let site = semantics
            .callable_environment_site(forward_site.call)
            .unwrap();
        let environment = crate::facts::CallableEnvironmentId {
            origin: site,
            application: site,
        };
        semantics
            .record_callable_environment(
                environment,
                crate::facts::CallableEnvironment {
                    owner: forward,
                    parameters: Default::default(),
                    generics: captured_values.clone(),
                },
            )
            .unwrap();
        let captured_bindings = semantics
            .native_residual_bindings(
                invoke,
                &captured_values,
                &effect_summary.residual_checks,
                invoke_site.call,
                Some(environment),
            )
            .unwrap();
        assert_eq!(
            captured_bindings[&effect_parameter],
            GenericArgumentIdentity::EffectRowParameter(forward_parameter)
        );
        let captured_summary = semantics
            .specialize_summary_for_call(
                crate::EffectUnit::Item(invoke),
                &invoke_site,
                &Default::default(),
                &effect_summary,
                Some(environment),
            )
            .unwrap();
        assert_eq!(
            captured_summary
                .residual_checks
                .iter()
                .next()
                .unwrap()
                .substitutions()[&effect_parameter],
            GenericArgumentIdentity::EffectRowParameter(forward_parameter)
        );
        let mut forged_values = captured_values.clone();
        let Some(CallableGenericValue::EffectRowFromCall { source, .. }) =
            forged_values.get_mut(&captured_key)
        else {
            panic!("expected captured row provenance");
        };
        *source = invoke_site.call;
        assert!(matches!(
            semantics.native_residual_bindings(
                invoke,
                &forged_values,
                &effect_summary.residual_checks,
                invoke_site.call,
                Some(environment),
            ),
            Err(EffectSpecializationError::UnsupportedResidualSubstitution)
        ));
        let alternate_call = hir.exprs.iter().find_map(|(id, expr)| match expr {
            HirExpr::Call { callee, .. }
                if index.expr_item.get(&id) == Some(&forward)
                    && matches!(hir.exprs.get(*callee), Some(HirExpr::Path(path)) if path.segments.last().is_some_and(|segment| segment.name == "alternate")) => Some(id),
            _ => None,
        }).unwrap();
        let mut wrong_target_values = captured_values.clone();
        let Some(CallableGenericValue::EffectRowFromCall { source, .. }) =
            wrong_target_values.get_mut(&captured_key)
        else {
            panic!("expected captured row provenance");
        };
        *source = alternate_call;
        let wrong_site = semantics.callable_environment_site(alternate_call).unwrap();
        let wrong_target_environment = crate::facts::CallableEnvironmentId {
            origin: CodeFingerprint::from_bytes([98; 32]),
            application: wrong_site,
        };
        semantics
            .record_callable_environment(
                wrong_target_environment,
                crate::facts::CallableEnvironment {
                    owner: forward,
                    parameters: Default::default(),
                    generics: wrong_target_values.clone(),
                },
            )
            .unwrap();
        assert!(matches!(
            semantics.native_residual_bindings(
                invoke,
                &wrong_target_values,
                &effect_summary.residual_checks,
                invoke_site.call,
                Some(wrong_target_environment)
            ),
            Err(EffectSpecializationError::UnsupportedResidualSubstitution)
        ));
        let forged_environment = crate::facts::CallableEnvironmentId {
            origin: CodeFingerprint::from_bytes([99; 32]),
            application: site,
        };
        semantics
            .record_callable_environment(
                forged_environment,
                crate::facts::CallableEnvironment {
                    owner: forward,
                    parameters: Default::default(),
                    generics: forged_values.clone(),
                },
            )
            .unwrap();
        assert!(matches!(
            semantics.native_residual_bindings(
                invoke,
                &forged_values,
                &effect_summary.residual_checks,
                invoke_site.call,
                Some(forged_environment),
            ),
            Err(EffectSpecializationError::UnsupportedResidualSubstitution)
        ));
        let outer_site = hir.exprs.iter().find_map(|(id, expr)| match expr {
            HirExpr::Call { callee: target, .. }
                if index.expr_item.get(&id) == Some(&main)
                    && matches!(hir.exprs.get(*target), Some(HirExpr::Path(path)) if path.segments.last().is_some_and(|segment| segment.name == "forward")) =>
            {
                Some(CallSite::unresolved(id, *target, expr.span(&hir.blocks)))
            }
            _ => None,
        }).unwrap();
        let composed = semantics
            .specialize_summary_for_call(
                crate::EffectUnit::Item(forward),
                &outer_site,
                &Default::default(),
                &forwarded,
                None,
            )
            .unwrap();
        let composed_reference = composed.residual_checks.iter().next().unwrap();
        assert_eq!(composed_reference.obligation(), obligation);
        assert_eq!(
            composed_reference.substitutions()[&effect_parameter],
            GenericArgumentIdentity::EffectRow(
                graph
                    .row_fingerprint(etas_code::descriptor::EffectRowInfoRef(0))
                    .unwrap()
            ),
        );
        let mut inconsistent_types = types.clone();
        let fact = Arc::make_mut(&mut inconsistent_types.facts)
            .generic_instantiations
            .get_mut(&forward_site.call)
            .unwrap();
        fact.effect_row_bindings[0].1.tail = Some("Different".into());
        let mut inconsistent =
            EffectSemantics::new(&hir, &inconsistent_types, &std, &registry, &[], &[]);
        inconsistent.source_snapshots = semantics.source_snapshots.clone();
        assert!(matches!(
            inconsistent.specialize_summary_for_call(
                crate::EffectUnit::Item(invoke),
                &forward_site,
                &Default::default(),
                &effect_summary,
                None,
            ),
            Err(EffectSpecializationError::UnsupportedResidualSubstitution)
        ));
        let effectful_site = hir.exprs.iter().find_map(|(id, expr)| match expr {
            HirExpr::Call { callee: target, .. }
                if index.expr_item.get(&id) == Some(&use_effect)
                    && matches!(hir.exprs.get(*target), Some(HirExpr::Path(path)) if path.segments.last().is_some_and(|segment| segment.name == "invoke")) =>
            {
                Some(CallSite::unresolved(id, *target, expr.span(&hir.blocks)))
            }
            _ => None,
        }).unwrap();
        assert!(matches!(
            semantics.specialize_summary_for_call(
                crate::EffectUnit::Item(invoke),
                &effectful_site,
                &Default::default(),
                &effect_summary,
                None,
            ),
            Err(EffectSpecializationError::UnsupportedResidualSubstitution)
        ));

        semantics.source_snapshots = Arc::default();
        assert!(matches!(
            semantics.specialize_summary_for_call(
                crate::EffectUnit::Item(callee),
                &call(main),
                &Default::default(),
                &summary,
                None
            ),
            Err(EffectSpecializationError::UnsupportedResidualSubstitution)
        ));
    }
}
