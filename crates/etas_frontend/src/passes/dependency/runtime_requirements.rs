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
    let modules = context
        .modules
        .as_ref()
        .ok_or("runtime source planning requires a module index")?;
    let mut planner = RuntimeSourceRequirementPlanner {
        checked_library: context.standard_library_interface().and_then(|library| {
            environment
                .external_definition_origins
                .iter()
                .find(|origin| origin.origins.package == library.origins.package)
                .map(|origin| origin.package)
        }),
        namespaces: &modules.dependency_namespaces,
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
        method_owners: environment
            .external_public_metadata
            .iter()
            .flat_map(|metadata| {
                metadata
                    .methods
                    .iter()
                    .map(|method| (method.identity.clone(), metadata.package))
            })
            .chain(
                environment
                    .external_definition_origins
                    .iter()
                    .flat_map(|origin| {
                        origin
                            .descriptors
                            .iter()
                            .flat_map(|set| set.headers.iter())
                            .flat_map(|headers| &headers.callables)
                            .filter_map(|header| {
                                header
                                    .id
                                    .as_method()
                                    .map(|method| (method.clone(), origin.package))
                            })
                    }),
            )
            .collect(),
        module_paths: environment
            .external_definition_origins
            .iter()
            .flat_map(|origin| {
                origin.origins.modules.iter().map(|module| {
                    (
                        (origin.package, module.canonical_path.clone()),
                        module.path.clone(),
                    )
                })
            })
            .collect(),
        tool_bindings: environment
            .tool_bindings
            .iter()
            .map(|binding| binding.tool.as_str())
            .collect(),
        loaded_overlays: modules
            .modules
            .iter()
            .filter_map(|(_, module)| match module.origin {
                crate::ModuleOrigin::DependencySourceOverlay { package, .. } => {
                    Some((package, module.path.clone()))
                }
                _ => None,
            })
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
    if let Some(types) = types {
        let specs = std::cell::OnceCell::new();
        let hir = &context.hir.as_ref().expect("typed runtime HIR").hir;
        let store = &context.types.as_ref().expect("typed runtime linkage").store;
        for expr in &linked.exprs {
            if matches!(hir.exprs.get(*expr), Some(HirExpr::Record(_)))
                && let Some(ty) = types.expression_producer_type(*expr)
                && etas_types::active_agent::has_main(types, store, ty)
                && let Some(main) = etas_types::active_agent::autonomous_main(
                    types,
                    store,
                    specs.get_or_init(|| etas_types::SpecFacts::from(types)),
                    ty,
                )?
                && let etas_types::method::MethodTarget::External { identity } =
                    &main.declaration.target
            {
                planner.collect_external_method(identity)?;
            }
            if let Some(pack) = types.existential_packs.get(expr) {
                for witness in &pack.witnesses {
                    if let etas_types::method::MethodTarget::External { identity } = &witness.target
                    {
                        planner.collect_external_method(identity)?;
                    }
                }
            }
            if let Some(call) = types.checked_method_calls.get(expr)
                && let etas_types::method::MethodTarget::External { identity } = &call.target
            {
                planner.collect_external_method(identity)?;
            }
        }
    }
    facts.runtime_source_requirements = planner.requirements;
    Ok(facts)
}

struct RuntimeSourceRequirementPlanner<'a> {
    checked_library: Option<ExternalPackageId>,
    namespaces: &'a crate::project::DependencyModuleNamespaces,
    packages: BTreeMap<ExternalPackageId, &'a ProjectExternalPackageInput>,
    metadata: BTreeMap<ExternalPackageId, &'a ProjectExternalPublicMetadataInput>,
    method_owners: BTreeMap<etas_code::MethodId, ExternalPackageId>,
    module_paths: BTreeMap<(ExternalPackageId, Vec<String>), Vec<String>>,
    tool_bindings: BTreeSet<&'a str>,
    loaded_overlays: BTreeSet<(ExternalPackageId, ModulePath)>,
    requirements: RuntimeSourceRequirements,
}

