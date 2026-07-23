use etas_core::{Diagnostic, EffectDiagnosticCode, Span};
use etas_hir::{
    HirArg, HirExpr, HirExprId, HirItem, HirItemId, HirModuleId, ImportAliasOrigin,
    PartialResolutionReason, ResolveResult, SymbolDef,
};
use etas_hir_analysis::interprocedural::CallSite;
use etas_std::{IntrinsicRuntimeRequirement, StdDecl, TypeDeclKind};
use etas_types::{EffectArgRef, SymbolTypeFact, Type, TypeId};

use crate::{
    ActionEventSource, ActionInstanceRef, ActionRef, COMMAND_RUN_ACTION, COMMAND_TAG, Effect,
    EffectActionId, MEMORY_READ_ACTION, MEMORY_TAG, MEMORY_WRITE_ACTION,
};

use super::engine::EffectSemantics;
use super::state::EffectState;

impl EffectSemantics<'_> {
    pub(crate) fn apply_external_call(
        &mut self,
        site: CallSite<crate::infer::unit::EffectUnit>,
        mut state: EffectState,
    ) -> EffectState {
        if self.std_pure_support_call(site.callee_expr) {
            return state;
        }
        if self.pure_type_constructor_call(site.callee_expr) {
            return state;
        }
        if let Some(state) = self.std_memory_wrapper_call(site, state.clone()) {
            return state;
        }
        if let Some(state) = self.std_command_wrapper_call(site, state.clone()) {
            return state;
        }
        if let Some(summary) = self.std_flow_summary(site.callee_expr, site.span) {
            state.summary.seq_assign(&summary);
            return state;
        }
        if let Some(symbol) = self.callee_symbol(site.callee_expr)
            && let Some(symbol_data) = self.hir.symbols.get(symbol)
            && let SymbolDef::ImportAlias {
                path,
                origin: ImportAliasOrigin::SourceImport,
            } = &symbol_data.def
        {
            if let Some(item) = self.source_item_for_path(path)
                && let Some(summary) = self.summary_for_item_public_effects(item, site.span)
            {
                state.summary.seq_assign(&summary);
                return state;
            }
            if path.first().is_some_and(|segment| segment == "std") {
                if let Some(decl) = self
                    .std_registry
                    .lookup_qualified(&path.iter().map(String::as_str).collect::<Vec<_>>())
                {
                    if std_decl_is_pure_call_support(&decl.decl) {
                        return state;
                    }
                    return self.reject_effect_state(
                        site.span,
                        "std import call target requires callable effect metadata",
                        state,
                    );
                }
            }
            match self.external_call_summary_for_path(path, &site, &state) {
                Ok(Some(summary)) => {
                    state.summary.seq_assign(&summary);
                    return state;
                }
                Ok(None) => {}
                Err(error) => {
                    return self.incomplete_at(
                        site.span,
                        format!("external call effect selector specialization failed: {error}"),
                        state,
                    );
                }
            }
            return self.reject_effect_state(
                site.span,
                "source import target requires checked effect facts",
                state,
            );
        }
        if let Some(item) = self.callee_symbol(site.callee_expr).and_then(|symbol| {
            self.hir
                .symbols
                .get(symbol)
                .and_then(|symbol| match symbol.def {
                    SymbolDef::Item { item } => Some(item),
                    _ => None,
                })
        }) {
            if let Some(summary) = self.summary_for_item_public_effects(item, site.span) {
                state.summary.seq_assign(&summary);
            }
        }
        state
    }

    pub(crate) fn std_pure_support_call(&self, callee: HirExprId) -> bool {
        let Some(path) = self.std_qualified_path_for_expr(callee) else {
            return false;
        };
        let Some(decl) = self
            .std_registry
            .lookup_qualified(&path.iter().map(String::as_str).collect::<Vec<_>>())
        else {
            return false;
        };
        std_decl_is_pure_call_support(&decl.decl)
            || matches!(&decl.decl, StdDecl::Flow(flow) if flow.public_effects.is_empty() && flow.requested_actions.is_empty())
    }

    pub(crate) fn partially_resolved_std_method_call(
        &mut self,
        site: CallSite<crate::infer::unit::EffectUnit>,
        mut state: EffectState,
    ) -> Option<EffectState> {
        let HirExpr::Path(path) = self.hir.exprs.get(site.callee_expr)? else {
            return None;
        };
        let ResolveResult::PartiallyResolved(partial) = &path.resolution else {
            return None;
        };
        if partial.reason != PartialResolutionReason::MemberRequiresTypeChecking {
            return None;
        }
        let (method, receiver_projection) = partial.remaining.split_last()?;
        let actions = match method.as_str() {
            "get" | "contains" | "keys" | "select" | "query" | "scan" | "related_to" => {
                Some(&[MEMORY_READ_ACTION][..])
            }
            "put" | "put_versioned" | "insert" | "update" | "delete" | "delete_versioned"
            | "clear" => Some(&[MEMORY_WRITE_ACTION][..]),
            "upsert" => Some(&[MEMORY_READ_ACTION, MEMORY_WRITE_ACTION][..]),
            _ => None,
        };
        if let Some(actions) = actions {
            let Some(prefix) = partial.resolved_prefix else {
                return Some(self.reject_effect_state(
                    site.span,
                    "std store method effect solving requires a resolved receiver prefix",
                    state,
                ));
            };
            let Some(arg) =
                self.memory_place_arg_for_symbol_projection(prefix, receiver_projection)
            else {
                return Some(self.reject_effect_state(
                    site.span,
                    "std store method effect solving requires checked memory place facts",
                    state,
                ));
            };
            for action in actions {
                self.apply_memory_action(&mut state, *action, arg.clone(), site.span);
            }
            return Some(state);
        }

        if self.types.facts.expr_types.contains_key(&site.call)
            && self.std_method_name_is_pure_support(method)
        {
            return Some(state);
        }
        None
    }

    pub(crate) fn std_method_call(
        &mut self,
        expr: HirExprId,
        span: Span,
        mut state: EffectState,
    ) -> EffectState {
        if let Some(state) = self.std_conversation_method_call(expr, span, state.clone()) {
            return state;
        }
        if let Some(state) = self.std_store_method_call(expr, span, state.clone()) {
            return state;
        }
        if self.method_receiver_type_missing(expr) {
            return self.reject_effect_state(
                span,
                "method effect propagation requires a checked receiver type fact",
                state,
            );
        }
        if self.agent_run_source_import_target_missing(expr) {
            return self.reject_effect_state(
                span,
                "source import target requires checked effect facts",
                state,
            );
        }
        if let Some(summary) = self.agent_run_summary(expr, span) {
            state.summary.seq_assign(&summary);
        } else if self.is_agent_run_call(expr) {
            state = self.reject_effect_state(
                span,
                "agent run effect propagation requires a resolved agent receiver",
                state,
            );
        }
        state
    }

    fn std_conversation_method_call(
        &mut self,
        expr: HirExprId,
        span: Span,
        mut state: EffectState,
    ) -> Option<EffectState> {
        let HirExpr::MethodCall {
            receiver, method, ..
        } = self.hir.exprs.get(expr)?
        else {
            return None;
        };
        let (module_path, name) = self.std_import_path(*receiver)?;
        if module_path.as_slice() != ["std", "agent", "session"] || name != "Conversation" {
            return None;
        }
        let effect = match method.as_str() {
            "load" => self.session_memory_action("Memory.read"),
            "compact" => self.session_memory_action("Memory.write"),
            _ => return None,
        };
        let Some(effect) = effect else {
            return Some(self.reject_effect_state(
                span,
                "std conversation method effect solving requires standard session memory action metadata",
                state,
            ));
        };
        self.apply_requested_action_to_summary_with_source(
            &mut state.summary,
            effect,
            span,
            ActionEventSource::StdIntrinsic,
        );
        Some(state)
    }

    fn session_memory_action(&self, action_name: &str) -> Option<Effect> {
        let action = self.registry.action_by_name(action_name)?;
        Some(Effect::AppliedAction(ActionInstanceRef {
            action,
            args: vec![EffectArgRef::Path(vec![
                "std".to_owned(),
                "agent".to_owned(),
                "session".to_owned(),
                "SessionId".to_owned(),
            ])],
        }))
    }

    fn std_store_method_call(
        &mut self,
        expr: HirExprId,
        span: Span,
        mut state: EffectState,
    ) -> Option<EffectState> {
        let HirExpr::MethodCall {
            receiver, method, ..
        } = self.hir.exprs.get(expr)?
        else {
            return None;
        };
        let actions = match method.as_str() {
            "get" | "contains" | "keys" | "select" | "query" | "scan" | "related_to" => {
                Some(&[MEMORY_READ_ACTION][..])
            }
            "put" | "put_versioned" | "insert" | "update" | "delete" | "delete_versioned"
            | "clear" => Some(&[MEMORY_WRITE_ACTION][..]),
            "upsert" => Some(&[MEMORY_READ_ACTION, MEMORY_WRITE_ACTION][..]),
            "limit" => return Some(state),
            _ => None,
        }?;
        let Some(receiver_ty) = self.types.facts.expr_types.get(receiver).copied() else {
            return Some(self.reject_effect_state(
                span,
                "std store method effect solving requires a checked receiver type fact",
                state,
            ));
        };
        if !matches!(
            self.types.store.get(receiver_ty),
            Some(Type::Store { .. } | Type::MemorySelection(_))
        ) {
            return None;
        }
        let Some(arg) = self.memory_place_arg_for_expr(*receiver) else {
            return Some(self.reject_effect_state(
                span,
                "std store method effect solving requires checked memory place facts",
                state,
            ));
        };
        for action in actions {
            self.apply_memory_action(&mut state, *action, arg.clone(), span);
        }
        Some(state)
    }

    fn std_memory_wrapper_call(
        &mut self,
        site: CallSite<crate::infer::unit::EffectUnit>,
        mut state: EffectState,
    ) -> Option<EffectState> {
        let (module_path, name) = self.std_import_path(site.callee_expr)?;
        if module_path.as_slice() != ["std", "memory"] {
            return None;
        }
        let actions = match name.as_str() {
            "get" | "contains" | "keys" | "select" | "query" | "scan" | "related_to" => {
                Some(&[MEMORY_READ_ACTION][..])
            }
            "put" | "put_versioned" | "insert" | "update" | "delete" | "delete_versioned"
            | "clear" => Some(&[MEMORY_WRITE_ACTION][..]),
            "upsert" => Some(&[MEMORY_READ_ACTION, MEMORY_WRITE_ACTION][..]),
            "limit" => return Some(state),
            _ => None,
        }?;
        let Some(HirExpr::Call { args, .. }) = self.hir.exprs.get(site.call) else {
            return Some(self.reject_effect_state(
                site.span,
                "std memory wrapper effect solving requires a receiver argument",
                state,
            ));
        };
        let Some(receiver) = args.first().and_then(arg_expr) else {
            return Some(self.reject_effect_state(
                site.span,
                "std memory wrapper effect solving requires a receiver argument",
                state,
            ));
        };
        let Some(arg) = self.memory_place_arg_for_expr(receiver) else {
            return Some(self.reject_effect_state(
                site.span,
                "std memory wrapper effect solving requires a receiver argument",
                state,
            ));
        };
        for action in actions {
            self.apply_memory_action(&mut state, *action, arg.clone(), site.span);
        }
        Some(state)
    }

    fn apply_memory_action(
        &self,
        state: &mut EffectState,
        action: EffectActionId,
        arg: etas_types::EffectArgRef,
        span: Span,
    ) {
        let effect = Effect::AppliedAction(ActionInstanceRef {
            action: ActionRef {
                tag: MEMORY_TAG,
                action,
            },
            args: vec![arg],
        });
        self.apply_effect_to_summary_with_source(
            &mut state.summary,
            effect,
            span,
            ActionEventSource::StdIntrinsic,
        );
    }

    fn std_command_wrapper_call(
        &mut self,
        site: CallSite<crate::infer::unit::EffectUnit>,
        mut state: EffectState,
    ) -> Option<EffectState> {
        let (module_path, name) = self.std_import_path(site.callee_expr)?;
        if module_path.as_slice() != ["std", "host", "command"] || name != "run" {
            return None;
        }
        let Some(HirExpr::Call { args, .. }) = self.hir.exprs.get(site.call) else {
            return Some(self.reject_effect_state(
                site.span,
                "std command wrapper effect solving requires checked call arguments",
                state,
            ));
        };
        let Some(sandbox) = args.get(1).and_then(arg_expr) else {
            return Some(self.reject_effect_state(
                site.span,
                "std command wrapper effect solving requires a sandbox argument",
                state,
            ));
        };
        let Some(arg) = self.static_resource_path_arg_for_expr(sandbox) else {
            return Some(self.reject_effect_state(
                site.span,
                "std command wrapper effect solving requires checked sandbox value path facts",
                state,
            ));
        };
        let effect = Effect::AppliedAction(ActionInstanceRef {
            action: ActionRef {
                tag: COMMAND_TAG,
                action: COMMAND_RUN_ACTION,
            },
            args: vec![arg],
        });
        self.apply_requested_action_to_summary_with_source(
            &mut state.summary,
            effect,
            site.span,
            ActionEventSource::StdIntrinsic,
        );
        Some(state)
    }

    fn memory_place_arg_for_expr(&self, expr: HirExprId) -> Option<etas_types::EffectArgRef> {
        let ty = self.types.facts.expr_memory_places.get(&expr)?;
        let etas_types::Type::MemoryPlace(place) = self.types.store.get(*ty)? else {
            return None;
        };
        Some(etas_types::EffectArgRef::Path(place.segments.clone()))
    }

    fn static_resource_path_arg_for_expr(
        &self,
        expr: HirExprId,
    ) -> Option<etas_types::EffectArgRef> {
        let HirExpr::Path(path) = self.hir.exprs.get(expr)? else {
            return None;
        };
        if let Some(segments) = self.canonical_static_resource_path_segments(path) {
            return Some(etas_types::EffectArgRef::Path(segments));
        }
        Some(etas_types::EffectArgRef::Path(
            path.segments
                .iter()
                .map(|segment| segment.name.clone())
                .collect(),
        ))
    }

    fn std_import_path(&self, callee: HirExprId) -> Option<(Vec<String>, String)> {
        let symbol = self.callee_symbol(callee)?;
        let symbol = self.hir.symbols.get(symbol)?;
        let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
            return None;
        };
        let (name, module_path) = path.split_last()?;
        Some((module_path.to_vec(), name.clone()))
    }

    fn std_qualified_path_for_expr(&self, expr: HirExprId) -> Option<Vec<String>> {
        let HirExpr::Path(path) = self.hir.exprs.get(expr)? else {
            return None;
        };
        match &path.resolution {
            ResolveResult::Resolved(symbol) => {
                let symbol = self.hir.symbols.get(*symbol)?;
                let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
                    return None;
                };
                path.first()
                    .is_some_and(|segment| segment == "std")
                    .then(|| path.clone())
            }
            ResolveResult::PartiallyResolved(partial)
                if partial.reason == PartialResolutionReason::MemberRequiresTypeChecking =>
            {
                let symbol = self.hir.symbols.get(partial.resolved_prefix?)?;
                let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
                    return None;
                };
                if path.first().is_none_or(|segment| segment != "std") {
                    return None;
                }
                let mut qualified = path.clone();
                qualified.extend(partial.remaining.iter().cloned());
                Some(qualified)
            }
            _ => None,
        }
    }

    fn std_method_name_is_pure_support(&self, method: &str) -> bool {
        self.std_registry.symbols().any(|symbol| {
            symbol.name == method
                && matches!(
                    &symbol.decl,
                    StdDecl::Flow(flow)
                        if flow.public_effects.is_empty() && flow.requested_actions.is_empty()
                )
        })
    }

    fn memory_place_arg_for_symbol_projection(
        &self,
        symbol: etas_hir::SymbolId,
        projection: &[String],
    ) -> Option<etas_types::EffectArgRef> {
        let ty = self.symbol_value_type(symbol)?;
        let mut segments = match self.types.store.get(ty)? {
            Type::MemoryPlace(place) => place.segments.clone(),
            Type::MemoryRegion(_)
            | Type::ResourceHandle(etas_types::ResourceHandleType::MemoryRegion { .. }) => {
                self.canonical_symbol_path(symbol)?
            }
            _ => return None,
        };
        segments.extend(projection.iter().cloned());
        Some(etas_types::EffectArgRef::Path(segments))
    }

    fn canonical_symbol_path(&self, symbol: etas_hir::SymbolId) -> Option<Vec<String>> {
        let symbol = self.hir.symbols.get(symbol)?;
        if let SymbolDef::ImportAlias { path, .. } = &symbol.def {
            return Some(path.clone());
        }
        let module = self.hir.modules_arena.get(symbol.defining_module)?;
        let mut segments = module
            .name
            .as_ref()
            .map(|module| {
                module
                    .segments
                    .iter()
                    .map(|segment| segment.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        segments.push(symbol.name.clone());
        Some(segments)
    }

    fn method_receiver_type_missing(&self, expr: HirExprId) -> bool {
        let Some(HirExpr::MethodCall {
            receiver, method, ..
        }) = self.hir.exprs.get(expr)
        else {
            return false;
        };
        matches!(method.as_str(), "len" | "is_empty" | "unwrap" | "run")
            && !self.types.facts.expr_types.contains_key(receiver)
    }

    fn is_agent_run_call(&self, expr: HirExprId) -> bool {
        matches!(
            self.hir.exprs.get(expr),
            Some(HirExpr::MethodCall { method, .. }) if method == "run"
        )
    }

    fn agent_run_source_import_target_missing(&self, expr: HirExprId) -> bool {
        let Some(HirExpr::MethodCall {
            receiver, method, ..
        }) = self.hir.exprs.get(expr)
        else {
            return false;
        };
        if method != "run" {
            return false;
        }
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(*receiver) else {
            return false;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        let Some(symbol) = self.hir.symbols.get(symbol) else {
            return false;
        };
        let SymbolDef::ImportAlias {
            path,
            origin: ImportAliasOrigin::SourceImport,
        } = &symbol.def
        else {
            return false;
        };
        self.source_agent_item_for_path(path).is_none()
    }

    fn agent_run_summary(&self, expr: HirExprId, span: Span) -> Option<crate::EffectSummary> {
        let HirExpr::MethodCall {
            receiver, method, ..
        } = self.hir.exprs.get(expr)?
        else {
            return None;
        };
        if method != "run" {
            return None;
        }
        let item = self.agent_item_for_receiver(*receiver)?;
        self.summary_for_agent_call(item, span)
    }

    fn agent_item_for_receiver(&self, receiver: HirExprId) -> Option<HirItemId> {
        let HirExpr::Path(path) = self.hir.exprs.get(receiver)? else {
            return None;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        let symbol = self.hir.symbols.get(symbol)?;
        match &symbol.def {
            SymbolDef::Item { item }
                if matches!(self.hir.items.get(*item), Some(HirItem::Agent(_))) =>
            {
                Some(*item)
            }
            SymbolDef::ImportAlias {
                path,
                origin: ImportAliasOrigin::SourceImport,
            } => self.source_agent_item_for_path(path),
            _ => None,
        }
    }

    fn source_agent_item_for_path(&self, path: &[String]) -> Option<HirItemId> {
        let (name, module_path) = path.split_last()?;
        for (item, hir_item) in self.hir.items.iter() {
            let HirItem::Agent(agent) = hir_item else {
                continue;
            };
            let Some(symbol) = self.hir.symbols.get(agent.symbol) else {
                continue;
            };
            if symbol.name != *name {
                continue;
            }
            if self.module_path(symbol.defining_module).as_deref() == Some(module_path) {
                return Some(item);
            }
        }
        None
    }

    fn module_path(&self, module: HirModuleId) -> Option<Vec<String>> {
        self.hir
            .modules_arena
            .get(module)?
            .name
            .as_ref()
            .map(|path| {
                path.segments
                    .iter()
                    .map(|segment| segment.name.clone())
                    .collect()
            })
    }

    fn std_flow_summary(&mut self, callee: HirExprId, span: Span) -> Option<crate::EffectSummary> {
        let symbol = self.callee_symbol(callee)?;
        let symbol_data = self.hir.symbols.get(symbol)?;
        let SymbolDef::ImportAlias { path, .. } = &symbol_data.def else {
            return None;
        };
        let decl = self
            .std_registry
            .lookup_qualified(&path.iter().map(String::as_str).collect::<Vec<_>>())?;
        let StdDecl::Flow(flow) = &decl.decl else {
            return None;
        };
        let mut summary = self.summary_from_std_flow(symbol, flow, span);
        if let Some(intrinsic) = &decl.intrinsic {
            match intrinsic.runtime_requirement {
                IntrinsicRuntimeRequirement::None => {}
                IntrinsicRuntimeRequirement::Checkpoint => {
                    summary.require_runtime(crate::RuntimeRequirementReason::Checkpoint);
                }
            }
        }
        Some(summary)
    }

    fn summary_from_std_flow(
        &mut self,
        symbol: etas_hir::SymbolId,
        flow: &etas_std::FlowDecl,
        span: Span,
    ) -> crate::EffectSummary {
        let mut summary = crate::EffectSummary::local();
        let Some(SymbolTypeFact::Flow { signature }) =
            self.type_symbols
                .symbol_fact(self.hir, &self.types.facts, symbol)
        else {
            self.diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                span,
                format!(
                    "standard library flow `{}` requires checked std flow signature facts",
                    flow.name
                ),
            ));
            return summary;
        };
        if let Some(row) = &signature.effects {
            self.apply_public_row_to_summary(&mut summary, &self.row_from_type_ref(row), span);
        }
        let Some(requested_actions) = &signature.requested_actions else {
            if !flow.requested_actions.is_empty() {
                self.diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    span,
                    format!(
                        "standard library flow `{}` requires checked requested-action facts",
                        flow.name
                    ),
                ));
            }
            return summary;
        };
        let requested_actions = self.row_from_type_ref(requested_actions);
        for action_ref in requested_actions.effects.iter().cloned() {
            self.apply_requested_action_to_summary_with_source(
                &mut summary,
                action_ref,
                span,
                crate::ActionEventSource::StdIntrinsic,
            );
        }
        summary
    }

    pub(crate) fn action_effect_with_omitted_selector(&self, action: ActionRef) -> Effect {
        let Some(signature) = self.registry.action_signature(&action) else {
            return Effect::Action(action);
        };
        if signature.effect_args.is_empty() {
            return Effect::Action(action);
        }
        let args = signature
            .effect_args
            .iter()
            .enumerate()
            .map(|(index, _)| {
                signature
                    .selector_defaults
                    .get(index)
                    .and_then(Option::as_ref)
                    .cloned()
                    .unwrap_or(etas_types::EffectArgRef::Wildcard)
            })
            .collect();
        Effect::AppliedAction(ActionInstanceRef { action, args })
    }

    pub(crate) fn type_id_for_external_effect_path(&self, path: &[String]) -> Option<TypeId> {
        if path.len() == 1 && !self.is_standard_type_name(path.first()?) {
            return None;
        }
        if let Some(ty) = self.type_id_for_symbol_effect_path(path) {
            return Some(ty);
        }
        let qualified = path.join(".");
        let mut matches = self
            .types
            .store
            .iter()
            .filter_map(|(id, ty)| {
                let name = type_effect_name(ty)?;
                (name == qualified).then_some(id)
            })
            .collect::<Vec<_>>();
        matches.sort();
        matches.dedup();
        if matches.len() == 1 {
            Some(matches[0])
        } else {
            None
        }
    }

    fn is_standard_type_name(&self, name: &str) -> bool {
        self.std_registry
            .symbols()
            .any(|symbol| symbol.name == name && matches!(&symbol.decl, StdDecl::Type(_)))
    }

    fn type_id_for_symbol_effect_path(&self, path: &[String]) -> Option<TypeId> {
        if path.len() < 2 {
            return None;
        }
        let mut matches = self
            .hir
            .symbols
            .iter()
            .filter_map(|symbol| {
                let SymbolDef::ImportAlias {
                    path: alias_path, ..
                } = &symbol.def
                else {
                    return None;
                };
                if alias_path != path {
                    return None;
                }
                match self
                    .type_symbols
                    .symbol_fact(self.hir, &self.types.facts, symbol.id)?
                {
                    SymbolTypeFact::Type { constructor } => Some(TypeId(constructor.0)),
                    SymbolTypeFact::Value { ty }
                    | SymbolTypeFact::TopLevelLet { ty, .. }
                    | SymbolTypeFact::Param { ty }
                    | SymbolTypeFact::Local { ty, .. }
                    | SymbolTypeFact::Field { ty } => Some(*ty),
                    _ => None,
                }
            })
            .collect::<Vec<_>>();
        matches.sort();
        matches.dedup();
        if matches.len() == 1 {
            Some(matches[0])
        } else {
            None
        }
    }
}

fn std_decl_is_pure_call_support(decl: &StdDecl) -> bool {
    match decl {
        StdDecl::Value(_) | StdDecl::Requirement(_) => true,
        StdDecl::Type(ty) => matches!(
            ty.kind,
            TypeDeclKind::Struct
                | TypeDeclKind::Enum
                | TypeDeclKind::Wrapper
                | TypeDeclKind::Support
        ),
        StdDecl::Flow(_)
        | StdDecl::Effect(_)
        | StdDecl::EffectAction(_)
        | StdDecl::Tool(_)
        | StdDecl::Impl(_) => false,
    }
}

pub(super) fn type_effect_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Primitive(primitive) => Some(primitive.source_name().to_owned()),
        Type::Enum(enum_ref) => Some(enum_ref.name.clone()),
        Type::Named(named) => Some(named.name.clone()),
        Type::Nominal(nominal) => Some(nominal.name.clone()),
        _ => None,
    }
}

fn arg_expr(arg: &HirArg) -> Option<HirExprId> {
    match arg {
        HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => Some(*expr),
    }
}
