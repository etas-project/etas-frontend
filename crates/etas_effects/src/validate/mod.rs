use std::collections::BTreeSet;

use etas_core::{Diagnostic, EffectDiagnosticCode};
use etas_hir::{
    HirExprId, HirItem, HirItemId, SymbolId, Visibility,
    view::{HirOwner, HirTreeView},
};

use crate::{
    AGENTIC_INFER_ACTION, AGENTIC_TAG, ActionRef, Effect, EffectAcknowledgement,
    EffectActionArgKind, EffectOutput, EffectPipelineError, EffectRegistry,
    FrontendRejectionReason, InterpreterSupport, LatentEffectContract, PublicEffectContract,
    PublicEffectContractSource, PublicEffectValue, QualifiedName, RequirementFactsForContract,
    ToolProviderBindingMetadata,
};

pub struct EffectContractValidator;

impl EffectContractValidator {
    pub fn validate(
        hir: &etas_hir::HirProgram,
        types: &etas_types::TypeOutput,
        registry: &EffectRegistry,
        tool_bindings: &[ToolProviderBindingMetadata],
        effects: &mut EffectOutput,
        reachable_items: Option<&BTreeSet<HirItemId>>,
    ) -> Result<(), EffectPipelineError> {
        let mut validator = ContractValidator {
            hir,
            tree: HirTreeView::new(hir),
            types,
            registry,
            tool_bindings,
            effects,
            reachable_items,
        };
        validator.validate_items();
        validator.validate_latent_effect_bounds()?;
        validator.materialize_public_contracts();
        Ok(())
    }
}

struct ContractValidator<'a, 'b> {
    hir: &'a etas_hir::HirProgram,
    tree: HirTreeView<'a>,
    types: &'a etas_types::TypeOutput,
    registry: &'a EffectRegistry,
    tool_bindings: &'a [ToolProviderBindingMetadata],
    effects: &'b mut EffectOutput,
    reachable_items: Option<&'a BTreeSet<HirItemId>>,
}

impl ContractValidator<'_, '_> {
    fn validate_items(&mut self) {
        for (item, hir_item) in self.hir.items.iter() {
            if !self.item_is_reachable(item) {
                continue;
            }
            match hir_item {
                HirItem::Flow(flow) if flow.effects.is_some() => {
                    self.validate_declared_item(item, flow.span);
                }
                HirItem::Agent(agent) if agent.effects.is_some() => {
                    self.validate_declared_item(item, agent.span);
                }
                HirItem::Tool(tool) => {
                    if matches!(tool.body, etas_hir::HirToolBody::Decl { .. }) {
                        let Some(binding) = self.tool_binding(item).cloned() else {
                            self.reject_item(
                                item,
                                tool.span,
                                EffectDiagnosticCode::MissingToolProviderBinding,
                                "bodyless tool declarations require a resolved provider binding or external metadata",
                            );
                            continue;
                        };
                        if tool.effects.is_none() {
                            self.reject_item(
                                item,
                                tool.span,
                                EffectDiagnosticCode::IncompleteEffectFacts,
                                "bodyless tool declarations require explicit source effect/action metadata",
                            );
                        } else {
                            self.validate_declared_item(item, tool.span);
                            self.validate_tool_binding_row(item, tool.span, &binding);
                        }
                    } else if tool.effects.is_some() {
                        self.validate_declared_item(item, tool.span);
                    } else {
                        self.validate_implicit_source_tool_contract(item, tool.span);
                    }
                }
                _ => {}
            }
        }
    }

    fn item_is_reachable(&self, item: HirItemId) -> bool {
        self.reachable_items
            .is_none_or(|reachable| reachable.contains(&item))
    }

    fn tool_binding(&self, item: HirItemId) -> Option<&ToolProviderBindingMetadata> {
        let HirItem::Tool(tool) = self.hir.items.get(item)? else {
            return None;
        };
        let name = exported_name(self.hir, tool.symbol);
        self.tool_bindings
            .iter()
            .find(|binding| binding.tool == name.segments)
    }

