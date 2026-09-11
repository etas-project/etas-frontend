use std::collections::{BTreeMap, BTreeSet, HashMap};

use etas_core::{Diagnostic, DiagnosticCode, NameDiagnosticCode, SourceId, Span};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::{CatalogModuleProvider, ProjectModuleResolver};
use crate::{
    ImportTarget, ModuleCatalog, ModuleExport, ModuleExportTarget, ModuleId, ModulePartId,
    ProjectContext, ResolvedImport, ResolvedImports, ResolvedWildcardImport,
};

use super::super::artifacts::{
    HIR_OUTPUT, MODULE_CATALOG, MODULE_INDEX, PARSED_SOURCE_SET, RESOLVED_IMPORTS,
    VALIDATED_EXTERNAL_ENVIRONMENT, global_with_diagnostics,
};
use super::imports_common::{
    ImportResolution, duplicate_explicit_import_diagnostics, parsed_by_source, part_module,
    resolve_import_tree,
};

pub struct ResolveImportTargetsPass;

impl Pass<ProjectContext> for ResolveImportTargetsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ResolveImportTargetsPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                MODULE_INDEX,
                MODULE_CATALOG,
                PARSED_SOURCE_SET,
            ]))
            .produces(global_with_diagnostics([RESOLVED_IMPORTS]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        materialize_catalog_re_export_closure(context);
        let (imports, wildcard_imports, re_exports, diagnostics) = {
            let modules = context.modules.as_ref().expect("module index should exist");
            let catalog = context
                .module_catalog
                .as_ref()
                .expect("module catalog should exist");
            let catalog_provider = CatalogModuleProvider::new(catalog);
            let resolver = ProjectModuleResolver::new().with_provider(&catalog_provider);
            let parsed_by_source = parsed_by_source(context);
            let mut imports = Vec::new();
            let mut wildcard_imports = Vec::new();
            let mut re_exports = Vec::new();
            let mut diagnostics = Vec::new();

            for (_, part) in modules.parts.iter() {
                let parsed = parsed_by_source
                    .get(&part.source)
                    .expect("module part source should be parsed");
                for import_ref in &part.imports {
                    let import = parsed
                        .parse
                        .value
                        .imports
                        .get(import_ref.index)
                        .expect("import ref should point into parsed source");
                    for resolution in resolve_import_tree(
                        import,
                        import_ref,
                        part.module,
                        part.id,
                        modules,
                        &resolver,
                        &mut diagnostics,
                    ) {
                        match resolution {
                            ImportResolution::Explicit(resolved) => {
                                if import.visibility == etas_syntax::ast::Visibility::Public {
                                    re_exports.push(crate::ReExport {
                                        import: import_ref.clone(),
                                        target: resolved.target.module_path(modules),
                                        span: import.span,
                                    });
                                }
                                imports.push(resolved);
                            }
                            ImportResolution::Wildcard(resolved) => {
                                if import.visibility == etas_syntax::ast::Visibility::Public {
                                    re_exports.push(crate::ReExport {
                                        import: import_ref.clone(),
                                        target: resolved.target_module.module_path(modules),
                                        span: import.span,
                                    });
                                }
                                wildcard_imports.push(resolved);
                            }
                        }
                    }
                }
            }
            diagnostics.extend(duplicate_explicit_import_diagnostics(&imports));
            (imports, wildcard_imports, re_exports, diagnostics)
        };
        for re_export in &re_exports {
            let modules = context.modules.as_mut().expect("module index should exist");
            let module_id = part_module(modules, &re_export.import);
            if let Some(module) = modules.modules.get_mut(module_id) {
                module.visibility_exports.re_exports.push(re_export.clone());
            }
        }
        apply_re_exports_to_catalog(context, &imports, &wildcard_imports);
        context.diagnostics.extend(diagnostics);
        context.resolved_imports = Some(ResolvedImports {
            imports,
            wildcard_imports,
            re_exports,
        });
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(RESOLVED_IMPORTS))
    }
}

