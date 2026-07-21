use std::collections::{HashMap, HashSet, VecDeque};

use etas_core::{Diagnostic, Span, SyntaxDiagnosticCode, TextSize};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::{
    AffectedModuleSet, BODY_UNIT_KIND, ITEM_UNIT_KIND, ImportEdge, ImportGraph, ImportTarget,
    MODULE_PART_UNIT_KIND, MODULE_UNIT_KIND, ModuleId, ModulePath, ModuleTopoOrder, ProjectContext,
    ResolvedModuleTarget, UnitKind, UnitTarget,
};

use crate::passes::artifacts::{
    AFFECTED_MODULE_SET, DIAGNOSTICS, IMPORT_GRAPH, MODULE_INDEX, MODULE_TOPO_ORDER,
    RESOLVED_IMPORTS, UNIT_TREE,
};

pub struct BuildImportGraphPass;

impl Pass<ProjectContext> for BuildImportGraphPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("BuildImportGraphPass", PassKind::Analysis)
            .requires(ArtifactSet::from([MODULE_INDEX, RESOLVED_IMPORTS]))
            .produces(ArtifactSet::one(IMPORT_GRAPH))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let modules = context.modules.as_ref().expect("module index should exist");
        let resolved_imports = context
            .resolved_imports
            .as_ref()
            .expect("resolved import targets should exist");
        let mut graph = ImportGraph::default();

        for import in &resolved_imports.imports {
            let Some((candidate, resolved)) = source_edge_target(modules, &import.target) else {
                continue;
            };
            push_source_edge(
                &mut graph,
                import.from,
                import.from_part,
                candidate,
                resolved,
                import.import.clone(),
                import.span,
            );
        }

        for import in &resolved_imports.wildcard_imports {
            let Some((candidate, resolved)) =
                source_module_edge_target(modules, &import.target_module)
            else {
                continue;
            };
            push_source_edge(
                &mut graph,
                import.from,
                import.from_part,
                candidate,
                resolved,
                import.import.clone(),
                import.span,
            );
        }

        context.import_graph = Some(graph);
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(IMPORT_GRAPH))
    }
}

fn push_source_edge(
    graph: &mut ImportGraph,
    from: ModuleId,
    from_part: crate::ModulePartId,
    candidate: ModulePath,
    resolved: ModuleId,
    import: crate::AstImportRef,
    span: Span,
) {
    let edge_index = graph.edges.len();
    graph.edges.push(ImportEdge {
        from,
        from_part,
        candidate,
        resolved: Some(resolved),
        import,
        span,
    });
    graph
        .reverse_edges
        .entry(resolved)
        .or_default()
        .push(edge_index);
}

fn source_edge_target(
    modules: &crate::ModuleIndex,
    target: &ImportTarget,
) -> Option<(ModulePath, ModuleId)> {
    match target {
        ImportTarget::Module(module) => source_module_edge_target(modules, module),
        ImportTarget::SourceItem { module, .. } => {
            source_module_path(modules, *module).map(|path| (path, *module))
        }
        ImportTarget::StdItem { .. } | ImportTarget::ExternalItem { .. } => None,
    }
}

fn source_module_edge_target(
    modules: &crate::ModuleIndex,
    target: &ResolvedModuleTarget,
) -> Option<(ModulePath, ModuleId)> {
    match target {
        ResolvedModuleTarget::Source { module, .. } => {
            source_module_path(modules, *module).map(|path| (path, *module))
        }
        ResolvedModuleTarget::Std { .. } | ResolvedModuleTarget::External { .. } => None,
    }
}

fn source_module_path(modules: &crate::ModuleIndex, module: ModuleId) -> Option<ModulePath> {
    modules
        .modules
        .get(module)
        .map(|module| module.path.clone())
}

pub struct DetectImportCyclesPass;

impl Pass<ProjectContext> for DetectImportCyclesPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("DetectImportCyclesPass", PassKind::Analysis)
            .requires(ArtifactSet::one(IMPORT_GRAPH))
            .produces(ArtifactSet::one(DIAGNOSTICS))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let graph = context
            .import_graph
            .as_ref()
            .expect("import graph should exist");
        let mut adjacency = HashMap::<ModuleId, Vec<(ModuleId, usize)>>::new();
        for (edge_index, edge) in graph.edges.iter().enumerate() {
            if let Some(to) = edge.resolved {
                adjacency
                    .entry(edge.from)
                    .or_default()
                    .push((to, edge_index));
            }
        }
        for edges in adjacency.values_mut() {
            edges.sort_by_key(|(module, _)| module.0);
        }

        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        let mut stack = Vec::<(ModuleId, Option<usize>)>::new();
        let mut cycle_edges = HashSet::<usize>::new();
        let mut modules = adjacency.keys().copied().collect::<Vec<_>>();
        modules.sort_by_key(|module| module.0);

        for module in modules {
            detect_cycles_from(
                module,
                None,
                &adjacency,
                &mut visiting,
                &mut visited,
                &mut stack,
                &mut cycle_edges,
            );
        }

        let mut diagnostics = cycle_edges
            .into_iter()
            .map(|edge_index| {
                let edge = &graph.edges[edge_index];
                project_diagnostic(
                    edge.span,
                    &format!(
                        "import cycle detected involving module `{}`",
                        module_path_text(&edge.candidate)
                    ),
                )
            })
            .collect::<Vec<_>>();
        diagnostics.sort_by_key(|diagnostic| diagnostic.primary.span.range.start);
        context.diagnostics.extend(diagnostics);
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::new())
    }
}