    fn validate_tool_binding_row(
        &mut self,
        item: HirItemId,
        span: etas_core::Span,
        binding: &ToolProviderBindingMetadata,
    ) {
        if binding.provider.trim().is_empty() {
            self.reject_item(
                item,
                span,
                EffectDiagnosticCode::MissingToolProviderBinding,
                "bodyless tool provider binding has an empty provider",
            );
            return;
        }
        if binding.effect_row.is_empty() && binding.action_row.is_empty() {
            self.reject_item(
                item,
                span,
                EffectDiagnosticCode::IncompleteEffectFacts,
                "bodyless tool provider binding requires effect/action metadata",
            );
            return;
        }
        let mut declared_binding_row = binding.effect_row.clone();
        declared_binding_row.extend(binding.action_row.clone());
        let Some(binding_row) = binding_effect_row(self.registry, &declared_binding_row) else {
            self.reject_item(
                item,
                span,
                EffectDiagnosticCode::IncompleteEffectFacts,
                "bodyless tool provider binding references unknown effect/action metadata",
            );
            return;
        };
        let declared = match declared_row(self.types, self.registry, item, span, self.effects) {
            Ok(Some(declared)) => declared,
            Ok(None) => {
                self.reject_item(
                    item,
                    span,
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    "bodyless tool provider binding validation requires a checked item signature",
                );
                return;
            }
            Err(error) => {
                self.reject_item_with_reason(item, span, error.code, error.message, error.reason);
                return;
            }
        };
        let coverage = crate::EffectCoverage {
            registry: self.registry,
            types: &self.types.store,
        };
        if !coverage.row_covers(&declared, &binding_row) {
            self.reject_item(
                item,
                span,
                EffectDiagnosticCode::EffectOutsideDeclaredRow,
                "bodyless tool provider binding effect/action metadata exceeds the source signature",
            );
        }
    }