fn materialize_catalog_re_export_closure(context: &mut ProjectContext) {
    let module_part_count = context
        .modules
        .as_ref()
        .map(|modules| modules.parts.len())
        .unwrap_or_default();
    for _ in 0..=module_part_count {
        let (imports, wildcards) = resolve_public_imports_for_catalog(context);
        if !apply_re_exports_to_catalog(context, &imports, &wildcards) {
            break;
        }
    }
}

fn resolve_public_imports_for_catalog(
    context: &ProjectContext,
) -> (Vec<ResolvedImport>, Vec<ResolvedWildcardImport>) {
    let Some(modules) = context.modules.as_ref() else {
        return (Vec::new(), Vec::new());
    };
    let Some(catalog) = context.module_catalog.as_ref() else {
        return (Vec::new(), Vec::new());
    };
    let catalog_provider = CatalogModuleProvider::new(catalog);
    let resolver = ProjectModuleResolver::new().with_provider(&catalog_provider);
    let parsed_by_source = parsed_by_source(context);
    let mut imports = Vec::new();
    let mut wildcard_imports = Vec::new();
    let mut diagnostics = Vec::new();

    for (_, part) in modules.parts.iter() {
        let parsed = parsed_by_source
            .get(&part.source)
            .expect("module part source should be parsed");
        for import_ref in &part.imports {
            let import = parsed
                .parse
                .value
                .imports
                .get(import_ref.index)
                .expect("import ref should point into parsed source");
            if import.visibility != etas_syntax::ast::Visibility::Public {
                continue;
            }
            for resolution in resolve_import_tree(
                import,
                import_ref,
                part.module,
                part.id,
                modules,
                &resolver,
                &mut diagnostics,
            ) {
                match resolution {
                    ImportResolution::Explicit(resolved) => imports.push(resolved),
                    ImportResolution::Wildcard(resolved) => wildcard_imports.push(resolved),
                }
            }
        }
    }
    (imports, wildcard_imports)
}

fn apply_re_exports_to_catalog(
    context: &mut ProjectContext,
    imports: &[ResolvedImport],
    wildcard_imports: &[ResolvedWildcardImport],
) -> bool {
    let Some(catalog) = context.module_catalog.as_mut() else {
        return false;
    };
    let explicit_exports = imports
        .iter()
        .filter(|import| import.visibility == etas_hir::Visibility::Public)
        .filter_map(|import| {
            let from_key = catalog.source_module_key(import.from)?;
            let target = module_export_target_for_import(&import.target)?;
            Some((
                from_key,
                import.local_name.clone(),
                ModuleExport {
                    name: import.local_name.clone(),
                    visibility: etas_hir::Visibility::Public,
                    target,
                },
            ))
        })
        .collect::<Vec<_>>();

    let wildcard_exports = wildcard_imports
        .iter()
        .filter(|import| import.visibility == etas_hir::Visibility::Public)
        .filter_map(|import| {
            let from_key = catalog.source_module_key(import.from)?;
            let target_key = import.target_module.key();
            let target_exports = catalog
                .modules
                .get(target_key)?
                .exports
                .items
                .iter()
                .filter(|(name, _)| import.exported_names.iter().any(|export| export == *name))
                .map(|(name, export)| {
                    (
                        from_key,
                        name.clone(),
                        ModuleExport {
                            name: name.clone(),
                            visibility: etas_hir::Visibility::Public,
                            target: export.target.clone(),
                        },
                    )
                })
                .collect::<Vec<_>>();
            Some(target_exports)
        })
        .flatten()
        .collect::<Vec<_>>();

    let mut changed = false;
    for (from_key, name, export) in explicit_exports.into_iter().chain(wildcard_exports) {
        if let Some(record) = catalog.modules.get_mut(from_key) {
            if let std::collections::hash_map::Entry::Vacant(entry) =
                record.exports.items.entry(name)
            {
                entry.insert(export);
                changed = true;
            }
        }
    }
    changed
}

