use std::collections::{BTreeMap, BTreeSet};

use etas_core::Diagnostic;
use etas_hir::{HirExprId, SymbolId};

use crate::{interprocedural::SummaryStore, unit::HirSemanticUnit};

use super::{
    context::ContextualAliasUnit,
    domain::AliasValue,
    place::Place,
    summary::{AliasSummary, AliasValueExpr},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AliasFacts {
    pub expr_aliases: BTreeMap<HirExprId, AliasValue>,
    pub symbol_aliases: BTreeMap<SymbolId, AliasValue>,
    pub escaped: BTreeSet<Place>,
    pub unit_summaries: BTreeMap<HirSemanticUnit, AliasSummary>,
    pub unknown_exprs: BTreeSet<HirExprId>,
    pub incomplete_units: BTreeSet<HirSemanticUnit>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasAnalysisOutput {
    pub facts: AliasFacts,
    pub summaries: SummaryStore<HirSemanticUnit, AliasSummary>,
    pub contextual_summaries: SummaryStore<ContextualAliasUnit, AliasSummary>,
    pub diagnostics: Vec<Diagnostic>,
    pub convergence: Vec<crate::interprocedural::ComponentConvergence<ContextualAliasUnit>>,
}

impl AliasFacts {
    pub fn record_expr(&mut self, expr: HirExprId, value: AliasValue) {
        if value.is_unknown() {
            self.unknown_exprs.insert(expr);
        }
        self.expr_aliases.insert(expr, value);
    }

    pub fn record_symbol(&mut self, symbol: SymbolId, value: AliasValue) {
        self.symbol_aliases.insert(symbol, value);
    }

    pub fn record_summary(&mut self, unit: HirSemanticUnit, summary: AliasSummary) {
        if summary.incomplete || matches!(summary.return_alias, AliasValueExpr::Unknown) {
            self.incomplete_units.insert(unit);
        }
        self.unit_summaries.insert(unit, summary);
    }
}
