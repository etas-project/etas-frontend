use etas_hir::{HirItem, HirItemId, SymbolId};
use etas_utils::UnitKey;

use crate::incremental::{BodyArtifactIdentity, dependency_fingerprints_match};
use crate::{BODY_UNIT_KIND, ProjectContext};

pub(super) fn record_reused_effect_body_artifacts(context: &mut ProjectContext) {
    if !context.incremental || context.body_artifact_reuse.effect_outputs.is_empty() {
        return;
    }
    let Some(units) = context.units.as_ref() else {
        return;
    };
    let Some(sources) = context.sources.as_ref() else {
        return;
    };
    let current_manifest = context.current_artifact_manifest();
    let mut reused = context.body_artifact_reuse.effect_outputs.clone();
    reused.sort_by_key(|artifact| artifact.unit.0);

    for artifact in reused {
        if !context.type_body_outputs.contains_key(&artifact.unit) {
            continue;
        }
        let unit_key = UnitKey::new(BODY_UNIT_KIND, artifact.unit.0 as u64);
        if context.effect_body_outputs.contains_key(&artifact.unit) {
            continue;
        }
        let Some(current_identity) = BodyArtifactIdentity::for_unit(artifact.unit, units, sources)
        else {
            continue;
        };
        if current_identity != artifact.identity {
            continue;
        }
        if !dependency_fingerprints_match(&artifact.dependencies, &current_manifest) {
            continue;
        }
        let expected_key = crate::artifact::FrontendArtifactKey::unit(
            crate::artifact::FrontendArtifactKind::EffectFacts,
            unit_key,
        )
        .to_cache_key();
        if artifact.cache_key == expected_key
            && !context.reused_cache_artifacts.contains(&artifact.cache_key)
        {
            context.reused_cache_artifacts.push(artifact.cache_key);
        }
    }
}

pub(super) fn local_effect_output(
    mut output: etas_effects::EffectOutput,
    hir: &etas_hir::HirProgram,
    item: HirItemId,
) -> etas_effects::EffectOutput {
    let item_symbol = hir.items.get(item).and_then(item_symbol);
    let effect_symbols = hir
        .items
        .iter()
        .filter_map(|(id, hir_item)| matches!(hir_item, HirItem::Effect(_)).then_some(id))
        .collect::<std::collections::HashSet<_>>();
    let effect_symbol_ids = hir
        .items
        .iter()
        .filter_map(|(_, hir_item)| match hir_item {
            HirItem::Effect(effect) => Some(effect.symbol),
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();

    output
        .facts
        .item_effects
        .retain(|id, _| *id == item || effect_symbols.contains(id));
    output
        .facts
        .symbol_effects
        .retain(|symbol, _| Some(*symbol) == item_symbol || effect_symbol_ids.contains(symbol));
    output.facts.requirements.items.retain(|id, _| *id == item);
    output
        .facts
        .requirements
        .symbols
        .retain(|symbol, _| Some(*symbol) == item_symbol);
    output.facts.trace_specs.items.retain(|id, _| *id == item);
    output
        .facts
        .trace_specs
        .symbols
        .retain(|symbol, _| Some(*symbol) == item_symbol);
    output
        .facts
        .interpreter_support
        .items
        .retain(|id, _| *id == item);

    if item_symbol.is_none_or(|symbol| {
        hir.symbols
            .get(symbol)
            .is_none_or(|symbol| symbol.name != "main")
    }) {
        output.facts.interpreter_support.entry = None;
    }

    output
}

fn item_symbol(item: &HirItem) -> Option<SymbolId> {
    match item {
        HirItem::Effect(item) => Some(item.symbol),
        HirItem::Tool(item) => Some(item.symbol),
        HirItem::Agent(item) => Some(item.symbol),
        HirItem::Flow(item) => Some(item.symbol),
        _ => None,
    }
}
