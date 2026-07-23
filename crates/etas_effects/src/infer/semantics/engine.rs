use etas_core::{Diagnostic, EffectDiagnosticCode, Span};
use etas_hir::{
    HirAnnotationArg, HirArg, HirExpr, HirExprId, HirItemId, HirProgram, ResolveResult,
};
use etas_hir_analysis::HirAnalysisContext;
use etas_std::StdLimitKind;
use etas_types::{
    Assignable, SymbolTypeFact, Type, TypeId, TypeOutput, TypeRelation,
    pipeline::symbols::TypeSymbolIndex,
};

use crate::diagnostic_anchor::{DiagnosticAnchor, materialize_effect_diagnostic};
use crate::{
    Effect, EffectMaterializationInputs, EffectRegistry, EffectRow, EffectSet, EffectSummary,
    ExternalEffectSummaryMetadata, FrontendRejectionReason, InterpreterSupport, LimitKind,
    RequirementFact, RequirementSet, ToolProviderBindingMetadata, effect_var_id_from_name,
};

use super::state::EffectState;
use crate::infer::unit::EffectUnit;

pub(crate) struct EffectSemantics<'a> {
    pub(crate) hir: &'a HirProgram,
    pub(crate) context: HirAnalysisContext,
    pub(crate) types: &'a TypeOutput,
    pub(crate) std_registry: &'a etas_std::StdRegistry,
    pub(crate) type_symbols: TypeSymbolIndex,
    pub(crate) registry: &'a EffectRegistry,
    pub(crate) inputs: EffectMaterializationInputs,
    pub(crate) diagnostics: Vec<Diagnostic>,
    diagnostic_materialization_errors: Vec<crate::EffectPipelineError>,
    pub(crate) tool_bindings: &'a [ToolProviderBindingMetadata],
    pub(crate) external_summaries:
        &'a [crate::AnchoredExternalMetadata<ExternalEffectSummaryMetadata>],
}

impl<'a> EffectSemantics<'a> {
    pub(crate) fn with_context(
        hir: &'a HirProgram,
        context: HirAnalysisContext,
        types: &'a TypeOutput,
        std_registry: &'a etas_std::StdRegistry,
        registry: &'a EffectRegistry,
        tool_bindings: &'a [ToolProviderBindingMetadata],
        external_summaries: &'a [crate::AnchoredExternalMetadata<ExternalEffectSummaryMetadata>],
    ) -> Self {
        Self {
            hir,
            context,
            types,
            std_registry,
            type_symbols: TypeSymbolIndex::build(hir),
            registry,
            inputs: EffectMaterializationInputs::default(),
            diagnostics: Vec::new(),
            diagnostic_materialization_errors: Vec::new(),
            tool_bindings,
            external_summaries,
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        EffectMaterializationInputs,
        Vec<Diagnostic>,
        Vec<crate::EffectPipelineError>,
    ) {
        (
            self.inputs,
            self.diagnostics,
            self.diagnostic_materialization_errors,
        )
    }

    pub(crate) fn incomplete_at(
        &mut self,
        span: Span,
        message: impl Into<String>,
        mut state: EffectState,
    ) -> EffectState {
        self.diagnostics.push(Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            span,
            message,
        ));
        state.mark_incomplete();
        state
    }

    pub(crate) fn incomplete_at_anchor(
        &mut self,
        anchor: DiagnosticAnchor,
        message: impl Into<String>,
        mut state: EffectState,
    ) -> EffectState {
        match materialize_effect_diagnostic(
            self.hir,
            EffectDiagnosticCode::IncompleteEffectFacts,
            anchor,
            message,
        ) {
            Ok(diagnostic) => self.diagnostics.push(diagnostic),
            Err(error) => self.diagnostic_materialization_errors.push(error),
        }
        state.mark_incomplete();
        state
    }

    pub(crate) fn incomplete_summary_at_anchor(
        &mut self,
        anchor: DiagnosticAnchor,
        message: impl Into<String>,
    ) -> EffectSummary {
        match materialize_effect_diagnostic(
            self.hir,
            EffectDiagnosticCode::IncompleteEffectFacts,
            anchor,
            message,
        ) {
            Ok(diagnostic) => self.diagnostics.push(diagnostic),
            Err(error) => self.diagnostic_materialization_errors.push(error),
        }
        let mut summary = EffectSummary::local();
        summary.support = InterpreterSupport::Rejected(FrontendRejectionReason::UnresolvedEffect);
        summary
    }

