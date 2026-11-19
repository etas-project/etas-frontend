use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

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
pub(super) enum PreludeContext {
    Any,
    Type,
    Effect,
    Value,
}

impl PreludeContext {
    fn accepts(self, kind: etas_std::StdSymbolKind) -> bool {
        self.accepts_namespace(kind.namespace())
    }

    fn accepts_namespace(self, namespace: etas_std::StdNamespace) -> bool {
        use etas_std::StdNamespace;
        match self {
            Self::Any => true,
            Self::Type => namespace == StdNamespace::Type,
            Self::Effect => namespace == StdNamespace::Effect,
            Self::Value => matches!(
                namespace,
                StdNamespace::Type | StdNamespace::Value | StdNamespace::Module
            ),
        }
    }
}

/// Calling names from a resolved source prelude. Source targets retain their
/// declaration coordinates until project symbols have been predeclared.
#[derive(Clone, Debug)]
pub struct HirSourcePreludeBinding {
    pub name: String,
    pub namespace: etas_std::StdNamespace,
    pub target: HirSourcePreludeTarget,
}

#[derive(Clone, Debug)]
pub enum HirSourcePreludeTarget {
    Declaration {
        span: etas_core::Span,
        member: Option<String>,
    },
    Module {
        path: Vec<String>,
    },
    Primitive {
        symbol: etas_std::StdSymbolId,
    },
}

#[derive(Clone)]
struct SourcePreludeCandidate {
    namespace: etas_std::StdNamespace,
    target: SourcePreludeTarget,
}

#[derive(Clone)]
enum SourcePreludeTarget {
    Declaration(SymbolId),
    Primitive(etas_std::StdSymbolId),
}

#[derive(Clone, Copy)]
pub struct HirProjectModule<'a> {
    pub parts: &'a [&'a ast::Program],
    pub namespace: Option<HirProjectNamespace<'a>>,
}

#[derive(Clone, Copy)]
pub struct HirProjectNamespace<'a> {
    pub path: &'a [String],
    pub group: u32,
}

impl HirProjectModule<'_> {
    fn qualified_name(&self) -> Option<String> {
        match self.namespace {
            Some(namespace) => Some(namespace.path.join(".")),
            None => self
                .parts
                .first()
                .and_then(|part| part.module.as_ref())
                .map(|module| crate::path_text(&module.path)),
        }
    }
}

