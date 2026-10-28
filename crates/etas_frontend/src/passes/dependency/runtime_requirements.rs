use std::collections::{BTreeMap, BTreeSet};

use etas_hir::{
    ExprView, HirExpr, HirExprId, HirItem, HirItemId, HirTreeView, HirVisitor, SymbolDef,
    walk_expr, walk_item,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{
    HIR_OUTPUT, MODULE_INDEX, PROJECT_ANALYSIS_ROOTS, PROJECT_ENTRY, REACHABILITY_FACTS,
    RESOLVED_PATHS, RUNTIME_SOURCE_REQUIREMENTS, VALIDATED_EXTERNAL_ENVIRONMENT,
};
use crate::{
    ExternalPackageId, HirItemBindings, HirPathResolution, ImportTarget, ModulePath,
    ProjectContext, ProjectEnvironmentInput, ProjectExternalPackageInput,
    ProjectExternalPublicMetadataInput, ReachabilityFacts, ResolvedModuleTarget,
    RuntimeSourceReason, RuntimeSourceReasonKind, RuntimeSourceRequirement,
    RuntimeSourceRequirements, UnitTarget,
};

pub struct ComputeEntryReachabilityPass;

impl Pass<ProjectContext> for ComputeEntryReachabilityPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ComputeEntryReachabilityPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                MODULE_INDEX,
                RESOLVED_PATHS,
                PROJECT_ENTRY,
                PROJECT_ANALYSIS_ROOTS,
                VALIDATED_EXTERNAL_ENVIRONMENT,
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
        let Some(environment) = context
            .validated_external_environment
            .as_ref()
            .map(|validated| validated.environment())
        else {
            return PassResult::failed("entry reachability requires validated external metadata");
        };
        if let Some(entry) = context
            .entry
            .as_ref()
            .and_then(|entry| entry.resolved.as_ref())
            && !context.analysis_roots.contains(&entry.item)
        {
            return PassResult::failed(
                "entry reachability requires its resolved primary entry in the analysis roots",
            );
        }
        let reachability = match compute_reachability(context, environment, None) {
            Ok(reachability) => reachability,
            Err(error) => return PassResult::failed(error),
        };
        context.runtime_source_requirements =
            Some(reachability.runtime_source_requirements.clone());
        context.reachability = Some(reachability);
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::from([REACHABILITY_FACTS, RUNTIME_SOURCE_REQUIREMENTS]),
        )
    }
}

