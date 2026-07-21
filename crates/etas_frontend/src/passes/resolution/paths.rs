use std::collections::HashMap;

use etas_core::{Diagnostic, SourceId, Span, SyntaxDiagnosticCode, TextSize};
use etas_hir::PartialResolution;
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::{
    HirExprPathResolution, HirPathResolution, ModulePartId, ModulePath, ProjectContext,
    ResolvedImport, ResolvedImports, ResolvedModulePath, ResolvedModuleTarget, ResolvedPaths,
};

use super::super::artifacts::{
    HIR_OUTPUT, RESOLVED_IMPORTS, RESOLVED_PATHS, global_with_diagnostics,
};

pub struct ResolvePathsPass;

impl Pass<ProjectContext> for ResolvePathsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ResolvePathsPass", PassKind::Analysis)
            .requires(ArtifactSet::from([HIR_OUTPUT, RESOLVED_IMPORTS]))
            .produces(global_with_diagnostics([RESOLVED_PATHS]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let hir = context.hir.as_ref().expect("HIR output should exist");
        let modules = context.modules.as_ref().expect("module index should exist");
        let resolved_imports = context
            .resolved_imports
            .as_ref()
            .expect("resolved imports should exist");
        let mut module_names = Vec::new();
        let mut import_targets = Vec::new();
        let mut expr_paths = Vec::new();
        let mut diagnostics = Vec::new();
        for module_id in &hir.hir.modules {
            let module = hir
                .hir
                .modules_arena
                .get(*module_id)
                .expect("HIR module id should resolve");
            if let Some(path) = &module.name {
                module_names.push(ResolvedModulePath {
                    module: *module_id,
                    path: path.clone(),
                });
            }
            import_targets.extend(module.imports.iter().map(|import| import.target.clone()));
        }
        let source_to_module = modules
            .by_source
            .iter()
            .map(|(source, module)| (*source, *module))
            .collect::<HashMap<SourceId, crate::ModuleId>>();
        let source_to_part = modules
            .parts
            .iter()
            .map(|(_, part)| (part.source, part.id))
            .collect::<HashMap<SourceId, ModulePartId>>();
        for (expr_id, expr) in hir.hir.exprs.iter() {
            let etas_hir::HirExpr::Path(path) = expr else {
                continue;
            };
            let result = match &path.resolution {
                etas_hir::ResolveResult::Resolved(symbol) => resolved_symbol_expr_path_fact(
                    *symbol,
                    path,
                    &hir.hir,
                    &source_to_module,
                    &source_to_part,
                    resolved_imports,
                ),
                etas_hir::ResolveResult::Ambiguous(symbols) => HirPathResolution::PartialSymbol {
                    prefix: symbols
                        .first()
                        .copied()
                        .expect("ambiguous path should include at least one symbol"),
                    resolved_segments: 0,
                    remaining: path
                        .segments
                        .iter()
                        .map(|segment| segment.name.clone())
                        .collect(),
                    reason: etas_hir::PartialResolutionReason::UnsupportedPathShape,
                },
                etas_hir::ResolveResult::PartiallyResolved(partial) => partial_expr_path_fact(
                    partial,
                    path,
                    &hir.hir,
                    &source_to_module,
                    &source_to_part,
                    resolved_imports,
                ),
                etas_hir::ResolveResult::Unresolved => unresolved_expr_path_fact(
                    path,
                    &source_to_module,
                    &source_to_part,
                    resolved_imports,
                    &mut diagnostics,
                ),
            };
            expr_paths.push(HirExprPathResolution {
                expr: expr_id,
                path: path.clone(),
                result,
            });
        }
        context.diagnostics.extend(diagnostics);
        context.resolved_paths = Some(ResolvedPaths {
            module_names,
            import_targets,
            expr_paths,
        });
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(RESOLVED_PATHS))
    }
}

fn resolved_symbol_expr_path_fact(
    symbol: etas_hir::SymbolId,
    path: &etas_hir::ResolvedPath,
    hir: &etas_hir::HirProgram,
    source_to_module: &HashMap<SourceId, crate::ModuleId>,
    source_to_part: &HashMap<SourceId, ModulePartId>,
    resolved_imports: &ResolvedImports,
) -> HirPathResolution {
    if hir.symbols.get(symbol).is_some_and(source_import_alias)
        && path.segments.len() == 1
        && let Some(name) = path.segments.first().map(|segment| segment.name.as_str())
        && let Some(module) = source_to_module.get(&path.span.source).copied()
        && let Some(part) = source_to_part.get(&path.span.source).copied()
    {
        if let Some(import) = explicit_import_for_name(resolved_imports, module, part, name) {
            return HirPathResolution::ExplicitImport {
                target: import.target.clone(),
            };
        }
        if let [target] = wildcard_sources_for_name(resolved_imports, module, part, name).as_slice()
        {
            return HirPathResolution::WildcardImport {
                target: target.clone(),
            };
        }
    }
    HirPathResolution::DirectSymbol(symbol)
}