pub struct ComputeModuleTopoOrderPass;

impl Pass<ProjectContext> for ComputeModuleTopoOrderPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ComputeModuleTopoOrderPass", PassKind::Analysis)
            .requires(ArtifactSet::from([MODULE_INDEX, IMPORT_GRAPH]))
            .produces(ArtifactSet::one(MODULE_TOPO_ORDER))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let modules = context.modules.as_ref().expect("module index should exist");
        let graph = context
            .import_graph
            .as_ref()
            .expect("import graph should exist");
        let mut indegree = modules
            .modules
            .iter()
            .map(|(module, _)| (module, 0usize))
            .collect::<HashMap<_, _>>();
        let mut dependents = HashMap::<ModuleId, Vec<ModuleId>>::new();

        for edge in &graph.edges {
            if let Some(dependency) = edge.resolved {
                if dependency == edge.from {
                    continue;
                }
                *indegree.entry(edge.from).or_default() += 1;
                dependents.entry(dependency).or_default().push(edge.from);
            }
        }
        for modules in dependents.values_mut() {
            modules.sort_by_key(|module| module.0);
            modules.dedup();
        }

        let mut ready = indegree
            .iter()
            .filter_map(|(module, indegree)| (*indegree == 0).then_some(*module))
            .collect::<Vec<_>>();
        ready.sort_by_key(|module| module.0);
        let mut ordered = Vec::new();

        while let Some(module) = ready.first().copied() {
            ready.remove(0);
            ordered.push(module);
            for dependent in dependents.get(&module).into_iter().flatten() {
                let count = indegree
                    .get_mut(dependent)
                    .expect("dependent should have an indegree entry");
                *count -= 1;
                if *count == 0 {
                    ready.push(*dependent);
                    ready.sort_by_key(|module| module.0);
                }
            }
        }

        let mut cyclic_modules = indegree
            .into_iter()
            .filter_map(|(module, indegree)| (indegree > 0).then_some(module))
            .collect::<Vec<_>>();
        cyclic_modules.sort_by_key(|module| module.0);
        ordered.extend(cyclic_modules.iter().copied());

        context.module_topo_order = Some(ModuleTopoOrder {
            modules: ordered,
            cyclic_modules,
        });
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(MODULE_TOPO_ORDER))
    }
}

pub struct ComputeAffectedModulesPass;

impl Pass<ProjectContext> for ComputeAffectedModulesPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ComputeAffectedModulesPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                MODULE_INDEX,
                UNIT_TREE,
                IMPORT_GRAPH,
                MODULE_TOPO_ORDER,
            ]))
            .produces(ArtifactSet::one(AFFECTED_MODULE_SET))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let modules = context.modules.as_ref().expect("module index should exist");
        let units = context.units.as_ref().expect("unit tree should exist");
        let order = context
            .module_topo_order
            .as_ref()
            .expect("module topo order should exist");
        let graph = context
            .import_graph
            .as_ref()
            .expect("import graph should exist");

        let affected_module_ids = if context.incremental && !context.project_wide_change {
            affected_module_closure(modules, graph, order, &context.changed_sources)
        } else {
            order.modules.clone()
        };

        let mut affected_modules = module_units(units, &affected_module_ids);
        dedup_units(&mut affected_modules);

        let mut affected_parts = module_part_units(modules, units, &affected_module_ids);
        dedup_units(&mut affected_parts);

        let mut affected_items = item_units_for_modules(units, &affected_module_ids);
        affected_items.sort_by_key(|unit| unit.id);
        affected_items.dedup();

        let mut affected_bodies = body_units_for_modules(units, &affected_module_ids);
        affected_bodies.sort_by_key(|unit| unit.id);
        affected_bodies.dedup();

        context.affected_modules = Some(AffectedModuleSet {
            modules: affected_modules,
            module_parts: affected_parts,
            items: affected_items,
            bodies: affected_bodies,
        });
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(AFFECTED_MODULE_SET),
        )
    }
}