pub(super) fn compute_reachability(
    context: &ProjectContext,
    environment: &ProjectEnvironmentInput,
    types: Option<&etas_types::TypeFacts>,
) -> Result<ReachabilityFacts, String> {
    let mut facts = ReachabilityFacts::default();
    let Some(reachable) = entry_reachability(context, types, ReachabilityPurpose::Calls)? else {
        return Ok(facts);
    };
    facts.entry_item = Some(reachable.entry);
    facts.reachable_items = reachable.items;
    facts.reachable_bodies = reachable_bodies(context, &facts.reachable_items);
    facts.reachable_modules = reachable_modules(context, &facts.reachable_items);
    let linked = entry_reachability(context, types, ReachabilityPurpose::ExecutableValues)?
        .ok_or("runtime linkage lost its resolved entry")?;
    if types.is_some() {
        let analysis = context
            .reachability
            .as_ref()
            .ok_or("runtime linkage requires analysis reachability")?;
        if !linked.items.is_subset(&analysis.analysis_items)
            || !reachable_bodies(context, &linked.items).is_subset(&analysis.analysis_bodies)
        {
            return Err("runtime callable link has no checked analysis body".into());
        }
    }

    let Some(resolved_paths) = context.resolved_paths.as_ref() else {
        return Ok(facts);
    };
    let mut planner = RuntimeSourceRequirementPlanner {
        namespaces: &context
            .modules
            .as_ref()
            .ok_or("runtime source planning requires a module index")?
            .dependency_namespaces,
        packages: environment
            .external_packages
            .iter()
            .map(|package| (package.id, package))
            .collect(),
        metadata: environment
            .external_public_metadata
            .iter()
            .map(|metadata| (metadata.package, metadata))
            .collect(),
        tool_bindings: environment
            .tool_bindings
            .iter()
            .map(|binding| binding.tool.as_str())
            .collect(),
        requirements: RuntimeSourceRequirements::default(),
    };

    for expr in &resolved_paths.expr_paths {
        if !linked.exprs.contains(&expr.expr) {
            continue;
        }
        let expression = context
            .hir
            .as_ref()
            .and_then(|hir| hir.hir.exprs.get(expr.expr))
            .ok_or_else(|| format!("runtime source path has no HIR expression {:?}", expr.expr))?;
        if matches!(expression, HirExpr::Reflect(_)) {
            continue;
        }
        match &expr.result {
            HirPathResolution::DirectSymbol(symbol) => {
                if let Some(member) = context
                    .hir_item_bindings
                    .as_ref()
                    .and_then(|bindings| bindings.external_import_members.get(symbol))
                {
                    planner.collect_external_item(Some(member.package), &member.path)?;
                }
            }
            HirPathResolution::ExplicitImport { target }
            | HirPathResolution::PartialImport { target, .. } => {
                planner.collect_import_target(target)?;
            }
            HirPathResolution::WildcardImport { item, .. } => {
                planner.collect_import_target(item)?;
            }
            HirPathResolution::AmbiguousWildcard { targets } => {
                for target in targets {
                    planner.collect_module_target(target)?;
                }
            }
            _ => {}
        }
    }
    facts.runtime_source_requirements = planner.requirements;
    Ok(facts)
}

struct RuntimeSourceRequirementPlanner<'a> {
    namespaces: &'a crate::project::DependencyModuleNamespaces,
    packages: BTreeMap<ExternalPackageId, &'a ProjectExternalPackageInput>,
    metadata: BTreeMap<ExternalPackageId, &'a ProjectExternalPublicMetadataInput>,
    tool_bindings: BTreeSet<&'a str>,
    requirements: RuntimeSourceRequirements,
}