    pub(crate) fn is_unresolved_try_error(&self, ty: etas_types::TypeId) -> bool {
        matches!(self.types.store.get(ty), Some(Type::Var(_)))
    }

    pub(crate) fn try_error_types_match(
        &self,
        target: etas_types::TypeId,
        escaping: etas_types::TypeId,
    ) -> bool {
        if target == escaping {
            return true;
        }
        let relation = TypeRelation::new(&self.types.store);
        relation.assignable(target, escaping).is_ok()
            && relation.assignable(escaping, target).is_ok()
    }

    pub(crate) fn try_capture_error(
        &self,
        target_error: Option<etas_types::TypeId>,
        escaping_errors: &[etas_types::TypeId],
    ) -> Option<(etas_types::TypeId, etas_types::TypeId)> {
        let [escaping] = escaping_errors else {
            return None;
        };
        match target_error {
            Some(target) if self.is_unresolved_try_error(target) => Some((*escaping, *escaping)),
            Some(target) if self.try_error_types_match(target, *escaping) => {
                Some((target, *escaping))
            }
            Some(_) => None,
            None => Some((*escaping, *escaping)),
        }
    }

    pub(crate) fn summary_from_public_row(&self, row: EffectRow, span: Span) -> EffectSummary {
        let mut summary = EffectSummary::local();
        self.apply_public_row_to_summary(&mut summary, &row, span);
        summary
    }

    pub(crate) fn apply_public_row_to_summary(
        &self,
        summary: &mut EffectSummary,
        row: &EffectRow,
        span: Span,
    ) {
        for effect in row.effects.iter() {
            self.apply_effect_to_summary(summary, effect.clone(), span);
        }
    }

    pub(crate) fn apply_effect_to_summary(
        &self,
        summary: &mut EffectSummary,
        effect: Effect,
        _span: Span,
    ) {
        match &effect {
            Effect::Action(action) => {
                summary.record_escaping_effect(effect.clone());
                if let Some(reason) = self
                    .registry
                    .runtime_requirement_reason_for_action_ref(action)
                {
                    summary.require_runtime(reason);
                }
            }
            Effect::AppliedAction(action) => {
                summary.record_escaping_effect(effect.clone());
                if let Some(reason) = self
                    .registry
                    .runtime_requirement_reason_for_action_ref(&action.action)
                {
                    summary.require_runtime(reason);
                }
            }
            Effect::Tag(tag) => {
                summary.record_escaping_effect(effect.clone());
                if let Some(reason) = self.registry.runtime_requirement_reason(*tag) {
                    summary.require_runtime(reason);
                }
            }
            Effect::Applied { tag, .. } => {
                summary.record_escaping_effect(effect.clone());
                if let Some(reason) = self.registry.runtime_requirement_reason(*tag) {
                    summary.require_runtime(reason);
                }
            }
            Effect::Error(_) | Effect::Var(_) => {
                summary.record_escaping_effect(effect);
            }
        }
    }

    pub(crate) fn apply_runtime_support_for_row(
        &self,
        summary: &mut EffectSummary,
        row: &EffectRow,
    ) {
        for effect in row.effects.iter() {
            match effect {
                Effect::Action(action) => {
                    if let Some(reason) = self
                        .registry
                        .runtime_requirement_reason_for_action_ref(action)
                    {
                        summary.require_runtime(reason);
                    }
                }
                Effect::AppliedAction(action) => {
                    if let Some(reason) = self
                        .registry
                        .runtime_requirement_reason_for_action_ref(&action.action)
                    {
                        summary.require_runtime(reason);
                    }
                }
                Effect::Tag(tag) => {
                    if let Some(reason) = self.registry.runtime_requirement_reason(*tag) {
                        summary.require_runtime(reason);
                    }
                }
                Effect::Applied { tag, .. } => {
                    if let Some(reason) = self.registry.runtime_requirement_reason(*tag) {
                        summary.require_runtime(reason);
                    }
                }
                Effect::Error(_) | Effect::Var(_) => {}
            }
        }
    }

    pub(crate) fn row_from_type_ref(&self, row: &etas_types::EffectRowRef) -> EffectRow {
        let effects = row
            .effects
            .iter()
            .filter_map(|effect| self.effect_from_type_ref(effect))
            .collect::<Vec<_>>();
        EffectRow {
            effects: EffectSet::from_iter(effects),
            open: row.tail.as_deref().map(effect_var_id_from_name),
        }
    }

