use super::program::{LowerCtx, PreludeContext, SyntaxKey};
use crate::*;
use etas_core::Span;
use etas_std::{StdDecl, StdSymbolKind};
use etas_syntax::ast;

impl LowerCtx {
    pub(super) fn lower_params(
        &mut self,
        params: &[ast::Param],
        scope: ScopeId,
        item_id: HirItemId,
    ) -> Vec<SymbolId> {
        self.lower_params_with_receiver(params, scope, item_id, None)
    }

    pub(super) fn lower_params_with_receiver(
        &mut self,
        params: &[ast::Param],
        scope: ScopeId,
        item_id: HirItemId,
        receiver_type: Option<&ast::TypeExpr>,
    ) -> Vec<SymbolId> {
        params
            .iter()
            .enumerate()
            .map(|(param_index, param)| {
                let ty = match &param.ty {
                    ast::ParamType::Explicit(ty) => self.lower_type(ty, scope, item_id),
                    ast::ParamType::SelfReceiver { span } if param_index == 0 => receiver_type
                        .map(|ty| self.lower_type(ty, scope, item_id))
                        .unwrap_or_else(|| {
                            self.diagnostics.push(etas_core::Diagnostic::type_check(
                                etas_core::TypeDiagnosticCode::InvalidImplTargetKind,
                                *span,
                                "implicit self receiver requires a type or spec method owner",
                            ));
                            self.lower_type(&ast::TypeExpr::Error(*span), scope, item_id)
                        }),
                    ast::ParamType::SelfReceiver { span } => {
                        self.diagnostics.push(etas_core::Diagnostic::type_check(
                            etas_core::TypeDiagnosticCode::InvalidImplTargetKind,
                            *span,
                            "implicit self receiver must be the first method parameter",
                        ));
                        self.lower_type(&ast::TypeExpr::Error(*span), scope, item_id)
                    }
                };
                self.bind_symbol_with_def(
                    scope,
                    SymbolData {
                        name: param.name.text.clone(),
                        kind: SymbolKind::Param,
                        visibility: crate::Visibility::Private,
                        defining_module: self.current_module,
                        defining_item: Some(item_id),
                        def: SymbolDef::Param {
                            owner: item_id,
                            param_index: param_index.min(u32::MAX as usize) as u32,
                            pattern: None,
                            ty: Some(ty),
                        },
                        declared_type: Some(ty),
                        definition_span: param.name.span,
                    },
                )
            })
            .collect()
    }

    pub(super) fn lower_type_params(
        &mut self,
        params: &[ast::TypeParam],
        scope: ScopeId,
        item_id: HirItemId,
    ) -> Vec<SymbolId> {
        let mut lowered = Vec::with_capacity(params.len());
        for (param_index, param) in params.iter().enumerate() {
            let (kind, def) = match param.kind {
                ast::TypeParamKind::Type => (
                    SymbolKind::TypeParam,
                    SymbolDef::TypeParam {
                        owner: item_id,
                        param_index: param_index.min(u32::MAX as usize) as u32,
                        bounds: Vec::new(),
                    },
                ),
                ast::TypeParamKind::Effect => (
                    SymbolKind::EffectParam,
                    SymbolDef::EffectParam {
                        owner: item_id,
                        param_index: param_index.min(u32::MAX as usize) as u32,
                    },
                ),
            };
            let symbol = self.bind_symbol_with_def(
                scope,
                SymbolData {
                    name: param.name.text.clone(),
                    kind,
                    visibility: crate::Visibility::Private,
                    defining_module: self.current_module,
                    defining_item: Some(item_id),
                    def,
                    declared_type: None,
                    definition_span: param.name.span,
                },
            );
            lowered.push(symbol);
        }

        for (param_index, param) in params.iter().enumerate() {
            if !matches!(param.kind, ast::TypeParamKind::Type) || param.bounds.is_empty() {
                continue;
            }
            let bounds = param
                .bounds
                .iter()
                .map(|bound| self.lower_type_param_bound(bound, scope, item_id))
                .collect::<Vec<_>>();
            if let Some(symbol) = self.symbols.get_mut(lowered[param_index]) {
                symbol.def = SymbolDef::TypeParam {
                    owner: item_id,
                    param_index: param_index.min(u32::MAX as usize) as u32,
                    bounds,
                };
            }
        }

        lowered
    }