impl RuntimeSourceRequirementPlanner<'_> {
    fn collect_import_target(&mut self, target: &ImportTarget) -> Result<(), String> {
        let ImportTarget::ExternalItem {
            package,
            module_path,
            name,
            ..
        } = target
        else {
            return Ok(());
        };
        let mut item_path = module_path.segments.clone();
        item_path.push(name.clone());
        self.collect_external_item(*package, &item_path)
    }

    fn collect_external_item(
        &mut self,
        package: Option<ExternalPackageId>,
        item_path: &[String],
    ) -> Result<(), String> {
        let Some(package) = self.package_for_external_path(package, item_path) else {
            return Ok(());
        };
        let Some(runtime_item_path) = self.resolve_runtime_item_path(package.id, item_path) else {
            return Ok(());
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
        )
    }

    fn collect_module_target(&mut self, target: &ResolvedModuleTarget) -> Result<(), String> {
        let ResolvedModuleTarget::External { package, path, .. } = target else {
            return Ok(());
        };
        let Some(package) = self.package_for_external_path(*package, &path.segments) else {
            return Ok(());
        };
        if !self.module_has_runtime_items(package.id, &path.segments) {
            return Ok(());
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
        )
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
    ) -> Result<(), String> {
        let module_path = self
            .namespaces
            .source_for_namespace(package, &ModulePath { segments: module })?;
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
        Ok(())
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

fn entry_reachability(
    context: &ProjectContext,
    types: Option<&etas_types::TypeFacts>,
    purpose: ReachabilityPurpose,
) -> Result<Option<EntryReachability>, String> {
    let Some(entry) = context
        .entry
        .as_ref()
        .and_then(|entry| entry.resolved.as_ref())
    else {
        return Ok(None);
    };
    let entry = entry.item;
    let hir = context
        .hir
        .as_ref()
        .ok_or("entry reachability requires finalized HIR")?;
    let resolved_paths = context
        .resolved_paths
        .as_ref()
        .ok_or("entry reachability requires resolved paths")?;
    let item_bindings = context
        .hir_item_bindings
        .as_ref()
        .ok_or("entry reachability requires source item bindings")?;
    let view = hir.view();
    let mut collector = ReachableExprCollector {
        hir: &hir.hir,
        resolved_paths,
        item_bindings,
        types,
        purpose,
        view: &view,
        visited_items: BTreeSet::new(),
        pending_items: context.analysis_roots.clone(),
        exprs: BTreeSet::new(),
    };
    collector.collect()?;
    Ok(Some(EntryReachability {
        entry,
        items: collector.visited_items,
        exprs: collector.exprs,
    }))
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
    let Some(bindings) = context.hir_body_bindings.as_ref() else {
        return BTreeSet::new();
    };
    let Some(units) = context.units.as_ref() else {
        return BTreeSet::new();
    };
    bindings
        .body_items
        .iter()
        .filter(|(_, item)| reachable_items.contains(item))
        .filter_map(|(body, _)| {
            units
                .by_target
                .get(&UnitTarget::AstBody(body.clone()))
                .copied()
        })
        .collect()
}

pub(super) fn source_analysis_closure(
    context: &ProjectContext,
    roots: Vec<HirItemId>,
) -> Result<(BTreeSet<HirItemId>, BTreeSet<crate::UnitId>), &'static str> {
    let (Some(hir), Some(resolved_paths), Some(item_bindings)) = (
        &context.hir,
        &context.resolved_paths,
        &context.hir_item_bindings,
    ) else {
        return Err("definition analysis requires finalized HIR, resolved paths and item bindings");
    };
    let view = hir.view();
    let mut collector = ReachableExprCollector {
        hir: &hir.hir,
        resolved_paths,
        item_bindings,
        types: context.types.as_ref().map(|types| types.facts.as_ref()),
        purpose: ReachabilityPurpose::ExecutableValues,
        view: &view,
        visited_items: BTreeSet::new(),
        pending_items: roots,
        exprs: BTreeSet::new(),
    };
    collector
        .collect()
        .map_err(|_| "typed invocation has no checked source method body")?;
    let bodies = reachable_bodies(context, &collector.visited_items);
    Ok((collector.visited_items, bodies))
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
    types: Option<&'a etas_types::TypeFacts>,
    purpose: ReachabilityPurpose,
    view: &'a HirTreeView<'a>,
    visited_items: BTreeSet<HirItemId>,
    pending_items: Vec<HirItemId>,
    exprs: BTreeSet<HirExprId>,
}

#[derive(Clone, Copy)]
enum ReachabilityPurpose {
    Calls,
    ExecutableValues,
}