pub fn lower_project(modules: &[HirProjectModule<'_>]) -> HirProgram {
    let mut lowering = HirProjectLowering::new(modules);
    for (module_index, module) in modules.iter().enumerate() {
        for part in module.parts {
            lowering.normalize_part(module_index, part);
            lowering.lower_part(module_index, part);
        }
    }
    lowering.ctx.finish()
}

pub struct HirProjectLowering {
    ctx: LowerCtx,
    modules: Vec<HirProjectLoweringModule>,
    body_slots: HashSet<SyntaxKey>,
}

#[derive(Clone)]
struct HirProjectLoweringModule {
    module_id: HirModuleId,
    scope: ScopeId,
    part_scopes: HashMap<etas_core::SourceId, ScopeId>,
}

impl HirProjectLowering {
    pub fn with_source_prelude(
        mut self,
        bindings: Vec<HirSourcePreludeBinding>,
    ) -> Result<Self, String> {
        let mut prelude = HashMap::<String, Vec<SourcePreludeCandidate>>::new();
        for binding in bindings {
            let target = match binding.target {
                HirSourcePreludeTarget::Declaration { span, member } => {
                    let key = SyntaxKey {
                        source: span.source,
                        start: span.range.start.0,
                        end: span.range.end.0,
                    };
                    let symbol = *self
                        .ctx
                        .top_item_symbols
                        .get(&key)
                        .ok_or("source prelude target has no predeclared declaration")?;
                    let symbol = match member {
                        Some(member) => match self.ctx.symbols.resolve_member(symbol, &member) {
                            ResolveResult::Resolved(symbol) => symbol,
                            _ => {
                                return Err(
                                    "source prelude member has no predeclared symbol".into()
                                );
                            }
                        },
                        None => symbol,
                    };
                    SourcePreludeTarget::Declaration(symbol)
                }
                HirSourcePreludeTarget::Module { path } => {
                    let name = path.join(".");
                    let symbol = self
                        .ctx
                        .symbols
                        .iter()
                        .find(|symbol| {
                            symbol.name == name && matches!(symbol.def, SymbolDef::Module { .. })
                        })
                        .map(|symbol| symbol.id)
                        .ok_or("source prelude module has no predeclared symbol")?;
                    SourcePreludeTarget::Declaration(symbol)
                }
                HirSourcePreludeTarget::Primitive { symbol } => {
                    SourcePreludeTarget::Primitive(symbol)
                }
            };
            let candidates = prelude.entry(binding.name).or_default();
            let duplicate = candidates
                .iter()
                .any(|candidate| match (&candidate.target, &target) {
                    (SourcePreludeTarget::Declaration(a), SourcePreludeTarget::Declaration(b)) => {
                        a == b
                    }
                    (SourcePreludeTarget::Primitive(a), SourcePreludeTarget::Primitive(b)) => {
                        a == b
                    }
                    _ => false,
                });
            if !duplicate {
                candidates.push(SourcePreludeCandidate {
                    namespace: binding.namespace,
                    target,
                });
            }
        }
        self.ctx.source_prelude = Some(prelude);
        Ok(self)
    }

    pub fn with_source_standard_types(mut self, enabled: bool) -> Self {
        self.ctx.hir.source_standard_types = enabled;
        self
    }

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
            let module_qualified_prefix = module.qualified_name();
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
            let module_name = first_part.module.as_ref().map(|declaration| ResolvedPath {
                syntax_path: declaration.path.clone(),
                segments: match module.namespace {
                    Some(namespace) => namespace
                        .path
                        .iter()
                        .map(|name| crate::PathSegment {
                            name: name.clone(),
                            span: declaration.span,
                        })
                        .collect(),
                    None => crate::path_segments(&declaration.path),
                },
                resolution: ResolveResult::Unresolved,
                span: declaration.span,
            });
            let hir_module = ctx.hir.modules_arena.get_mut(shell.module_id).unwrap();
            hir_module.name = module_name;
            hir_module.scope = shell.scope;
            ctx.hir.modules.push(shell.module_id);
        }

        for (module_index, module) in modules.iter().enumerate() {
            let shell = &shells[module_index];
            ctx.current_module = shell.module_id;
            ctx.current_module_scope = Some(shell.scope);
            for part in module.parts {
                ctx.predeclare_agent_methods(&part.items, shell.scope);
            }
        }

        for (target_index, module) in modules.iter().enumerate() {
            let Some(module_decl) = module.parts.first().and_then(|part| part.module.as_ref())
            else {
                continue;
            };
            let target = &shells[target_index];
            ctx.current_module = target.module_id;
            ctx.current_module_scope = Some(target.scope);
            let Some(name) = module.qualified_name() else {
                continue;
            };
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
            let public_members = ctx
                .top_item_symbols
                .values()
                .filter_map(|symbol| {
                    let member = ctx.symbols.get(*symbol)?;
                    (member.defining_module == target.module_id
                        && member.visibility == crate::Visibility::Public)
                        .then(|| (member.name.clone(), *symbol))
                })
                .collect::<Vec<_>>();
            for (name, member) in public_members {
                ctx.symbols.insert_member(symbol, name, member);
            }
            for shell in &shells {
                ctx.insert_module_symbol(shell.scope, &name, symbol, module_decl.span);
            }
            if let Some(namespace) = module.namespace {
                let canonical_name = crate::path_text(&module_decl.path);
                if canonical_name != name {
                    for (owner, shell) in modules.iter().zip(&shells) {
                        if owner
                            .namespace
                            .is_some_and(|owner| owner.group == namespace.group)
                        {
                            ctx.insert_module_symbol(
                                shell.scope,
                                &canonical_name,
                                symbol,
                                module_decl.span,
                            );
                        }
                    }
                }
            }
        }

        let mut body_slots = HashSet::new();
        for module in modules {
            for part in module.parts {
                super::body_overlay::callable_slots(part, &mut body_slots);
            }
        }
        Self {
            ctx,
            modules: shells,
            body_slots,
        }
    }

    /// Installs structured syntax into an existing callable's authoritative
    /// slot. Declaration headers, imports, and lexical owners stay unchanged.
    pub fn replace_body(
        &mut self,
        callable: etas_core::Span,
        body: Arc<etas_syntax::RelocatedBodySyntax>,
    ) -> Result<(), super::BodyOverlayError> {
        let key = super::body_overlay::key(callable);
        if !self.body_slots.contains(&key) {
            return Err(super::BodyOverlayError::MissingCallable(callable));
        }
        if self.ctx.body_overlays.contains_key(&key) {
            return Err(super::BodyOverlayError::DuplicateCallable(callable));
        }
        if !self
            .ctx
            .source_map
            .bind_lexical_source(body.source().id, callable.source)
        {
            return Err(super::BodyOverlayError::ConflictingLexicalSource(callable));
        }
        self.ctx.body_overlays.insert(key, body);
        Ok(())
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

    pub fn finish(self) -> Result<HirProgram, super::BodyOverlayError> {
        if let Some(key) = self
            .ctx
            .body_overlays
            .keys()
            .min_by_key(|key| (key.source.0, key.start, key.end))
        {
            return Err(super::BodyOverlayError::UnappliedCallable(
                etas_core::Span::new(
                    key.source,
                    etas_core::TextRange::new(
                        etas_core::TextSize(key.start),
                        etas_core::TextSize(key.end),
                    ),
                ),
            ));
        }
        Ok(self.ctx.finish())
    }
}