impl RuntimeSourceRequirementPlanner<'_> {
    fn collect_external_method(&mut self, identity: &etas_code::MethodId) -> Result<(), String> {
        // Ownership belongs to the published calling header. Its executable
        // behavior can remain unloaded until effect analysis needs it.
        let owner = *self
            .method_owners
            .get(identity)
            .ok_or("checked external method has no published package owner")?;
        let package = self
            .packages
            .get(&owner)
            .ok_or("external method package is missing")?;
        let module = self
            .module_paths
            .get(&(owner, identity.implementation().module().to_vec()))
            .ok_or("external method has no checked source module origin")?
            .clone();
        if self.loaded_overlays.contains(&(
            owner,
            ModulePath {
                segments: module.clone(),
            },
        )) {
            return Ok(());
        }
        let mut item_path = identity.implementation().module().to_vec();
        item_path.push(identity.name().into());
        self.add_seed_module(
            owner,
            package.import_root.clone(),
            module.clone(),
            RuntimeSourceReason {
                item_path,
                module_path: ModulePath { segments: module },
                kind: RuntimeSourceReasonKind::ExternalSpecImplementation,
            },
        )
    }

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
        if self.checked_library == Some(package) {
            // SDK implementations are linked from the verified checked image.
            return Ok(());
        }
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
        types: context
            .types
            .as_ref()
            .or(context.signature_types.as_ref())
            .map(|types| types.facts.as_ref()),
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
        let mut paths =
            std::collections::HashMap::with_capacity(self.resolved_paths.expr_paths.len());
        for path in &self.resolved_paths.expr_paths {
            paths.entry(path.expr).or_insert(&path.result);
        }
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
            let mut missing_method = false;
            let mut visitor = ReachableItemVisitor {
                hir: self.hir,
                resolved_paths: &paths,
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
            .execution_annotations(item)
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
                pending_items: &mut self.pending_items,
            };
            walk_expr(self.view, value, &mut visitor);
        }
    }
}

struct ToolAnnotationReachabilityVisitor<'a> {
    hir: &'a etas_hir::HirProgram,
    pending_items: &'a mut Vec<HirItemId>,
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for ToolAnnotationReachabilityVisitor<'_> {
    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        let HirExpr::Path(path) = expr.data() else {
            return;
        };
        if let etas_hir::ResolveResult::Resolved(symbol) = path.resolution
            && let Some(symbol) = self.hir.canonical_source_symbol(symbol)
            && let Some(SymbolDef::Item { item }) = self.hir.symbols.get(symbol).map(|row| &row.def)
            && matches!(self.hir.items.get(*item), Some(HirItem::Tool(_)))
        {
            self.pending_items.push(*item);
        }
    }
}

struct ReachableItemVisitor<'a> {
    hir: &'a etas_hir::HirProgram,
    resolved_paths: &'a std::collections::HashMap<HirExprId, &'a HirPathResolution>,
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
        if let Some(agent) = constructor_agent(self.hir, expr_id)
            && let Some(types) = self.types
        {
            self.pending_items.extend(
                types
                    .agent_methods
                    .values()
                    .filter(|method| method.agent == agent && method.name == "main")
                    .map(|method| method.body),
            );
        }
        if let Some(symbol) = self
            .types
            .and_then(|types| types.source_flow_calls.get(&expr_id))
        {
            match self.hir.symbols.get(*symbol).map(|symbol| &symbol.def) {
                Some(etas_hir::SymbolDef::Item { item }) => self.pending_items.push(*item),
                _ => *self.missing_method = true,
            }
        }
        if let Some(fact) = self
            .types
            .and_then(|types| types.checked_method_calls.get(&expr_id))
        {
            match &fact.target {
                etas_types::method::MethodTarget::Source { callable, .. }
                | etas_types::method::MethodTarget::Agent { callable, .. } => {
                    self.pending_items.push(*callable)
                }
                _ => {}
            }
        }
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
            Some(
                HirExpr::MethodCall { receiver, .. } | HirExpr::SpecMethodCall { receiver, .. },
            ) => {
                self.enqueue_local_item_for_expr_path(*receiver);
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
    resolved_paths: &std::collections::HashMap<HirExprId, &HirPathResolution>,
    item_bindings: &HirItemBindings,
    mut expr: HirExprId,
) -> Option<HirItemId> {
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(expr) || !matches!(hir.exprs.get(expr), Some(HirExpr::Path(_))) {
            return None;
        }
        match *resolved_paths.get(&expr)? {
            HirPathResolution::PartialSymbol {
                prefix, remaining, ..
            } => {
                if let SymbolDef::TopLevelLet { item, .. } = hir.symbols.get(*prefix)?.def {
                    return Some(item);
                }
                if remaining.len() == 1 {
                    return agent_method_for_binding(hir, *prefix, &remaining[0]);
                }
                return None;
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

fn constructor_agent(hir: &etas_hir::HirProgram, expr: HirExprId) -> Option<HirItemId> {
    let HirExpr::Record(record) = hir.exprs.get(expr)? else {
        return None;
    };
    let mut path = record.path.as_ref()?;
    let mut seen = BTreeSet::new();
    loop {
        let etas_hir::ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        let symbol = hir.canonical_source_symbol(symbol)?;
        if !seen.insert(symbol) {
            return None;
        }
        let SymbolDef::Item { item } = hir.symbols.get(symbol)?.def else {
            return None;
        };
        match &hir.items[item] {
            HirItem::Agent(_) => return Some(item),
            HirItem::TypeAlias(alias) => {
                let etas_hir::HirType::Path { path: target, .. } = &hir.types[alias.target] else {
                    return None;
                };
                path = target;
            }
            _ => return None,
        }
    }
}
