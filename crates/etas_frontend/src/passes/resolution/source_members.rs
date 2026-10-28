use etas_hir::{
    HirExpr, HirItem, HirItemId, HirPat, HirProgram, PathSegment, ResolveResult, ResolvedPath,
    SymbolId,
};

pub(super) fn bind_module_members(
    hir: &mut HirProgram,
    alias: SymbolId,
    exports: &crate::ModuleExportTable,
    bindings: &crate::HirItemBindings,
    symbols_by_item: &std::collections::HashMap<HirItemId, SymbolId>,
    std_registry: &etas_std::StdRegistry,
    external_modules: &[crate::ProjectExternalModuleInput],
) -> Result<Vec<(SymbolId, crate::project::HirExternalImportMember)>, String> {
    let owner = hir
        .symbols
        .get(alias)
        .ok_or("module import alias has no HIR symbol")?
        .clone();
    let mut exports = exports.items.iter().collect::<Vec<_>>();
    let mut external_bindings = Vec::new();
    exports.sort_by_key(|(name, _)| *name);
    for (name, export) in exports {
        if export.visibility != etas_hir::Visibility::Public {
            continue;
        }
        let symbol = match &export.target {
            crate::ModuleExportTarget::Source { item, .. } => {
                let item = bindings
                    .ast_to_hir
                    .get(item)
                    .ok_or_else(|| format!("module export `{name}` has no HIR item binding"))?;
                *symbols_by_item
                    .get(item)
                    .ok_or_else(|| format!("module export `{name}` has no HIR symbol"))?
            }
            crate::ModuleExportTarget::Std { symbol, .. } => {
                let declaration = std_registry.symbol(*symbol).ok_or_else(|| {
                    format!("module export `{name}` is absent from the standard registry")
                })?;
                hir.symbols.alloc(etas_hir::SymbolData {
                    name: name.clone(),
                    kind: etas_hir::SymbolKind::Import,
                    visibility: owner.visibility,
                    defining_module: owner.defining_module,
                    defining_item: None,
                    def: etas_hir::SymbolDef::ImportAlias {
                        path: declaration.qualified_path.clone(),
                        origin: etas_hir::ImportAliasOrigin::SourceImport,
                    },
                    declared_type: None,
                    definition_span: owner.definition_span,
                })
            }
            crate::ModuleExportTarget::External {
                package,
                module,
                module_path,
                symbol,
            } => {
                let package = package.ok_or("external module member has no package identity")?;
                let metadata = external_modules
                    .iter()
                    .find(|metadata| metadata.package == Some(package) && metadata.id == *module)
                    .ok_or("external module member has no validated module")?;
                if metadata.path != *module_path {
                    return Err("external module member disagrees with its catalog path".into());
                }
                let declaration = metadata
                    .exports
                    .iter()
                    .find(|export| export.symbol == *symbol)
                    .ok_or("external module member has no validated export")?;
                if declaration.visibility != etas_hir::Visibility::Public {
                    return Err("external module member is not public".into());
                }
                let mut path = module_path.segments.clone();
                path.push(declaration.name.clone());
                let symbol = hir.symbols.alloc(etas_hir::SymbolData {
                    name: name.clone(),
                    kind: etas_hir::SymbolKind::Import,
                    visibility: owner.visibility,
                    defining_module: owner.defining_module,
                    defining_item: None,
                    def: etas_hir::SymbolDef::ImportAlias {
                        path: path.clone(),
                        origin: etas_hir::ImportAliasOrigin::SourceImport,
                    },
                    declared_type: None,
                    definition_span: owner.definition_span,
                });
                external_bindings.push((
                    symbol,
                    crate::project::HirExternalImportMember {
                        package,
                        path,
                        span: owner.definition_span,
                    },
                ));
                symbol
            }
        };
        hir.symbols.insert_member(alias, name.clone(), symbol);
    }
    Ok(external_bindings)
}

pub(super) fn source_item_symbols(
    hir: &HirProgram,
) -> std::collections::HashMap<HirItemId, SymbolId> {
    hir.items
        .iter()
        .filter_map(|(item, decl)| match decl {
            HirItem::TypeAlias(decl) => Some((item, decl.symbol)),
            HirItem::Type(decl) => Some((item, decl.symbol)),
            HirItem::Enum(decl) => Some((item, decl.symbol)),
            HirItem::Spec(decl) => Some((item, decl.symbol)),
            HirItem::Effect(decl) => Some((item, decl.symbol)),
            HirItem::Flow(decl) => Some((item, decl.symbol)),
            HirItem::Agent(decl) => Some((item, decl.symbol)),
            HirItem::Tool(decl) => Some((item, decl.symbol)),
            HirItem::Protocol(decl) => Some((item, decl.symbol)),
            HirItem::TopLevelLet(decl) => Some((item, decl.symbol)),
            _ => None,
        })
        .collect()
}

