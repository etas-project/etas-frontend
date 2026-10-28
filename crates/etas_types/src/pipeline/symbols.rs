use std::collections::{HashMap, HashSet};

use etas_hir::{HirProgram, ResolveResult, SymbolDef, SymbolId};

use crate::{SymbolTypeFact, TypeFacts, TypeId};

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
pub struct TypeSymbolIndex {
    import_targets: HashMap<SymbolId, SymbolId>,
    canonical_paths: HashMap<Vec<String>, SymbolId>,
}

impl TypeSymbolIndex {
    pub fn build(hir: &HirProgram) -> Self {
        let mut index = Self::default();
        index.import_targets.extend(
            hir.source_import_targets
                .iter()
                .map(|(alias, target)| (*alias, *target)),
        );
        for symbol in hir.symbols.iter() {
            if hir.agent_methods.contains_key(&symbol.id) {
                continue;
            }
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

        // Public re-export bindings are namespace entries too. Private imports
        // stay local to their source part; they are followed through import_targets.
        for (_, module) in hir.modules_arena.iter() {
            let module_path = module
                .name
                .as_ref()
                .map(|name| {
                    name.segments
                        .iter()
                        .map(|segment| segment.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            for import in &module.imports {
                if import.visibility != etas_hir::Visibility::Public {
                    continue;
                }
                if let Some(binding) = &import.binding {
                    let mut path = module_path.clone();
                    path.push(binding.local_name.clone());
                    index.canonical_paths.entry(path).or_insert(binding.symbol);
                }
            }
        }

        for method in hir.agent_methods.values() {
            let Some(owner) = hir.symbols.get(method.agent) else {
                continue;
            };
            let Some(module) = hir.modules_arena.get(owner.defining_module) else {
                continue;
            };
            let mut path = module
                .name
                .as_ref()
                .map(|name| {
                    name.segments
                        .iter()
                        .map(|segment| segment.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            path.push(owner.name.clone());
            if let Some(symbol) = hir.symbols.get(method.symbol) {
                path.push(symbol.name.clone());
                if hir.agent_method_is_inherent(method) {
                    index.canonical_paths.insert(path, method.symbol);
                }
            }
        }

        for module in hir.modules_arena.iter().map(|(_, module)| module) {
            for import in &module.imports {
                let Some(binding) = &import.binding else {
                    continue;
                };
                if hir.source_import_targets.contains_key(&binding.symbol) {
                    continue;
                }
                if let Some(symbol) = hir.symbols.get(binding.symbol)
                    && let SymbolDef::ImportAlias { path, .. } = &symbol.def
                    && let Some(target) = index.canonical_paths.get(path)
                {
                    if binding.symbol != *target {
                        index.import_targets.insert(binding.symbol, *target);
                    }
                    continue;
                }
                if let ResolveResult::Resolved(target) = import.target.resolution {
                    if binding.symbol != target {
                        index.import_targets.insert(binding.symbol, target);
                    }
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
        SymbolTypeFact::Agent { signature } => Some(signature.component),
        SymbolTypeFact::Type { constructor } | SymbolTypeFact::NominalType { constructor, .. } => {
            Some(TypeId(constructor.0))
        }
        SymbolTypeFact::TypeAlias { target, .. } => Some(*target),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_import_binding_does_not_become_a_self_cycle() {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
            etas_core::SourceId(0),
            None,
            "module app; import std.io.println; flow main() -> unit { return; }",
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let mut hir = etas_hir::lower_program(&parsed.value);
        let binding = hir
            .modules_arena
            .iter_mut()
            .find_map(|(_, module)| {
                let import = module.imports.first_mut()?;
                let binding = import.binding.as_ref()?.symbol;
                import.target.resolution = ResolveResult::Resolved(binding);
                Some(binding)
            })
            .unwrap();
        assert_eq!(
            TypeSymbolIndex::build(&hir).canonical_symbol(&hir, binding),
            Some(binding)
        );
    }

    #[test]
    fn a_source_import_is_followed_to_its_actual_declaration() {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
            etas_core::SourceId(0),
            None,
            "module app; import std.io.println; flow local() -> unit { return; }",
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let mut hir = etas_hir::lower_program(&parsed.value);
        let alias = hir
            .modules_arena
            .iter()
            .find_map(|(_, module)| {
                module
                    .imports
                    .first()?
                    .binding
                    .as_ref()
                    .map(|binding| binding.symbol)
            })
            .unwrap();
        let item = hir
            .symbols
            .iter()
            .find(|symbol| symbol.name == "local")
            .unwrap()
            .id;
        hir.source_import_targets.insert(alias, item);
        assert_eq!(
            TypeSymbolIndex::build(&hir).canonical_symbol(&hir, alias),
            Some(item)
        );
    }
}