    fn validate_latent_effect_bounds(&mut self) -> Result<(), EffectPipelineError> {
        let coverage = crate::EffectCoverage {
            registry: self.registry,
            types: &self.types.store,
        };
        for latent in self.effects.facts.latent_effects.values() {
            let Some(bound) = &latent.declared_bound else {
                continue;
            };
            if coverage.row_covers(bound, &latent.inferred) {
                continue;
            }
            let span = latent_effect_span(self.hir, latent.value)?;
            self.effects.diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::EscapedEffect,
                span,
                "latent flow effect escapes the declared function effect bound",
            ));
        }
        Ok(())
    }

    fn validate_declared_item(&mut self, item: HirItemId, span: etas_core::Span) {
        let Some(summary) = self.effects.facts.item_effects.get(&item).cloned() else {
            self.reject_item(
                item,
                span,
                EffectDiagnosticCode::IncompleteEffectFacts,
                "effect contract validation requires a solved item summary",
            );
            return;
        };
        let declared = match declared_row(self.types, self.registry, item, span, self.effects) {
            Ok(Some(declared)) => declared,
            Ok(None) => {
                self.reject_item(
                    item,
                    span,
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    "effect contract validation requires a checked item signature",
                );
                return;
            }
            Err(error) => {
                self.reject_item_with_reason(item, span, error.code, error.message, error.reason);
                return;
            }
        };
        let coverage = crate::EffectCoverage {
            registry: self.registry,
            types: &self.types.store,
        };
        if matches!(self.hir.items.get(item), Some(HirItem::Agent(_)))
            && row_contains_agentic_call_action(&declared)
        {
            self.reject_item(
                item,
                span,
                EffectDiagnosticCode::EffectOutsideDeclaredRow,
                "Agentic.infer<A> is an internal requested action of agent calls, not an agent declaration public effect",
            );
        }
        let boundary = declared_public_boundary(&declared);
        let uncovered_escaping = coverage.row_uncovered_by(&boundary, &summary.escaping_effects);
        if !uncovered_escaping.is_empty() {
            self.reject_item(
                item,
                span,
                EffectDiagnosticCode::EffectOutsideDeclaredRow,
                format!(
                    "inferred effect {} escapes the declared effect row",
                    effect_row_label(self.registry, &self.types.store, &uncovered_escaping)
                ),
            );
        }
        let unhandled_requested =
            coverage.subtract_handled(&summary.requested_actions, &summary.handled_actions);
        let unhandled_requested =
            coverage.subtract_handled(&unhandled_requested, &summary.default_actions);
        let uncovered_actions = coverage.row_uncovered_by(&declared, &unhandled_requested);
        if !uncovered_actions.is_empty() {
            self.reject_item(
                item,
                span,
                EffectDiagnosticCode::EscapedEffect,
                format!(
                    "requested action {} is not handled by a scoped or default handler and is outside the declared effect row",
                    effect_row_label(self.registry, &self.types.store, &uncovered_actions)
                ),
            );
        }
        if matches!(self.hir.items.get(item), Some(HirItem::Tool(_)))
            && declared_requires_tool_sandbox_action(self.registry, &declared)
        {
            self.reject_item_with_reason(
                item,
                span,
                EffectDiagnosticCode::MissingSandboxActionArgument,
                "command tools require an explicit sandbox profile action argument",
                FrontendRejectionReason::MissingRequirement,
            );
        }
    }

    fn validate_implicit_source_tool_contract(&mut self, item: HirItemId, span: etas_core::Span) {
        let Some(summary) = self.effects.facts.item_effects.get(&item).cloned() else {
            self.reject_item(
                item,
                span,
                EffectDiagnosticCode::IncompleteEffectFacts,
                "effect contract validation requires a solved source tool summary",
            );
            return;
        };
        if summary.escaping_effects.effects.is_empty()
            && summary.requested_actions.effects.is_empty()
        {
            return;
        }
        self.reject_item(
            item,
            span,
            EffectDiagnosticCode::EffectOutsideDeclaredRow,
            "source-bodied tool effects require an explicit effect row",
        );
    }

    fn materialize_public_contracts(&mut self) {
        for (item, hir_item) in self.hir.items.iter() {
            if !self.item_is_reachable(item) {
                continue;
            }
            let Some(symbol) = item_symbol(hir_item) else {
                continue;
            };
            let Some(symbol_data) = self.hir.symbols.get(symbol) else {
                continue;
            };
            if symbol_data.visibility != Visibility::Public {
                continue;
            }
            let span = hir_item.span();
            let Some(signature) = self.types.facts.item_signatures.get(&item).cloned() else {
                self.reject_item(
                    item,
                    span,
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    "public effect contract generation requires checked exported value facts",
                );
                continue;
            };
            let Some(summary) = self.effects.facts.item_effects.get(&item).cloned() else {
                self.reject_item(
                    item,
                    span,
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    "public effect contract generation requires solved exported effect facts",
                );
                continue;
            };
            if rejected_bodyless_tool(hir_item, &summary) {
                continue;
            }
            let declared = match declared_row(self.types, self.registry, item, span, self.effects) {
                Ok(declared) => declared,
                Err(error) => {
                    self.reject_item_with_reason(
                        item,
                        span,
                        error.code,
                        error.message,
                        error.reason,
                    );
                    continue;
                }
            };
            let source = if declared.is_some() {
                PublicEffectContractSource::ExplicitSourceAnnotation
            } else {
                PublicEffectContractSource::GeneratedCheckedMetadata
            };
            let latent_flows = self
                .effects
                .facts
                .latent_effects
                .values()
                .filter(|latent| latent_declared_by_item(&self.tree, item, latent.value))
                .map(|latent| LatentEffectContract {
                    value: latent.value,
                    body: latent.body,
                    flow_type: latent.flow_type,
                    declared_bound: latent.declared_bound.clone(),
                    inferred: latent.inferred.clone(),
                })
                .collect::<Vec<_>>();
            let mut inferred = summary.escaping_effects.clone();
            for latent in &latent_flows {
                inferred.union_assign(&latent.inferred);
            }
            let mut public_row = declared.clone().unwrap_or_else(|| inferred.clone());
            for latent in &latent_flows {
                public_row.union_assign(&latent.inferred);
            }
            let high_impact_ack = if public_row_requires_high_impact_ack(self.registry, &public_row)
                || public_row_requires_high_impact_ack(self.registry, &summary.requested_actions)
            {
                match source {
                    PublicEffectContractSource::ExplicitSourceAnnotation => {
                        Some(EffectAcknowledgement {
                            effects: public_row.clone(),
                        })
                    }
                    PublicEffectContractSource::GeneratedCheckedMetadata => {
                        self.effects.diagnostics.push(Diagnostic::effect_check(
                            EffectDiagnosticCode::MissingHighImpactAcknowledgement,
                            span,
                            "public inferred high-impact effect contract requires explicit acknowledgement",
                        ));
                        None
                    }
                    PublicEffectContractSource::DependencyMetadata => None,
                }
            } else {
                None
            };
            let contract = PublicEffectContract {
                item,
                exported_name: exported_name(self.hir, symbol),
                value: match signature {
                    etas_types::ItemSignature::TopLevelLet(top_level) => {
                        PublicEffectValue::ValueType(top_level.ty)
                    }
                    other => PublicEffectValue::ItemSignature(other),
                },
                declared,
                inferred,
                public_row,
                requested_actions: summary.requested_actions.clone(),
                residual_checks: summary.residual_checks.clone(),
                source,
                latent_flows,
                requirements: RequirementFactsForContract {
                    requirements: summary.requirements.clone(),
                },
                high_impact_ack,
            };
            self.effects.facts.public_contracts.push(contract);
        }
    }

    fn reject_item(
        &mut self,
        item: HirItemId,
        span: etas_core::Span,
        code: EffectDiagnosticCode,
        message: impl Into<String>,
    ) {
        self.reject_item_with_reason(
            item,
            span,
            code,
            message,
            FrontendRejectionReason::EscapedEffect,
        );
    }

    fn reject_item_with_reason(
        &mut self,
        item: HirItemId,
        span: etas_core::Span,
        code: EffectDiagnosticCode,
        message: impl Into<String>,
        reason: FrontendRejectionReason,
    ) {
        self.effects
            .diagnostics
            .push(Diagnostic::effect_check(code, span, message));
        if let Some(summary) = self.effects.facts.item_effects.get_mut(&item) {
            summary.support = InterpreterSupport::Rejected(reason);
            self.effects
                .facts
                .interpreter_support
                .items
                .insert(item, summary.support.clone());
        }
    }
}

