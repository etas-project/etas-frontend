use etas_core::{Diagnostic, EffectDiagnosticCode, Span};
use etas_hir::{
    HirArg, HirEffectArg, HirExprId, HirGenericArg, HirPrimitiveType, HirType,
    PartialResolutionReason, ResolveResult, ResolvedActionRef, SymbolDef, SyntheticSymbolReason,
    TopLevelLetClassification,
};
use etas_types::{NamedTypeRef, PrimitiveType, SymbolTypeFact, Type, TypeId};

use crate::{
    ActionEventSource, ActionInstanceRef, ActionRef, Effect, EffectActionArgKind,
    FrontendRejectionReason, InterpreterSupport, PerformedActionFact,
};

use super::engine::EffectSemantics;
use super::state::EffectState;

impl EffectSemantics<'_> {
    pub(crate) fn perform_action(
        &mut self,
        expr: HirExprId,
        action: &ResolvedActionRef,
        generic_args: &[HirGenericArg],
        args: &[HirArg],
        span: Span,
        mut state: EffectState,
    ) -> EffectState {
        let Some(effect) = self.effect_from_action_ref(action) else {
            let (code, message) = if matches!(action.action_symbol, ResolveResult::Resolved(_)) {
                (
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    "performed action requires a checked action signature fact",
                )
            } else {
                (
                    EffectDiagnosticCode::UnresolvedPerformedAction,
                    "performed action requires a resolved action target",
                )
            };
            self.diagnostics
                .push(Diagnostic::effect_check(code, span, message));
            state.summary.support =
                InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect);
            return state;
        };
        let Some(effect) = self.materialize_performed_action_effect(
            &effect,
            action,
            generic_args,
            state.owner,
            span,
        ) else {
            state.summary.support =
                InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect);
            return state;
        };

        self.apply_effect_to_summary_with_source(
            &mut state.summary,
            effect.clone(),
            span,
            ActionEventSource::Perform,
        );
        self.inputs.performed_actions.insert(
            expr,
            PerformedActionFact {
                expr,
                effect_segments: action
                    .effect
                    .path
                    .segments
                    .iter()
                    .map(|segment| segment.name.clone())
                    .collect(),
                action: action.action.clone(),
                action_symbol: match action.action_symbol {
                    ResolveResult::Resolved(symbol) => Some(symbol),
                    _ => None,
                },
                args: args
                    .iter()
                    .map(|arg| match arg {
                        HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => *expr,
                    })
                    .collect(),
                span,
                summary: EffectSummaryForFact::one(effect, span, self),
            },
        );
        state
    }

    pub(crate) fn effect_from_action_ref(&self, action: &ResolvedActionRef) -> Option<Effect> {
        let owner = action
            .effect
            .path
            .segments
            .last()
            .map(|segment| segment.name.as_str())?;
        if owner == "Error" && action.action == "raise" {
            let error = self.error_type_from_hir_arg(action.effect.args.first()?)?;
            return Some(Effect::Error(error));
        }
        match action.action_symbol {
            ResolveResult::Resolved(symbol) => self
                .action_ref_for_resolved_action(action, symbol)
                .map(Effect::Action),
            _ => None,
        }
    }

    pub(crate) fn signature_for_action_ref(
        &self,
        action: &ResolvedActionRef,
    ) -> Option<&crate::EffectActionSig> {
        match action.action_symbol {
            ResolveResult::Resolved(symbol) => self.registry.action(symbol).or_else(|| {
                self.standard_action_signature_for_synthetic(action, symbol)
                    .or_else(|| self.external_action_signature_for_synthetic(action, symbol))
            }),
            _ => None,
        }
    }

    fn action_ref_for_resolved_action(
        &self,
        action: &ResolvedActionRef,
        symbol: etas_hir::SymbolId,
    ) -> Option<ActionRef> {
        if let Some(sig) = self.registry.action(symbol) {
            return Some(ActionRef {
                tag: sig.owner,
                action: sig.id,
            });
        }
        self.standard_action_ref_from_resolved_synthetic(action, symbol)
            .or_else(|| self.external_action_ref_from_resolved_synthetic(action, symbol))
    }

    fn standard_action_signature_for_synthetic(
        &self,
        action: &ResolvedActionRef,
        symbol: etas_hir::SymbolId,
    ) -> Option<&crate::EffectActionSig> {
        let action_ref = self.standard_action_ref_from_resolved_synthetic(action, symbol)?;
        self.registry.action_signature(&action_ref)
    }

    fn standard_action_ref_from_resolved_synthetic(
        &self,
        action: &ResolvedActionRef,
        symbol: etas_hir::SymbolId,
    ) -> Option<ActionRef> {
        if !is_synthetic_standard_action(self.hir, symbol) {
            return None;
        }
        let owner = action
            .effect
            .path
            .segments
            .last()
            .map(|segment| segment.name.as_str())?;
        self.registry
            .action_by_name(&format!("{owner}.{}", action.action))
    }

    fn external_action_signature_for_synthetic(
        &self,
        action: &ResolvedActionRef,
        symbol: etas_hir::SymbolId,
    ) -> Option<&crate::EffectActionSig> {
        let action_ref = self.external_action_ref_from_resolved_synthetic(action, symbol)?;
        self.registry.action_signature(&action_ref)
    }

    fn external_action_ref_from_resolved_synthetic(
        &self,
        action: &ResolvedActionRef,
        symbol: etas_hir::SymbolId,
    ) -> Option<ActionRef> {
        if !is_synthetic_external_action(self.hir, symbol) {
            return None;
        }
        let path = external_action_path(self.hir, action)?;
        self.registry.action_by_name(&path.join("."))
    }

    fn materialize_performed_action_effect(
        &mut self,
        effect: &Effect,
        action: &ResolvedActionRef,
        generic_args: &[HirGenericArg],
        owner: Option<etas_hir::HirItemId>,
        span: Span,
    ) -> Option<Effect> {
        for owner_arg in &action.effect.args {
            if self.type_arg_from_hir_effect_arg(owner_arg).is_none() {
                self.diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    span,
                    "owner effect arguments require checked type argument facts",
                ));
                return None;
            }
        }
        let Effect::Action(action_ref) = effect else {
            return Some(effect.clone());
        };
        let signature = self.registry.action_signature(action_ref)?;
        if !generic_args.is_empty() && generic_args.len() != signature.effect_args.len() {
            self.diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                span,
                format!(
                    "performed action selector expects {} static argument(s), got {}",
                    signature.effect_args.len(),
                    generic_args.len()
                ),
            ));
            return None;
        }
        if signature.effect_args.is_empty() {
            return Some(effect.clone());
        }
        let mut effect_args = Vec::new();
        for (index, kind) in signature.effect_args.iter().enumerate() {
            let Some(arg) = self.materialize_action_effect_arg(
                kind,
                generic_args.get(index),
                signature.selector_param_names.get(index),
                signature
                    .selector_defaults
                    .get(index)
                    .and_then(Option::as_ref),
                owner,
            ) else {
                self.diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    span,
                    "performed action static selector requires checked generic action argument facts",
                ));
                return None;
            };
            effect_args.push(arg);
        }
        Some(Effect::AppliedAction(ActionInstanceRef {
            action: action_ref.clone(),
            args: effect_args,
        }))
    }

    fn materialize_action_effect_arg(
        &self,
        kind: &EffectActionArgKind,
        generic_arg: Option<&HirGenericArg>,
        selector_param_name: Option<&String>,
        selector_default: Option<&etas_types::EffectArgRef>,
        owner: Option<etas_hir::HirItemId>,
    ) -> Option<etas_types::EffectArgRef> {
        let Some(generic_arg) = generic_arg else {
            if let Some(default) = selector_default {
                return Some(default.clone());
            }
            if let Some(inferred) = self.infer_action_selector_arg(kind, selector_param_name, owner)
            {
                return Some(inferred);
            }
            return Some(etas_types::EffectArgRef::Wildcard);
        };
        match kind {
            EffectActionArgKind::Type => self.type_selector_from_generic_arg(generic_arg),
            EffectActionArgKind::MemoryPlace => self.memory_selector_from_generic_arg(generic_arg),
            EffectActionArgKind::StaticResourcePath { .. } => {
                self.static_selector_from_generic_arg(generic_arg)
            }
            EffectActionArgKind::StringPattern => {
                self.string_selector_from_generic_arg(generic_arg)
            }
        }
    }

    fn infer_action_selector_arg(
        &self,
        kind: &EffectActionArgKind,
        selector_param_name: Option<&String>,
        owner: Option<etas_hir::HirItemId>,
    ) -> Option<etas_types::EffectArgRef> {
        let EffectActionArgKind::Type = kind else {
            return None;
        };
        let selector_param_name = selector_param_name?;
        let owner = owner?;
        let type_param = self.owner_type_params(owner)?.into_iter().find(|symbol| {
            self.hir
                .symbols
                .get(*symbol)
                .is_some_and(|symbol| symbol.name == *selector_param_name)
        })?;
        let symbol = self.hir.symbols.get(type_param)?;
        let expected = Type::Named(NamedTypeRef {
            name: symbol.name.clone(),
        });
        self.types
            .store
            .iter()
            .find_map(|(id, ty)| (ty == &expected).then_some(id))
            .map(etas_types::EffectArgRef::Type)
    }

    fn owner_type_params(&self, owner: etas_hir::HirItemId) -> Option<Vec<etas_hir::SymbolId>> {
        match self.hir.items.get(owner)? {
            etas_hir::HirItem::Flow(flow) => Some(flow.type_params.clone()),
            etas_hir::HirItem::Tool(tool) => Some(tool.type_params.clone()),
            etas_hir::HirItem::Agent(_)
            | etas_hir::HirItem::TopLevelLet(_)
            | etas_hir::HirItem::TypeAlias(_)
            | etas_hir::HirItem::Type(_)
            | etas_hir::HirItem::Enum(_)
            | etas_hir::HirItem::Spec(_)
            | etas_hir::HirItem::Impl(_)
            | etas_hir::HirItem::Effect(_)
            | etas_hir::HirItem::Protocol(_)
            | etas_hir::HirItem::Error { .. } => None,
        }
    }

    fn type_selector_from_generic_arg(
        &self,
        generic_arg: &HirGenericArg,
    ) -> Option<etas_types::EffectArgRef> {
        match generic_arg {
            HirGenericArg::Type(ty) => self
                .type_id_from_type_selector(*ty)
                .map(etas_types::EffectArgRef::Type),
            HirGenericArg::Wildcard { .. } => Some(etas_types::EffectArgRef::Wildcard),
            HirGenericArg::EffectRow(_) => None,
        }
    }

    fn memory_selector_from_generic_arg(
        &self,
        generic_arg: &HirGenericArg,
    ) -> Option<etas_types::EffectArgRef> {
        match generic_arg {
            HirGenericArg::Type(ty) => {
                if let Some(type_id) = self.types.facts.type_refs.get(ty).copied()
                    && let Some(etas_types::Type::MemoryPlace(place)) =
                        self.types.store.get(type_id)
                {
                    return Some(etas_types::EffectArgRef::Path(place.segments.clone()));
                }
                self.static_path_from_type(*ty)
            }
            HirGenericArg::Wildcard { .. } => Some(etas_types::EffectArgRef::Wildcard),
            HirGenericArg::EffectRow(_) => None,
        }
    }

    fn static_selector_from_generic_arg(
        &self,
        generic_arg: &HirGenericArg,
    ) -> Option<etas_types::EffectArgRef> {
        match generic_arg {
            HirGenericArg::Type(ty) => self.static_path_from_type(*ty),
            HirGenericArg::Wildcard { .. } => Some(etas_types::EffectArgRef::Wildcard),
            HirGenericArg::EffectRow(_) => None,
        }
    }

    fn string_selector_from_generic_arg(
        &self,
        generic_arg: &HirGenericArg,
    ) -> Option<etas_types::EffectArgRef> {
        match generic_arg {
            HirGenericArg::Type(ty) => self.static_path_from_type(*ty),
            HirGenericArg::Wildcard { .. } => Some(etas_types::EffectArgRef::Wildcard),
            HirGenericArg::EffectRow(_) => None,
        }
    }

    fn static_path_from_type(&self, ty: etas_hir::HirTypeId) -> Option<etas_types::EffectArgRef> {
        let HirType::Path { path, .. } = self.hir.types.get(ty)? else {
            return None;
        };
        self.canonical_static_resource_path_segments(path)
            .map(etas_types::EffectArgRef::Path)
    }

    fn type_id_from_type_selector(&self, ty: etas_hir::HirTypeId) -> Option<TypeId> {
        if let Some(type_id) = self.types.facts.type_refs.get(&ty).copied() {
            return Some(type_id);
        }
        if let HirType::Primitive { kind, .. } = self.hir.types.get(ty)? {
            return self.primitive_type_id(*kind);
        }
        let HirType::Path { path, .. } = self.hir.types.get(ty)? else {
            return None;
        };
        let symbol = match &path.resolution {
            ResolveResult::Resolved(symbol) => *symbol,
            ResolveResult::PartiallyResolved(partial)
                if partial.reason == PartialResolutionReason::MemberRequiresTypeChecking
                    && partial.remaining.is_empty() =>
            {
                partial.resolved_prefix?
            }
            _ => return None,
        };
        if let Some(symbol_data) = self.hir.symbols.get(symbol)
            && let SymbolDef::TypeParam { .. } = symbol_data.def
        {
            let expected = Type::Named(NamedTypeRef {
                name: symbol_data.name.clone(),
            });
            return self
                .types
                .store
                .iter()
                .find_map(|(id, ty)| (ty == &expected).then_some(id));
        }
        match self
            .type_symbols
            .symbol_fact(self.hir, &self.types.facts, symbol)?
        {
            SymbolTypeFact::Type { constructor }
            | SymbolTypeFact::NominalType { constructor, .. } => Some(TypeId(constructor.0)),
            SymbolTypeFact::TypeAlias { target, .. } => Some(*target),
            _ => None,
        }
    }

    fn primitive_type_id(&self, kind: HirPrimitiveType) -> Option<TypeId> {
        let primitive = match kind {
            HirPrimitiveType::Bool => PrimitiveType::Bool,
            HirPrimitiveType::I8 => PrimitiveType::I8,
            HirPrimitiveType::I16 => PrimitiveType::I16,
            HirPrimitiveType::I32 => PrimitiveType::I32,
            HirPrimitiveType::I64 => PrimitiveType::I64,
            HirPrimitiveType::I128 => PrimitiveType::I128,
            HirPrimitiveType::Isize => PrimitiveType::ISize,
            HirPrimitiveType::U8 => PrimitiveType::U8,
            HirPrimitiveType::U16 => PrimitiveType::U16,
            HirPrimitiveType::U32 => PrimitiveType::U32,
            HirPrimitiveType::U64 => PrimitiveType::U64,
            HirPrimitiveType::U128 => PrimitiveType::U128,
            HirPrimitiveType::Usize => PrimitiveType::USize,
            HirPrimitiveType::F32 => PrimitiveType::F32,
            HirPrimitiveType::F64 => PrimitiveType::F64,
            HirPrimitiveType::Char => PrimitiveType::Char,
            HirPrimitiveType::String => PrimitiveType::String,
            HirPrimitiveType::Bytes => PrimitiveType::Bytes,
            HirPrimitiveType::Unit => PrimitiveType::Unit,
            HirPrimitiveType::Never => PrimitiveType::Never,
        };
        self.types
            .store
            .iter()
            .find_map(|(id, ty)| (ty == &Type::Primitive(primitive)).then_some(id))
    }

    pub(crate) fn canonical_static_resource_path_segments(
        &self,
        path: &etas_hir::ResolvedPath,
    ) -> Option<Vec<String>> {
        let (symbol, remaining) = match &path.resolution {
            ResolveResult::Resolved(symbol) => (*symbol, Vec::new()),
            ResolveResult::PartiallyResolved(partial)
                if partial.reason == PartialResolutionReason::MemberRequiresTypeChecking =>
            {
                (partial.resolved_prefix?, partial.remaining.clone())
            }
            _ => return None,
        };
        let symbol = self.hir.symbols.get(symbol)?;
        if let SymbolDef::ImportAlias { path, .. } = &symbol.def {
            let mut path = path.clone();
            path.extend(remaining);
            return Some(path);
        }
        let is_static_symbol = match &symbol.def {
            SymbolDef::Item { .. }
            | SymbolDef::EnumVariant { .. }
            | SymbolDef::Synthetic { .. } => true,
            SymbolDef::TopLevelLet { classification, .. } => matches!(
                classification,
                TopLevelLetClassification::Unknown
                    | TopLevelLetClassification::Const
                    | TopLevelLetClassification::ResourceHandle(_)
                    | TopLevelLetClassification::Handler
            ),
            _ => false,
        };
        if !is_static_symbol {
            return None;
        }
        let module = self.hir.modules_arena.get(symbol.defining_module)?;
        let mut segments = module
            .name
            .as_ref()
            .map(|module_name| {
                module_name
                    .segments
                    .iter()
                    .map(|segment| segment.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        segments.push(symbol.name.clone());
        segments.extend(remaining);
        Some(segments)
    }

    fn error_type_from_hir_arg(&self, arg: &HirEffectArg) -> Option<TypeId> {
        self.type_arg_from_hir_effect_arg(arg)
    }

    pub(crate) fn type_arg_from_hir_effect_arg(&self, arg: &HirEffectArg) -> Option<TypeId> {
        match arg {
            HirEffectArg::Type(ty) => self.types.facts.type_refs.get(ty).copied(),
            HirEffectArg::Path(path) => {
                let ResolveResult::Resolved(symbol) = path.resolution else {
                    return None;
                };
                match self
                    .type_symbols
                    .symbol_fact(self.hir, &self.types.facts, symbol)?
                {
                    SymbolTypeFact::Type { constructor }
                    | SymbolTypeFact::NominalType { constructor, .. } => {
                        Some(TypeId(constructor.0))
                    }
                    SymbolTypeFact::TypeAlias { target, .. } => Some(*target),
                    _ => None,
                }
            }
            HirEffectArg::Wildcard { .. }
            | HirEffectArg::String { .. }
            | HirEffectArg::Int { .. } => None,
        }
    }

    pub(crate) fn apply_effect_to_summary_with_source(
        &self,
        summary: &mut crate::EffectSummary,
        effect: Effect,
        span: Span,
        source: ActionEventSource,
    ) {
        if self.record_performed_action_effect(summary, &effect, span, source) {
            return;
        }
        self.apply_effect_to_summary(summary, effect, span);
    }

    pub(crate) fn apply_requested_action_to_summary_with_source(
        &self,
        summary: &mut crate::EffectSummary,
        effect: Effect,
        span: Span,
        source: ActionEventSource,
    ) {
        if self.record_default_requested_action_effect(summary, &effect, span, source) {
            return;
        }
        self.apply_effect_to_summary(summary, effect, span);
    }

    fn record_performed_action_effect(
        &self,
        summary: &mut crate::EffectSummary,
        effect: &Effect,
        span: Span,
        source: ActionEventSource,
    ) -> bool {
        let action = match effect {
            Effect::Action(action) => action,
            Effect::AppliedAction(action) => &action.action,
            _ => return false,
        };
        summary.record_requested_action(effect.clone());
        summary.record_escaping_effect(effect.clone());
        summary.record_action_trace_event(effect.clone(), span, source);
        if let Some(reason) = self
            .registry
            .runtime_requirement_reason_for_action_ref(action)
        {
            summary.require_runtime(reason);
        }
        true
    }

    fn record_default_requested_action_effect(
        &self,
        summary: &mut crate::EffectSummary,
        effect: &Effect,
        span: Span,
        source: ActionEventSource,
    ) -> bool {
        let action = match effect {
            Effect::Action(action) => action,
            Effect::AppliedAction(action) => &action.action,
            _ => return false,
        };
        summary.record_requested_action(effect.clone());
        summary.record_default_handled_action(effect.clone());
        summary.record_action_trace_event(effect.clone(), span, source);
        if let Some(reason) = self
            .registry
            .runtime_requirement_reason_for_action_ref(action)
        {
            summary.require_runtime(reason);
        }
        true
    }
}

fn is_synthetic_standard_action(hir: &etas_hir::HirProgram, symbol: etas_hir::SymbolId) -> bool {
    let Some(symbol) = hir.symbols.get(symbol) else {
        return false;
    };
    matches!(
        symbol.def,
        SymbolDef::Synthetic {
            reason: SyntheticSymbolReason::QualifiedEffectAction
        }
    )
}

fn is_synthetic_external_action(hir: &etas_hir::HirProgram, symbol: etas_hir::SymbolId) -> bool {
    let Some(symbol) = hir.symbols.get(symbol) else {
        return false;
    };
    matches!(
        symbol.def,
        SymbolDef::Synthetic {
            reason: SyntheticSymbolReason::ExternalEffectAction
        }
    )
}

fn external_action_path(
    hir: &etas_hir::HirProgram,
    action: &ResolvedActionRef,
) -> Option<Vec<String>> {
    let ResolveResult::Resolved(effect_symbol) = action.effect.path.resolution else {
        return None;
    };
    let symbol = hir.symbols.get(effect_symbol)?;
    let SymbolDef::ImportAlias { path, origin } = &symbol.def else {
        return None;
    };
    if *origin != etas_hir::ImportAliasOrigin::SourceImport {
        return None;
    }
    let mut path = path.clone();
    path.push(action.action.clone());
    Some(path)
}

struct EffectSummaryForFact;

impl EffectSummaryForFact {
    fn one(effect: Effect, span: Span, _semantics: &EffectSemantics<'_>) -> crate::EffectSummary {
        let mut summary = crate::EffectSummary::local();
        match &effect {
            Effect::Action(_) => {
                summary.record_requested_action(effect.clone());
                summary.record_escaping_effect(effect.clone());
                summary.record_action_trace_event(effect.clone(), span, ActionEventSource::Perform);
            }
            Effect::AppliedAction(_) => {
                summary.record_requested_action(effect.clone());
                summary.record_escaping_effect(effect.clone());
                summary.record_action_trace_event(effect.clone(), span, ActionEventSource::Perform);
            }
            _ => summary.record_escaping_effect(effect),
        }
        summary
    }
}