impl ReachableExprCollector<'_> {
    fn collect(&mut self) -> Result<(), String> {
        while let Some(item) = self.pending_items.pop() {
            if !self.visited_items.insert(item) {
                continue;
            }
            self.enqueue_runtime_annotation_items(item);
            if let Some(HirItem::Agent(agent)) = self.hir.items.get(item)
                && let Some(method) = self.hir.agent_method(agent.symbol, "run")
                && method.body != item
            {
                self.pending_items.push(method.body);
            }
            let agent = self
                .hir
                .agent_methods
                .values()
                .find(|method| method.body == item)
                .and_then(|method| self.hir.symbols.get(method.agent))
                .and_then(|symbol| match symbol.def {
                    SymbolDef::Item { item } => Some(item),
                    _ => None,
                });
            if let Some(agent) = agent.filter(|agent| *agent != item) {
                self.enqueue_runtime_annotation_items(agent);
            }
            let mut missing_method = false;
            let mut visitor = ReachableItemVisitor {
                hir: self.hir,
                resolved_paths: self.resolved_paths,
                item_bindings: self.item_bindings,
                types: self.types,
                purpose: self.purpose,
                missing_method: &mut missing_method,
                exprs: &mut self.exprs,
                pending_items: &mut self.pending_items,
            };
            walk_item(self.view, item, &mut visitor);
            if missing_method {
                return Err("typed invocation has no checked source method body".into());
            }
        }
        Ok(())
    }

    fn enqueue_runtime_annotation_items(&mut self, item: HirItemId) {
        let tool_values = self
            .hir
            .item_annotations
            .get(&item)
            .into_iter()
            .flatten()
            .filter(|annotation| {
                annotation
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| segment.name == "tools")
            })
            .flat_map(|annotation| annotation.args.iter())
            .map(|arg| match arg {
                etas_hir::HirAnnotationArg::Positional { value, .. }
                | etas_hir::HirAnnotationArg::Named { value, .. } => *value,
            })
            .collect::<Vec<_>>();
        for value in tool_values {
            let mut visitor = ToolAnnotationReachabilityVisitor {
                hir: self.hir,
                resolved_paths: self.resolved_paths,
                item_bindings: self.item_bindings,
                pending_items: &mut self.pending_items,
            };
            walk_expr(self.view, value, &mut visitor);
        }
    }
}

struct ToolAnnotationReachabilityVisitor<'a> {
    hir: &'a etas_hir::HirProgram,
    resolved_paths: &'a crate::ResolvedPaths,
    item_bindings: &'a HirItemBindings,
    pending_items: &'a mut Vec<HirItemId>,
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for ToolAnnotationReachabilityVisitor<'_> {
    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        let Some(item) =
            local_item_for_expr_path(self.hir, self.resolved_paths, self.item_bindings, expr.id())
        else {
            return;
        };
        if matches!(self.hir.items.get(item), Some(HirItem::Tool(_))) {
            self.pending_items.push(item);
        }
    }
}

struct ReachableItemVisitor<'a> {
    hir: &'a etas_hir::HirProgram,
    resolved_paths: &'a crate::ResolvedPaths,
    item_bindings: &'a HirItemBindings,
    types: Option<&'a etas_types::TypeFacts>,
    purpose: ReachabilityPurpose,
    missing_method: &'a mut bool,
    exprs: &'a mut BTreeSet<HirExprId>,
    pending_items: &'a mut Vec<HirItemId>,
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for ReachableItemVisitor<'_> {
    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        let expr_id = expr.id();
        self.exprs.insert(expr_id);
        if let Some(types) = self.types
            && let Some(symbol) = types.agent_method_calls.get(&expr_id)
        {
            match types.agent_methods.get(symbol) {
                Some(method) => self.pending_items.push(method.body),
                None => *self.missing_method = true,
            }
        }
        match self.hir.exprs.get(expr_id) {
            Some(HirExpr::Path(_))
                if matches!(self.purpose, ReachabilityPurpose::ExecutableValues) =>
            {
                if let Some(item) = local_item_for_expr_path(
                    self.hir,
                    self.resolved_paths,
                    self.item_bindings,
                    expr_id,
                ) && matches!(
                    self.hir.items.get(item),
                    Some(
                        HirItem::Flow(_)
                            | HirItem::Tool(_)
                            | HirItem::Agent(_)
                            | HirItem::TopLevelLet(_)
                    )
                ) {
                    // Executable values retain their implementation closure;
                    // analysis/linking does not grant invocation authority.
                    self.pending_items.push(item);
                }
            }
            Some(HirExpr::Call { callee, .. }) => {
                self.enqueue_local_item_for_expr_path(*callee);
            }
            Some(HirExpr::MethodCall { receiver, .. }) => {
                self.enqueue_local_item_for_expr_path(*receiver);
                if let Some(HirExpr::MethodCall { method, .. }) = self.hir.exprs.get(expr_id)
                    && let Some(HirExpr::Path(path)) = self.hir.exprs.get(*receiver)
                    && let etas_hir::ResolveResult::Resolved(symbol) = path.resolution
                    && let Some(item) = agent_method_for_binding(self.hir, symbol, method)
                {
                    self.pending_items.push(item);
                }
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
        } else if let Some(HirExpr::Path(path)) = self.hir.exprs.get(expr)
            && let etas_hir::ResolveResult::Resolved(symbol) = path.resolution
            && let Some(item) = agent_method_for_binding(self.hir, symbol, "run")
        {
            self.pending_items.push(item);
        }
    }
}