fn latent_effect_span(
    hir: &etas_hir::HirProgram,
    value: HirExprId,
) -> Result<etas_core::Span, EffectPipelineError> {
    hir.exprs
        .get(value)
        .map(|expr| expr.span(&hir.blocks))
        .ok_or(EffectPipelineError::MissingExprFact { expr: value })
}

fn declared_row(
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    item: HirItemId,
    span: etas_core::Span,
    effects: &mut EffectOutput,
) -> Result<Option<crate::EffectRow>, DeclaredRowError> {
    let Some(signature) = types.facts.item_signatures.get(&item) else {
        return Ok(None);
    };
    let row = match signature {
        etas_types::ItemSignature::Flow(sig) => sig.effects.as_ref(),
        etas_types::ItemSignature::Agent(sig) => sig.effects.as_ref(),
        etas_types::ItemSignature::Tool(sig) => sig.effects.as_ref(),
        etas_types::ItemSignature::TopLevelLet(_) => {
            return Ok(Some(crate::EffectRow::empty()));
        }
    };
    let Some(row) = row else {
        return Ok(None);
    };
    let mut output = crate::EffectRow::empty();
    for effect in &row.effects {
        let summary = if effect.args.is_empty() {
            effect_name(registry, &types.store, &effect.name, span, effects)?
        } else if let Some(action) = registry.action_by_name(&effect.name) {
            declared_action_effect(registry, &types.store, action, &effect.args)?
        } else if is_core_error_effect_name(registry, &effect.name) {
            let args = effect_args_as_types(&effect.args)?;
            let [error] = args.as_slice() else {
                return Err(DeclaredRowError::invalid_type_arg());
            };
            crate::Effect::Error(*error)
        } else {
            let Some(tag) = registry.tag_by_name(&effect.name) else {
                return Err(DeclaredRowError::unknown_effect());
            };
            let args = effect_args_as_types(&effect.args)?;
            crate::Effect::Applied { tag, args }
        };
        output.effects.insert(summary);
    }
    output.open = row.tail.as_deref().map(crate::effect_var_id_from_name);
    validate_declared_effect_row(registry, &output)?;
    Ok(Some(output))
}

fn is_core_error_effect_name(registry: &EffectRegistry, name: &str) -> bool {
    name == "Error"
        || registry.tag_by_name(name) == Some(crate::ERROR_TAG)
        || (name.starts_with("std.") && name.rsplit('.').next() == Some("Error"))
}

fn binding_effect_row(registry: &EffectRegistry, effects: &[String]) -> Option<crate::EffectRow> {
    let mut row = crate::EffectRow::empty();
    for effect in effects {
        let parsed = if let Some(action) = registry.action_by_name(effect) {
            crate::Effect::Action(action)
        } else {
            crate::Effect::Tag(registry.tag_by_name(effect)?)
        };
        row.effects.insert(parsed);
    }
    Some(row)
}

fn declared_action_effect(
    registry: &EffectRegistry,
    types: &etas_types::TypeStore,
    action: ActionRef,
    args: &[etas_types::EffectArgRef],
) -> Result<crate::Effect, DeclaredRowError> {
    let Some(signature) = registry.action_signature(&action) else {
        return Err(DeclaredRowError::missing_action_descriptor());
    };
    if signature.effect_args.len() != args.len() {
        return Err(DeclaredRowError::missing_action_arg(signature));
    }
    for (kind, arg) in signature.effect_args.iter().zip(args) {
        validate_declared_action_arg(registry, types, kind, arg)?;
    }
    Ok(if args.is_empty() {
        crate::Effect::Action(action)
    } else {
        crate::Effect::AppliedAction(crate::ActionInstanceRef {
            action,
            args: args.to_vec(),
        })
    })
}