    pub(crate) fn effect_from_type_ref(&self, effect: &etas_types::EffectRef) -> Option<Effect> {
        if self.is_core_error_effect_name(&effect.name) {
            if let Some(etas_types::EffectArgRef::Type(error)) = effect.args.first() {
                return Some(Effect::Error(*error));
            }
        }
        if let Some(action) = self.registry.action_by_name(&effect.name) {
            let args = effect.args.clone();
            return if args.is_empty() {
                Some(Effect::Action(action))
            } else {
                Some(Effect::AppliedAction(crate::ActionInstanceRef {
                    action,
                    args,
                }))
            };
        }
        self.registry.tag_by_name(&effect.name).map(Effect::Tag)
    }

    pub(crate) fn is_core_error_effect_name(&self, name: &str) -> bool {
        name == "Error"
            || self.registry.tag_by_name(name) == Some(crate::ERROR_TAG)
            || (name.starts_with("std.") && name.rsplit('.').next() == Some("Error"))
    }

    pub(crate) fn summary_for_item_signature(
        &self,
        item: HirItemId,
        span: Span,
    ) -> Option<EffectSummary> {
        let signature = self.types.facts.item_signatures.get(&item)?;
        match signature {
            etas_types::ItemSignature::Flow(signature) => signature.effects.as_ref(),
            etas_types::ItemSignature::Agent(signature) => signature.effects.as_ref(),
            etas_types::ItemSignature::Tool(signature) => signature.effects.as_ref(),
            etas_types::ItemSignature::TopLevelLet(_) => None,
        }
        .map(|row| self.summary_from_public_row(self.row_from_type_ref(row), span))
        .or_else(|| match signature {
            etas_types::ItemSignature::Flow(_)
            | etas_types::ItemSignature::Agent(_)
            | etas_types::ItemSignature::Tool(_) => Some(EffectSummary::local()),
            etas_types::ItemSignature::TopLevelLet(_) => None,
        })
    }

    pub(crate) fn summary_for_item_public_effects(
        &self,
        item: HirItemId,
        span: Span,
    ) -> Option<EffectSummary> {
        self.summary_for_item_signature(item, span)
    }

    pub(crate) fn summary_for_agent_call(
        &self,
        item: HirItemId,
        span: Span,
    ) -> Option<EffectSummary> {
        if !matches!(self.hir.items.get(item), Some(etas_hir::HirItem::Agent(_))) {
            return None;
        }
        let mut summary = self
            .summary_for_item_public_effects(item, span)
            .unwrap_or_else(EffectSummary::local);
        if let Some(declaration_summary) = self.inputs.unit_effects.get(&EffectUnit::Item(item)) {
            summary.seq_assign(declaration_summary);
        }
        if let Some(call) = self.agent_infer_summary(item, span) {
            summary.seq_assign(&call);
        }
        self.apply_agent_annotation_requirements(item, &mut summary);
        Some(summary)
    }

    pub(crate) fn apply_agent_annotation_requirements(
        &self,
        item: HirItemId,
        summary: &mut EffectSummary,
    ) {
        let Some(annotations) = self.hir.item_annotations.get(&item) else {
            return;
        };
        let mut requirements = RequirementSet::new();
        for annotation in annotations {
            if annotation_name(annotation) != "limits" {
                continue;
            }
            for arg in &annotation.args {
                let value = match arg {
                    HirAnnotationArg::Positional { value, .. }
                    | HirAnnotationArg::Named { value, .. } => *value,
                };
                for limit in self.agent_limit_annotation_entries(value) {
                    if let Some(requirement) = self.limit_requirement_from_expr(limit) {
                        requirements.insert(RequirementFact::Limit(requirement));
                    }
                }
            }
        }
        summary.requirements.union_assign(&requirements);
    }

    pub(crate) fn agent_limit_annotation_entries(&self, expr: HirExprId) -> Vec<HirExprId> {
        match self.hir.exprs.get(expr) {
            Some(HirExpr::Array { elems, .. }) | Some(HirExpr::List { elems, .. }) => elems.clone(),
            _ => vec![expr],
        }
    }