pub(super) fn canonicalize_agent_method_owners(hir: &mut HirProgram) {
    for method in hir.agent_methods.values_mut() {
        if let Some(target) = hir.source_import_targets.get(&method.agent).copied() {
            hir.symbols.rebind_member_owner(method.agent, target);
            method.agent = target;
        }
    }
    for (_, item) in hir.items.iter_mut() {
        if let HirItem::Impl(decl) = item
            && let etas_hir::HirImplTarget::Agent { target, .. } = &mut decl.target
            && let ResolveResult::Resolved(symbol) = &target.resolution
            && let Some(canonical) = hir.source_import_targets.get(symbol)
        {
            target.resolution = ResolveResult::Resolved(*canonical);
        }
    }
    let bindings = hir
        .source_import_targets
        .iter()
        .map(|(alias, target)| (*alias, *target))
        .collect::<Vec<_>>();
    for (alias, target) in bindings {
        if let Some(etas_hir::SymbolDef::Item { item }) =
            hir.symbols.get(target).map(|symbol| &symbol.def)
        {
            bind_members(hir, alias, *item);
        }
    }
}

/// Install associated members only after the import resolver has checked visibility.
pub(super) fn bind_members(hir: &mut HirProgram, alias: SymbolId, item: HirItemId) {
    if matches!(hir.items.get(item), Some(HirItem::Agent(_))) {
        let methods = hir.agent_methods.values().filter(|method| {
            matches!(hir.symbols.get(method.agent).map(|symbol| &symbol.def), Some(etas_hir::SymbolDef::Item { item: owner }) if *owner == item)
                && method.implementation.is_none_or(|implementation| {
                    matches!(hir.items.get(implementation), Some(HirItem::Impl(decl)) if matches!(&decl.target, etas_hir::HirImplTarget::Agent { specs, .. } if specs.is_empty()))
                })
        }).map(|method| method.symbol).collect::<Vec<_>>();
        for method in methods {
            if let Some(symbol) = hir.symbols.get(method)
                && symbol.visibility == etas_hir::Visibility::Public
            {
                hir.symbols
                    .insert_member(alias, symbol.name.clone(), method);
            }
        }
        return;
    }
    let Some(HirItem::Enum(decl)) = hir.items.get(item) else {
        return;
    };
    for variant in &decl.variants {
        if let Some(symbol) = hir.symbols.get(variant.symbol) {
            hir.symbols
                .insert_member(alias, symbol.name.clone(), variant.symbol);
        }
    }
}

fn resolve_path(
    path: &mut ResolvedPath,
    symbols: &etas_hir::SymbolTable,
    scopes: &etas_hir::ScopeTree,
    source_map: &etas_hir::SourceMap,
) -> bool {
    if matches!(path.resolution, ResolveResult::Unresolved)
        && let [owner, name] = path.segments.as_slice()
        && let Some(scope) = scopes.iter().find(|scope| {
            matches!(scope.owner, etas_hir::ScopeOwner::Module(_))
                && scope.span.source == source_map.lexical_source(path.span.source)
                && scope.parent.is_some_and(|parent| {
                    scopes
                        .get(parent)
                        .is_some_and(|parent| parent.owner == scope.owner)
                })
        })
        && let ResolveResult::Resolved(owner) = scopes.lookup(scope.id, &owner.name)
    {
        let resolution = symbols.resolve_member(owner, &name.name);
        if !matches!(resolution, ResolveResult::Unresolved) {
            path.resolution = resolution;
            return true;
        }
    }
    if let ResolveResult::PartiallyResolved(partial) = &path.resolution
        && let Some(owner) = partial.resolved_prefix
        && !partial.remaining.is_empty()
    {
        let mut owner = owner;
        for (index, name) in partial.remaining.iter().enumerate() {
            match symbols.resolve_member(owner, name) {
                ResolveResult::Resolved(member) => owner = member,
                resolution @ ResolveResult::Ambiguous(_) => {
                    path.resolution = resolution;
                    return true;
                }
                ResolveResult::Unresolved if index > 0 => {
                    let Some(resolved_segments) = u32::try_from(index)
                        .ok()
                        .and_then(|count| partial.resolved_segments.checked_add(count))
                    else {
                        return false;
                    };
                    path.resolution =
                        ResolveResult::PartiallyResolved(etas_hir::PartialResolution {
                            resolved_prefix: Some(owner),
                            resolved_segments,
                            remaining: partial.remaining[index..].to_vec(),
                            reason: etas_hir::PartialResolutionReason::MemberRequiresTypeChecking,
                        });
                    return true;
                }
                _ => return false,
            }
        }
        path.resolution = ResolveResult::Resolved(owner);
        return true;
    }
    false
}