fn unresolved_expr_path_fact(
    path: &etas_hir::ResolvedPath,
    source_to_module: &HashMap<SourceId, crate::ModuleId>,
    source_to_part: &HashMap<SourceId, ModulePartId>,
    resolved_imports: &ResolvedImports,
    diagnostics: &mut Vec<Diagnostic>,
) -> HirPathResolution {
    if path.segments.len() != 1 {
        return HirPathResolution::Unresolved;
    }
    let Some(name) = path.segments.first().map(|segment| segment.name.as_str()) else {
        return HirPathResolution::Unresolved;
    };
    let Some(module) = source_to_module.get(&path.span.source).copied() else {
        return HirPathResolution::Unresolved;
    };
    let Some(part) = source_to_part.get(&path.span.source).copied() else {
        return HirPathResolution::Unresolved;
    };
    if let Some(import) = explicit_import_for_name(resolved_imports, module, part, name) {
        return HirPathResolution::ExplicitImport {
            target: import.target.clone(),
        };
    }
    let wildcard_sources = wildcard_sources_for_name(resolved_imports, module, part, name);
    match wildcard_sources.as_slice() {
        [] => HirPathResolution::Unresolved,
        [target] => HirPathResolution::WildcardImport {
            target: target.clone(),
        },
        _ => {
            diagnostics.push(project_diagnostic(
                path.span,
                &format!("wildcard import ambiguity for `{name}`"),
            ));
            HirPathResolution::AmbiguousWildcard {
                targets: wildcard_sources,
            }
        }
    }
}

fn partial_expr_path_fact(
    partial: &PartialResolution,
    path: &etas_hir::ResolvedPath,
    hir: &etas_hir::HirProgram,
    source_to_module: &HashMap<SourceId, crate::ModuleId>,
    source_to_part: &HashMap<SourceId, ModulePartId>,
    resolved_imports: &ResolvedImports,
) -> HirPathResolution {
    let Some(prefix) = partial.resolved_prefix else {
        return HirPathResolution::PartiallyResolved {
            resolved_segments: partial.resolved_segments,
            remaining: partial.remaining.clone(),
            reason: partial.reason,
        };
    };

    if partial.resolved_segments == 1
        && hir.symbols.get(prefix).is_some_and(source_import_alias)
        && let Some(module) = source_to_module.get(&path.span.source).copied()
        && let Some(part) = source_to_part.get(&path.span.source).copied()
        && let Some(local_name) = path.segments.first().map(|segment| segment.name.as_str())
        && let Some(import) = explicit_import_for_name(resolved_imports, module, part, local_name)
    {
        return HirPathResolution::PartialImport {
            target: import.target.clone(),
            resolved_segments: partial.resolved_segments,
            remaining: partial.remaining.clone(),
            reason: partial.reason,
        };
    }

    HirPathResolution::PartialSymbol {
        prefix,
        resolved_segments: partial.resolved_segments,
        remaining: partial.remaining.clone(),
        reason: partial.reason,
    }
}

fn source_import_alias(symbol: &etas_hir::Symbol) -> bool {
    matches!(
        &symbol.def,
        etas_hir::SymbolDef::ImportAlias {
            origin: etas_hir::ImportAliasOrigin::SourceImport,
            ..
        }
    )
}

fn explicit_import_for_name<'a>(
    imports: &'a ResolvedImports,
    module: crate::ModuleId,
    part: ModulePartId,
    name: &str,
) -> Option<&'a ResolvedImport> {
    imports.imports.iter().find(|import| {
        import.from == module && import.from_part == part && import.local_name == name
    })
}

fn wildcard_sources_for_name(
    imports: &ResolvedImports,
    module: crate::ModuleId,
    part: ModulePartId,
    name: &str,
) -> Vec<ResolvedModuleTarget> {
    let mut sources = imports
        .wildcard_imports
        .iter()
        .filter(|import| import.from == module && import.from_part == part)
        .filter(|import| import.exported_names.iter().any(|export| export == name))
        .map(|import| import.target_module.clone())
        .collect::<Vec<_>>();
    sources.sort_by_key(resolved_module_target_sort_key);
    sources.dedup();
    sources
}

fn resolved_module_target_sort_key(target: &ResolvedModuleTarget) -> String {
    match target {
        ResolvedModuleTarget::Source { key, module } => {
            format!("source:{}:{}", key.0, module.0)
        }
        ResolvedModuleTarget::Std { path, .. } => format!("std:{}", module_path_text(path)),
        ResolvedModuleTarget::External { path, .. } => {
            format!("external:{}", module_path_text(path))
        }
    }
}

fn module_path_text(path: &ModulePath) -> String {
    path.segments.join(".")
}

fn project_diagnostic(span: Span, message: &str) -> Diagnostic {
    let span = if span.range.start == span.range.end {
        Span::empty(span.source, TextSize::ZERO)
    } else {
        span
    };
    Diagnostic::syntax(SyntaxDiagnosticCode::InvalidItem, span, message)
}
