use std::collections::{HashMap, HashSet};

use etas_hir::{HirProgram, ResolveResult, SymbolDef, SymbolId};

use crate::{SymbolTypeFact, TypeFacts, TypeId};

#[derive(Clone, Debug, Default)]
pub struct TypeSymbolIndex {
    import_targets: HashMap<SymbolId, SymbolId>,
    canonical_paths: HashMap<Vec<String>, SymbolId>,
}

impl TypeSymbolIndex {
    pub fn build(hir: &HirProgram) -> Self {
        let mut index = Self::default();
        for symbol in hir.symbols.iter() {
            if !is_canonical_item_symbol(symbol.kind) {
                continue;
            }
            let mut path = hir
                .modules_arena
                .get(symbol.defining_module)
                .and_then(|module| module.name.as_ref())
                .map(|name| {
                    name.segments
                        .iter()
                        .map(|segment| segment.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            path.push(symbol_local_name(&symbol.name).to_owned());
            index.canonical_paths.entry(path).or_insert(symbol.id);
        }

        for module in hir.modules_arena.iter().map(|(_, module)| module) {
            for import in &module.imports {
                let Some(binding) = &import.binding else {
                    continue;
                };
                if let Some(symbol) = hir.symbols.get(binding.symbol)
                    && let SymbolDef::ImportAlias { path, .. } = &symbol.def
                    && let Some(target) = index.canonical_paths.get(path)
                {
                    index.import_targets.insert(binding.symbol, *target);
                    continue;
                }
                if let ResolveResult::Resolved(target) = import.target.resolution {
                    index.import_targets.insert(binding.symbol, target);
                    continue;
                }
            }
        }

        index
    }

    pub fn canonical_symbol(&self, hir: &HirProgram, symbol: SymbolId) -> Option<SymbolId> {
        let mut current = symbol;
        let mut visited = HashSet::new();
        loop {
            if !visited.insert(current) {
                return None;
            }
            if let Some(target) = self.import_targets.get(&current).copied() {
                current = target;
                continue;
            }
            let Some(symbol_data) = hir.symbols.get(current) else {
                return Some(current);
            };
            let SymbolDef::ImportAlias { path, .. } = &symbol_data.def else {
                return Some(current);
            };
            let Some(target) = self.canonical_paths.get(path).copied() else {
                return Some(current);
            };
            current = target;
        }
    }

    pub fn symbol_fact<'a>(
        &self,
        hir: &HirProgram,
        facts: &'a TypeFacts,
        symbol: SymbolId,
    ) -> Option<&'a SymbolTypeFact> {
        let canonical = self.canonical_symbol(hir, symbol).unwrap_or(symbol);
        facts
            .symbol_types
            .get(&canonical)
            .or_else(|| facts.symbol_types.get(&symbol))
    }

    pub fn type_fact_as_type_id(
        &self,
        hir: &HirProgram,
        facts: &TypeFacts,
        symbol: SymbolId,
    ) -> Option<TypeId> {
        symbol_type_fact_as_type_id(self.symbol_fact(hir, facts, symbol)?)
    }
}

fn symbol_local_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

fn is_canonical_item_symbol(kind: etas_hir::SymbolKind) -> bool {
    matches!(
        kind,
        etas_hir::SymbolKind::TypeAlias
            | etas_hir::SymbolKind::Type
            | etas_hir::SymbolKind::Enum
            | etas_hir::SymbolKind::Spec
            | etas_hir::SymbolKind::EnumVariant
            | etas_hir::SymbolKind::Flow
            | etas_hir::SymbolKind::Agent
            | etas_hir::SymbolKind::Tool
            | etas_hir::SymbolKind::TopLevelLet
            | etas_hir::SymbolKind::Protocol
            | etas_hir::SymbolKind::Effect
            | etas_hir::SymbolKind::EffectAction
    )
}

pub fn symbol_type_fact_as_type_id(fact: &SymbolTypeFact) -> Option<TypeId> {
    match fact {
        SymbolTypeFact::Type { constructor } | SymbolTypeFact::NominalType { constructor, .. } => {
            Some(TypeId(constructor.0))
        }
        SymbolTypeFact::TypeAlias { target, .. } => Some(*target),
        _ => None,
    }
}
