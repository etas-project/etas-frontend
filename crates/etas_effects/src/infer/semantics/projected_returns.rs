//! Select values through checked constructors before their origins are joined.
//! In particular, an iterator's yielded element is not its returned next call.
use crate::infer::analysis::dependencies::InputFact;
use std::collections::BTreeSet;

use etas_code::descriptor::ValueProjectionStep as Step;
use etas_hir::{HirExpr, HirExprId, HirFieldInit, HirMatchArmBody, ResolveResult};
use etas_types::Type;

use super::{engine::EffectSemantics, state::EffectState};
use crate::{CallableExitSummary, ProjectedReturnSources};

pub(super) fn element_origin_path(
    path: &[etas_code::descriptor::ValueProjectionStep],
) -> Vec<etas_code::descriptor::ValueProjectionStep> {
    use etas_code::descriptor::ValueProjectionStep as Step;
    // List origins join all elements. Selecting an element after a tail
    // therefore uses the same checked set of possible callable origins.
    path.iter()
        .enumerate()
        .filter_map(|(position, step)| match step {
            Step::ListTail
                if matches!(
                    path.get(position + 1),
                    Some(Step::SequenceElement | Step::ListHead | Step::ListTail)
                ) =>
            {
                None
            }
            Step::ListHead => Some(Step::SequenceElement),
            _ => Some(step.clone()),
        })
        .collect()
}

