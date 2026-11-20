//! Dependency facts finalized with the checked program, before SDK packaging.
use etas_hir::{HirItem, HirItemId};
use etas_types::TypeId;
use std::collections::{BTreeMap, BTreeSet};

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum SemanticDependency {
    Definition(HirItemId),
    Type(TypeId),
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct CheckedSemanticDependencies {
    pub edges: BTreeMap<SemanticDependency, BTreeSet<SemanticDependency>>,
}
impl CheckedSemanticDependencies {
    pub(crate) fn build(checked: &crate::CheckedProgramData) -> Result<Self, String> {
        use SemanticDependency::{Definition, Type};
        let mut graph = Self::default();
        for (ty, value) in checked.type_store.iter() {
            graph
                .edges
                .entry(Type(ty))
                .or_default()
                .extend(etas_types::type_dependencies(value).into_iter().map(Type));
        }
        let effect_output = etas_effects::EffectOutput {
            facts: checked.effects.as_ref().clone(),
            diagnostics: Vec::new(),
        };
        let body_effects = etas_effects::BodyEffectOutput::project(
            &effect_output,
            &checked.hir,
            checked.types.item_signatures.keys().copied(),
        )?;
        for (item, declaration) in checked.hir.items.iter() {
            let mut dependencies = BTreeSet::new();
            if matches!(
                declaration,
                HirItem::Flow(_) | HirItem::Tool(_) | HirItem::Agent(_) | HirItem::TopLevelLet(_)
            ) {
                dependencies.extend(
                    checked
                        .direct_checked_dependencies(item)
                        .map_err(|error| error.to_string())?
                        .into_iter()
                        .map(Definition),
                );
            }
            let mut specs = checked
                .types
                .body_spec_dependencies
                .get(&item)
                .cloned()
                .unwrap_or_default();
            if let Some(body) = body_effects.get(&item) {
                let (types, source_specs) = etas_types::semantic_type_references(body);
                dependencies.extend(types.into_iter().map(Type));
                specs.extend(source_specs);
            }
            if let Some(roots) = checked.types.body_type_dependencies.get(&item) {
                dependencies.extend(roots.iter().copied().map(Type));
            }
            if let Some(signature) = checked.types.item_signatures.get(&item) {
                let (types, source_specs) = etas_types::semantic_type_references(signature);
                dependencies.extend(types.into_iter().map(Type));
                specs.extend(source_specs);
            }
            if let Some(fact) = declaration
                .symbol()
                .and_then(|symbol| checked.types.symbol_types.get(&symbol))
            {
                dependencies.extend(etas_types::type_dependencies(fact).into_iter().map(Type));
                let constructor = match fact {
                    etas_types::SymbolTypeFact::Type { constructor }
                    | etas_types::SymbolTypeFact::NominalType { constructor, .. } => {
                        Some(TypeId(constructor.0))
                    }
                    _ => None,
                };
                if let Some(ty) = constructor {
                    graph
                        .edges
                        .entry(Type(ty))
                        .or_default()
                        .insert(Definition(item));
                }
            }
            for symbol in specs {
                if let Some(item) = checked
                    .hir
                    .symbols
                    .get(symbol)
                    .and_then(|symbol| symbol.defining_item)
                {
                    dependencies.insert(Definition(item));
                }
            }
            graph
                .edges
                .entry(Definition(item))
                .or_default()
                .extend(dependencies);
        }
        Ok(graph)
    }
    pub fn closure(
        &self,
        roots: impl IntoIterator<Item = SemanticDependency>,
    ) -> BTreeSet<SemanticDependency> {
        let mut pending = roots.into_iter().collect::<Vec<_>>();
        let mut found = BTreeSet::new();
        while let Some(node) = pending.pop() {
            if found.insert(node) {
                pending.extend(self.edges.get(&node).into_iter().flatten().copied());
            }
        }
        found
    }
}