pub(super) struct LowerCtx {
    source_prelude: Option<HashMap<String, Vec<SourcePreludeCandidate>>>,
    pub(super) body_overlays: HashMap<SyntaxKey, Arc<etas_syntax::RelocatedBodySyntax>>,
    pub(super) hir: HirProgram,
    pub(super) diagnostics: HirDiagnostics,
    pub(super) current_module: HirModuleId,
    pub(super) current_module_scope: Option<ScopeId>,
    pub(super) scopes: ScopeBuilder,
    pub(super) symbols: SymbolBuilder,
    pub(super) source_map: SourceMapBuilder,
    pub(super) top_item_symbols: HashMap<SyntaxKey, SymbolId>,
    pub(super) flow_declarations: HashMap<(HirModuleId, String), SymbolId>,
    pub(super) action_symbols: HashMap<SyntaxKey, SymbolId>,
    pub(super) method_symbols: HashMap<SyntaxKey, SymbolId>,
    pub(super) local_binding: Option<LocalBindingContext>,
    pub(super) pattern_binding: Option<PatternBindingContext>,
    pub(super) std_prelude_aliases: HashMap<(HirModuleId, String, etas_std::StdSymbolId), SymbolId>,
    pub(super) std_qualified_aliases: HashMap<(HirModuleId, Vec<String>), SymbolId>,
    pub(super) std_action_aliases: HashMap<etas_std::StdSymbolId, SymbolId>,
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
            source_prelude: None,
            body_overlays: HashMap::new(),
            hir: HirProgram::default(),
            diagnostics: HirDiagnostics::default(),
            current_module: HirModuleId(0),
            current_module_scope: None,
            scopes: ScopeBuilder::default(),
            symbols: SymbolBuilder::default(),
            source_map: SourceMapBuilder::default(),
            top_item_symbols: HashMap::new(),
            flow_declarations: HashMap::new(),
            action_symbols: HashMap::new(),
            method_symbols: HashMap::new(),
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
        self.predeclare_agent_methods(&program.items, module_scope);