fn affected_module_closure(
    modules: &crate::ModuleIndex,
    graph: &ImportGraph,
    order: &ModuleTopoOrder,
    changed_sources: &[etas_core::SourceId],
) -> Vec<ModuleId> {
    let mut seen = HashSet::new();
    let mut queue = changed_sources
        .iter()
        .filter_map(|source| modules.by_source.get(source).copied())
        .collect::<VecDeque<_>>();
    while let Some(module) = queue.pop_front() {
        if !seen.insert(module) {
            continue;
        }
        if let Some(edges) = graph.reverse_edges.get(&module) {
            for edge in edges {
                if let Some(importer) = graph.edges.get(*edge).map(|edge| edge.from) {
                    queue.push_back(importer);
                }
            }
        }
    }
    order
        .modules
        .iter()
        .copied()
        .filter(|module| seen.contains(module))
        .collect()
}

fn module_units(units: &crate::UnitTree, modules: &[ModuleId]) -> Vec<etas_utils::UnitKey> {
    modules
        .iter()
        .filter_map(|module| {
            units
                .by_target
                .get(&UnitTarget::Module(*module))
                .map(|unit| etas_utils::UnitKey::new(MODULE_UNIT_KIND, unit.0 as u64))
        })
        .collect()
}

fn module_part_units(
    modules: &crate::ModuleIndex,
    units: &crate::UnitTree,
    affected_modules: &[ModuleId],
) -> Vec<etas_utils::UnitKey> {
    affected_modules
        .iter()
        .filter_map(|module| modules.modules.get(*module))
        .flat_map(|module| module.parts.iter())
        .filter_map(|part| {
            units
                .by_target
                .get(&UnitTarget::ModulePart(*part))
                .map(|unit| etas_utils::UnitKey::new(MODULE_PART_UNIT_KIND, unit.0 as u64))
        })
        .collect()
}

fn body_units_for_modules(
    units: &crate::UnitTree,
    affected_modules: &[ModuleId],
) -> Vec<etas_utils::UnitKey> {
    units_for_modules(units, affected_modules, UnitKind::Body, BODY_UNIT_KIND)
}

fn item_units_for_modules(
    units: &crate::UnitTree,
    affected_modules: &[ModuleId],
) -> Vec<etas_utils::UnitKey> {
    units_for_modules(units, affected_modules, UnitKind::Item, ITEM_UNIT_KIND)
}

fn units_for_modules(
    units: &crate::UnitTree,
    affected_modules: &[ModuleId],
    kind: UnitKind,
    unit_kind: etas_utils::UnitKindKey,
) -> Vec<etas_utils::UnitKey> {
    let affected = affected_modules
        .iter()
        .filter_map(|module| units.by_target.get(&UnitTarget::Module(*module)).copied())
        .collect::<HashSet<_>>();
    units
        .nodes
        .iter()
        .filter_map(|(unit, node)| {
            (node.kind == kind
                && module_ancestor(units, node.parent)
                    .is_some_and(|module| affected.contains(&module)))
            .then_some(etas_utils::UnitKey::new(unit_kind, unit.0 as u64))
        })
        .collect()
}

fn module_ancestor(
    units: &crate::UnitTree,
    mut current: Option<crate::UnitId>,
) -> Option<crate::UnitId> {
    while let Some(unit) = current {
        let node = units.nodes.get(unit)?;
        if node.kind == UnitKind::Module {
            return Some(unit);
        }
        current = node.parent;
    }
    None
}

fn dedup_units(units: &mut Vec<etas_utils::UnitKey>) {
    let mut seen = HashSet::new();
    units.retain(|unit| seen.insert(*unit));
}

fn detect_cycles_from(
    module: ModuleId,
    incoming_edge: Option<usize>,
    adjacency: &HashMap<ModuleId, Vec<(ModuleId, usize)>>,
    visiting: &mut HashSet<ModuleId>,
    visited: &mut HashSet<ModuleId>,
    stack: &mut Vec<(ModuleId, Option<usize>)>,
    cycle_edges: &mut HashSet<usize>,
) {
    if visited.contains(&module) {
        return;
    }
    if visiting.contains(&module) {
        return;
    }
    visiting.insert(module);
    stack.push((module, incoming_edge));

    for (next, edge_index) in adjacency.get(&module).into_iter().flatten() {
        if let Some(cycle_start) = stack.iter().position(|(stacked, _)| stacked == next) {
            cycle_edges.insert(*edge_index);
            for (_, incoming_edge) in stack.iter().skip(cycle_start + 1) {
                if let Some(incoming_edge) = incoming_edge {
                    cycle_edges.insert(*incoming_edge);
                }
            }
            continue;
        }
        detect_cycles_from(
            *next,
            Some(*edge_index),
            adjacency,
            visiting,
            visited,
            stack,
            cycle_edges,
        );
    }

    stack.pop();
    visiting.remove(&module);
    visited.insert(module);
}

fn project_diagnostic(span: Span, message: &str) -> Diagnostic {
    let span = if span.range.start == span.range.end {
        Span::empty(span.source, TextSize::ZERO)
    } else {
        span
    };
    Diagnostic::syntax(SyntaxDiagnosticCode::InvalidItem, span, message)
}

fn module_path_text(path: &ModulePath) -> String {
    path.segments.join(".")
}