fn module_export_target_for_import(target: &ImportTarget) -> Option<ModuleExportTarget> {
    match target {
        ImportTarget::Module(_) => None,
        ImportTarget::SourceItem { module, item, .. } => Some(ModuleExportTarget::Source {
            module: *module,
            item: item.clone(),
        }),
        ImportTarget::StdItem {
            module,
            module_path,
            symbol,
            ..
        } => Some(ModuleExportTarget::Std {
            module: *module,
            module_path: module_path.clone(),
            symbol: *symbol,
        }),
        ImportTarget::ExternalItem {
            package,
            module,
            module_path,
            symbol,
            ..
        } => Some(ModuleExportTarget::External {
            package: *package,
            module: *module,
            module_path: module_path.clone(),
            symbol: *symbol,
        }),
    }
}

pub struct ApplyResolvedImportsToHirPass;

impl Pass<ProjectContext> for ApplyResolvedImportsToHirPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ApplyResolvedImportsToHirPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                MODULE_INDEX,
                HIR_OUTPUT,
                RESOLVED_IMPORTS,
                VALIDATED_EXTERNAL_ENVIRONMENT,
            ]))
            .produces(global_with_diagnostics([HIR_OUTPUT]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let resolved_imports = context
            .resolved_imports
            .as_ref()
            .expect("resolved import targets should exist")
            .clone();
        install_explicit_import_aliases(context, &resolved_imports.imports);
        let wildcard_imports = resolved_imports.wildcard_imports;
        install_wildcard_import_aliases(context, &wildcard_imports);
        install_external_effect_action_refs(context);
        if let Some(output) = &mut context.hir {
            let resolved = super::source_members::resolve_members(&mut output.hir);
            prune_resolved_wildcard_diagnostics(&mut output.hir.diagnostics, &resolved);
            prune_resolved_wildcard_diagnostics(&mut context.diagnostics, &resolved);
            output.tree_index = match etas_hir::HirTreeIndex::try_build(&output.hir) {
                Ok(index) => index,
                Err(error) => {
                    return PassResult::failed(format!("invalid imported member HIR: {error}"));
                }
            };
        }
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(HIR_OUTPUT))
    }
}

fn install_external_effect_action_refs(context: &mut ProjectContext) {
    let Some(modules) = context.modules.as_ref() else {
        return;
    };
    let action_paths = external_action_paths(context);
    if action_paths.is_empty() {
        return;
    }
    let Some(hir_output) = context.hir.as_mut() else {
        return;
    };
    let hir = &mut hir_output.hir;
    let module_to_hir = modules
        .modules
        .iter()
        .filter_map(|(module_id, module_info)| {
            let hir_module = hir.modules.iter().find_map(|hir_module| {
                let hir_module_data = hir.modules_arena.get(*hir_module)?;
                let path = hir_module_data.name.as_ref()?;
                (path
                    .segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .eq(module_info.path.segments.iter().map(String::as_str)))
                .then_some(*hir_module)
            })?;
            Some((module_id, hir_module))
        })
        .collect::<HashMap<ModuleId, etas_hir::HirModuleId>>();
    let source_to_hir_module = modules
        .by_source
        .iter()
        .filter_map(|(source, module)| module_to_hir.get(module).copied().map(|hir| (*source, hir)))
        .collect::<HashMap<SourceId, etas_hir::HirModuleId>>();
    let mut synthetic_symbols =
        BTreeMap::<(etas_hir::HirModuleId, Vec<String>, String), etas_hir::SymbolId>::new();
    let mut fixed_spans = Vec::new();

    let expr_updates = hir
        .exprs
        .iter()
        .filter_map(|(expr_id, expr)| {
            let etas_hir::HirExpr::Perform { action, .. } = expr else {
                return None;
            };
            if !matches!(action.action_symbol, etas_hir::ResolveResult::Unresolved) {
                return None;
            }
            Some((
                expr_id,
                external_action_resolution(action, hir, &action_paths, &source_to_hir_module)?,
            ))
        })
        .collect::<Vec<_>>();
    for (expr_id, resolution) in expr_updates {
        let symbol = external_action_symbol(hir, &mut synthetic_symbols, resolution);
        if let Some(etas_hir::HirExpr::Perform { action, .. }) = hir.exprs.get_mut(expr_id) {
            action.action_symbol = etas_hir::ResolveResult::Resolved(symbol);
            fixed_spans.push(action.span);
        }
    }

    let handler_updates = hir
        .handler_arms
        .iter()
        .filter_map(|(arm_id, arm)| {
            if !matches!(
                arm.action.action_symbol,
                etas_hir::ResolveResult::Unresolved
            ) {
                return None;
            }
            Some((
                arm_id,
                external_action_resolution(&arm.action, hir, &action_paths, &source_to_hir_module)?,
            ))
        })
        .collect::<Vec<_>>();
    for (arm_id, resolution) in handler_updates {
        let symbol = external_action_symbol(hir, &mut synthetic_symbols, resolution);
        if let Some(arm) = hir.handler_arms.get_mut(arm_id) {
            arm.action.action_symbol = etas_hir::ResolveResult::Resolved(symbol);
            fixed_spans.push(arm.action.span);
        }
    }

    if !fixed_spans.is_empty() {
        prune_resolved_external_action_diagnostics(&mut hir.diagnostics, &fixed_spans);
        prune_resolved_external_action_diagnostics(&mut context.diagnostics, &fixed_spans);
    }
}

