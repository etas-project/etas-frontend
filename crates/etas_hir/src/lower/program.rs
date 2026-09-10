use std::{collections::HashMap, sync::Arc};

use etas_core::SourceFile;
use etas_syntax::{ast, parse_program};

use crate::{
    HirDiagnostics, HirExprId, HirImport, HirImportBinding, HirImportKind, HirImportSource,
    HirModule, HirModuleId, HirProgram, HirStmtId, HirTypeId, ImportAliasOrigin,
    PatternBindingOwner, ResolveResult, ResolvedPath, ScopeBuilder, ScopeId, ScopeOwner,
    SourceMapBuilder, SymbolBuilder, SymbolData, SymbolDef, SymbolId, SymbolKind,
};

pub fn lower_source(source: SourceFile) -> HirProgram {
    let parsed = parse_program(source);
    let mut hir = lower_program(&parsed.value);
    hir.diagnostics.extend(parsed.diagnostics);
    hir
}

pub fn lower_program(program: &ast::Program) -> HirProgram {
    LowerCtx::new().lower_program(program)
}

#[derive(Clone, Copy)]
pub struct HirProjectModule<'a> {
    pub parts: &'a [&'a ast::Program],
}

pub fn lower_project(modules: &[HirProjectModule<'_>]) -> HirProgram {
    let mut lowering = HirProjectLowering::new(modules);
    for (module_index, module) in modules.iter().enumerate() {
        for part in module.parts {
            lowering.normalize_part(module_index, part);
            lowering.lower_part(module_index, part);
        }
    }
    lowering.finish()
}

pub struct HirProjectLowering {
    ctx: LowerCtx,
    modules: Vec<HirProjectLoweringModule>,
}

#[derive(Clone)]
struct HirProjectLoweringModule {
    module_id: HirModuleId,
    scope: ScopeId,
    part_scopes: HashMap<etas_core::SourceId, ScopeId>,
}

impl HirProjectLowering {
    pub fn new(modules: &[HirProjectModule<'_>]) -> Self {
        Self::new_with_std_registry(modules, Arc::new(etas_std::standard_registry()))
    }

    pub fn new_with_std_registry(
        modules: &[HirProjectModule<'_>],
        std_registry: Arc<etas_std::StdRegistry>,
    ) -> Self {
        let mut ctx = LowerCtx::new_with_std_registry(std_registry);
        let mut shells = Vec::new();
        for module in modules {
            let first_part = module
                .parts
                .first()
                .expect("project HIR module should contain at least one part");
            let module_id = ctx.hir.modules_arena.alloc_with_id(|id| HirModule {
                id,
                name: None,
                imports: Vec::new(),
                items: Vec::new(),
                scope: ScopeId(0),
                span: first_part.span,
            });
            let module_scope =
                ctx.scopes
                    .alloc(None, ScopeOwner::Module(module_id), first_part.span);
            let part_scopes = module
                .parts
                .iter()
                .map(|part| {
                    (
                        part.span.source,
                        ctx.scopes.alloc(
                            Some(module_scope),
                            ScopeOwner::Module(module_id),
                            part.span,
                        ),
                    )
                })
                .collect();
            shells.push(HirProjectLoweringModule {
                module_id,
                scope: module_scope,
                part_scopes,
            });
        }

        for (module_index, module) in modules.iter().enumerate() {
            let shell = &shells[module_index];
            ctx.current_module = shell.module_id;
            ctx.current_module_scope = Some(shell.scope);
            let module_qualified_prefix = module
                .parts
                .first()
                .and_then(|part| part.module.as_ref())
                .map(|module| crate::path_text(&module.path));
            for part in module.parts {
                ctx.predeclare_items(&part.items, shell.scope, module_qualified_prefix.as_deref());
            }
        }

        for (module_index, module) in modules.iter().enumerate() {
            let shell = &shells[module_index];
            let first_part = module
                .parts
                .first()
                .expect("project HIR module should contain at least one part");
            let module_name = first_part.module.as_ref().map(|module| ResolvedPath {
                syntax_path: module.path.clone(),
                segments: crate::path_segments(&module.path),
                resolution: ResolveResult::Unresolved,
                span: module.span,
            });
            let hir_module = ctx.hir.modules_arena.get_mut(shell.module_id).unwrap();
            hir_module.name = module_name;
            hir_module.scope = shell.scope;
            ctx.hir.modules.push(shell.module_id);
        }

        for (target_index, module) in modules.iter().enumerate() {
            let Some(module_decl) = module.parts.first().and_then(|part| part.module.as_ref())
            else {
                continue;
            };
            let target = &shells[target_index];
            ctx.current_module = target.module_id;
            ctx.current_module_scope = Some(target.scope);
            let name = crate::path_text(&module_decl.path);
            let symbol = ctx.alloc_symbol_with_def(SymbolData {
                name: name.clone(),
                kind: SymbolKind::Module,
                visibility: crate::Visibility::Public,
                defining_module: ctx.current_module,
                defining_item: None,
                def: SymbolDef::Module {
                    module: target.module_id,
                },
                declared_type: None,
                definition_span: module_decl.span,
            });
            for shell in &shells {
                ctx.insert_module_symbol(shell.scope, &name, symbol, module_decl.span);
            }
        }

        Self {
            ctx,
            modules: shells,
        }
    }

    pub fn normalize_part(&mut self, module_index: usize, part: &ast::Program) {
        let shell = &self.modules[module_index];
        self.ctx.current_module = shell.module_id;
        self.ctx.current_module_scope = Some(shell.scope);
        let part_scope = shell
            .part_scopes
            .get(&part.span.source)
            .copied()
            .expect("module part scope should exist");
        let imports = part
            .imports
            .iter()
            .flat_map(|import| self.ctx.lower_import(import, part_scope))
            .collect::<Vec<_>>();
        let module = self
            .ctx
            .hir
            .modules_arena
            .get_mut(shell.module_id)
            .expect("project HIR module shell should exist");
        module.imports.extend(imports);
    }

    pub fn lower_part(
        &mut self,
        module_index: usize,
        part: &ast::Program,
    ) -> Vec<crate::HirItemId> {
        let shell = &self.modules[module_index];
        self.ctx.current_module = shell.module_id;
        self.ctx.current_module_scope = Some(shell.scope);
        let part_scope = shell
            .part_scopes
            .get(&part.span.source)
            .copied()
            .expect("module part scope should exist");
        let item_ids = part
            .items
            .iter()
            .map(|item| self.ctx.lower_item(item, part_scope))
            .collect::<Vec<_>>();
        let module = self
            .ctx
            .hir
            .modules_arena
            .get_mut(shell.module_id)
            .expect("project HIR module shell should exist");
        module.items.extend(item_ids.iter().copied());
        item_ids
    }

    pub fn finish(self) -> HirProgram {
        self.ctx.finish()
    }
}

pub(super) struct LowerCtx {
    pub(super) hir: HirProgram,
    pub(super) diagnostics: HirDiagnostics,
    pub(super) current_module: HirModuleId,
    pub(super) current_module_scope: Option<ScopeId>,
    pub(super) scopes: ScopeBuilder,
    pub(super) symbols: SymbolBuilder,
    pub(super) source_map: SourceMapBuilder,
    pub(super) top_item_symbols: HashMap<SyntaxKey, SymbolId>,
    pub(super) action_symbols: HashMap<SyntaxKey, SymbolId>,
    pub(super) local_binding: Option<LocalBindingContext>,
    pub(super) pattern_binding: Option<PatternBindingContext>,
    pub(super) std_prelude_aliases: HashMap<(HirModuleId, String), SymbolId>,
    pub(super) std_qualified_aliases: HashMap<(HirModuleId, Vec<String>), SymbolId>,
    pub(super) std_action_aliases: HashMap<(HirModuleId, String), SymbolId>,
    pub(super) std_registry: Arc<etas_std::StdRegistry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct SyntaxKey {
    pub(super) source: etas_core::SourceId,
    pub(super) start: u32,
    pub(super) end: u32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct LocalBindingContext {
    pub(super) binding: HirStmtId,
    pub(super) ty: Option<HirTypeId>,
    pub(super) initializer: Option<HirExprId>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PatternBindingContext {
    pub(super) owner: PatternBindingOwner,
    pub(super) ty: Option<HirTypeId>,
    pub(super) initializer: Option<HirExprId>,
}

impl LowerCtx {
    pub(super) fn new() -> Self {
        Self::new_with_std_registry(Arc::new(etas_std::standard_registry()))
    }

    pub(super) fn new_with_std_registry(std_registry: Arc<etas_std::StdRegistry>) -> Self {
        Self {
            hir: HirProgram::default(),
            diagnostics: HirDiagnostics::default(),
            current_module: HirModuleId(0),
            current_module_scope: None,
            scopes: ScopeBuilder::default(),
            symbols: SymbolBuilder::default(),
            source_map: SourceMapBuilder::default(),
            top_item_symbols: HashMap::new(),
            action_symbols: HashMap::new(),
            local_binding: None,
            pattern_binding: None,
            std_prelude_aliases: HashMap::new(),
            std_qualified_aliases: HashMap::new(),
            std_action_aliases: HashMap::new(),
            std_registry,
        }
    }

    pub(super) fn lower_program(mut self, program: &ast::Program) -> HirProgram {
        let module_id = self.hir.modules_arena.alloc_with_id(|id| HirModule {
            id,
            name: None,
            imports: Vec::new(),
            items: Vec::new(),
            scope: ScopeId(0),
            span: program.span,
        });
        self.current_module = module_id;
        let module_scope = self
            .scopes
            .alloc(None, ScopeOwner::Module(module_id), program.span);
        self.current_module_scope = Some(module_scope);

        let module_name = program.module.as_ref().map(|module| ResolvedPath {
            syntax_path: module.path.clone(),
            segments: crate::path_segments(&module.path),
            resolution: ResolveResult::Unresolved,
            span: module.span,
        });
        let mut imports = Vec::new();
        for import in &program.imports {
            imports.extend(self.lower_import(import, module_scope));
        }

        let module_qualified_prefix = program
            .module
            .as_ref()
            .map(|module| crate::path_text(&module.path));
        self.predeclare_items(
            &program.items,
            module_scope,
            module_qualified_prefix.as_deref(),
        );

        let mut item_ids = Vec::new();
        for item in &program.items {
            let item_id = self.lower_item(item, module_scope);
            item_ids.push(item_id);
        }

        let module = self.hir.modules_arena.get_mut(module_id).unwrap();
        module.name = module_name;
        module.imports = imports;
        module.items = item_ids;
        module.scope = module_scope;
        self.hir.modules.push(module_id);

        let diagnostics = self.diagnostics.finish();
        let scopes = self.scopes.finish();
        let symbols = self.symbols.finish();
        let source_map = self.source_map.finish();
        self.hir.diagnostics = diagnostics;
        self.hir.scopes = scopes;
        self.hir.symbols = symbols;
        self.hir.source_map = source_map;
        self.hir
    }

    fn finish(mut self) -> HirProgram {
        let diagnostics = self.diagnostics.finish();
        let scopes = self.scopes.finish();
        let symbols = self.symbols.finish();
        let source_map = self.source_map.finish();
        self.hir.diagnostics = diagnostics;
        self.hir.scopes = scopes;
        self.hir.symbols = symbols;
        self.hir.source_map = source_map;
        self.hir
    }

    pub(super) fn lower_import(
        &mut self,
        import: &ast::ImportDecl,
        scope: ScopeId,
    ) -> Vec<HirImport> {
        let visibility = hir_visibility(import.visibility);
        match &import.tree {
            ast::ImportTree::Single { path, alias, span } => {
                vec![self.lower_single_import(path, alias.as_ref(), visibility, *span, scope)]
            }
            ast::ImportTree::Group {
                prefix,
                items,
                span,
            } => items
                .iter()
                .map(|item| self.lower_group_member_import(prefix, item, visibility, *span, scope))
                .collect(),
            ast::ImportTree::Wildcard {
                prefix,
                star_span,
                span,
            } => {
                let target = self.resolve_import_path(prefix, scope, false);
                vec![HirImport {
                    source: HirImportSource::Wildcard {
                        prefix: crate::path_segments(prefix),
                        star_span: *star_span,
                    },
                    kind: HirImportKind::Wildcard,
                    target,
                    binding: None,
                    visibility,
                    span: *span,
                }]
            }
            ast::ImportTree::Error { span } => {
                self.diagnostics.invalid_import_path("<error>", *span);
                Vec::new()
            }
        }
    }

    fn lower_single_import(
        &mut self,
        path: &ast::Path,
        alias: Option<&ast::Name>,
        visibility: crate::Visibility,
        span: etas_core::Span,
        scope: ScopeId,
    ) -> HirImport {
        let binding = self.import_binding(path, alias, visibility, scope);
        let target = self.resolve_import_path(path, scope, false);
        HirImport {
            source: HirImportSource::Single {
                path_span: path.span,
            },
            kind: HirImportKind::Single,
            target,
            binding,
            visibility,
            span,
        }
    }

    fn lower_group_member_import(
        &mut self,
        prefix: &ast::Path,
        item: &ast::ImportItem,
        visibility: crate::Visibility,
        group_span: etas_core::Span,
        scope: ScopeId,
    ) -> HirImport {
        let mut segments = prefix.segments.clone();
        segments.push(item.name.clone());
        let target_path = ast::Path {
            span: prefix.span.cover(item.name.span),
            segments,
        };
        let binding = self.import_binding(&target_path, item.alias.as_ref(), visibility, scope);
        let target = self.resolve_import_path(&target_path, scope, false);
        HirImport {
            source: HirImportSource::GroupMember {
                prefix: crate::path_segments(prefix),
                member: crate::PathSegment {
                    name: item.name.text.clone(),
                    span: item.name.span,
                },
                group_span,
            },
            kind: HirImportKind::GroupMember,
            target,
            binding,
            visibility,
            span: item.span,
        }
    }

    fn import_binding(
        &mut self,
        path: &ast::Path,
        alias: Option<&ast::Name>,
        visibility: crate::Visibility,
        scope: ScopeId,
    ) -> Option<HirImportBinding> {
        let (local_name, local_name_span, is_alias) = alias.map_or_else(
            || {
                path.segments
                    .last()
                    .map(|segment| (segment.text.clone(), segment.span, false))
                    .unwrap_or_else(|| (String::new(), path.span, false))
            },
            |alias| (alias.text.clone(), alias.span, true),
        );
        if local_name.is_empty() || local_name == "<error>" {
            self.diagnostics
                .invalid_import_path(&crate::path_text(path), path.span);
            return None;
        }

        let symbol = self.alloc_symbol_with_def(SymbolData {
            name: local_name.clone(),
            kind: SymbolKind::Import,
            visibility,
            defining_module: self.current_module,
            defining_item: None,
            def: SymbolDef::ImportAlias {
                path: crate::path_segments(path)
                    .into_iter()
                    .map(|segment| segment.name)
                    .collect(),
                origin: ImportAliasOrigin::SourceImport,
            },
            declared_type: None,
            definition_span: local_name_span,
        });
        if let Some(previous) = self.scopes.insert(scope, local_name.clone(), symbol) {
            let previous_span = self
                .symbols
                .get(previous)
                .map_or(local_name_span, |symbol| symbol.definition_span);
            self.diagnostics
                .import_alias_conflict(&local_name, local_name_span, previous_span);
        }
        Some(HirImportBinding {
            symbol,
            local_name,
            local_name_span,
            is_alias,
        })
    }

    pub(super) fn resolve_std_prelude_name(
        &mut self,
        name: &str,
        span: etas_core::Span,
    ) -> Option<SymbolId> {
        let module_scope = self.current_module_scope?;
        let key = (self.current_module, name.to_owned());
        if let Some(symbol) = self.std_prelude_aliases.get(&key) {
            return Some(*symbol);
        }

        let symbol_ref = self.std_registry.lookup_prelude(name)?;
        let qualified_path = self
            .std_registry
            .symbol(symbol_ref.id)?
            .qualified_path
            .clone();
        let symbol = self.alloc_symbol_with_def(SymbolData {
            name: name.to_owned(),
            kind: SymbolKind::StdPreludeAlias,
            visibility: crate::Visibility::Public,
            defining_module: self.current_module,
            defining_item: None,
            def: SymbolDef::ImportAlias {
                path: qualified_path,
                origin: ImportAliasOrigin::StdPrelude,
            },
            declared_type: None,
            definition_span: span,
        });
        self.scopes.insert(module_scope, name.to_owned(), symbol);
        self.std_prelude_aliases.insert(key, symbol);
        Some(symbol)
    }

    pub(super) fn resolve_std_qualified_path(
        &mut self,
        path: &[String],
        span: etas_core::Span,
    ) -> Option<SymbolId> {
        let module_scope = self.current_module_scope?;
        let key = (self.current_module, path.to_vec());
        if let Some(symbol) = self.std_qualified_aliases.get(&key) {
            return Some(*symbol);
        }

        let qualified_path = self
            .std_registry
            .lookup_qualified(path)?
            .qualified_path
            .clone();
        let name = path.join(".");
        let symbol = self.alloc_symbol_with_def(SymbolData {
            name,
            kind: SymbolKind::StdPreludeAlias,
            visibility: crate::Visibility::Public,
            defining_module: self.current_module,
            defining_item: None,
            def: SymbolDef::ImportAlias {
                path: qualified_path,
                origin: ImportAliasOrigin::StdPrelude,
            },
            declared_type: None,
            definition_span: span,
        });
        self.scopes.insert(module_scope, path.join("."), symbol);
        self.std_qualified_aliases.insert(key, symbol);
        Some(symbol)
    }

    pub(super) fn resolve_std_enum_member(
        &mut self,
        prefix: SymbolId,
        remaining: &[String],
        span: etas_core::Span,
    ) -> Option<SymbolId> {
        let [member] = remaining else {
            return None;
        };
        let SymbolDef::ImportAlias { path, .. } = &self.symbols.get(prefix)?.def else {
            return None;
        };
        let owner = self.std_registry.lookup_qualified(path)?;
        let path = self
            .std_registry
            .enum_constructor(owner.id, member)?
            .qualified_path
            .clone();
        self.resolve_std_qualified_path(&path, span)
    }
}

fn hir_visibility(visibility: ast::Visibility) -> crate::Visibility {
    match visibility {
        ast::Visibility::Private => crate::Visibility::Private,
        ast::Visibility::Public => crate::Visibility::Public,
    }
}
