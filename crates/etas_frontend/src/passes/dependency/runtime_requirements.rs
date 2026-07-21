use std::collections::{BTreeMap, BTreeSet};

use etas_hir::{
    ExprView, HirExpr, HirExprId, HirItemId, HirTreeView, HirVisitor, SymbolDef, walk_item,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{
    HIR_OUTPUT, PROJECT_ENTRY, REACHABILITY_FACTS, RESOLVED_PATHS, RUNTIME_SOURCE_REQUIREMENTS,
};
use crate::{
    AstBodyRef, ExternalPackageId, HirItemBindings, HirPathResolution, ImportTarget, ModulePath,
    ProjectContext, ProjectExternalPackageInput, ProjectExternalPublicMetadataInput,
    ReachabilityFacts, ResolvedModuleTarget, RuntimeSourceReason, RuntimeSourceReasonKind,
    RuntimeSourceRequirement, RuntimeSourceRequirements, UnitTarget,
};

pub struct ComputeEntryReachabilityPass;

impl Pass<ProjectContext> for ComputeEntryReachabilityPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ComputeEntryReachabilityPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                RESOLVED_PATHS,
                PROJECT_ENTRY,
            ]))
            .produces(ArtifactSet::from([
                REACHABILITY_FACTS,
                RUNTIME_SOURCE_REQUIREMENTS,
            ]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let reachability = compute_reachability(context);
        context.runtime_source_requirements =
            Some(reachability.runtime_source_requirements.clone());
        context.reachability = Some(reachability);
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::from([REACHABILITY_FACTS, RUNTIME_SOURCE_REQUIREMENTS]),
        )
    }
}

fn compute_reachability(context: &ProjectContext) -> ReachabilityFacts {
    let mut facts = ReachabilityFacts::default();
    let Some(reachable) = entry_reachability(context) else {
        return facts;
    };
    facts.entry_item = Some(reachable.entry);
    facts.reachable_items = reachable.items;
    facts.reachable_bodies = reachable_bodies(context, &facts.reachable_items);
    facts.reachable_modules = reachable_modules(context, &facts.reachable_items);

    let Some(resolved_paths) = context.resolved_paths.as_ref() else {
        return facts;
    };
    let mut planner = RuntimeSourceRequirementPlanner {
        packages: context
            .input
            .environment
            .external_packages
            .iter()
            .map(|package| (package.id, package))
            .collect(),
        metadata: context
            .input
            .environment
            .external_public_metadata
            .iter()
            .map(|metadata| (metadata.package, metadata))
            .collect(),
        tool_bindings: context
            .input
            .environment
            .tool_bindings
            .iter()
            .map(|binding| binding.tool.as_str())
            .collect(),
        requirements: RuntimeSourceRequirements::default(),
    };

    for expr in &resolved_paths.expr_paths {
        if !reachable.exprs.contains(&expr.expr) {
            continue;
        }
        match &expr.result {
            HirPathResolution::ExplicitImport { target }
            | HirPathResolution::PartialImport { target, .. } => {
                planner.collect_import_target(target);
            }
            HirPathResolution::WildcardImport { target } => {
                planner.collect_module_target(target);
            }
            HirPathResolution::AmbiguousWildcard { targets } => {
                for target in targets {
                    planner.collect_module_target(target);
                }
            }
            _ => {}
        }
    }
    facts.runtime_source_requirements = planner.requirements;
    facts
}

struct RuntimeSourceRequirementPlanner<'a> {
    packages: BTreeMap<ExternalPackageId, &'a ProjectExternalPackageInput>,
    metadata: BTreeMap<ExternalPackageId, &'a ProjectExternalPublicMetadataInput>,
    tool_bindings: BTreeSet<&'a str>,
    requirements: RuntimeSourceRequirements,
}

