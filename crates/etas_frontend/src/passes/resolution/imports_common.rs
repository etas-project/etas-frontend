use std::collections::HashMap;

use etas_core::{Diagnostic, Span, SyntaxDiagnosticCode, TextSize};
use etas_syntax::ast;

use crate::passes::{
    ModuleResolution, ProjectModuleResolver, ProviderExport, ProviderExportTarget,
};
use crate::{
    ImportTarget, ModulePartId, ModulePath, ParsedSource, ResolvedImport, ResolvedModuleTarget,
    ResolvedWildcardImport,
};

pub(super) enum ImportResolution {
    Explicit(ResolvedImport),
    Wildcard(ResolvedWildcardImport),
}

impl ImportTarget {
    pub(super) fn module_path(&self, modules: &crate::ModuleIndex) -> ModulePath {
        match self {
            Self::Module(module) => module.module_path(modules),
            Self::SourceItem { module, .. } => modules
                .modules
                .get(*module)
                .expect("source import module should exist")
                .path
                .clone(),
            Self::StdItem { module_path, .. } | Self::ExternalItem { module_path, .. } => {
                module_path.clone()
            }
        }
    }
}

impl ResolvedModuleTarget {
    pub(super) fn module_path(&self, modules: &crate::ModuleIndex) -> ModulePath {
        match self {
            Self::Source { module, .. } => modules
                .modules
                .get(*module)
                .expect("source import module should exist")
                .path
                .clone(),
            Self::Std { path, .. } | Self::External { path, .. } => path.clone(),
        }
    }
}