#[derive(Clone, Debug)]
struct ExternalActionResolution {
    hir_module: etas_hir::HirModuleId,
    owner_path: Vec<String>,
    action: String,
    span: Span,
}

fn external_action_paths(context: &ProjectContext) -> BTreeSet<Vec<String>> {
    context
        .validated_external_environment
        .as_ref()
        .expect("external environment should be validated before applying imports")
        .environment()
        .external_public_metadata
        .iter()
        .flat_map(|metadata| metadata.actions.iter().map(|action| action.path.clone()))
        .collect()
}

fn external_action_resolution(
    action: &etas_hir::ResolvedActionRef,
    hir: &etas_hir::HirProgram,
    action_paths: &BTreeSet<Vec<String>>,
    source_to_hir_module: &HashMap<SourceId, etas_hir::HirModuleId>,
) -> Option<ExternalActionResolution> {
    let etas_hir::ResolveResult::Resolved(effect_symbol) = action.effect.path.resolution else {
        return None;
    };
    let symbol = hir.symbols.get(effect_symbol)?;
    let etas_hir::SymbolDef::ImportAlias { path, origin } = &symbol.def else {
        return None;
    };
    if *origin != etas_hir::ImportAliasOrigin::SourceImport {
        return None;
    }
    let mut action_path = path.clone();
    action_path.push(action.action.clone());
    if !action_paths.contains(&action_path) {
        return None;
    }
    Some(ExternalActionResolution {
        hir_module: source_to_hir_module.get(&action.span.source).copied()?,
        owner_path: path.clone(),
        action: action.action.clone(),
        span: action.span,
    })
}

fn external_action_symbol(
    hir: &mut etas_hir::HirProgram,
    synthetic_symbols: &mut BTreeMap<
        (etas_hir::HirModuleId, Vec<String>, String),
        etas_hir::SymbolId,
    >,
    resolution: ExternalActionResolution,
) -> etas_hir::SymbolId {
    let key = (
        resolution.hir_module,
        resolution.owner_path.clone(),
        resolution.action.clone(),
    );
    if let Some(symbol) = synthetic_symbols.get(&key) {
        return *symbol;
    }
    let local_owner = resolution
        .owner_path
        .last()
        .cloned()
        .unwrap_or_else(|| "<external-effect>".to_owned());
    let symbol = hir.symbols.alloc(etas_hir::SymbolData {
        name: format!("{local_owner}.{}", resolution.action),
        kind: etas_hir::SymbolKind::EffectAction,
        visibility: etas_hir::Visibility::Public,
        defining_module: resolution.hir_module,
        defining_item: None,
        def: etas_hir::SymbolDef::Synthetic {
            reason: etas_hir::SyntheticSymbolReason::ExternalEffectAction,
        },
        declared_type: None,
        definition_span: resolution.span,
    });
    synthetic_symbols.insert(key, symbol);
    symbol
}