impl RuntimeSourceRequirementPlanner<'_> {
    fn collect_import_target(&mut self, target: &ImportTarget) {
        let ImportTarget::ExternalItem {
            package,
            module_path,
            name,
            ..
        } = target
        else {
            return;
        };
        let Some(package) = self.package_for_external_path(*package, &module_path.segments) else {
            return;
        };
        let mut item_path = module_path.segments.clone();
        item_path.push(name.clone());
        let Some(runtime_item_path) = self.resolve_runtime_item_path(package.id, &item_path) else {
            return;
        };
        self.add_seed_module(
            package.id,
            package.import_root.clone(),
            path_parent(&runtime_item_path),
            RuntimeSourceReason {
                item_path: runtime_item_path.clone(),
                module_path: ModulePath {
                    segments: path_parent(&runtime_item_path),
                },
                kind: RuntimeSourceReasonKind::ExternalRuntimeItem,
            },
        );
    }

    fn collect_module_target(&mut self, target: &ResolvedModuleTarget) {
        let ResolvedModuleTarget::External { package, path, .. } = target else {
            return;
        };
        let Some(package) = self.package_for_external_path(*package, &path.segments) else {
            return;
        };
        if !self.module_has_runtime_items(package.id, &path.segments) {
            return;
        }
        self.add_seed_module(
            package.id,
            package.import_root.clone(),
            path.segments.clone(),
            RuntimeSourceReason {
                item_path: path.segments.clone(),
                module_path: path.clone(),
                kind: RuntimeSourceReasonKind::ExternalRuntimeModule,
            },
        );
    }

    fn package_for_external_path(
        &self,
        package: Option<ExternalPackageId>,
        path: &[String],
    ) -> Option<&ProjectExternalPackageInput> {
        if let Some(package) = package {
            return self.packages.get(&package).copied();
        }
        self.packages.values().copied().find(|package| {
            let root = import_root_segments(&package.import_root);
            !root.is_empty() && path.starts_with(&root)
        })
    }

    fn add_seed_module(
        &mut self,
        package: ExternalPackageId,
        import_root: String,
        module: Vec<String>,
        reason: RuntimeSourceReason,
    ) {
        let module_path = ModulePath { segments: module };
        let requirement = self
            .requirements
            .dependencies
            .entry(package)
            .or_insert_with(|| RuntimeSourceRequirement {
                package,
                import_root,
                seed_modules: BTreeSet::new(),
                required_modules: BTreeSet::new(),
                reasons: Vec::new(),
            });
        requirement.seed_modules.insert(module_path.clone());
        requirement.required_modules.insert(module_path);
        requirement.reasons.push(reason);
    }

    fn resolve_runtime_item_path(
        &self,
        package: ExternalPackageId,
        item_path: &[String],
    ) -> Option<Vec<String>> {
        let mut current = item_path.to_vec();
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(current.clone()) {
                return None;
            }
            if let Some(re_export) = self
                .metadata
                .get(&package)?
                .re_exports
                .iter()
                .find(|re_export| re_export.exported == current)
            {
                current = re_export.from.clone();
                continue;
            }
            if self.runtime_item_requires_source(package, &current) {
                return Some(current);
            }
            return None;
        }
    }

    fn runtime_item_requires_source(
        &self,
        package: ExternalPackageId,
        item_path: &[String],
    ) -> bool {
        let Some(metadata) = self.metadata.get(&package) else {
            return false;
        };
        metadata
            .flows
            .iter()
            .any(|signature| signature.visibility == "public" && signature.path == item_path)
            || metadata
                .agents
                .iter()
                .any(|signature| signature.visibility == "public" && signature.path == item_path)
            || metadata.tools.iter().any(|signature| {
                signature.visibility == "public"
                    && signature.path == item_path
                    && !self.tool_has_provider_binding(&signature.path)
            })
            || metadata
                .values
                .iter()
                .any(|signature| signature.visibility == "public" && signature.path == item_path)
    }

    fn module_has_runtime_items(&self, package: ExternalPackageId, module_path: &[String]) -> bool {
        let Some(metadata) = self.metadata.get(&package) else {
            return false;
        };
        metadata.flows.iter().any(|signature| {
            signature.visibility == "public" && path_parent(&signature.path) == module_path
        }) || metadata.agents.iter().any(|signature| {
            signature.visibility == "public" && path_parent(&signature.path) == module_path
        }) || metadata.tools.iter().any(|signature| {
            signature.visibility == "public"
                && path_parent(&signature.path) == module_path
                && !self.tool_has_provider_binding(&signature.path)
        }) || metadata.values.iter().any(|signature| {
            signature.visibility == "public" && path_parent(&signature.path) == module_path
        })
    }

    fn tool_has_provider_binding(&self, path: &[String]) -> bool {
        self.tool_bindings.contains(path.join(".").as_str())
    }
}

fn entry_reachability(context: &ProjectContext) -> Option<EntryReachability> {
    let hir = context.hir.as_ref()?;
    let entry = context.entry.as_ref()?.resolved.as_ref()?.item;
    let resolved_paths = context.resolved_paths.as_ref()?;
    let item_bindings = context.hir_item_bindings.as_ref()?;
    let view = hir.view();
    let mut collector = ReachableExprCollector {
        hir: &hir.hir,
        resolved_paths,
        item_bindings,
        view: &view,
        visited_items: BTreeSet::new(),
        pending_items: vec![entry],
        exprs: BTreeSet::new(),
    };
    collector.collect();
    Some(EntryReachability {
        entry,
        items: collector.visited_items,
        exprs: collector.exprs,
    })
}

struct EntryReachability {
    entry: HirItemId,
    items: BTreeSet<HirItemId>,
    exprs: BTreeSet<HirExprId>,
}

fn reachable_bodies(
    context: &ProjectContext,
    reachable_items: &BTreeSet<HirItemId>,
) -> BTreeSet<crate::UnitId> {
    let Some(bindings) = context.hir_item_bindings.as_ref() else {
        return BTreeSet::new();
    };
    let Some(units) = context.units.as_ref() else {
        return BTreeSet::new();
    };
    reachable_items
        .iter()
        .filter_map(|item| bindings.hir_to_ast.get(item))
        .map(|item| AstBodyRef {
            source: item.source,
            item: item.clone(),
            span: item.span,
        })
        .filter_map(|body| units.by_target.get(&UnitTarget::AstBody(body)).copied())
        .collect()
}

