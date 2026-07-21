use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};

use etas_core::{Diagnostic, Span, SyntaxDiagnosticCode, TextSize};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{
    MODULE_INDEX, PARSED_SOURCE_SET, SOURCE_SET, global_with_diagnostics,
};
use crate::{
    ExportTable, ExportedItem, ModuleIndex, ModuleInfo, ModuleOrigin, ModulePart, ModulePath,
    ProjectContext, SourceFile, SourceKind,
};

pub struct BuildModuleIndexPass;

impl Pass<ProjectContext> for BuildModuleIndexPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("BuildModuleIndexPass", PassKind::Transform)
            .requires(ArtifactSet::from([SOURCE_SET, PARSED_SOURCE_SET]))
            .produces(global_with_diagnostics([MODULE_INDEX]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let mut index = ModuleIndex::default();
        let mut declared_names: HashMap<(crate::ModuleId, String), Span> = HashMap::new();
        let source_files = context
            .sources
            .as_ref()
            .map(|sources| {
                sources
                    .files
                    .iter()
                    .map(|file| (file.id, file))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        let source_root = context
            .sources
            .as_ref()
            .map(|sources| sources.source_root.as_path());
        let mut canonical_files: HashMap<ModulePath, (etas_core::SourceId, CanonicalRoot, Span)> =
            HashMap::new();
        let mut diagnostics = Vec::new();

        for parsed_index in 0..context.parsed_sources.len() {
            let parsed = &context.parsed_sources[parsed_index];
            let source_file = source_files
                .get(&parsed.source)
                .copied()
                .expect("parsed source should have a source file");
            let module_path = match parsed.declared_module.clone() {
                Some(path) => path,
                None if source_file.kind == SourceKind::SingleFileInput => {
                    ModulePath::synthetic_single_file()
                }
                None => {
                    diagnostics.push(project_diagnostic(
                        parsed.source,
                        parsed.parse.value.span,
                        "package source file is missing a `module` declaration",
                    ));
                    continue;
                }
            };

            if source_file.kind.is_source_project_file() {
                match source_path_mapping(source_file, source_root, &module_path) {
                    Some(SourcePathMapping::ModulePart {
                        canonical_root: Some(canonical_root),
                    }) => {
                        if let Some((previous_source, previous_root, previous_span)) =
                            canonical_files.insert(
                                module_path.clone(),
                                (parsed.source, canonical_root, parsed.parse.value.span),
                            )
                        {
                            if previous_source != parsed.source && previous_root != canonical_root {
                                diagnostics.push(project_diagnostic(
                                    parsed.source,
                                    parsed.parse.value.span,
                                    &format!(
                                        "ambiguous module files for `{}`",
                                        module_path_text(&module_path)
                                    ),
                                ));
                                diagnostics.push(project_diagnostic(
                                    previous_source,
                                    previous_span,
                                    "other canonical root for the same module path is here",
                                ));
                            }
                        }
                    }
                    Some(SourcePathMapping::ModulePart {
                        canonical_root: None,
                    }) => {}
                    Some(SourcePathMapping::Mismatch { expected }) => {
                        diagnostics.push(project_diagnostic(
                            parsed.source,
                            parsed
                                .parse
                                .value
                                .module
                                .as_ref()
                                .map_or(parsed.parse.value.span, |module| module.span),
                            &format!(
                                "module declaration does not match source path; expected `{}`",
                                module_path_text(&expected)
                            ),
                        ));
                    }
                    Some(SourcePathMapping::InvalidPlacement) => {
                        diagnostics.push(project_diagnostic(
                            parsed.source,
                            parsed.parse.value.span,
                            "invalid module part placement; package sources must be `.es` files under the source root",
                        ));
                    }
                    None => {}
                }
            }

            let module_id = if let Some(existing) = index.by_path.get(&module_path).copied() {
                existing
            } else {
                let span = parsed
                    .parse
                    .value
                    .module
                    .as_ref()
                    .map_or(parsed.parse.value.span, |module| module.span);
                let id = index.modules.alloc_with_id(|id| ModuleInfo {
                    id,
                    path: module_path.clone(),
                    parts: Vec::new(),
                    origin: module_origin_for_source(source_file),
                    visibility_exports: ExportTable::default(),
                    span,
                });
                index.by_path.insert(module_path.clone(), id);
                id
            };

            if index.by_source.insert(parsed.source, module_id).is_some() {
                diagnostics.push(project_diagnostic(
                    parsed.source,
                    parsed.parse.value.span,
                    "source file was assigned to more than one module",
                ));
                continue;
            }

            let part_id = index.parts.alloc_with_id(|id| ModulePart {
                id,
                module: module_id,
                source: parsed.source,
                declared_module_span: parsed.parse.value.module.as_ref().map(|module| module.span),
                imports: parsed
                    .imports
                    .iter()
                    .cloned()
                    .map(|mut import| {
                        import.module_part = Some(id);
                        import
                    })
                    .collect(),
                items: parsed
                    .items
                    .iter()
                    .cloned()
                    .map(|mut item| {
                        item.module_part = Some(id);
                        item
                    })
                    .collect(),
            });
            index
                .modules
                .get_mut(module_id)
                .expect("module should exist")
                .parts
                .push(part_id);

            for (item_index, item) in parsed.parse.value.items.iter().enumerate() {
                let Some((name, span)) = item_decl_name(&item.item) else {
                    continue;
                };
                if let Some(previous) = declared_names.insert((module_id, name.clone()), span) {
                    diagnostics.push(project_diagnostic(
                        parsed.source,
                        span,
                        &format!("duplicate top-level declaration `{name}` in module"),
                    ));
                    diagnostics.push(project_diagnostic(
                        parsed.source,
                        previous,
                        &format!("previous declaration of `{name}` is here"),
                    ));
                }
                if let Some(item_ref) = index
                    .parts
                    .get(part_id)
                    .and_then(|part| part.items.get(item_index))
                    .cloned()
                {
                    index
                        .modules
                        .get_mut(module_id)
                        .expect("module should exist")
                        .visibility_exports
                        .items
                        .entry(name.clone())
                        .or_insert_with(|| ExportedItem {
                            name,
                            module: module_id,
                            part: part_id,
                            item: item_ref,
                            visibility: hir_visibility(item_visibility(&item.item)),
                            span,
                        });
                }
            }
        }

        context.diagnostics.extend(diagnostics);
        context.modules = Some(index);
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(MODULE_INDEX))
    }
}

fn module_origin_for_source(source_file: &SourceFile) -> ModuleOrigin {
    match &source_file.kind {
        SourceKind::DependencySourceOverlay {
            package,
            import_root,
        } => ModuleOrigin::DependencySourceOverlay {
            package: *package,
            import_root: import_root.clone(),
        },
        _ => ModuleOrigin::Source,
    }
}

fn item_decl_name(item: &etas_syntax::ast::Item) -> Option<(String, Span)> {
    match item {
        etas_syntax::ast::Item::Alias(item) => Some((item.name.text.clone(), item.name.span)),
        etas_syntax::ast::Item::Type(item) => Some((item.name.text.clone(), item.name.span)),
        etas_syntax::ast::Item::Enum(item) => Some((item.name.text.clone(), item.name.span)),
        etas_syntax::ast::Item::Spec(item) => Some((item.name.text.clone(), item.name.span)),
        etas_syntax::ast::Item::Effect(item) => Some((item.name.text.clone(), item.name.span)),
        etas_syntax::ast::Item::TopLevelLet(item) => Some((item.name.text.clone(), item.name.span)),
        etas_syntax::ast::Item::Tool(item) => item
            .path
            .segments
            .last()
            .map(|name| (name.text.clone(), name.span)),
        etas_syntax::ast::Item::Agent(item) => Some((item.name.text.clone(), item.name.span)),
        etas_syntax::ast::Item::Protocol(item) => Some((item.name.text.clone(), item.name.span)),
        etas_syntax::ast::Item::Flow(item) => Some((item.name.text.clone(), item.name.span)),
        etas_syntax::ast::Item::Impl(_) | etas_syntax::ast::Item::Error(_) => None,
    }
}

fn item_visibility(item: &etas_syntax::ast::Item) -> etas_syntax::ast::Visibility {
    match item {
        etas_syntax::ast::Item::Alias(item) => item.visibility,
        etas_syntax::ast::Item::Type(item) => item.visibility,
        etas_syntax::ast::Item::Enum(item) => item.visibility,
        etas_syntax::ast::Item::Spec(item) => item.visibility,
        etas_syntax::ast::Item::Effect(item) => item.visibility,
        etas_syntax::ast::Item::TopLevelLet(item) => item.visibility,
        etas_syntax::ast::Item::Tool(item) => item.visibility,
        etas_syntax::ast::Item::Agent(item) => item.visibility,
        etas_syntax::ast::Item::Protocol(item) => item.visibility,
        etas_syntax::ast::Item::Flow(item) => item.visibility,
        etas_syntax::ast::Item::Impl(_) | etas_syntax::ast::Item::Error(_) => {
            etas_syntax::ast::Visibility::Private
        }
    }
}

fn hir_visibility(visibility: etas_syntax::ast::Visibility) -> etas_hir::Visibility {
    match visibility {
        etas_syntax::ast::Visibility::Private => etas_hir::Visibility::Private,
        etas_syntax::ast::Visibility::Public => etas_hir::Visibility::Public,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CanonicalRoot {
    File,
    ModFile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SourcePathMapping {
    ModulePart {
        canonical_root: Option<CanonicalRoot>,
    },
    Mismatch {
        expected: ModulePath,
    },
    InvalidPlacement,
}

fn source_path_mapping(
    source: &SourceFile,
    source_root: Option<&Path>,
    declared: &ModulePath,
) -> Option<SourcePathMapping> {
    let path = source.path.as_ref()?;
    let relative = relative_source_path(path, source_root)?;
    if relative.extension().and_then(|ext| ext.to_str()) != Some("es") {
        return Some(SourcePathMapping::InvalidPlacement);
    }

    let components = relative
        .components()
        .filter_map(path_component_text)
        .collect::<Vec<_>>();
    if components.is_empty() {
        return Some(SourcePathMapping::InvalidPlacement);
    }

    let file_name = *components.last().expect("non-empty components");
    if file_name == "mod.es" {
        let module_segments = components
            .iter()
            .take(components.len() - 1)
            .map(|segment| (*segment).to_owned())
            .collect::<Vec<_>>();
        if module_segments.is_empty() {
            return Some(SourcePathMapping::InvalidPlacement);
        }
        let expected = ModulePath {
            segments: module_segments,
        };
        return Some(if &expected == declared {
            SourcePathMapping::ModulePart {
                canonical_root: Some(CanonicalRoot::ModFile),
            }
        } else {
            SourcePathMapping::Mismatch { expected }
        });
    }

    let stem = relative.file_stem().and_then(|stem| stem.to_str())?;
    let parent_segments = components
        .iter()
        .take(components.len() - 1)
        .map(|segment| (*segment).to_owned())
        .collect::<Vec<_>>();
    let mut canonical_segments = parent_segments.clone();
    canonical_segments.push(stem.to_owned());
    let canonical = ModulePath {
        segments: canonical_segments,
    };
    if &canonical == declared {
        return Some(SourcePathMapping::ModulePart {
            canonical_root: Some(CanonicalRoot::File),
        });
    }

    let parent = ModulePath {
        segments: parent_segments,
    };
    if !parent.segments.is_empty() && &parent == declared {
        return Some(SourcePathMapping::ModulePart {
            canonical_root: None,
        });
    }

    Some(SourcePathMapping::Mismatch {
        expected: canonical,
    })
}

fn relative_source_path(path: &Path, source_root: Option<&Path>) -> Option<PathBuf> {
    if let Some(source_root) = source_root {
        return path.strip_prefix(source_root).ok().and_then(|relative| {
            (!relative.as_os_str().is_empty()).then(|| relative.to_path_buf())
        });
    }

    path.file_name().map(PathBuf::from)
}

fn path_component_text(component: Component<'_>) -> Option<&str> {
    match component {
        Component::Normal(text) => text.to_str(),
        _ => None,
    }
}

fn module_path_text(path: &ModulePath) -> String {
    path.segments.join(".")
}

fn project_diagnostic(source: etas_core::SourceId, span: Span, message: &str) -> Diagnostic {
    let span = if span.range.start == span.range.end {
        Span::empty(source, TextSize::ZERO)
    } else {
        span
    };
    Diagnostic::syntax(SyntaxDiagnosticCode::InvalidItem, span, message)
}