    pub(super) fn lower_type_param_bound(
        &mut self,
        bound: &ast::TypeParamBound,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirTypeParamBound {
        HirTypeParamBound {
            path: self.resolve_path(&bound.path, scope, true),
            args: bound
                .args
                .iter()
                .map(|arg| self.lower_type(arg, scope, item_id))
                .collect(),
            span: bound.span,
        }
    }

    pub(super) fn resolve_action(
        &mut self,
        effect: crate::HirEffectRef,
        action: &ast::Name,
    ) -> ResolvedActionRef {
        let qualified = format!(
            "{}.{}",
            crate::path_text(&effect.path.syntax_path),
            action.text
        );
        let action_symbol = if let Some(symbol) =
            self.resolve_standard_action_symbol(&effect, &action.text, action.span)
        {
            ResolveResult::Resolved(symbol)
        } else {
            match effect.path.resolution {
                ResolveResult::Resolved(owner) => self.symbols.resolve_member(owner, &action.text),
                ResolveResult::Ambiguous(ref symbols) => ResolveResult::Ambiguous(symbols.clone()),
                _ => ResolveResult::Unresolved,
            }
        };
        match &action_symbol {
            ResolveResult::Unresolved => self
                .diagnostics
                .unresolved_effect_action(&qualified, action.span),
            ResolveResult::Ambiguous(_) => self.diagnostics.ambiguous_name(&qualified, action.span),
            _ => {}
        }
        let span = effect.span.cover(action.span);

        ResolvedActionRef {
            effect,
            action: action.text.clone(),
            action_symbol,
            span,
        }
    }

    pub(super) fn resolve_compiler_action(
        &mut self,
        role: etas_std::CompilerActionRole,
        span: Span,
    ) -> Option<ResolvedActionRef> {
        let (owner, action) = self.std_registry.symbols().find_map(|symbol| {
            let StdDecl::EffectAction(action) = &symbol.decl else {
                return None;
            };
            (action.compiler_role == Some(role))
                .then(|| (action.owner.clone(), action.name.clone()))
        })?;
        let owner_path = self.std_registry.symbols().find_map(|symbol| {
            matches!(&symbol.decl, StdDecl::Effect(effect) if effect.name == owner)
                .then(|| symbol.qualified_path.clone())
        })?;
        let owner_symbol = self.resolve_std_qualified_path(&owner_path, span)?;
        let mut path = crate::unresolved_path_from_segments(
            &owner_path.iter().map(String::as_str).collect::<Vec<_>>(),
            span,
        );
        path.resolution = ResolveResult::Resolved(owner_symbol);
        let effect = HirEffectRef {
            path,
            args: Vec::new(),
            span,
        };
        let action_symbol = self.resolve_standard_action_symbol(&effect, &action, span)?;
        Some(ResolvedActionRef {
            effect,
            action,
            action_symbol: ResolveResult::Resolved(action_symbol),
            span,
        })
    }

    fn resolve_standard_action_symbol(
        &mut self,
        effect: &crate::HirEffectRef,
        action: &str,
        span: Span,
    ) -> Option<SymbolId> {
        let owner = effect.path.segments.last()?.name.as_str();
        let qualified = format!("{owner}.{action}");
        if !self.effect_ref_resolves_to_standard_owner(effect, owner) {
            return None;
        }

        let key = {
            let mut actions = self.std_registry.symbols().filter(|symbol| {
                symbol.kind == StdSymbolKind::EffectAction
                    && matches!(&symbol.decl, StdDecl::EffectAction(decl) if decl.owner == owner && decl.name == action)
            });
            let key = actions.next()?.id;
            if actions.next().is_some() {
                return None;
            }
            key
        };
        if let Some(symbol) = self.std_action_aliases.get(&key) {
            return Some(*symbol);
        }

        let symbol = self.alloc_symbol_with_def(SymbolData {
            name: qualified.clone(),
            kind: SymbolKind::EffectAction,
            visibility: crate::Visibility::Public,
            defining_module: self.current_module,
            defining_item: None,
            def: SymbolDef::Synthetic {
                reason: SyntheticSymbolReason::QualifiedEffectAction,
            },
            declared_type: None,
            definition_span: span,
        });
        self.std_action_aliases.insert(key, symbol);
        self.hir.standard_action_targets.insert(symbol, key);
        Some(symbol)
    }

    fn effect_ref_resolves_to_standard_owner(
        &self,
        effect: &crate::HirEffectRef,
        owner: &str,
    ) -> bool {
        let ResolveResult::Resolved(symbol) = effect.path.resolution else {
            return false;
        };
        let Some(symbol) = self.symbols.get(symbol) else {
            return false;
        };
        match &symbol.def {
            SymbolDef::ImportAlias { path, .. } => {
                matches!(self.std_registry.lookup_qualified(path).map(|symbol| &symbol.decl),
                    Some(StdDecl::Effect(decl)) if decl.name == owner)
            }
            _ => false,
        }
    }

    pub(super) fn resolve_path(
        &mut self,
        path: &ast::Path,
        scope: ScopeId,
        report: bool,
    ) -> ResolvedPath {
        self.resolve_path_in_context(path, scope, report, PreludeContext::Any)
    }

    pub(super) fn resolve_path_in_context(
        &mut self,
        path: &ast::Path,
        scope: ScopeId,
        report: bool,
        context: PreludeContext,
    ) -> ResolvedPath {
        let full_path = crate::path_text(path);
        let resolution = if path.segments.len() > 1 {
            match self.resolve_name_in_context(&full_path, path.span, scope, false, context) {
                ResolveResult::Unresolved => {
                    self.resolve_qualified_prefix(path, scope, report, context)
                }
                ResolveResult::Ambiguous(symbols) => {
                    if report {
                        self.diagnostics.ambiguous_name(&full_path, path.span);
                    }
                    ResolveResult::Ambiguous(symbols)
                }
                result => result,
            }
        } else {
            let name = path
                .segments
                .first()
                .map(|segment| segment.text.as_str())
                .unwrap_or_default();
            self.resolve_name_in_context(name, path.span, scope, report, context)
        };
        ResolvedPath {
            syntax_path: path.clone(),
            segments: crate::path_segments(path),
            resolution,
            span: path.span,
        }
    }

    pub(super) fn resolve_import_path(
        &mut self,
        path: &ast::Path,
        scope: ScopeId,
        report: bool,
    ) -> ResolvedPath {
        let full_path = crate::path_text(path);
        let resolution = if path.segments.len() > 1 {
            match self.resolve_module_name(&full_path, path.span, scope, false) {
                ResolveResult::Unresolved => {
                    self.resolve_module_qualified_prefix(path, scope, report)
                }
                ResolveResult::Ambiguous(symbols) => {
                    if report {
                        self.diagnostics.ambiguous_name(&full_path, path.span);
                    }
                    ResolveResult::Ambiguous(symbols)
                }
                result => result,
            }
        } else {
            let name = path
                .segments
                .first()
                .map(|segment| segment.text.as_str())
                .unwrap_or_default();
            self.resolve_module_name(name, path.span, scope, report)
        };
        ResolvedPath {
            syntax_path: path.clone(),
            segments: crate::path_segments(path),
            resolution,
            span: path.span,
        }
    }

    pub(super) fn resolve_name(
        &mut self,
        name: &str,
        span: Span,
        scope: ScopeId,
        report: bool,
    ) -> ResolveResult {
        self.resolve_name_in_context(name, span, scope, report, PreludeContext::Any)
    }

    fn resolve_name_in_context(
        &mut self,
        name: &str,
        span: Span,
        scope: ScopeId,
        report: bool,
        context: PreludeContext,
    ) -> ResolveResult {
        match crate::resolve_name(
            name,
            span,
            scope,
            &self.scopes,
            &mut self.diagnostics,
            false,
        ) {
            ResolveResult::Unresolved => {
                let result = self.resolve_std_prelude_name(name, span, context);
                if report && !name.is_empty() {
                    match &result {
                        ResolveResult::Unresolved => self.diagnostics.unresolved_name(name, span),
                        ResolveResult::Ambiguous(_) => self.diagnostics.ambiguous_name(name, span),
                        _ => {}
                    }
                }
                result
            }
            ResolveResult::Ambiguous(symbols) => {
                if report && !name.is_empty() {
                    self.diagnostics.ambiguous_name(name, span);
                }
                ResolveResult::Ambiguous(symbols)
            }
            result => result,
        }
    }

    fn resolve_module_name(
        &mut self,
        name: &str,
        span: Span,
        scope: ScopeId,
        report: bool,
    ) -> ResolveResult {
        match self.scopes.lookup_module(scope, name) {
            ResolveResult::Ambiguous(symbols) => {
                if report && !name.is_empty() {
                    self.diagnostics.ambiguous_name(name, span);
                }
                ResolveResult::Ambiguous(symbols)
            }
            ResolveResult::Unresolved => {
                if report && !name.is_empty() {
                    self.diagnostics.unresolved_name(name, span);
                }
                ResolveResult::Unresolved
            }
            result => result,
        }
    }

    fn resolve_qualified_prefix(
        &mut self,
        path: &ast::Path,
        scope: ScopeId,
        report: bool,
        context: PreludeContext,
    ) -> ResolveResult {
        let full_path = crate::path_text(path);
        for prefix_len in (1..path.segments.len()).rev() {
            let prefix = path.segments[..prefix_len]
                .iter()
                .map(|segment| segment.text.as_str())
                .collect::<Vec<_>>()
                .join(".");
            match self.resolve_name_in_context(&prefix, path.span, scope, false, context) {
                ResolveResult::Resolved(symbol) => {
                    let remaining = path.segments[prefix_len..]
                        .iter()
                        .map(|segment| segment.text.clone())
                        .collect::<Vec<_>>();
                    if let [name] = remaining.as_slice() {
                        let member = self.symbols.resolve_member(symbol, name);
                        if !matches!(member, ResolveResult::Unresolved) {
                            return member;
                        }
                    }
                    if let Some(member) =
                        self.resolve_std_enum_member(symbol, &remaining, path.span)
                    {
                        return ResolveResult::Resolved(member);
                    }
                    return ResolveResult::PartiallyResolved(PartialResolution {
                        resolved_prefix: Some(symbol),
                        resolved_segments: prefix_len.min(u32::MAX as usize) as u32,
                        remaining,
                        reason: partial_reason_for_prefix(
                            symbol,
                            &self.symbols,
                            self.std_registry.as_ref(),
                        ),
                    });
                }
                ResolveResult::Ambiguous(symbols) => {
                    if report {
                        self.diagnostics.ambiguous_name(&prefix, path.span);
                    }
                    return ResolveResult::Ambiguous(symbols);
                }
                ResolveResult::PartiallyResolved(partial) => {
                    return ResolveResult::PartiallyResolved(partial);
                }
                ResolveResult::Unresolved => {}
            }
        }

        let path_segments = path
            .segments
            .iter()
            .map(|segment| segment.text.clone())
            .collect::<Vec<_>>();
        if let Some(symbol) = self.resolve_std_qualified_path(&path_segments, path.span) {
            return ResolveResult::Resolved(symbol);
        }

        if package_resolver_required(path) {
            ResolveResult::PartiallyResolved(PartialResolution {
                resolved_prefix: None,
                resolved_segments: 0,
                remaining: path
                    .segments
                    .iter()
                    .map(|segment| segment.text.clone())
                    .collect(),
                reason: PartialResolutionReason::PackageResolverRequired,
            })
        } else if unsupported_path_shape(path) {
            ResolveResult::PartiallyResolved(PartialResolution {
                resolved_prefix: None,
                resolved_segments: 0,
                remaining: path
                    .segments
                    .iter()
                    .map(|segment| segment.text.clone())
                    .collect(),
                reason: PartialResolutionReason::UnsupportedPathShape,
            })
        } else {
            if report && !full_path.is_empty() {
                self.diagnostics.unresolved_name(&full_path, path.span);
            }
            ResolveResult::Unresolved
        }
    }

    fn resolve_module_qualified_prefix(
        &mut self,
        path: &ast::Path,
        scope: ScopeId,
        report: bool,
    ) -> ResolveResult {
        let full_path = crate::path_text(path);
        for prefix_len in (1..path.segments.len()).rev() {
            let prefix = path.segments[..prefix_len]
                .iter()
                .map(|segment| segment.text.as_str())
                .collect::<Vec<_>>()
                .join(".");
            match self.resolve_module_name(&prefix, path.span, scope, false) {
                ResolveResult::Resolved(symbol) => {
                    let remaining = path.segments[prefix_len..]
                        .iter()
                        .map(|segment| segment.text.clone())
                        .collect::<Vec<_>>();
                    if let Some(member) =
                        self.resolve_std_enum_member(symbol, &remaining, path.span)
                    {
                        return ResolveResult::Resolved(member);
                    }
                    return ResolveResult::PartiallyResolved(PartialResolution {
                        resolved_prefix: Some(symbol),
                        resolved_segments: prefix_len.min(u32::MAX as usize) as u32,
                        remaining,
                        reason: partial_reason_for_prefix(
                            symbol,
                            &self.symbols,
                            self.std_registry.as_ref(),
                        ),
                    });
                }
                ResolveResult::Ambiguous(symbols) => {
                    if report {
                        self.diagnostics.ambiguous_name(&prefix, path.span);
                    }
                    return ResolveResult::Ambiguous(symbols);
                }
                ResolveResult::PartiallyResolved(partial) => {
                    return ResolveResult::PartiallyResolved(partial);
                }
                ResolveResult::Unresolved => {}
            }
        }

        let path_segments = path
            .segments
            .iter()
            .map(|segment| segment.text.clone())
            .collect::<Vec<_>>();
        if let Some(symbol) = self.resolve_std_qualified_path(&path_segments, path.span) {
            return ResolveResult::Resolved(symbol);
        }

        if package_resolver_required(path) {
            ResolveResult::PartiallyResolved(PartialResolution {
                resolved_prefix: None,
                resolved_segments: 0,
                remaining: path
                    .segments
                    .iter()
                    .map(|segment| segment.text.clone())
                    .collect(),
                reason: PartialResolutionReason::PackageResolverRequired,
            })
        } else if unsupported_path_shape(path) {
            ResolveResult::PartiallyResolved(PartialResolution {
                resolved_prefix: None,
                resolved_segments: 0,
                remaining: path
                    .segments
                    .iter()
                    .map(|segment| segment.text.clone())
                    .collect(),
                reason: PartialResolutionReason::UnsupportedPathShape,
            })
        } else {
            if report && !full_path.is_empty() {
                self.diagnostics.unresolved_name(&full_path, path.span);
            }
            ResolveResult::Unresolved
        }
    }

    pub(super) fn bind_symbol_with_def(&mut self, scope: ScopeId, data: SymbolData) -> SymbolId {
        let name = data.name.clone();
        let span = data.definition_span;
        let symbol = self.alloc_symbol_with_def(data);
        self.insert_symbol(scope, &name, symbol, span);
        symbol
    }

    pub(super) fn insert_symbol(
        &mut self,
        scope: ScopeId,
        name: &str,
        symbol: SymbolId,
        span: Span,
    ) {
        if let Some(previous) = self.scopes.insert(scope, name.to_owned(), symbol) {
            let previous_span = self
                .symbols
                .get(previous)
                .map_or(span, |symbol| symbol.definition_span);
            self.diagnostics.duplicate_symbol(name, span, previous_span);
        }
    }

    pub(super) fn insert_module_symbol(
        &mut self,
        scope: ScopeId,
        name: &str,
        symbol: SymbolId,
        span: Span,
    ) {
        if let Some(previous) = self.scopes.insert_module(scope, name.to_owned(), symbol) {
            let previous_span = self
                .symbols
                .get(previous)
                .map_or(span, |symbol| symbol.definition_span);
            self.diagnostics.duplicate_symbol(name, span, previous_span);
        }
    }

    pub(super) fn alloc_symbol_with_def(&mut self, data: SymbolData) -> SymbolId {
        let span = data.definition_span;
        let symbol = self.symbols.alloc(data);
        self.source_map.map_symbol(symbol, span);
        symbol
    }

    pub(super) fn item_symbol_at(&mut self, item_id: HirItemId, span: Span) -> SymbolId {
        let key = super::body_overlay::key(span);
        let symbol = *self
            .top_item_symbols
            .get(&key)
            .expect("top-level item should be predeclared");
        self.set_symbol_item(symbol, item_id);
        symbol
    }

    pub(super) fn set_symbol_item(&mut self, symbol: SymbolId, item_id: HirItemId) {
        if let Some(symbol) = self.symbols.get_mut(symbol) {
            symbol.defining_item = Some(item_id);
            symbol.def = SymbolDef::Item { item: item_id };
        }
    }

    pub(super) fn map_item(&mut self, id: HirItemId, span: Span) {
        self.source_map.map_item(id, span);
    }
}

pub(super) fn item_symbol(item: &ast::Item) -> Option<(String, SymbolKind, Span)> {
    Some(match item {
        ast::Item::Alias(item) => (
            item.name.text.clone(),
            SymbolKind::TypeAlias,
            item.name.span,
        ),
        ast::Item::Type(item) => (item.name.text.clone(), SymbolKind::Type, item.name.span),
        ast::Item::Enum(item) => (item.name.text.clone(), SymbolKind::Enum, item.name.span),
        ast::Item::Spec(item) => (item.name.text.clone(), SymbolKind::Spec, item.name.span),
        ast::Item::Effect(item) => (item.name.text.clone(), SymbolKind::Effect, item.name.span),
        ast::Item::TopLevelLet(item) => (
            item.name.text.clone(),
            SymbolKind::TopLevelLet,
            item.name.span,
        ),
        ast::Item::Tool(item) => (
            crate::path_text(&item.path),
            SymbolKind::Tool,
            item.path.span,
        ),
        ast::Item::Agent(item) => (item.name.text.clone(), SymbolKind::Agent, item.name.span),
        ast::Item::Protocol(item) => (item.name.text.clone(), SymbolKind::Protocol, item.name.span),
        ast::Item::Flow(item) => (item.name.text.clone(), SymbolKind::Flow, item.name.span),
        ast::Item::Impl(_) | ast::Item::Error(_) => return None,
    })
}

pub(super) fn item_key(item: &ast::Item) -> SyntaxKey {
    let span = item.span();
    SyntaxKey {
        source: span.source,
        start: span.range.start.to_usize().min(u32::MAX as usize) as u32,
        end: span.range.end.to_usize().min(u32::MAX as usize) as u32,
    }
}

pub(super) fn action_key(action: &ast::EffectActionDecl) -> SyntaxKey {
    let span = action.span;
    SyntaxKey {
        source: span.source,
        start: span.range.start.to_usize().min(u32::MAX as usize) as u32,
        end: span.range.end.to_usize().min(u32::MAX as usize) as u32,
    }
}