pub(super) fn resolve_import_tree(
    import: &ast::ImportDecl,
    import_ref: &crate::AstImportRef,
    from: crate::ModuleId,
    from_part: ModulePartId,
    modules: &crate::ModuleIndex,
    resolver: &ProjectModuleResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<ImportResolution> {
    match &import.tree {
        ast::ImportTree::Single { path, alias, span } => resolve_single_import(
            import,
            import_ref,
            from,
            from_part,
            path,
            alias.as_ref(),
            *span,
            modules,
            resolver,
            diagnostics,
        ),
        ast::ImportTree::Group { prefix, items, .. } => {
            let Some(module) = resolve_module_target(
                &ModulePath::from_ast(prefix),
                import.span,
                resolver,
                diagnostics,
            ) else {
                return Vec::new();
            };
            items
                .iter()
                .filter_map(|item| {
                    let export = resolve_exported_item(
                        module.clone(),
                        from,
                        &item.name.text,
                        item.span,
                        modules,
                        resolver,
                        diagnostics,
                    )?;
                    let target = import_target_for_export(&item.name.text, export);
                    Some(ImportResolution::Explicit(ResolvedImport {
                        from,
                        from_part,
                        import: import_ref.clone(),
                        target,
                        local_name: item
                            .alias
                            .as_ref()
                            .map_or_else(|| item.name.text.clone(), |alias| alias.text.clone()),
                        visibility: hir_visibility(import.visibility),
                        span: item.span,
                    }))
                })
                .collect()
        }
        ast::ImportTree::Wildcard { prefix, span, .. } => {
            let Some(module) =
                resolve_module_target(&ModulePath::from_ast(prefix), *span, resolver, diagnostics)
            else {
                return Vec::new();
            };
            let exported_names = resolver
                .exports(&module)
                .expect("resolved wildcard module should expose exports")
                .items
                .values()
                .filter(|item| {
                    item.visibility == etas_hir::Visibility::Public
                        || matches!(module, ResolvedModuleTarget::Source { module: source, .. } if source == from)
                })
                .map(|item| item.name.clone())
                .collect();
            vec![ImportResolution::Wildcard(ResolvedWildcardImport {
                from,
                from_part,
                import: import_ref.clone(),
                target_module: module,
                exported_names,
                visibility: hir_visibility(import.visibility),
                span: *span,
            })]
        }
        ast::ImportTree::Error { .. } => Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_single_import(
    import: &ast::ImportDecl,
    import_ref: &crate::AstImportRef,
    from: crate::ModuleId,
    from_part: ModulePartId,
    path: &ast::Path,
    alias: Option<&ast::Name>,
    span: Span,
    modules: &crate::ModuleIndex,
    resolver: &ProjectModuleResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<ImportResolution> {
    let full_path = ModulePath::from_ast(path);
    match resolver.resolve_module(&full_path) {
        ModuleResolution::Source { key, module } => {
            return vec![ImportResolution::Explicit(ResolvedImport {
                from,
                from_part,
                import: import_ref.clone(),
                target: ImportTarget::Module(ResolvedModuleTarget::Source { key, module }),
                local_name: alias.map_or_else(
                    || full_path.segments.last().cloned().unwrap_or_default(),
                    |alias| alias.text.clone(),
                ),
                visibility: hir_visibility(import.visibility),
                span,
            })];
        }
        ModuleResolution::Missing => {}
        ModuleResolution::Ambiguous(_) => {
            diagnostics.push(project_diagnostic(
                span,
                &format!(
                    "ambiguous imported module `{}`",
                    module_path_text(&full_path)
                ),
            ));
            return Vec::new();
        }
        ModuleResolution::VirtualStd { key, module, path } => {
            return vec![ImportResolution::Explicit(ResolvedImport {
                from,
                from_part,
                import: import_ref.clone(),
                target: ImportTarget::Module(ResolvedModuleTarget::Std {
                    key,
                    module,
                    path: path.clone(),
                }),
                local_name: alias.map_or_else(
                    || full_path.segments.last().cloned().unwrap_or_default(),
                    |alias| alias.text.clone(),
                ),
                visibility: hir_visibility(import.visibility),
                span,
            })];
        }
        ModuleResolution::External {
            key,
            package,
            module,
            path,
        } => {
            return vec![ImportResolution::Explicit(ResolvedImport {
                from,
                from_part,
                import: import_ref.clone(),
                target: ImportTarget::Module(ResolvedModuleTarget::External {
                    key,
                    package,
                    module,
                    path: path.clone(),
                }),
                local_name: alias.map_or_else(
                    || full_path.segments.last().cloned().unwrap_or_default(),
                    |alias| alias.text.clone(),
                ),
                visibility: hir_visibility(import.visibility),
                span,
            })];
        }
    }

    let Some((module_path, member)) = split_module_member_path(&full_path, resolver) else {
        diagnostics.push(project_diagnostic(
            span,
            &format!("missing imported module `{}`", module_path_text(&full_path)),
        ));
        return Vec::new();
    };
    let Some(module) = resolve_module_target(&module_path, span, resolver, diagnostics) else {
        return Vec::new();
    };
    let Some(export) =
        resolve_exported_item(module, from, &member, span, modules, resolver, diagnostics)
    else {
        return Vec::new();
    };
    let target = import_target_for_export(&member, export);
    vec![ImportResolution::Explicit(ResolvedImport {
        from,
        from_part,
        import: import_ref.clone(),
        target,
        local_name: alias.map_or(member, |alias| alias.text.clone()),
        visibility: hir_visibility(import.visibility),
        span,
    })]
}

fn resolve_module_target(
    path: &ModulePath,
    span: Span,
    resolver: &ProjectModuleResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<ResolvedModuleTarget> {
    match resolver.resolve_module(path) {
        ModuleResolution::Source { key, module } => {
            Some(ResolvedModuleTarget::Source { key, module })
        }
        ModuleResolution::Missing => {
            diagnostics.push(project_diagnostic(
                span,
                &format!("missing imported module `{}`", module_path_text(path)),
            ));
            None
        }
        ModuleResolution::Ambiguous(_) => {
            diagnostics.push(project_diagnostic(
                span,
                &format!("ambiguous imported module `{}`", module_path_text(path)),
            ));
            None
        }
        ModuleResolution::VirtualStd { key, module, path } => {
            Some(ResolvedModuleTarget::Std { key, module, path })
        }
        ModuleResolution::External {
            key,
            package,
            module,
            path,
        } => Some(ResolvedModuleTarget::External {
            key,
            package,
            module,
            path,
        }),
    }
}

fn resolve_exported_item(
    module: ResolvedModuleTarget,
    from: crate::ModuleId,
    name: &str,
    span: Span,
    modules: &crate::ModuleIndex,
    resolver: &ProjectModuleResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<ProviderExport> {
    let module_path = module.module_path(modules);
    let Some(exports) = resolver.exports(&module) else {
        diagnostics.push(project_diagnostic(
            span,
            &format!(
                "missing exports for module `{}`",
                module_path_text(&module_path)
            ),
        ));
        return None;
    };
    let Some(export) = exports.items.get(name) else {
        diagnostics.push(project_diagnostic(
            span,
            &format!(
                "missing exported item `{}` in module `{}`",
                name,
                module_path_text(&module_path)
            ),
        ));
        return None;
    };
    if !matches!(module, ResolvedModuleTarget::Source { module: source, .. } if source == from)
        && export.visibility != etas_hir::Visibility::Public
    {
        diagnostics.push(project_diagnostic(
            span,
            &format!(
                "imported item `{}` from module `{}` is private",
                name,
                module_path_text(&module_path)
            ),
        ));
        return None;
    }
    Some(export.clone())
}

fn import_target_for_export(name: &str, export: ProviderExport) -> ImportTarget {
    match export.target {
        ProviderExportTarget::Source {
            module_key,
            module,
            item,
        } => ImportTarget::SourceItem {
            module_key,
            module,
            name: name.to_owned(),
            item,
        },
        ProviderExportTarget::Std {
            module_key,
            module,
            module_path,
            symbol,
        } => ImportTarget::StdItem {
            module_key,
            module,
            module_path,
            name: name.to_owned(),
            symbol,
        },
        ProviderExportTarget::External {
            module_key,
            package,
            module,
            module_path,
            symbol,
        } => ImportTarget::ExternalItem {
            module_key,
            package,
            module,
            module_path,
            name: name.to_owned(),
            symbol,
        },
    }
}

fn split_module_member_path(
    path: &ModulePath,
    resolver: &ProjectModuleResolver<'_>,
) -> Option<(ModulePath, String)> {
    (1..path.segments.len()).rev().find_map(|len| {
        let module_path = ModulePath {
            segments: path.segments[..len].to_vec(),
        };
        matches!(
            resolver.resolve_module(&module_path),
            ModuleResolution::Source { .. }
                | ModuleResolution::VirtualStd { .. }
                | ModuleResolution::External { .. }
        )
        .then(|| (module_path, path.segments[len..].join(".")))
    })
}

pub(super) fn duplicate_explicit_import_diagnostics(imports: &[ResolvedImport]) -> Vec<Diagnostic> {
    let mut seen = HashMap::<(ModulePartId, String), Span>::new();
    let mut diagnostics = Vec::new();
    for import in imports {
        let key = (import.from_part, import.local_name.clone());
        if let Some(previous) = seen.insert(key, import.span) {
            diagnostics.push(project_diagnostic(
                import.span,
                &format!("duplicate explicit import `{}`", import.local_name),
            ));
            diagnostics.push(project_diagnostic(
                previous,
                &format!("previous explicit import `{}` is here", import.local_name),
            ));
        }
    }
    diagnostics
}

pub(super) fn part_module(
    modules: &crate::ModuleIndex,
    import: &crate::AstImportRef,
) -> crate::ModuleId {
    let part = import
        .module_part
        .expect("resolved import should be attached to a module part");
    modules
        .parts
        .get(part)
        .expect("import module part should exist")
        .module
}

fn hir_visibility(visibility: ast::Visibility) -> etas_hir::Visibility {
    match visibility {
        ast::Visibility::Private => etas_hir::Visibility::Private,
        ast::Visibility::Public => etas_hir::Visibility::Public,
    }
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

pub(super) fn parsed_by_source(
    context: &crate::ProjectContext,
) -> HashMap<etas_core::SourceId, &ParsedSource> {
    context
        .parsed_sources
        .iter()
        .map(|parsed| (parsed.source, parsed))
        .collect()
}