impl EffectSemantics<'_> {
    fn position_payload_carrier(&self, expr: HirExprId, path: &[Step]) -> bool {
        let [step] = path else {
            return false;
        };
        let Some(mut ty) = self.types.facts.expr_types.get(&expr).copied() else {
            return false;
        };
        let mut seen = BTreeSet::new();
        while seen.insert(ty) {
            match self.types.store.get(ty) {
                Some(Type::Array(_) | Type::List(_) | Type::Set(_) | Type::Slice(_)) => {
                    return matches!(
                        step,
                        Step::SequenceElement | Step::ListHead | Step::ListTail
                    );
                }
                Some(Type::Record(record)) => {
                    let Step::Field(name) = step else {
                        return false;
                    };
                    return record.fields.iter().any(|field| field.name == *name)
                        && record
                            .fields
                            .iter()
                            .filter(|field| field.name != *name)
                            .all(|field| {
                                !self.type_contains_flow(field.ty)
                                    && !self.type_has_parameters(field.ty)
                            });
                }
                Some(Type::Nominal(_) | Type::Applied { .. }) => {
                    let Ok(Some(representation)) = self.applied_representation(ty) else {
                        break;
                    };
                    ty = representation;
                }
                _ => break,
            }
        }
        // The native iterator ABI has the same sole stored `next` field.
        matches!(step, Step::Field(name) if name == "next")
            && self.types.facts.expr_types.get(&expr).is_some_and(|ty| {
                etas_types::source_iteration::checked_cursor_indexed(
                    &self.types.store,
                    &self.types.facts,
                    self.type_lookup(),
                    *ty,
                )
                .ok()
                .flatten()
                .is_some()
            })
    }

    fn projection_constructor(&self, callee: HirExprId) -> Option<String> {
        let HirExpr::Path(path) = self.hir.exprs.get(callee)? else {
            return None;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        let symbol = self.type_symbols.canonical_symbol(self.hir, symbol)?;
        let definition = self.hir.symbols.get(symbol)?;
        if let Some((_, _, variant)) = self
            .types
            .facts
            .enum_constructor(&self.types.store, definition)
        {
            return Some(variant.name.clone());
        }
        if self.checked_enum_constructor(symbol) {
            return Some(definition.name.clone());
        }
        let path = self.std_qualified_path_for_expr(callee)?;
        let constructor = self
            .std_registry
            .lookup_qualified(&path)?
            .intrinsic
            .as_ref()?;
        match constructor.id.0 {
            etas_std::intrinsic::pure::OPTION_SOME => Some("Some".into()),
            etas_std::intrinsic::pure::OPTION_NONE => Some("None".into()),
            _ => None,
        }
    }

    pub(super) fn record_iteration_pattern_sources(
        &self,
        pat: etas_hir::HirPatId,
        next: &[super::state::LatentSource],
        application: HirExprId,
        state: &mut EffectState,
    ) -> Result<(), &'static str> {
        for (symbol, selected) in self.pattern_projection_bindings(pat)? {
            let ty = self
                .symbol_value_type(symbol)
                .ok_or("iteration binding has no checked type")?;
            if !self.type_contains_flow(ty) && !self.type_has_parameters(ty) {
                continue;
            }
            if next.is_empty() {
                return Err("iteration has no checked next callable origins");
            }
            let mut prefix = vec![
                Step::VariantPayload {
                    variant: "Some".into(),
                    position: 0,
                },
                Step::TupleElement(0),
            ];
            prefix.extend(selected);
            let paths = self
                .value_payload_paths(ty)
                .ok_or("iteration payload paths exceed the analysis budget")?;
            let mut sources = BTreeSet::new();
            if paths.is_empty() {
                sources = self
                    .invoked_value_projection(next, &prefix, state, application)
                    .unwrap_or_else(|| {
                        [super::state::LatentSource::Unavailable { expr: application }].into()
                    });
            } else {
                for path in paths {
                    let mut selection = prefix.clone();
                    selection.extend_from_slice(&path);
                    let projected = self
                        .invoked_value_projection(next, &selection, state, application)
                        .unwrap_or_else(|| {
                            [super::state::LatentSource::Unavailable { expr: application }].into()
                        });
                    sources.extend(projected.iter().copied());
                    state
                        .local_latent_values
                        .symbol_projections
                        .entry((symbol, path))
                        .or_default()
                        .extend(projected);
                }
            }
            state
                .local_latent_values
                .record_symbol_sources(symbol, sources);
        }
        Ok(())
    }

    pub(super) fn record_value_return(
        &self,
        expr: HirExprId,
        state: &EffectState,
        exits: &mut CallableExitSummary,
    ) {
        let Some(ty) = self.types.facts.expr_types.get(&expr) else {
            return;
        };
        let Some(paths) = self.value_payload_paths(*ty) else {
            exits
                .gaps
                .insert(crate::OutcomeAnalysisGap::MissingReturnObservation);
            return;
        };
        let mut projected = CallableExitSummary::default();
        for path in paths {
            let found = self
                .projected_sources(expr, &path, state)
                .and_then(|sources| {
                    self.returned_callable_sources(state, &sources.into_iter().collect::<Vec<_>>())
                });
            projected.value_projections.insert(
                path,
                found.map_or(
                    ProjectedReturnSources::Unavailable,
                    ProjectedReturnSources::Known,
                ),
            );
        }
        exits.join_assign(&projected);
    }

    pub(super) fn record_symbol_projections(
        &self,
        state: &mut EffectState,
        symbol: etas_hir::SymbolId,
        value: HirExprId,
        prefix: &[Step],
    ) {
        let Some(ty) = self.symbol_value_type(symbol) else {
            return;
        };
        let stable_state = self.stable_agent_state_layout(ty);
        let Some(paths) = self.value_payload_paths(stable_state.unwrap_or(ty)) else {
            return;
        };
        for path in paths {
            let mut selected = prefix.to_vec();
            selected.extend(path.iter().cloned());
            let sources = self.projected_sources(value, &selected, state).or_else(|| {
                stable_state.map(|_| {
                    BTreeSet::from([super::state::LatentSource::Unavailable { expr: value }])
                })
            });
            if let Some(sources) = sources {
                state
                    .local_latent_values
                    .symbol_projections
                    .entry((symbol, path))
                    .or_default()
                    .extend(sources);
            }
        }
    }

    pub(super) fn known_projection_sources(
        &self,
        state: &EffectState,
        expr: HirExprId,
        path: &[Step],
    ) -> Option<BTreeSet<super::state::LatentSource>> {
        let key = (expr, path.to_vec());
        if let Some(found) = state.local_latent_values.value_projections.get(&key) {
            if found.iter().all(|source| matches!(source, super::state::LatentSource::Unavailable { expr: missing } if *missing == expr)) {
                self.dependencies.read(InputFact::ValueProjections(expr));
                if let Some(known) = self.call_results.value_projections.get(&key) {
                    return Some(known.clone());
                }
            }
            return Some(found.clone());
        }
        self.dependencies.read(InputFact::ValueProjections(expr));
        if let Some(found) = self.call_results.value_projections.get(&key) {
            return Some(found.clone());
        }
        // A checked collection element set also covers a head or an element
        // selected after any number of tails. Keep the surrounding fields:
        // aggregate roots may contain unrelated callable payloads.
        let element_key = (expr, element_origin_path(path));
        if element_key != key
            && let Some(found) = state
                .local_latent_values
                .value_projections
                .get(&element_key)
                .or_else(|| self.call_results.value_projections.get(&element_key))
        {
            return Some(found.clone());
        }
        for values in [
            &state.local_latent_values.value_projections,
            &self.call_results.value_projections,
        ] {
            if let Some(found) = self.regular_projection_sources(
                values
                    .range((expr, Vec::new())..)
                    .take_while(|((value, _), _)| *value == expr)
                    .map(|((_, position), sources)| (position.as_slice(), sources)),
                path,
                state,
                expr,
            ) {
                return Some(found);
            }
        }
        let HirExpr::Path(value) = self.hir.exprs.get(expr)? else {
            return None;
        };
        let (symbol, mut selected) = match &value.resolution {
            ResolveResult::Resolved(symbol) => (*symbol, Vec::new()),
            ResolveResult::PartiallyResolved(partial) => (
                partial.resolved_prefix?,
                partial.remaining.iter().cloned().map(Step::Field).collect(),
            ),
            _ => return None,
        };
        selected.extend_from_slice(path);
        if !state.agent_state.is_empty()
            && let Some(allocation) = self.agent_allocation(symbol)
            && let Some(origins) = state.agent_state.get(&(allocation, selected.clone()))
        {
            return Some(origins.clone());
        }
        self.dependencies.read(InputFact::GlobalProjections(symbol));
        let key = (symbol, selected);
        state
            .local_latent_values
            .symbol_projections
            .get(&key)
            .or_else(|| self.global_values.symbol_projections.get(&key))
            .or_else(|| self.discovered_global_values.symbol_projections.get(&key))
            .or_else(|| {
                let element_key = (symbol, element_origin_path(&key.1));
                state
                    .local_latent_values
                    .symbol_projections
                    .get(&element_key)
                    .or_else(|| self.global_values.symbol_projections.get(&element_key))
                    .or_else(|| {
                        self.discovered_global_values
                            .symbol_projections
                            .get(&element_key)
                    })
            })
            .cloned()
            .or_else(|| {
                for values in [
                    &state.local_latent_values.symbol_projections,
                    &self.global_values.symbol_projections,
                    &self.discovered_global_values.symbol_projections,
                ] {
                    if let Some(found) = self.regular_projection_sources(
                        values
                            .range((symbol, Vec::new())..)
                            .take_while(|((value, _), _)| *value == symbol)
                            .map(|((_, position), sources)| (position.as_slice(), sources)),
                        &key.1,
                        state,
                        expr,
                    ) {
                        return Some(found);
                    }
                }
                None
            })
    }

    fn regular_projection_sources<'a>(
        &self,
        positions: impl IntoIterator<Item = (&'a [Step], &'a BTreeSet<super::state::LatentSource>)>,
        path: &[Step],
        state: &EffectState,
        application: HirExprId,
    ) -> Option<BTreeSet<super::state::LatentSource>> {
        let positions = positions.into_iter().collect::<Vec<_>>();
        if !positions.iter().any(|(position, _)| {
            position
                .iter()
                .any(|step| matches!(step, Step::Repeat { .. }))
        }) {
            return None;
        }
        let positions = super::return_projection::returned_positions(positions, path);
        if positions.is_empty() {
            return None;
        }
        positions
            .into_iter()
            .try_fold(BTreeSet::new(), |mut result, (sources, suffix)| {
                result.extend(self.project_value_origins(sources, &suffix, state, application)?);
                Some(result)
            })
    }

    pub(crate) fn projected_sources(
        &self,
        expr: HirExprId,
        path: &[Step],
        state: &EffectState,
    ) -> Option<BTreeSet<super::state::LatentSource>> {
        if self.checked_empty_store_projection(expr) {
            return Some(BTreeSet::new());
        }
        if let Some(position) = path
            .iter()
            .position(|step| matches!(step, Step::Invocation { .. }))
        {
            let Step::Invocation { inputs } = path[position] else {
                unreachable!()
            };
            let origins = self.projected_sources(expr, &path[..position], state)?;
            let origins = self
                .applied_callable_origins(&origins, inputs, state)
                .ok()?;
            let application = self.native_call_inputs(inputs)?.call;
            return self.project_value_origins(&origins, &path[position + 1..], state, application);
        }
        if let Some(found) = self.known_projection_sources(state, expr, path) {
            return Some(found);
        }
        // A checked aggregate field can be available before a longer query
        // through its callable result. Consume that field once, then evaluate
        // the remaining path with the same origin projection machinery.
        for prefix in (1..path.len()).rev() {
            if let Some(sources) = self.known_projection_sources(state, expr, &path[..prefix])
                && let Some(found) =
                    self.project_value_origins(&sources, &path[prefix..], state, expr)
            {
                return Some(found);
            }
        }
        if let Some(ty) = self.types.facts.expr_types.get(&expr)
            && let Ok(Some(selected)) = self.project_value(*ty, path)
        {
            if !self.type_contains_flow(selected) && !self.type_has_parameters(selected) {
                return Some(BTreeSet::new());
            }
            if let Some(children) = self.value_payload_paths(selected)
                && !children.is_empty()
            {
                let mut origins = BTreeSet::new();
                let mut complete = true;
                for suffix in children {
                    let mut child = path.to_vec();
                    child.extend(suffix);
                    let Some(found) = self.known_projection_sources(state, expr, &child) else {
                        complete = false;
                        break;
                    };
                    origins.extend(found);
                }
                if complete {
                    return Some(origins);
                }
            }
        }
        let mut pending = vec![(expr, path.to_vec())];
        let mut seen = BTreeSet::new();
        let mut result = BTreeSet::new();
        while let Some((expr, path)) = pending.pop() {
            if !seen.insert((expr, path.clone())) {
                continue;
            }
            if seen.len() > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
                return None;
            }
            if let Some(control) = self.query_control {
                control.checkpoint().ok()?;
            }
            if path.is_empty() {
                match self.latent_sources_for_expr(state, expr) {
                    Some(sources) => result.extend(sources),
                    None if self.expr_type_contains_flow(expr)
                        || self.expr_type_has_parameters(expr) =>
                    {
                        return None;
                    }
                    None => {}
                }
                continue;
            }
            if let Some(found) = self.known_projection_sources(state, expr, &path) {
                result.extend(found);
                continue;
            }
            let (step, rest) = path.split_first()?;
            if let Step::Repeat { alternatives } = step {
                if let Some(origins) = self.latent_sources_for_expr(state, expr)
                    && let Some(selected) = self.project_parameter_origins(origins, &path)
                {
                    result.extend(selected);
                    continue;
                }
                // A regular path is the union of zero traversals and every
                // alternative followed by the same closure. The existing
                // checked-expression worklist closes cycles without executing
                // a callback or inventing its behavior.
                pending.push((expr, rest.to_vec()));
                for alternative in alternatives {
                    let mut selected = alternative.clone();
                    selected.extend_from_slice(&path);
                    pending.push((expr, selected));
                }
                continue;
            }
            if matches!(step, Step::CallResult | Step::CallWithArguments { .. }) {
                let origins = self.latent_sources_for_expr(state, expr)?;
                let inputs = match step {
                    Step::CallWithArguments { inputs } => Some(*inputs),
                    _ => None,
                };
                result.extend(
                    self.invoked_value_projection_with_inputs(&origins, rest, state, expr, inputs)?,
                );
                continue;
            }
            match self.hir.exprs.get(expr)? {
                HirExpr::EmptySequence { .. }
                    if matches!(
                        step,
                        Step::SequenceElement | Step::ListHead | Step::ListTail
                    ) => {}
                HirExpr::EmptyRecordOrMap { .. }
                    if matches!(step, Step::MapKey | Step::MapValue) => {}
                HirExpr::Array { elems, .. }
                | HirExpr::List { elems, .. }
                | HirExpr::Set { elems, .. }
                    if matches!(step, Step::SequenceElement) =>
                {
                    pending.extend(elems.iter().map(|element| (*element, rest.to_vec())));
                }
                HirExpr::ListCons { head, tail, .. } => match step {
                    Step::SequenceElement => {
                        pending.push((*head, rest.to_vec()));
                        pending.push((*tail, path));
                    }
                    Step::ListHead => pending.push((*head, rest.to_vec())),
                    Step::ListTail => pending.push((*tail, rest.to_vec())),
                    _ => return None,
                },
                HirExpr::Binary {
                    lhs,
                    op: etas_hir::HirBinaryOp::Add,
                    rhs,
                    ..
                } if matches!(step, Step::SequenceElement)
                    && self.types.facts.expr_types.get(&expr).is_some_and(|ty| {
                        matches!(self.types.store.get(*ty), Some(Type::Array(_)))
                    }) =>
                {
                    pending.extend([(*lhs, path.clone()), (*rhs, path)]);
                }
                HirExpr::Map { entries, .. } if matches!(step, Step::MapKey | Step::MapValue) => {
                    pending.extend(entries.iter().map(|entry| {
                        (
                            if matches!(step, Step::MapKey) {
                                entry.key
                            } else {
                                entry.value
                            },
                            rest.to_vec(),
                        )
                    }));
                }
                HirExpr::Index { base, .. } => {
                    let ty = *self.types.facts.expr_types.get(base)?;
                    let step = match self.types.store.get(ty) {
                        Some(Type::Map { .. }) => Step::MapValue,
                        _ => Step::SequenceElement,
                    };
                    let mut selected = vec![step];
                    selected.extend(path);
                    pending.push((*base, selected));
                }
                HirExpr::Tuple { elems, .. } => {
                    let Step::TupleElement(position) = step else {
                        return None;
                    };
                    pending.push((*elems.get(*position as usize)?, rest.to_vec()));
                }
                HirExpr::Field { base, field, .. } => {
                    let mut fields = vec![Step::Field(field.clone())];
                    fields.extend(path);
                    pending.push((*base, fields));
                }
                HirExpr::Record(record) => {
                    let Step::Field(name) = step else { return None };
                    let field = record.fields.iter().find(|field| match field {
                        HirFieldInit::Named {
                            name: candidate, ..
                        }
                        | HirFieldInit::Shorthand {
                            name: candidate, ..
                        } => candidate == name,
                    })?;
                    match field {
                        HirFieldInit::Named { value, .. } => pending.push((*value, rest.to_vec())),
                        HirFieldInit::Shorthand {
                            resolution: ResolveResult::Resolved(symbol),
                            ..
                        } => {
                            result.extend(self.project_parameter_origins(
                                self.latent_sources_for_symbol(state, *symbol)?,
                                rest,
                            )?);
                        }
                        _ => return None,
                    }
                }
                HirExpr::If {
                    then_block,
                    else_branch,
                    ..
                } => {
                    let mut blocks = vec![*then_block];
                    match else_branch {
                        Some(etas_hir::HirElseBranch::Block(block)) => blocks.push(*block),
                        Some(etas_hir::HirElseBranch::If(value)) => {
                            if !self.expr_is_never(*value) {
                                pending.push((*value, path.clone()));
                            }
                        }
                        None => return None,
                    }
                    for block in blocks {
                        match self.hir.blocks.get(block)?.final_expr {
                            Some(value) if !self.expr_is_never(value) => {
                                pending.push((value, path.clone()));
                            }
                            Some(_) => {}
                            None if self.block_type_is_never(block) => {}
                            None => return None,
                        }
                    }
                }
                HirExpr::Match { arms, .. } => {
                    for arm in arms {
                        let value = match arm.body {
                            HirMatchArmBody::Expr(value) => value,
                            HirMatchArmBody::Block(block) => {
                                match self.hir.blocks.get(block)?.final_expr {
                                    Some(value) => value,
                                    None if self.block_type_is_never(block) => continue,
                                    None => return None,
                                }
                            }
                        };
                        if !self.expr_is_never(value) {
                            pending.push((value, path.clone()));
                        }
                    }
                }
                HirExpr::Block(block) | HirExpr::Limited { body: block, .. } => {
                    match self.hir.blocks.get(*block)?.final_expr {
                        Some(value) => pending.push((value, path)),
                        None if self.block_type_is_never(*block) => {}
                        None => return None,
                    }
                }
                HirExpr::Try { expr: operand, .. } if matches!(step, Step::VariantPayload { variant, position: 0 } if variant == "Ok") =>
                {
                    pending.push((*operand, rest.to_vec()));
                }
                HirExpr::Call { callee, args, .. }
                    if self.projection_constructor(*callee).is_some() =>
                {
                    let Step::VariantPayload { variant, position } = step else {
                        return None;
                    };
                    if self.projection_constructor(*callee)? == *variant {
                        let value = match args.get(*position as usize)? {
                            etas_hir::HirArg::Positional(value)
                            | etas_hir::HirArg::Named { value, .. } => *value,
                        };
                        pending.push((value, rest.to_vec()));
                    }
                }
                HirExpr::Call { callee, .. } => {
                    let sources = self.latent_sources_for_expr(state, *callee)?;
                    let Step::CallWithArguments { inputs } =
                        self.call_result_step(expr, state).ok()?
                    else {
                        return None;
                    };
                    result.extend(self.invoked_value_projection_with_inputs(
                        &sources,
                        &path,
                        state,
                        expr,
                        Some(inputs),
                    )?);
                }
                HirExpr::Path(callee)
                    if matches!(step, Step::VariantPayload { .. })
                        && matches!(callee.resolution, ResolveResult::Resolved(symbol)
                            if self.checked_empty_constructor_value(expr, symbol)) => {}
                HirExpr::Path(_) => {
                    let sources = self.latent_sources_for_expr(state, expr)?;
                    if let Some(selected) =
                        self.project_parameter_origins(sources.iter().copied(), &path)
                    {
                        result.extend(selected);
                    } else if self.position_payload_carrier(expr, &path) {
                        result.extend(sources);
                    } else {
                        return None;
                    }
                }
                // The selected field owns all possible callable payloads when
                // every sibling has a checked non-callable, non-generic type.
                // Captures inside the function are not extra record fields.
                _ if self.position_payload_carrier(expr, &path) => {
                    let sources = self.latent_sources_for_expr(state, expr)?;
                    if sources.is_empty() {
                        return None;
                    }
                    result.extend(sources);
                }
                _ => return None,
            }
        }
        Some(result)
    }
}