    pub(crate) fn has_tool_binding(&self, item: HirItemId) -> bool {
        let Some(etas_hir::HirItem::Tool(tool)) = self.hir.items.get(item) else {
            return false;
        };
        let Some(tool_name) = self.exported_symbol_name(tool.symbol) else {
            return false;
        };
        self.tool_bindings
            .iter()
            .any(|binding| binding.tool == tool_name)
    }

    pub(crate) fn exported_symbol_name(&self, symbol: etas_hir::SymbolId) -> Option<Vec<String>> {
        let symbol_data = self.hir.symbols.get(symbol)?;
        let mut segments = self
            .hir
            .modules_arena
            .get(symbol_data.defining_module)
            .and_then(|module| module.name.as_ref())
            .map(|name| {
                name.segments
                    .iter()
                    .map(|segment| segment.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        segments.push(symbol_data.name.clone());
        Some(segments)
    }

    pub(crate) fn callee_symbol(&self, callee: HirExprId) -> Option<etas_hir::SymbolId> {
        match self.hir.exprs.get(callee)? {
            HirExpr::Path(path) => match path.resolution {
                ResolveResult::Resolved(symbol) => Some(symbol),
                _ => None,
            },
            _ => None,
        }
    }

    pub(crate) fn is_typed_flow_expr(&self, expr: HirExprId) -> bool {
        self.types
            .facts
            .expr_types
            .get(&expr)
            .and_then(|ty| self.types.store.get(*ty))
            .is_some_and(|ty| matches!(ty, Type::Function(_)))
    }

    pub(crate) fn summary_for_flow_expr_type(
        &self,
        expr: HirExprId,
        span: Span,
    ) -> Option<EffectSummary> {
        let flow = self.flow_type_for_expr(expr)?;
        let row = flow.effects.as_ref()?;
        Some(self.summary_from_public_row(self.row_from_type_ref(row), span))
    }

    pub(crate) fn flow_type_for_expr(&self, expr: HirExprId) -> Option<&etas_types::FlowType> {
        let ty = self.types.facts.expr_types.get(&expr)?;
        let Type::Function(flow) = self.types.store.get(*ty)? else {
            return None;
        };
        Some(flow)
    }

    pub(crate) fn is_nominal_constructor_callee(&self, callee: HirExprId) -> bool {
        if let Some(ty) = self.types.facts.expr_types.get(&callee).copied() {
            match self.types.store.get(ty) {
                Some(Type::Nominal(_)) => return true,
                Some(Type::Applied { constructor, .. }) => {
                    if matches!(
                        self.types.store.get(TypeId(constructor.0)),
                        Some(Type::Nominal(_))
                    ) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(callee) else {
            return false;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        matches!(
            self.type_symbols
                .symbol_fact(self.hir, &self.types.facts, symbol),
            Some(
                SymbolTypeFact::Type { .. }
                    | SymbolTypeFact::NominalType { .. }
                    | SymbolTypeFact::TypeAlias { .. }
            )
        )
    }
}

pub(super) fn external_summary_matches_path(
    summary: &crate::ExternalEffectSummaryMetadata,
    path: &[String],
) -> bool {
    summary.item == path
        && summary
            .import_root
            .as_ref()
            .map(|root| module_path_has_import_root(path, root))
            .unwrap_or(true)
}

fn module_path_has_import_root(path: &[String], root: &str) -> bool {
    let root_segments = root
        .split('.')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    !root_segments.is_empty()
        && path.len() >= root_segments.len()
        && path
            .iter()
            .zip(root_segments)
            .all(|(segment, root_segment)| segment == root_segment)
}

pub(super) fn limit_kind_from_std(kind: StdLimitKind) -> LimitKind {
    match kind {
        StdLimitKind::Iterations => LimitKind::Iterations,
        StdLimitKind::Tokens => LimitKind::Tokens,
        StdLimitKind::ContextTokens => LimitKind::ContextTokens,
        StdLimitKind::Cost => LimitKind::Cost,
        StdLimitKind::WallTime => LimitKind::WallTime,
        StdLimitKind::Attempts => LimitKind::Attempts,
    }
}

pub(super) fn annotation_name(annotation: &etas_hir::HirAnnotation) -> String {
    annotation
        .path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

pub(crate) enum DeferredSpecialization {
    Solved(Box<EffectSummary>),
    Rejected { message: &'static str },
}

pub(super) fn arg_expr(arg: &HirArg) -> Option<HirExprId> {
    match arg {
        HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => Some(*expr),
    }
}

pub(super) fn insert_type_binding(
    bindings: &mut Vec<(String, TypeId)>,
    name: String,
    ty: TypeId,
) -> bool {
    if bindings.iter().any(|(existing, _)| existing == &name) {
        return false;
    }
    bindings.push((name, ty));
    true
}

pub(super) fn named_type_name(store: &etas_types::TypeStore, ty: TypeId) -> Option<String> {
    match store.get(ty)? {
        Type::Named(name) => Some(name.name.clone()),
        _ => None,
    }
}

pub(super) fn collect_type_bindings_from_type_pattern(
    store: &etas_types::TypeStore,
    pattern: TypeId,
    actual: TypeId,
    allowed_names: &[String],
    bindings: &mut Vec<(String, TypeId)>,
) -> bool {
    match store.get(pattern) {
        Some(Type::Named(name)) if allowed_names.iter().any(|allowed| allowed == &name.name) => {
            insert_type_binding(bindings, name.name.clone(), actual)
        }
        Some(Type::Array(pattern)) => match store.get(actual) {
            Some(Type::Array(actual)) => collect_type_bindings_from_type_pattern(
                store,
                *pattern,
                *actual,
                allowed_names,
                bindings,
            ),
            _ => false,
        },
        Some(Type::List(pattern)) => match store.get(actual) {
            Some(Type::List(actual)) => collect_type_bindings_from_type_pattern(
                store,
                *pattern,
                *actual,
                allowed_names,
                bindings,
            ),
            _ => false,
        },
        Some(Type::Set(pattern)) => match store.get(actual) {
            Some(Type::Set(actual)) => collect_type_bindings_from_type_pattern(
                store,
                *pattern,
                *actual,
                allowed_names,
                bindings,
            ),
            _ => false,
        },
        Some(Type::Slice(pattern)) => match store.get(actual) {
            Some(Type::Slice(actual)) => collect_type_bindings_from_type_pattern(
                store,
                *pattern,
                *actual,
                allowed_names,
                bindings,
            ),
            _ => false,
        },
        Some(Type::Option(pattern)) => match store.get(actual) {
            Some(Type::Option(actual)) => collect_type_bindings_from_type_pattern(
                store,
                *pattern,
                *actual,
                allowed_names,
                bindings,
            ),
            _ => false,
        },
        Some(Type::Result {
            ok: pattern_ok,
            err: pattern_err,
        }) => match store.get(actual) {
            Some(Type::Result {
                ok: actual_ok,
                err: actual_err,
            }) => {
                let ok_changed = collect_type_bindings_from_type_pattern(
                    store,
                    *pattern_ok,
                    *actual_ok,
                    allowed_names,
                    bindings,
                );
                let err_changed = collect_type_bindings_from_type_pattern(
                    store,
                    *pattern_err,
                    *actual_err,
                    allowed_names,
                    bindings,
                );
                ok_changed || err_changed
            }
            _ => false,
        },
        Some(Type::Map {
            key: pattern_key,
            value: pattern_value,
        })
        | Some(Type::Store {
            key: pattern_key,
            value: pattern_value,
        }) => match store.get(actual) {
            Some(Type::Map {
                key: actual_key,
                value: actual_value,
            })
            | Some(Type::Store {
                key: actual_key,
                value: actual_value,
            }) => {
                let key_changed = collect_type_bindings_from_type_pattern(
                    store,
                    *pattern_key,
                    *actual_key,
                    allowed_names,
                    bindings,
                );
                let value_changed = collect_type_bindings_from_type_pattern(
                    store,
                    *pattern_value,
                    *actual_value,
                    allowed_names,
                    bindings,
                );
                key_changed || value_changed
            }
            _ => false,
        },
        Some(Type::Tuple(pattern_elements)) => match store.get(actual) {
            Some(Type::Tuple(actual_elements))
                if pattern_elements.len() == actual_elements.len() =>
            {
                pattern_elements
                    .iter()
                    .copied()
                    .zip(actual_elements.iter().copied())
                    .fold(false, |changed, (pattern, actual)| {
                        collect_type_bindings_from_type_pattern(
                            store,
                            pattern,
                            actual,
                            allowed_names,
                            bindings,
                        ) || changed
                    })
            }
            _ => false,
        },
        Some(Type::Applied {
            constructor: pattern_constructor,
            args: pattern_args,
        }) => match store.get(actual) {
            Some(Type::Applied {
                constructor: actual_constructor,
                args: actual_args,
            }) if pattern_constructor == actual_constructor
                && pattern_args.len() == actual_args.len() =>
            {
                pattern_args
                    .iter()
                    .copied()
                    .zip(actual_args.iter().copied())
                    .fold(false, |changed, (pattern, actual)| {
                        collect_type_bindings_from_type_pattern(
                            store,
                            pattern,
                            actual,
                            allowed_names,
                            bindings,
                        ) || changed
                    })
            }
            _ => false,
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{EffectSemantics, external_summary_matches_path};
    use etas_core::SourceId;
    use etas_hir::{HirExpr, lower_program};
    use etas_hir_analysis::HirAnalysisContext;
    use etas_types::{EffectArgRef, TypeOutput};

    #[test]
    pub(crate) fn external_summary_import_root_matches_multi_segment_prefix() {
        let summary = crate::ExternalEffectSummaryMetadata {
            package: Some("company-agents@1.0.0#company.agents".to_owned()),
            import_root: Some("company.agents".to_owned()),
            item: vec![
                "company".to_owned(),
                "agents".to_owned(),
                "writer".to_owned(),
                "run".to_owned(),
            ],
            param_names: Vec::new(),
            public_effects: Default::default(),
            requested_actions: Default::default(),
            handled_requested_actions: Default::default(),
            latent_flows: Vec::new(),
        };

        assert!(external_summary_matches_path(
            &summary,
            &["company", "agents", "writer", "run"]
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        ));
        assert!(!external_summary_matches_path(
            &summary,
            &["company", "other", "writer", "run"]
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        ));
    }

    #[test]
    pub(crate) fn static_value_specialization_uses_resolved_source_and_intrinsic_semantics() {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
            SourceId(0),
            None,
            r#"
import std.text.trim;

flow normalize(input: string) -> string {
  return trim(input);
}

flow main() -> unit {
  normalize("  example.com  ");
  return;
}
"#,
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let call = hir
            .exprs
            .iter()
            .find_map(|(expr, data)| match data {
                HirExpr::Call { callee, .. }
                    if matches!(
                        hir.exprs.get(*callee),
                        Some(HirExpr::Path(path))
                            if path.segments.last().is_some_and(|segment| segment.name == "normalize")
                    ) =>
                {
                    Some(expr)
                }
                _ => None,
            })
            .expect("normalize call should lower");
        let types = TypeOutput::default();
        let context = HirAnalysisContext::new(&hir);
        let std_registry = etas_std::standard_registry();
        let registry = crate::EffectRegistry::with_standard_effects_from(&std_registry);
        let semantics = EffectSemantics::with_context(
            &hir,
            context,
            &types,
            &std_registry,
            &registry,
            &[],
            &[],
        );

        let value = semantics
            .effect_arg_from_expr_path(call, &[])
            .expect("resolved source and std intrinsic should evaluate");

        assert_eq!(value, EffectArgRef::String("example.com".to_owned()));
    }

    #[test]
    pub(crate) fn static_value_specialization_has_no_fixed_depth_limit() {
        let mut source = String::new();
        source.push_str("import std.text.trim;\n");
        for index in 0..32 {
            let next = index + 1;
            source.push_str(&format!(
                "flow f{index}(value: string) -> string {{ return f{next}(value); }}\n"
            ));
        }
        source.push_str(
            "flow f32(value: string) -> string { return trim(value); }\n\
             flow main() -> unit { f0(\"  deep.example  \"); return; }\n",
        );
        let parsed =
            etas_syntax::parse_program(etas_core::SourceFile::new(SourceId(0), None, source));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let call = hir
            .exprs
            .iter()
            .find_map(|(expr, data)| match data {
                HirExpr::Call { callee, .. }
                    if matches!(
                        hir.exprs.get(*callee),
                        Some(HirExpr::Path(path))
                            if path.segments.last().is_some_and(|segment| segment.name == "f0")
                    ) =>
                {
                    Some(expr)
                }
                _ => None,
            })
            .expect("f0 call should lower");
        let types = TypeOutput::default();
        let context = HirAnalysisContext::new(&hir);
        let std_registry = etas_std::standard_registry();
        let registry = crate::EffectRegistry::with_standard_effects_from(&std_registry);
        let semantics = EffectSemantics::with_context(
            &hir,
            context,
            &types,
            &std_registry,
            &registry,
            &[],
            &[],
        );

        let value = semantics
            .effect_arg_from_expr_path(call, &[])
            .expect("deep acyclic source call chain should evaluate");

        assert_eq!(value, EffectArgRef::String("deep.example".to_owned()));
    }

    #[test]
    pub(crate) fn static_value_specialization_does_not_guess_url_parser_semantics() {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
            SourceId(0),
            None,
            r#"
flow parse_url(input: string) -> i32 {
  return 0;
}

flow main() -> unit {
  parse_url("https://example.com/status");
  return;
}
"#,
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let call = hir
            .exprs
            .iter()
            .find_map(|(expr, data)| matches!(data, HirExpr::Call { .. }).then_some(expr))
            .expect("parse_url call should lower");
        let types = TypeOutput::default();
        let context = HirAnalysisContext::new(&hir);
        let std_registry = etas_std::standard_registry();
        let registry = crate::EffectRegistry::with_standard_effects_from(&std_registry);
        let semantics = EffectSemantics::with_context(
            &hir,
            context,
            &types,
            &std_registry,
            &registry,
            &[],
            &[],
        );

        let error = semantics
            .effect_arg_from_expr_path(call, &["host".to_owned()])
            .expect_err("arbitrary call shape must not be assigned URL semantics");

        assert!(
            matches!(
                error,
                etas_hir_analysis::static_string::StaticStringEvaluationError::UnsupportedExpression { .. }
            ),
            "{error:?}"
        );
    }

    #[test]
    pub(crate) fn static_value_specialization_reports_source_call_cycles() {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
            SourceId(0),
            None,
            r#"
flow recurse(value: string) -> string {
  return recurse(value);
}

flow main() -> unit {
  recurse("example.com");
  return;
}
"#,
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let call = hir
            .exprs
            .iter()
            .find_map(|(expr, data)| match data {
                HirExpr::Call { callee, args, .. }
                    if !args.is_empty()
                        && matches!(
                            hir.exprs.get(*callee),
                            Some(HirExpr::Path(path))
                                if path.segments.last().is_some_and(|segment| segment.name == "recurse")
                        ) =>
                {
                    Some(expr)
                }
                _ => None,
            })
            .expect("recurse call should lower");
        let types = TypeOutput::default();
        let context = HirAnalysisContext::new(&hir);
        let std_registry = etas_std::standard_registry();
        let registry = crate::EffectRegistry::with_standard_effects_from(&std_registry);
        let semantics = EffectSemantics::with_context(
            &hir,
            context,
            &types,
            &std_registry,
            &registry,
            &[],
            &[],
        );

        let error = semantics
            .effect_arg_from_expr_path(call, &[])
            .expect_err("recursive static evaluation must fail closed");

        assert!(
            matches!(
                error,
                etas_hir_analysis::static_string::StaticStringEvaluationError::Cycle { .. }
            ),
            "{error:?}"
        );
    }

    #[test]
    pub(crate) fn static_value_specialization_rejects_multiple_return_exits() {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
            SourceId(0),
            None,
            r#"
flow ambiguous(value: string) -> string {
  return value;
  return "wrong";
}

flow main() -> unit {
  ambiguous("expected");
  return;
}
"#,
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let call = hir
            .exprs
            .iter()
            .find_map(|(expr, data)| match data {
                HirExpr::Call { callee, .. }
                    if matches!(
                        hir.exprs.get(*callee),
                        Some(HirExpr::Path(path))
                            if path.segments.last().is_some_and(|segment| segment.name == "ambiguous")
                    ) =>
                {
                    Some(expr)
                }
                _ => None,
            })
            .expect("ambiguous call should lower");
        let types = TypeOutput::default();
        let context = HirAnalysisContext::new(&hir);
        let std_registry = etas_std::standard_registry();
        let registry = crate::EffectRegistry::with_standard_effects_from(&std_registry);
        let semantics = EffectSemantics::with_context(
            &hir,
            context,
            &types,
            &std_registry,
            &registry,
            &[],
            &[],
        );

        let error = semantics
            .effect_arg_from_expr_path(call, &[])
            .expect_err("multiple return exits must not be guessed");

        assert!(matches!(
            error,
            etas_hir_analysis::static_string::StaticStringEvaluationError::UnsupportedCall { .. }
        ));
    }
}