fn local_item_for_expr_path(
    hir: &etas_hir::HirProgram,
    resolved_paths: &crate::ResolvedPaths,
    item_bindings: &HirItemBindings,
    mut expr: HirExprId,
) -> Option<HirItemId> {
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(expr) || !matches!(hir.exprs.get(expr), Some(HirExpr::Path(_))) {
            return None;
        }
        let resolution = resolved_paths
            .expr_paths
            .iter()
            .find(|path| path.expr == expr)?;
        match &resolution.result {
            HirPathResolution::PartialSymbol {
                prefix, remaining, ..
            } if remaining.len() == 1 => {
                return agent_method_for_binding(hir, *prefix, &remaining[0]);
            }
            HirPathResolution::DirectSymbol(symbol) => match &hir.symbols.get(*symbol)?.def {
                SymbolDef::Item { item } | SymbolDef::TopLevelLet { item, .. } => {
                    return Some(*item);
                }
                SymbolDef::Local {
                    initializer: Some(initializer),
                    ..
                }
                | SymbolDef::PatternBinding {
                    initializer: Some(initializer),
                    ..
                } => expr = *initializer,
                _ => return None,
            },
            HirPathResolution::ExplicitImport {
                target: ImportTarget::SourceItem { item, .. },
            }
            | HirPathResolution::WildcardImport {
                item: ImportTarget::SourceItem { item, .. },
                ..
            }
            | HirPathResolution::PartialImport {
                target: ImportTarget::SourceItem { item, .. },
                ..
            } => {
                return item_bindings.ast_to_hir.get(item).copied();
            }
            _ => return None,
        }
    }
}

fn agent_method_for_binding(
    hir: &etas_hir::HirProgram,
    binding: etas_hir::SymbolId,
    name: &str,
) -> Option<HirItemId> {
    let mut pending = vec![binding];
    let mut seen = BTreeSet::new();
    while let Some(symbol) = pending.pop() {
        if !seen.insert(symbol) {
            continue;
        }
        if let etas_hir::ResolveResult::Resolved(method) = hir.symbols.resolve_member(symbol, name)
            && let Some(method) = hir.agent_methods.get(&method)
        {
            return Some(method.body);
        }
        if let Some(method) = hir.agent_method(symbol, name) {
            return Some(method.body);
        }
        let data = hir.symbols.get(symbol)?;
        let ty = match data.def {
            SymbolDef::Item { item } => match hir.items.get(item)? {
                HirItem::TypeAlias(alias) => Some(alias.target),
                _ => data.declared_type,
            },
            SymbolDef::Local {
                initializer: Some(expr),
                ..
            }
            | SymbolDef::PatternBinding {
                initializer: Some(expr),
                ..
            } => {
                if let Some(HirExpr::Path(path)) = hir.exprs.get(expr)
                    && let etas_hir::ResolveResult::Resolved(symbol) = path.resolution
                {
                    pending.push(symbol);
                }
                data.declared_type
            }
            _ => data.declared_type,
        };
        if let Some(etas_hir::HirType::Path { path, .. }) = ty.and_then(|ty| hir.types.get(ty))
            && let etas_hir::ResolveResult::Resolved(symbol) = path.resolution
        {
            pending.push(symbol);
        }
    }
    None
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