fn install_wildcard_import_aliases(
    context: &mut ProjectContext,
    wildcard_imports: &[ResolvedWildcardImport],
) {
    if wildcard_imports.is_empty() {
        return;
    }
    let Some(modules) = context.modules.as_ref() else {
        return;
    };
    let Some(hir_output) = context.hir.as_mut() else {
        return;
    };
    let hir = &mut hir_output.hir;
    let module_to_hir = modules
        .modules
        .iter()
        .filter_map(|(module_id, module_info)| {
            let hir_module = hir.modules.iter().find_map(|hir_module| {
                let hir_module_data = hir.modules_arena.get(*hir_module)?;
                let path = hir_module_data.name.as_ref()?;
                (path
                    .segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .eq(module_info.path.segments.iter().map(String::as_str)))
                .then_some(*hir_module)
            })?;
            Some((module_id, hir_module))
        })
        .collect::<HashMap<ModuleId, etas_hir::HirModuleId>>();
    let part_scopes = modules
        .parts
        .iter()
        .filter_map(|(part_id, part)| {
            let hir_module = module_to_hir.get(&part.module).copied()?;
            let scope = hir_part_scope(hir, hir_module, part.source)?;
            Some((part_id, (part.source, hir_module, scope)))
        })
        .collect::<HashMap<ModulePartId, (SourceId, etas_hir::HirModuleId, etas_hir::ScopeId)>>();
    let wildcard_name_counts = wildcard_import_name_counts(wildcard_imports);

    for wildcard in wildcard_imports {
        let Some((_, hir_module, part_scope)) = part_scopes.get(&wildcard.from_part).copied()
        else {
            continue;
        };
        let target_path = wildcard.target_module.module_path(modules).segments;
        for name in &wildcard.exported_names {
            let key = (wildcard.from, wildcard.from_part, name.clone());
            if wildcard_name_counts.get(&key).copied().unwrap_or_default() != 1 {
                continue;
            }
            if !matches!(
                hir.scopes.lookup(part_scope, name),
                etas_hir::ResolveResult::Unresolved
            ) {
                continue;
            }
            let mut alias_path = target_path.clone();
            alias_path.push(name.clone());
            let symbol = hir.symbols.alloc(etas_hir::SymbolData {
                name: name.clone(),
                kind: etas_hir::SymbolKind::Import,
                visibility: wildcard.visibility,
                defining_module: hir_module,
                defining_item: None,
                def: etas_hir::SymbolDef::ImportAlias {
                    path: alias_path,
                    origin: etas_hir::ImportAliasOrigin::SourceImport,
                },
                declared_type: None,
                definition_span: wildcard.span,
            });
            hir.scopes.insert(part_scope, name.clone(), symbol);
            if let crate::ResolvedModuleTarget::Source { module, .. } = &wildcard.target_module
                && let Some(export) = modules
                    .modules
                    .get(*module)
                    .and_then(|module| module.visibility_exports.items.get(name))
                && let Some(item) = context
                    .hir_item_bindings
                    .as_ref()
                    .and_then(|bindings| bindings.ast_to_hir.get(&export.item))
            {
                super::source_members::bind_members(hir, symbol, *item);
            }
        }
    }

    let resolved_alias_paths = resolve_wildcard_alias_paths(hir, &part_scopes);
    prune_resolved_wildcard_diagnostics(&mut hir.diagnostics, &resolved_alias_paths);
    prune_resolved_wildcard_diagnostics(&mut context.diagnostics, &resolved_alias_paths);
}