fn validate_declared_action_arg(
    registry: &EffectRegistry,
    types: &etas_types::TypeStore,
    kind: &EffectActionArgKind,
    arg: &etas_types::EffectArgRef,
) -> Result<(), DeclaredRowError> {
    match kind {
        EffectActionArgKind::Type => match arg {
            etas_types::EffectArgRef::Type(_) | etas_types::EffectArgRef::Wildcard => Ok(()),
            _ => Err(DeclaredRowError::invalid_action_arg(kind)),
        },
        EffectActionArgKind::MemoryPlace => match arg {
            etas_types::EffectArgRef::Type(ty) if registry.memory_place(*ty).is_some() => Ok(()),
            etas_types::EffectArgRef::Type(ty) => match types.get(*ty) {
                Some(etas_types::Type::MemoryPlace(_)) => Ok(()),
                _ => Err(DeclaredRowError::invalid_action_arg(kind)),
            },
            etas_types::EffectArgRef::Path(_) | etas_types::EffectArgRef::Wildcard => Ok(()),
            etas_types::EffectArgRef::String(_) | etas_types::EffectArgRef::Int(_) => {
                Err(DeclaredRowError::invalid_action_arg(kind))
            }
        },
        EffectActionArgKind::StaticResourcePath { .. } => match arg {
            etas_types::EffectArgRef::Path(_) | etas_types::EffectArgRef::String(_) => Ok(()),
            _ => Err(DeclaredRowError::invalid_action_arg(kind)),
        },
        EffectActionArgKind::StringPattern => match arg {
            etas_types::EffectArgRef::String(_)
            | etas_types::EffectArgRef::Path(_)
            | etas_types::EffectArgRef::Wildcard => Ok(()),
            _ => Err(DeclaredRowError::invalid_action_arg(kind)),
        },
    }
}

