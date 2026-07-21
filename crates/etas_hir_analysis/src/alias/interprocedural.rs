use std::collections::{BTreeMap, BTreeSet};

use etas_core::Diagnostic;
use etas_hir::{HirExpr, HirExprId, SymbolId};
use etas_utils::{FixpointEngine, IterationLimit};

use crate::{
    interprocedural::{InterproceduralAnalysis, SummaryStore},
    unit::HirSemanticUnit,
};

use super::{
    AliasContext, AliasPrecisionConfig, ContextSensitivity, ContextualAliasUnit,
    facts::{AliasAnalysisOutput, AliasFacts},
    oracle::AliasOracle,
    semantics::AliasSemantics,
    summary::{AliasSummary, AliasValueExpr},
};

pub struct AliasAnalysisInput<'a, O> {
    pub hir: &'a etas_hir::HirProgram,
    pub units: Vec<HirSemanticUnit>,
    pub config: AliasPrecisionConfig,
    pub oracle: O,
}

pub fn analyze_aliases<O>(input: AliasAnalysisInput<'_, O>) -> AliasAnalysisOutput
where
    O: AliasOracle,
{
    let engine = FixpointEngine::new(IterationLimit::new(input.config.max_fixpoint_iterations));
    let contextual_units = contextual_units(input.hir, &input.units, input.config);
    let semantics = AliasSemantics::new(input.hir, &input.units, input.config, input.oracle);
    let result = InterproceduralAnalysis::new(contextual_units, semantics)
        .with_engine(engine)
        .solve();

    let convergence = result.convergence;
    let mut contextual_summaries = result.summaries;
    let (mut facts, mut diagnostics, captures) = result.semantics.into_parts();
    diagnostics.extend(result.diagnostics.into_iter().map(|diagnostic| {
        Diagnostic::analysis(
            etas_core::AnalysisDiagnosticCode::MissingCheckedFact,
            etas_core::Span::empty(etas_core::SourceId(0), Default::default()),
            format!("interprocedural alias analysis failed: {diagnostic:?}"),
        )
    }));

    apply_captures_to_summaries(&mut contextual_summaries, captures, input.config);
    let summaries = semantic_summaries(&contextual_summaries);
    materialize_summaries(&mut facts, &summaries);
    AliasAnalysisOutput {
        facts,
        summaries,
        contextual_summaries,
        diagnostics,
        convergence,
    }
}

fn apply_captures_to_summaries(
    summaries: &mut SummaryStore<ContextualAliasUnit, AliasSummary>,
    captures: BTreeMap<ContextualAliasUnit, BTreeMap<SymbolId, AliasValueExpr>>,
    config: AliasPrecisionConfig,
) {
    for (unit, captured) in captures {
        let mut summary = summaries
            .get(unit)
            .cloned()
            .unwrap_or_else(|| AliasSummary::new(unit.semantic, config));
        for (symbol, value) in captured {
            summary.captures.insert(symbol, value);
        }
        summaries.insert(unit, summary);
    }
}

fn semantic_summaries(
    contextual: &SummaryStore<ContextualAliasUnit, AliasSummary>,
) -> SummaryStore<HirSemanticUnit, AliasSummary> {
    let mut summaries = SummaryStore::new();
    for (unit, summary) in contextual.iter() {
        let mut summary = summary.clone();
        summary.unit = Some(unit.semantic);
        summaries.join_summary(unit.semantic, summary);
    }
    summaries
}

fn materialize_summaries(
    facts: &mut AliasFacts,
    summaries: &SummaryStore<HirSemanticUnit, AliasSummary>,
) {
    for (unit, summary) in summaries.iter() {
        facts.record_summary(*unit, summary.clone());
    }
}

fn contextual_units(
    hir: &etas_hir::HirProgram,
    units: &[HirSemanticUnit],
    config: AliasPrecisionConfig,
) -> Vec<ContextualAliasUnit> {
    let contexts = contexts_for_config(hir, config);
    let mut result = Vec::new();
    for unit in units {
        for context in &contexts {
            result.push(ContextualAliasUnit::new(*unit, *context));
        }
    }
    result.sort();
    result.dedup();
    result
}

fn contexts_for_config(
    hir: &etas_hir::HirProgram,
    config: AliasPrecisionConfig,
) -> Vec<AliasContext> {
    let k = match config.context_sensitivity {
        ContextSensitivity::ContextInsensitive => 0,
        ContextSensitivity::CallString { k } => k.min(super::context::MAX_CALL_STRING),
    };
    if k == 0 {
        return vec![AliasContext::empty()];
    }
    let calls = collect_call_exprs(hir);
    let mut contexts = BTreeSet::new();
    contexts.insert(AliasContext::empty());
    build_contexts(&calls, k, &mut Vec::new(), &mut contexts);
    contexts.into_iter().collect()
}

fn collect_call_exprs(hir: &etas_hir::HirProgram) -> Vec<HirExprId> {
    let mut calls = hir
        .exprs
        .iter()
        .filter_map(|(expr, data)| match data {
            HirExpr::Call { .. } | HirExpr::MethodCall { .. } => Some(expr),
            _ => None,
        })
        .collect::<Vec<_>>();
    calls.sort();
    calls
}

fn build_contexts(
    calls: &[HirExprId],
    max_len: usize,
    current: &mut Vec<HirExprId>,
    out: &mut BTreeSet<AliasContext>,
) {
    if current.len() == max_len {
        return;
    }
    for call in calls {
        current.push(*call);
        out.insert(AliasContext::from_calls(current));
        build_contexts(calls, max_len, current, out);
        current.pop();
    }
}