fn install_explicit_import_aliases(context: &mut ProjectContext, imports: &[ResolvedImport]) {
    if imports.is_empty() {
        return;
    }
    let Some(modules) = context.modules.as_ref() else {
        return;
    };
    let Some(catalog) = context.module_catalog.as_ref() else {
        return;
    };
    let Some(hir_output) = context.hir.as_mut() else {
        return;
    };
    let hir = &mut hir_output.hir;
    let module_to_hir = modules
        .modules
        .iter()
        .filter_map(|(module_id, module_info)| {
            let hir_module = hir.modules.iter().find_map(|hir_module| {
                let hir_module_data = hir.modules_arena.get(*hir_module)?;
                let path = hir_module_data.name.as_ref()?;
                (path
                    .segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .eq(module_info.path.segments.iter().map(String::as_str)))
                .then_some(*hir_module)
            })?;
            Some((module_id, hir_module))
        })
        .collect::<HashMap<ModuleId, etas_hir::HirModuleId>>();
    let part_scopes = modules
        .parts
        .iter()
        .filter_map(|(part_id, part)| {
            let hir_module = module_to_hir.get(&part.module).copied()?;
            let scope = hir_part_scope(hir, hir_module, part.source)?;
            Some((part_id, scope))
        })
        .collect::<HashMap<ModulePartId, etas_hir::ScopeId>>();

    for import in imports {
        let Some(target_path) = canonical_import_target_path(&import.target, catalog) else {
            continue;
        };
        let Some(scope) = part_scopes.get(&import.from_part).copied() else {
            continue;
        };
        let etas_hir::ResolveResult::Resolved(symbol) =
            hir.scopes.lookup(scope, &import.local_name)
        else {
            continue;
        };
        let binding_symbol = symbol;
        let Some(symbol) = hir.symbols.get_mut(symbol) else {
            continue;
        };
        let etas_hir::SymbolDef::ImportAlias { path, .. } = &mut symbol.def else {
            continue;
        };
        *path = target_path;
        if let ImportTarget::SourceItem { item, .. } = &import.target
            && let Some(item) = context
                .hir_item_bindings
                .as_ref()
                .and_then(|bindings| bindings.ast_to_hir.get(item))
        {
            super::source_members::bind_members(hir, binding_symbol, *item);
        }
    }
}

fn canonical_import_target_path(
    target: &ImportTarget,
    catalog: &ModuleCatalog,
) -> Option<Vec<String>> {
    match target {
        ImportTarget::Module(module) => catalog
            .modules
            .get(module.key())
            .map(|record| record.path.segments.clone()),
        ImportTarget::SourceItem {
            module_key, name, ..
        }
        | ImportTarget::StdItem {
            module_key, name, ..
        }
        | ImportTarget::ExternalItem {
            module_key, name, ..
        } => {
            let mut path = catalog.modules.get(*module_key)?.path.segments.clone();
            path.push(name.clone());
            Some(path)
        }
    }
}

fn wildcard_import_name_counts(
    wildcard_imports: &[ResolvedWildcardImport],
) -> HashMap<(ModuleId, ModulePartId, String), usize> {
    let mut counts = HashMap::new();
    for wildcard in wildcard_imports {
        for name in &wildcard.exported_names {
            *counts
                .entry((wildcard.from, wildcard.from_part, name.clone()))
                .or_insert(0) += 1;
        }
    }
    counts
}

fn hir_part_scope(
    hir: &etas_hir::HirProgram,
    hir_module: etas_hir::HirModuleId,
    source: SourceId,
) -> Option<etas_hir::ScopeId> {
    let module_scope = hir.modules_arena.get(hir_module)?.scope;
    hir.scopes.iter().find_map(|scope| {
        (scope.owner == etas_hir::ScopeOwner::Module(hir_module)
            && scope.parent == Some(module_scope)
            && scope.span.source == source)
            .then_some(scope.id)
    })
}