pub(super) struct ResolvedMembers {
    pub paths: Vec<etas_core::Span>,
    pub actions: Vec<etas_core::Span>,
}

pub(super) fn resolve_members(hir: &mut HirProgram) -> ResolvedMembers {
    let mut resolved = Vec::new();
    let mut resolved_actions = Vec::new();
    let exprs = hir
        .exprs
        .iter()
        .map(|(id, expr)| (id, expr.clone()))
        .collect::<Vec<_>>();
    for (id, mut expr) in exprs {
        match &mut expr {
            HirExpr::Reflect(reflection) => {
                for index in 0..2 {
                    if let Some(path) = reflection.target.path_mut(index)
                        && resolve_path(path, &hir.symbols, &hir.scopes, &hir.source_map)
                    {
                        resolved.push(path.span);
                    }
                }
            }
            HirExpr::Path(path) => {
                if resolve_path(path, &hir.symbols, &hir.scopes, &hir.source_map) {
                    resolved.push(path.span);
                }
            }
            HirExpr::Perform { action, .. } => {
                if resolve_path(
                    &mut action.effect.path,
                    &hir.symbols,
                    &hir.scopes,
                    &hir.source_map,
                ) {
                    resolved.push(action.effect.path.span);
                }
                if let Some(resolution) = source_action_resolution(hir, action) {
                    if matches!(resolution, ResolveResult::Resolved(_)) {
                        resolved_actions.push(action.span);
                    }
                    action.action_symbol = resolution;
                }
            }
            HirExpr::Record(record) => {
                if let Some(path) = &mut record.path
                    && resolve_path(path, &hir.symbols, &hir.scopes, &hir.source_map)
                {
                    resolved.push(path.span);
                }
            }
            HirExpr::MethodCall {
                receiver,
                method,
                generic_args,
                args,
                span,
            } => {
                if let Some(HirExpr::Path(path)) = hir.exprs.get(*receiver)
                    && let ResolveResult::Resolved(owner) = path.resolution
                {
                    let resolution = hir.symbols.resolve_member(owner, method);
                    if !matches!(resolution, ResolveResult::Unresolved) {
                        let mut path = path.clone();
                        path.segments.push(PathSegment {
                            name: method.clone(),
                            span: *span,
                        });
                        path.resolution = resolution;
                        if let Some(slot) = hir.exprs.get_mut(*receiver) {
                            *slot = HirExpr::Path(path);
                        }
                        expr = HirExpr::Call {
                            callee: *receiver,
                            generic_args: generic_args.clone(),
                            args: args.clone(),
                            span: *span,
                        };
                    }
                }
            }
            _ => {}
        }
        if let Some(slot) = hir.exprs.get_mut(id) {
            *slot = expr;
        }
    }
    let patterns = hir
        .pats
        .iter()
        .map(|(id, pat)| (id, pat.clone()))
        .collect::<Vec<_>>();
    for (id, mut pat) in patterns {
        match &mut pat {
            HirPat::Variant { path, .. }
            | HirPat::Record {
                path: Some(path), ..
            } => {
                if resolve_path(path, &hir.symbols, &hir.scopes, &hir.source_map) {
                    resolved.push(path.span);
                }
            }
            _ => {}
        }
        if let Some(slot) = hir.pats.get_mut(id) {
            *slot = pat;
        }
    }
    let arms = hir
        .handler_arms
        .iter()
        .map(|(id, arm)| (id, arm.clone()))
        .collect::<Vec<_>>();
    for (id, mut arm) in arms {
        if resolve_path(
            &mut arm.action.effect.path,
            &hir.symbols,
            &hir.scopes,
            &hir.source_map,
        ) {
            resolved.push(arm.action.effect.path.span);
        }
        if let Some(resolution) = source_action_resolution(hir, &arm.action) {
            if matches!(resolution, ResolveResult::Resolved(_)) {
                resolved_actions.push(arm.action.span);
            }
            arm.action.action_symbol = resolution;
        }
        if let Some(slot) = hir.handler_arms.get_mut(id) {
            *slot = arm;
        }
    }
    ResolvedMembers {
        paths: resolved,
        actions: resolved_actions,
    }
}

fn source_action_resolution(
    hir: &HirProgram,
    action: &etas_hir::ResolvedActionRef,
) -> Option<ResolveResult> {
    let ResolveResult::Resolved(owner) = action.effect.path.resolution else {
        return None;
    };
    let owner = hir
        .source_import_targets
        .get(&owner)
        .copied()
        .unwrap_or(owner);
    let Some(etas_hir::SymbolDef::Item { item }) = hir.symbols.get(owner).map(|symbol| &symbol.def)
    else {
        return None;
    };
    if !matches!(hir.items.get(*item), Some(HirItem::Effect(_))) {
        return None;
    }
    Some(hir.symbols.resolve_member(owner, &action.action))
}