fn reachable_modules(
    context: &ProjectContext,
    reachable_items: &BTreeSet<HirItemId>,
) -> BTreeSet<crate::ModuleId> {
    let Some(bindings) = context.hir_item_bindings.as_ref() else {
        return BTreeSet::new();
    };
    let Some(modules) = context.modules.as_ref() else {
        return BTreeSet::new();
    };
    reachable_items
        .iter()
        .filter_map(|item| bindings.hir_to_ast.get(item))
        .filter_map(|item| item.module_part)
        .filter_map(|part| modules.parts.get(part).map(|part| part.module))
        .collect()
}

struct ReachableExprCollector<'a> {
    hir: &'a etas_hir::HirProgram,
    resolved_paths: &'a crate::ResolvedPaths,
    item_bindings: &'a HirItemBindings,
    view: &'a HirTreeView<'a>,
    visited_items: BTreeSet<HirItemId>,
    pending_items: Vec<HirItemId>,
    exprs: BTreeSet<HirExprId>,
}

impl ReachableExprCollector<'_> {
    fn collect(&mut self) {
        while let Some(item) = self.pending_items.pop() {
            if !self.visited_items.insert(item) {
                continue;
            }
            let mut visitor = ReachableItemVisitor {
                hir: self.hir,
                resolved_paths: self.resolved_paths,
                item_bindings: self.item_bindings,
                exprs: &mut self.exprs,
                pending_items: &mut self.pending_items,
            };
            walk_item(self.view, item, &mut visitor);
        }
    }
}

struct ReachableItemVisitor<'a> {
    hir: &'a etas_hir::HirProgram,
    resolved_paths: &'a crate::ResolvedPaths,
    item_bindings: &'a HirItemBindings,
    exprs: &'a mut BTreeSet<HirExprId>,
    pending_items: &'a mut Vec<HirItemId>,
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for ReachableItemVisitor<'_> {
    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        let expr_id = expr.id();
        self.exprs.insert(expr_id);
        match self.hir.exprs.get(expr_id) {
            Some(HirExpr::Call { callee, .. }) => {
                self.enqueue_local_item_for_expr_path(*callee);
            }
            Some(HirExpr::Handle { handler, .. }) => {
                self.enqueue_local_item_for_expr_path(*handler);
            }
            Some(HirExpr::StageCompose { stages, .. }) | Some(HirExpr::Pipeline { stages, .. }) => {
                for stage in stages {
                    self.enqueue_local_item_for_expr_path(stage.expr);
                }
            }
            _ => {}
        }
    }
}

impl ReachableItemVisitor<'_> {
    fn enqueue_local_item_for_expr_path(&mut self, expr: HirExprId) {
        if let Some(item) =
            local_item_for_expr_path(self.hir, self.resolved_paths, self.item_bindings, expr)
        {
            self.pending_items.push(item);
        }
    }
}

fn local_item_for_expr_path(
    hir: &etas_hir::HirProgram,
    resolved_paths: &crate::ResolvedPaths,
    item_bindings: &HirItemBindings,
    expr: HirExprId,
) -> Option<HirItemId> {
    let mut seen = BTreeSet::new();
    local_item_for_expr_path_inner(hir, resolved_paths, item_bindings, expr, &mut seen)
}

fn local_item_for_expr_path_inner(
    hir: &etas_hir::HirProgram,
    resolved_paths: &crate::ResolvedPaths,
    item_bindings: &HirItemBindings,
    expr: HirExprId,
    seen: &mut BTreeSet<HirExprId>,
) -> Option<HirItemId> {
    if !seen.insert(expr) {
        return None;
    }
    let resolution = resolved_paths
        .expr_paths
        .iter()
        .find(|path| path.expr == expr)?;
    match &resolution.result {
        HirPathResolution::DirectSymbol(symbol) => {
            let symbol = hir.symbols.get(*symbol)?;
            match &symbol.def {
                SymbolDef::Item { item } | SymbolDef::TopLevelLet { item, .. } => Some(*item),
                SymbolDef::Local {
                    initializer: Some(initializer),
                    ..
                }
                | SymbolDef::PatternBinding {
                    initializer: Some(initializer),
                    ..
                } => local_item_for_expr_path_inner(
                    hir,
                    resolved_paths,
                    item_bindings,
                    *initializer,
                    seen,
                ),
                _ => None,
            }
        }
        HirPathResolution::ExplicitImport {
            target: ImportTarget::SourceItem { item, .. },
        }
        | HirPathResolution::PartialImport {
            target: ImportTarget::SourceItem { item, .. },
            ..
        } => item_bindings.ast_to_hir.get(item).copied(),
        _ => None,
    }
}

fn path_parent(path: &[String]) -> Vec<String> {
    path.split_last()
        .map(|(_, parent)| parent.to_vec())
        .unwrap_or_default()
}

fn import_root_segments(import_root: &str) -> Vec<String> {
    import_root
        .split('.')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect()
}