fn resolve_wildcard_alias_paths(
    hir: &mut etas_hir::HirProgram,
    part_scopes: &HashMap<ModulePartId, (SourceId, etas_hir::HirModuleId, etas_hir::ScopeId)>,
) -> Vec<Span> {
    let source_scopes = part_scopes
        .values()
        .map(|(source, _, scope)| (*source, *scope))
        .collect::<HashMap<_, _>>();
    let mut resolved_spans = Vec::new();
    let expr_updates = hir
        .exprs
        .iter()
        .filter_map(|(expr_id, expr)| {
            let etas_hir::HirExpr::Path(path) = expr else {
                return None;
            };
            wildcard_alias_resolution(path, &source_scopes, hir)
                .map(|resolution| (expr_id, resolution, path.span))
        })
        .collect::<Vec<_>>();
    for (expr_id, resolution, span) in expr_updates {
        if let Some(etas_hir::HirExpr::Path(path)) = hir.exprs.get_mut(expr_id) {
            path.resolution = resolution;
            resolved_spans.push(span);
        }
    }

    let type_updates = hir
        .types
        .iter()
        .filter_map(|(type_id, ty)| {
            let etas_hir::HirType::Path { path, .. } = ty else {
                return None;
            };
            wildcard_alias_resolution(path, &source_scopes, hir)
                .map(|resolution| (type_id, resolution, path.span))
        })
        .collect::<Vec<_>>();
    for (type_id, resolution, span) in type_updates {
        if let Some(etas_hir::HirType::Path { path, .. }) = hir.types.get_mut(type_id) {
            path.resolution = resolution;
            resolved_spans.push(span);
        }
    }
    resolved_spans
}

fn wildcard_alias_resolution(
    path: &etas_hir::ResolvedPath,
    source_scopes: &HashMap<SourceId, etas_hir::ScopeId>,
    hir: &etas_hir::HirProgram,
) -> Option<etas_hir::ResolveResult> {
    if !matches!(path.resolution, etas_hir::ResolveResult::Unresolved) || path.segments.len() != 1 {
        return None;
    }
    let name = path.segments.first()?.name.as_str();
    let scope = source_scopes.get(&path.span.source).copied()?;
    match hir.scopes.lookup(scope, name) {
        etas_hir::ResolveResult::Resolved(symbol)
            if hir.symbols.get(symbol).is_some_and(|symbol| {
                matches!(
                    symbol.def,
                    etas_hir::SymbolDef::ImportAlias {
                        origin: etas_hir::ImportAliasOrigin::SourceImport,
                        ..
                    }
                )
            }) =>
        {
            Some(etas_hir::ResolveResult::Resolved(symbol))
        }
        _ => None,
    }
}

fn prune_resolved_wildcard_diagnostics(diagnostics: &mut Vec<Diagnostic>, resolved_spans: &[Span]) {
    diagnostics.retain(|diagnostic| {
        !matches!(
            diagnostic.code,
            DiagnosticCode::Name(NameDiagnosticCode::UnresolvedName)
        ) || !resolved_spans.contains(&diagnostic.primary.span)
    });
}

fn prune_resolved_external_action_diagnostics(
    diagnostics: &mut Vec<Diagnostic>,
    resolved_spans: &[Span],
) {
    diagnostics.retain(|diagnostic| {
        !matches!(
            diagnostic.code,
            DiagnosticCode::Name(NameDiagnosticCode::UnresolvedEffectAction)
        ) || !resolved_spans
            .iter()
            .any(|span| span_contains(*span, diagnostic.primary.span))
    });
}

fn span_contains(container: Span, contained: Span) -> bool {
    container.source == contained.source
        && container.range.start <= contained.range.start
        && container.range.end >= contained.range.end
}