        let mut item_ids = Vec::new();
        for item in &program.items {
            let item_id = self.lower_item(item, module_scope);
            item_ids.push(item_id);
        }

        let module = self.hir.modules_arena.get_mut(module_id).unwrap();
        module.name = module_name;
        module.imports = imports;
        module.items.extend(item_ids);
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
        // A source prelude publishes contextual names. Its public imports do
        // not compete for one lexical slot (for example Secret type/effect).
        let contextual_export = self.source_prelude.is_some()
            && visibility == crate::Visibility::Public
            && self.hir.modules_arena[self.current_module]
                .name
                .as_ref()
                .is_some_and(|path| {
                    path.segments
                        .iter()
                        .map(|part| part.name.as_str())
                        .eq(["std", "prelude"])
                });
        if !contextual_export
            && let Some(previous) = self.scopes.insert(scope, local_name.clone(), symbol)
        {
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
        context: PreludeContext,
    ) -> ResolveResult {
        if self.current_module_scope.is_none() {
            return ResolveResult::Unresolved;
        }
        if let Some(prelude) = &self.source_prelude {
            let bindings = prelude
                .get(name)
                .into_iter()
                .flatten()
                .filter(|binding| context.accepts_namespace(binding.namespace))
                .cloned()
                .collect::<Vec<_>>();
            let mut candidates = Vec::new();
            for binding in bindings {
                let symbol = match binding.target {
                    SourcePreludeTarget::Declaration(symbol) => symbol,
                    SourcePreludeTarget::Primitive(id) => {
                        let path = self
                            .std_registry
                            .symbol(id)
                            .expect("checked primitive prelude target")
                            .qualified_path
                            .clone();
                        self.std_prelude_alias(name, id, path, span)
                    }
                };
                if !candidates.contains(&symbol) {
                    candidates.push(symbol);
                }
            }
            // Scalar constructors are language capabilities. Other names must
            // have an actual public declaration in the source prelude.
            if candidates.is_empty() {
                let primitives = self.std_registry.prelude().candidates(name)
                    .filter_map(|binding| self.std_registry.symbol(binding.id))
                    .filter(|symbol| context.accepts(symbol.kind)
                        && matches!(&symbol.decl, etas_std::StdDecl::Type(declaration) if declaration.kind == etas_std::TypeDeclKind::Primitive))
                    .map(|symbol| (symbol.id, symbol.qualified_path.clone())).collect::<Vec<_>>();
                for (id, path) in primitives {
                    candidates.push(self.std_prelude_alias(name, id, path, span));
                }
            }
            return match candidates.as_slice() {
                [] => ResolveResult::Unresolved,
                [symbol] => ResolveResult::Resolved(*symbol),
                _ => ResolveResult::Ambiguous(candidates),
            };
        }
        let candidates = self
            .std_registry
            .prelude()
            .candidates(name)
            .filter_map(|binding| self.std_registry.symbol(binding.id))
            .filter(|symbol| context.accepts(symbol.kind))
            .map(|symbol| (symbol.id, symbol.qualified_path.clone()))
            .collect::<Vec<_>>();
        let candidates = candidates
            .into_iter()
            .map(|(id, path)| self.std_prelude_alias(name, id, path, span))
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [] => ResolveResult::Unresolved,
            [symbol] => ResolveResult::Resolved(*symbol),
            _ => ResolveResult::Ambiguous(candidates),
        }
    }

    fn std_prelude_alias(
        &mut self,
        name: &str,
        id: etas_std::StdSymbolId,
        qualified_path: Vec<String>,
        span: etas_core::Span,
    ) -> SymbolId {
        let key = (self.current_module, name.to_owned(), id);
        if let Some(symbol) = self.std_prelude_aliases.get(&key) {
            return *symbol;
        }
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
        self.std_prelude_aliases.insert(key, symbol);
        symbol
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
