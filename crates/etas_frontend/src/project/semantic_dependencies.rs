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
        let mut storage_callbacks = BTreeMap::<_, BTreeSet<_>>::new();
        for (method, _) in checked.types.storage_methods() {
            if let etas_types::method::MethodTarget::Source { callable, .. } = method.target {
                storage_callbacks
                    .entry(method.name.as_str())
                    .or_default()
                    .insert(Definition(callable));
            }
        }
        for (ty, value) in checked.type_store.iter() {
            graph
                .edges
                .entry(Type(ty))
                .or_default()
                .extend(etas_types::type_dependencies(value).into_iter().map(Type));
        }
        let body_effects = etas_effects::BodyEffectOutput::project_facts(
            &checked.effects,
            &checked.hir,
            checked.types.item_signatures.keys().copied(),
        )?;
        for (item, declaration) in checked.hir.items.iter() {
            let mut dependencies = BTreeSet::new();
            // The storage ABI dispatches logical collection traversal to checked
            // Etas adapters. These executable edges have no authored call site.
            if let HirItem::Flow(flow) = declaration
                && let etas_hir::HirFlowBody::Runtime { role, .. } = flow.body
            {
                let callback = match role {
                    etas_std::StdExecutionRole::CaptureStorage => {
                        Some(etas_std::SOURCE_STORAGE_VIEW)
                    }
                    etas_std::StdExecutionRole::RestoreStorage => {
                        Some(etas_std::SOURCE_STORAGE_BUILD)
                    }
                    _ => None,
                };
                if let Some(callbacks) = callback.and_then(|name| storage_callbacks.get(name)) {
                    dependencies.extend(callbacks);
                }
            }
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
        // These facts include action/default Effect signatures and generic
        // bounds owned by symbols nested inside their enclosing declaration.
        for symbol in checked.hir.symbols.iter() {
            let Some(owner) = symbol.defining_item else {
                continue;
            };
            if let Some(fact) = checked.types.symbol_types.get(&symbol.id) {
                graph.record(Definition(owner), checked, fact);
            }
            if let Some(bounds) = checked.types.type_param_bounds.get(&symbol.id) {
                graph.record(Definition(owner), checked, bounds);
            }
            if let Some(signature) = checked.types.spec_signatures.get(&symbol.id) {
                graph.record(Definition(owner), checked, signature);
            }
        }
        for (ty, layout) in &checked.types.enum_layouts {
            graph.record(Type(*ty), checked, layout);
        }
        for (ty, parameters) in &checked.types.type_generics {
            graph.record(Type(*ty), checked, parameters);
        }
        for method in checked.types.methods.entries() {
            if let etas_types::method::MethodTarget::Source { callable, .. }
            | etas_types::method::MethodTarget::Agent { callable, .. } = method.target
            {
                graph.record(Definition(callable), checked, method);
            }
        }
        for (method, _) in checked.types.value_equality_methods() {
            if let etas_types::method::MethodTarget::Source { callable, .. } = method.target {
                let owner = match checked.type_store.get(method.receiver_type) {
                    Some(etas_types::Type::Applied { constructor, .. }) => TypeId(constructor.0),
                    _ => method.receiver_type,
                };
                graph
                    .edges
                    .entry(Type(owner))
                    .or_default()
                    .insert(Definition(callable));
            }
        }
        let index = checked.hir.index();
        for (expression, action) in &checked.types.action_calls {
            if let Some(owner) = index.expr_item.get(expression) {
                graph.symbol_reference(*owner, action.action_symbol, checked);
            }
        }
        for (arm, action) in &checked.types.handler_actions {
            if let Some(etas_hir::HirOwner::Expr(expression)) = index.handler_arm_owner.get(arm)
                && let Some(owner) = index.expr_item.get(expression)
            {
                graph.symbol_reference(*owner, action.action_symbol, checked);
            }
        }
        Ok(graph)
    }
    fn symbol_reference(
        &mut self,
        owner: HirItemId,
        symbol: etas_hir::SymbolId,
        checked: &crate::CheckedProgramData,
    ) {
        let symbol = checked
            .hir
            .canonical_source_symbol(symbol)
            .unwrap_or(symbol);
        if let Some(target) = checked
            .hir
            .symbols
            .get(symbol)
            .and_then(|symbol| symbol.defining_item)
        {
            self.edges
                .entry(SemanticDependency::Definition(owner))
                .or_default()
                .insert(SemanticDependency::Definition(target));
        }
    }
    fn record(
        &mut self,
        owner: SemanticDependency,
        checked: &crate::CheckedProgramData,
        fact: &impl serde::Serialize,
    ) {
        let (types, specs) = etas_types::semantic_type_references(fact);
        let dependencies = self.edges.entry(owner).or_default();
        dependencies.extend(types.into_iter().map(SemanticDependency::Type));
        dependencies.extend(
            specs
                .into_iter()
                .filter_map(|symbol| checked.hir.symbols.get(symbol)?.defining_item)
                .map(SemanticDependency::Definition),
        );
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
