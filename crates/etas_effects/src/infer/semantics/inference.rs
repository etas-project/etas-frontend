use std::collections::BTreeSet;

use etas_hir::{HirAnnotationArg, HirItem, SymbolId};

use super::{
    engine::{EffectSemantics, annotation_name},
    shared::*,
};

pub(super) enum ModelToolSource {
    Item(HirItemId),
    External(Vec<String>),
}

impl EffectSemantics<'_> {
    pub(super) fn agent_model_tools(&self, agent: HirItemId) -> Option<Vec<ModelToolSource>> {
        let mut tools = Vec::new();
        let mut symbols = BTreeSet::new();
        for annotation in self.hir.item_annotations.get(&agent).into_iter().flatten() {
            if annotation_name(annotation) != "tools" {
                continue;
            }
            for arg in &annotation.args {
                let value = match arg {
                    HirAnnotationArg::Positional { value, .. } => *value,
                    HirAnnotationArg::Named { name, value, .. }
                        if name == "tools" || name == "items" =>
                    {
                        *value
                    }
                    HirAnnotationArg::Named { name, .. } if name == "choice" || name == "mode" => {
                        continue;
                    }
                    _ => return None,
                };
                let entries = match self.hir.exprs.get(value)? {
                    HirExpr::Array { elems, .. } | HirExpr::List { elems, .. } => elems.as_slice(),
                    _ => std::slice::from_ref(&value),
                };
                for entry in entries {
                    let symbol = self.callee_symbol(*entry)?;
                    if symbols.insert(symbol) {
                        tools.push(self.model_tool_source(symbol)?);
                    }
                }
            }
        }
        Some(tools)
    }

    fn model_tool_source(&self, symbol: SymbolId) -> Option<ModelToolSource> {
        let binding = self.hir.symbols.get(symbol)?;
        match &binding.def {
            SymbolDef::Item { item }
                if matches!(self.hir.items.get(*item), Some(HirItem::Tool(_))) =>
            {
                Some(ModelToolSource::Item(*item))
            }
            SymbolDef::ImportAlias { path, .. }
                if matches!(
                    self.types.facts.symbol_types.get(&symbol),
                    Some(etas_types::SymbolTypeFact::Tool { .. })
                ) =>
            {
                Some(ModelToolSource::External(path.clone()))
            }
            _ => None,
        }
    }

    pub(crate) fn agent_infer_summary(
        &self,
        fact: &etas_types::facts::inference::CheckedAgentInferenceFact,
        action: &ResolvedActionRef,
        span: Span,
    ) -> Option<(Effect, EffectSummary)> {
        let method = self.types.facts.agent_methods.get(&fact.method)?;
        if method.agent != fact.agent || method.body != fact.owner {
            return None;
        }
        let Effect::Action(action) = self.effect_from_action_ref(action)? else {
            return None;
        };
        let mut path = self.item_effect_path(fact.agent)?;
        path.push(method.name.clone());
        let mut summary = EffectSummary::local();
        let effect = Effect::AppliedAction(ActionInstanceRef {
            action: action.clone(),
            args: vec![EffectArgRef::Path(path), EffectArgRef::Type(fact.output)],
        });
        summary.record_requested_action(effect.clone());
        summary.record_default_handled_action(effect.clone());
        summary.record_action_trace_event(effect.clone(), span, crate::ActionEventSource::Perform);
        if let Some(reason) = self
            .registry
            .runtime_requirement_reason_for_action_ref(&action)
        {
            summary.require_runtime(reason);
        }
        let mut tool_choices = EffectSummary::local();
        for tool in self.agent_model_tools(fact.agent)? {
            let tool = match tool {
                ModelToolSource::Item(item) => self
                    .latent_source_summary(
                        super::shared::LatentSource::Unit(EffectUnit::Item(item)),
                        span,
                    )
                    .ok()?,
                ModelToolSource::External(path) => self.external_summary_for_path(&path, span)?,
            };
            tool_choices.join_branch(&tool);
        }
        summary.seq_assign(&tool_choices.repeated());
        Some((effect, summary))
    }
}