fn validate_declared_effect_row(
    registry: &EffectRegistry,
    row: &crate::EffectRow,
) -> Result<(), DeclaredRowError> {
    for effect in row.effects.iter() {
        match effect {
            Effect::Action(action)
                if registry
                    .action_signature(action)
                    .is_some_and(action_signature_requires_sandbox_profile) =>
            {
                return Err(DeclaredRowError::missing_sandbox_action_arg());
            }
            Effect::AppliedAction(action) => {
                let Some(signature) = registry.action_signature(&action.action) else {
                    return Err(DeclaredRowError::missing_action_descriptor());
                };
                if signature.effect_args.len() != action.args.len() {
                    return Err(DeclaredRowError::missing_action_arg(signature));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn declared_public_boundary(declared: &crate::EffectRow) -> crate::EffectRow {
    let mut boundary = declared.clone();
    for effect in declared.effects.iter() {
        match effect {
            Effect::Action(action) => {
                boundary.effects.insert(Effect::Tag(action.tag));
            }
            Effect::AppliedAction(action) => {
                boundary.effects.insert(Effect::Tag(action.action.tag));
            }
            Effect::Tag(_) | Effect::Applied { .. } | Effect::Var(_) | Effect::Error(_) => {}
        }
    }
    boundary
}

fn row_contains_agentic_call_action(row: &crate::EffectRow) -> bool {
    row.effects.iter().any(|effect| match effect {
        Effect::Tag(tag) => *tag == AGENTIC_TAG,
        Effect::Applied { tag, .. } => *tag == AGENTIC_TAG,
        Effect::Action(action) => {
            action.tag == AGENTIC_TAG && action.action == AGENTIC_INFER_ACTION
        }
        Effect::AppliedAction(action) => {
            action.action.tag == AGENTIC_TAG && action.action.action == AGENTIC_INFER_ACTION
        }
        Effect::Var(_) | Effect::Error(_) => false,
    })
}

fn declared_requires_tool_sandbox_action(
    registry: &EffectRegistry,
    declared: &crate::EffectRow,
) -> bool {
    declared.effects.iter().any(|effect| match effect {
        Effect::Tag(tag) | Effect::Applied { tag, .. } => {
            tag_declares_unparameterized_sandbox_boundary(registry, *tag)
        }
        Effect::Action(action) => registry
            .action_signature(action)
            .is_some_and(action_signature_requires_sandbox_profile),
        Effect::AppliedAction(_) | Effect::Var(_) | Effect::Error(_) => false,
    })
}

fn tag_declares_unparameterized_sandbox_boundary(
    registry: &EffectRegistry,
    tag: crate::EffectTagId,
) -> bool {
    registry
        .tag_by_core(crate::CoreEffect::Command)
        .is_some_and(|command| tag == command || registry.tag_extends(tag, command))
}

fn action_signature_requires_sandbox_profile(signature: &crate::EffectActionSig) -> bool {
    signature.effect_args.iter().any(|kind| {
        matches!(
            kind,
            EffectActionArgKind::StaticResourcePath { ty } if ty == "SandboxProfile"
        )
    })
}

fn effect_args_as_types(
    args: &[etas_types::EffectArgRef],
) -> Result<Vec<etas_types::TypeId>, DeclaredRowError> {
    args.iter()
        .map(|arg| match arg {
            etas_types::EffectArgRef::Type(ty) => Ok(*ty),
            etas_types::EffectArgRef::Path(path) if path.len() == 1 => {
                Err(DeclaredRowError::missing_type_arg_fact())
            }
            etas_types::EffectArgRef::Path(_) => Err(DeclaredRowError::invalid_type_arg()),
            etas_types::EffectArgRef::Wildcard
            | etas_types::EffectArgRef::String(_)
            | etas_types::EffectArgRef::Int(_) => Err(DeclaredRowError::invalid_type_arg()),
        })
        .collect()
}

fn effect_name(
    registry: &EffectRegistry,
    _types: &etas_types::TypeStore,
    name: &str,
    span: etas_core::Span,
    effects: &mut EffectOutput,
) -> Result<crate::Effect, DeclaredRowError> {
    if let Some(action) = registry.action_by_name(name) {
        return Ok(crate::Effect::Action(action));
    }
    if let Some(tag) = registry.tag_by_name(name) {
        return Ok(crate::Effect::Tag(tag));
    }
    effects.diagnostics.push(Diagnostic::effect_check(
        EffectDiagnosticCode::UnknownEffectTag,
        span,
        "effect tag is not known to the current effect registry",
    ));
    Err(DeclaredRowError::unknown_effect())
}

struct DeclaredRowError {
    code: EffectDiagnosticCode,
    message: &'static str,
    reason: FrontendRejectionReason,
}

impl DeclaredRowError {
    fn unknown_effect() -> Self {
        Self {
            code: EffectDiagnosticCode::UnknownEffectTag,
            message: "effect tag is not known to the current effect registry",
            reason: FrontendRejectionReason::UnresolvedEffect,
        }
    }

    fn invalid_type_arg() -> Self {
        Self {
            code: EffectDiagnosticCode::UnknownEffectTag,
            message: "effect tag arguments must be type references",
            reason: FrontendRejectionReason::UnresolvedEffect,
        }
    }

    fn missing_type_arg_fact() -> Self {
        Self {
            code: EffectDiagnosticCode::IncompleteEffectFacts,
            message: "effect tag arguments require checked type argument facts",
            reason: FrontendRejectionReason::EscapedEffect,
        }
    }

    fn missing_action_descriptor() -> Self {
        Self {
            code: EffectDiagnosticCode::IncompleteEffectFacts,
            message: "effect action arguments require checked action descriptor metadata",
            reason: FrontendRejectionReason::EscapedEffect,
        }
    }

    fn missing_action_arg(signature: &crate::EffectActionSig) -> Self {
        if action_signature_requires_sandbox_profile(signature) {
            return Self::missing_sandbox_action_arg();
        }
        Self {
            code: EffectDiagnosticCode::IncompleteEffectFacts,
            message: "effect action arguments do not match checked action descriptor metadata",
            reason: FrontendRejectionReason::EscapedEffect,
        }
    }

    fn missing_sandbox_action_arg() -> Self {
        Self {
            code: EffectDiagnosticCode::MissingSandboxActionArgument,
            message: "command actions require an explicit sandbox profile action argument",
            reason: FrontendRejectionReason::MissingRequirement,
        }
    }

    fn invalid_action_arg(kind: &EffectActionArgKind) -> Self {
        if matches!(
            kind,
            EffectActionArgKind::StaticResourcePath { ty } if ty == "SandboxProfile"
        ) {
            return Self::missing_sandbox_action_arg();
        }
        Self {
            code: EffectDiagnosticCode::IncompleteEffectFacts,
            message: "effect action argument does not match checked action descriptor metadata",
            reason: FrontendRejectionReason::EscapedEffect,
        }
    }
}

fn rejected_bodyless_tool(item: &HirItem, summary: &crate::EffectSummary) -> bool {
    matches!(
        item,
        HirItem::Tool(etas_hir::HirToolDecl {
            body: etas_hir::HirToolBody::Decl { .. },
            ..
        })
    ) && matches!(summary.support, InterpreterSupport::Rejected(_))
}

fn item_symbol(item: &HirItem) -> Option<SymbolId> {
    match item {
        HirItem::Flow(flow) => Some(flow.symbol),
        HirItem::Tool(tool) => Some(tool.symbol),
        HirItem::Agent(agent) => Some(agent.symbol),
        HirItem::TopLevelLet(value) => Some(value.symbol),
        _ => None,
    }
}

fn exported_name(hir: &etas_hir::HirProgram, symbol: SymbolId) -> QualifiedName {
    let Some(symbol_data) = hir.symbols.get(symbol) else {
        return QualifiedName {
            segments: vec!["<missing>".to_owned()],
        };
    };
    let mut segments = hir
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
    QualifiedName { segments }
}

fn latent_declared_by_item(tree: &HirTreeView<'_>, item: HirItemId, latent: HirExprId) -> bool {
    tree.expr(latent)
        .and_then(|expr| expr.owner())
        .and_then(|owner| owner_item(tree, owner))
        == Some(item)
}

fn owner_item(tree: &HirTreeView<'_>, mut owner: HirOwner) -> Option<HirItemId> {
    loop {
        owner = match owner {
            HirOwner::Item(item) => return Some(item),
            HirOwner::Module(_) => return None,
            HirOwner::Body(body) => tree.body(body)?.owner()?,
            HirOwner::Block(block) => tree.block(block)?.owner()?,
            HirOwner::Stmt(stmt) => tree.stmt(stmt)?.block()?.owner()?,
            HirOwner::Expr(expr) => tree.expr(expr)?.owner()?,
            HirOwner::HandlerArm(arm) => tree.index().handler_arm_owner.get(&arm).copied()?,
            HirOwner::Pattern(pat) => tree.pattern(pat)?.owner()?,
        };
    }
}

fn public_row_requires_high_impact_ack(registry: &EffectRegistry, row: &crate::EffectRow) -> bool {
    row.effects.iter().any(|effect| match effect {
        Effect::Tag(tag) | Effect::Applied { tag, .. } => {
            tag_requires_high_impact_ack(registry, *tag)
        }
        Effect::Action(action) | Effect::AppliedAction(crate::ActionInstanceRef { action, .. }) => {
            registry.action_requires_high_impact_ack(action)
                || tag_requires_high_impact_ack(registry, action.tag)
        }
        Effect::Var(_) | Effect::Error(_) => false,
    })
}

fn tag_requires_high_impact_ack(registry: &EffectRegistry, tag: crate::EffectTagId) -> bool {
    registry.tag_requires_high_impact_ack(tag)
        || registry
            .tag_by_core(crate::CoreEffect::Command)
            .is_some_and(|command| registry.tag_extends(tag, command))
        || registry
            .tag_by_core(crate::CoreEffect::Secret)
            .is_some_and(|secret| registry.tag_extends(tag, secret))
}

fn effect_row_label(
    registry: &EffectRegistry,
    types: &etas_types::TypeStore,
    row: &crate::EffectRow,
) -> String {
    let mut labels = row
        .effects
        .iter()
        .map(|effect| effect_label(registry, types, effect))
        .collect::<Vec<_>>();
    if row.open.is_some() {
        labels.push("<open effect row>".to_owned());
    }
    if labels.is_empty() {
        "[]".to_owned()
    } else {
        labels.join(", ")
    }
}

fn effect_label(
    registry: &EffectRegistry,
    types: &etas_types::TypeStore,
    effect: &Effect,
) -> String {
    match effect {
        Effect::Tag(tag) => registry.tag_name(*tag).unwrap_or("<effect>").to_owned(),
        Effect::Applied { tag, .. } => {
            format!("{}[...]", registry.tag_name(*tag).unwrap_or("<effect>"))
        }
        Effect::Action(action) | Effect::AppliedAction(crate::ActionInstanceRef { action, .. }) => {
            let owner = registry.tag_name(action.tag).unwrap_or("<effect>");
            let action = registry
                .action_name(action.tag, action.action)
                .unwrap_or("<action>");
            format!("{owner}.{action}")
        }
        Effect::Var(_) => "<effect-var>".to_owned(),
        Effect::Error(error) => format!("Error[{}]", type_label(types, *error)),
    }
}

fn type_label(types: &etas_types::TypeStore, ty: etas_types::TypeId) -> String {
    match types.get(ty).cloned() {
        Some(etas_types::Type::Primitive(primitive)) => primitive.source_name().to_owned(),
        Some(etas_types::Type::IntegerLiteral { .. }) => "i32".to_owned(),
        Some(etas_types::Type::Var(var)) => format!("T{}", var.0),
        Some(etas_types::Type::Array(inner)) => format!("Array[{}]", type_label(types, inner)),
        Some(etas_types::Type::List(inner)) => format!("List[{}]", type_label(types, inner)),
        Some(etas_types::Type::Map { key, value }) => {
            format!(
                "Map[{}, {}]",
                type_label(types, key),
                type_label(types, value)
            )
        }
        Some(etas_types::Type::Set(inner)) => format!("Set[{}]", type_label(types, inner)),
        Some(etas_types::Type::Range { index }) => {
            format!("Range[{}]", type_label(types, index))
        }
        Some(etas_types::Type::Slice(inner)) => format!("Slice[{}]", type_label(types, inner)),
        Some(etas_types::Type::Option(inner)) => format!("Option[{}]", type_label(types, inner)),
        Some(etas_types::Type::Result { ok, err }) => {
            format!(
                "Result[{}, {}]",
                type_label(types, ok),
                type_label(types, err)
            )
        }
        Some(etas_types::Type::Record(record)) => {
            let fields = record
                .fields
                .iter()
                .map(|field| format!("{}: {}", field.name, type_label(types, field.ty)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{fields}}}")
        }
        Some(etas_types::Type::Tuple(elems)) => {
            let elems = elems
                .iter()
                .map(|elem| type_label(types, *elem))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({elems})")
        }
        Some(etas_types::Type::Enum(enum_ref)) => enum_ref.name,
        Some(etas_types::Type::Function(flow)) => {
            let input = if flow.input.is_empty() {
                "unit".to_owned()
            } else if flow.input.len() == 1 {
                type_label(types, flow.input[0])
            } else {
                format!(
                    "({})",
                    flow.input
                        .iter()
                        .map(|input| type_label(types, *input))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            format!("{input} -> {}", type_label(types, flow.output))
        }
        Some(etas_types::Type::Handler(_)) => "handler".to_owned(),
        Some(etas_types::Type::Named(named)) => named.name,
        Some(etas_types::Type::Nominal(nominal)) => nominal.name,
        Some(etas_types::Type::Applied { constructor, args }) => {
            let args = args
                .iter()
                .map(|arg| type_label(types, *arg))
                .collect::<Vec<_>>()
                .join(", ");
            format!("type{}[{args}]", constructor.0)
        }
        Some(etas_types::Type::Refined { base, .. }) => type_label(types, base),
        Some(etas_types::Type::Trust { wrapper, inner }) => {
            format!("{wrapper}[{}]", type_label(types, inner))
        }
        Some(etas_types::Type::Schema(inner)) => format!("Schema[{}]", type_label(types, inner)),
        Some(etas_types::Type::Prompt) => "Prompt".to_owned(),
        Some(etas_types::Type::PromptPart) => "PromptPart".to_owned(),
        Some(etas_types::Type::Message(inner)) => format!("Message[{}]", type_label(types, inner)),
        Some(etas_types::Type::MemorySelection(inner)) => {
            format!("MemorySelection[{}]", type_label(types, inner))
        }
        Some(etas_types::Type::Store { key, value }) => {
            format!(
                "Store[{}, {}]",
                type_label(types, key),
                type_label(types, value)
            )
        }
        Some(etas_types::Type::MemoryPlace(place)) => place.segments.join("."),
        Some(etas_types::Type::MemoryRegion(inner)) => {
            format!("MemoryRegion[{}]", type_label(types, inner))
        }
        Some(etas_types::Type::ResourceHandle(handle)) => match handle {
            etas_types::ResourceHandleType::MemoryRegion { schema } => {
                format!("ResourceHandleMemoryRegion[{}]", type_label(types, schema))
            }
            etas_types::ResourceHandleType::ExternalTool { signature } => {
                format!("ExternalTool[{}]", type_label(types, signature))
            }
            etas_types::ResourceHandleType::Other { name, args } => {
                let args = args
                    .iter()
                    .map(|arg| type_label(types, *arg))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}[{args}]")
            }
        },
        None => "<unknown>".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_latent_effect_expression_returns_structural_error() {
        let error = latent_effect_span(&etas_hir::HirProgram::default(), 0_u32.into())
            .expect_err("missing expression must fail closed");
        assert!(matches!(error, EffectPipelineError::MissingExprFact { .. }));
    }
}
