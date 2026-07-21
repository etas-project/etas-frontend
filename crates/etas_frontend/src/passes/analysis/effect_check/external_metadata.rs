use std::collections::{HashMap, HashSet};

use etas_core::{Diagnostic, EffectDiagnosticCode, Span};
use etas_types::{EffectArgRef, TypeOutput};

use super::anchors::ExternalImportAnchorIndex;
use crate::{
    ProjectExternalActionArgKindInput, ProjectExternalEffectArgInput,
    ProjectExternalEffectRowInput, ProjectExternalPublicMetadataInput,
    ProjectExternalTraceSpecClauseKindInput, ProjectInput,
};

pub(super) fn external_effect_metadata(
    input: &ProjectInput,
    types: &TypeOutput,
    anchors: &ExternalImportAnchorIndex,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<etas_effects::DependencyEffectMetadata, String> {
    let mut metadata = input.environment.external_effect_metadata.clone();
    for public_metadata in &input.environment.external_public_metadata {
        if anchors.package_span(public_metadata.package).is_none() {
            continue;
        }
        metadata
            .tags
            .extend(public_metadata.effects.iter().map(|effect| {
                etas_effects::DependencyEffectTag {
                    path: effect.path.clone(),
                    runtime_requirement: None,
                }
            }));
        for action in &public_metadata.actions {
            match external_dependency_action(action, types) {
                Ok(action) => metadata.actions.push(action),
                Err(message) => match anchors.item_span(public_metadata.package, &action.path) {
                    Some(span) => push_external_metadata_diagnostic(diagnostics, span, message),
                    None => {
                        return Err(format!(
                            "invalid external metadata for action `{}` has no resolved import anchor: {message}",
                            action.path.join(".")
                        ));
                    }
                },
            }
        }
    }
    Ok(metadata)
}

fn external_dependency_action(
    action: &crate::ProjectExternalActionSignatureInput,
    types: &TypeOutput,
) -> Result<etas_effects::DependencyEffectAction, String> {
    if action.selector_param_names.len() != action.effect_args.len() {
        return Err(format!(
            "invalid external package metadata: action `{}` selector_param_names length {} does not match effect_args length {}",
            action.path.join("."),
            action.selector_param_names.len(),
            action.effect_args.len()
        ));
    }
    if action.selector_defaults.len() != action.effect_args.len() {
        return Err(format!(
            "invalid external package metadata: action `{}` selector_defaults length {} does not match effect_args length {}",
            action.path.join("."),
            action.selector_defaults.len(),
            action.effect_args.len()
        ));
    }
    for (index, (kind, default)) in action
        .effect_args
        .iter()
        .zip(&action.selector_defaults)
        .enumerate()
    {
        let Some(default) = default else {
            continue;
        };
        if !external_selector_default_matches_kind(default, kind) {
            return Err(format!(
                "invalid external package metadata: action `{}` selector default at index {index} does not match selector kind",
                action.path.join(".")
            ));
        }
    }
    let selector_defaults = action
        .selector_defaults
        .iter()
        .map(|default| match default {
            Some(arg) => external_effect_arg(arg, types).map(Some),
            None => Ok(None),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(etas_effects::DependencyEffectAction {
        path: action.path.clone(),
        effect_args: action
            .effect_args
            .iter()
            .map(external_action_arg_kind)
            .collect(),
        selector_param_names: action.selector_param_names.clone(),
        selector_defaults,
        returns_never: action.returns_never,
        runtime_requirement: None,
    })
}

fn external_selector_default_matches_kind(
    default: &ProjectExternalEffectArgInput,
    kind: &ProjectExternalActionArgKindInput,
) -> bool {
    if matches!(default, ProjectExternalEffectArgInput::Wildcard) {
        return true;
    }
    match kind {
        ProjectExternalActionArgKindInput::Type => {
            matches!(default, ProjectExternalEffectArgInput::Type(_))
        }
        ProjectExternalActionArgKindInput::MemoryPlace
        | ProjectExternalActionArgKindInput::StaticResourcePath { .. } => {
            matches!(default, ProjectExternalEffectArgInput::Path(_))
        }
        ProjectExternalActionArgKindInput::StringPattern => matches!(
            default,
            ProjectExternalEffectArgInput::String(_) | ProjectExternalEffectArgInput::Path(_)
        ),
    }
}

fn external_action_arg_kind(
    kind: &ProjectExternalActionArgKindInput,
) -> etas_effects::EffectActionArgKind {
    match kind {
        ProjectExternalActionArgKindInput::Type => etas_effects::EffectActionArgKind::Type,
        ProjectExternalActionArgKindInput::MemoryPlace => {
            etas_effects::EffectActionArgKind::MemoryPlace
        }
        ProjectExternalActionArgKindInput::StaticResourcePath { ty } => {
            etas_effects::EffectActionArgKind::StaticResourcePath { ty: ty.clone() }
        }
        ProjectExternalActionArgKindInput::StringPattern => {
            etas_effects::EffectActionArgKind::StringPattern
        }
    }
}

fn external_effect_arg(
    arg: &ProjectExternalEffectArgInput,
    types: &TypeOutput,
) -> Result<EffectArgRef, String> {
    etas_types::lower::external::lower_external_effect_arg_from_output(
        types,
        &super::super::type_check_bridge::convert_external_effect_arg(arg),
    )
    .map_err(|error| error.message)
}

fn push_external_metadata_diagnostic(
    diagnostics: &mut Vec<Diagnostic>,
    span: Span,
    message: String,
) {
    diagnostics.push(Diagnostic::effect_check(
        EffectDiagnosticCode::IncompleteEffectFacts,
        span,
        message,
    ));
}

pub(super) fn tool_provider_bindings(
    input: &ProjectInput,
) -> Vec<etas_effects::ToolProviderBindingMetadata> {
    input
        .environment
        .tool_bindings
        .iter()
        .map(|binding| etas_effects::ToolProviderBindingMetadata {
            tool: binding
                .tool
                .split('.')
                .filter(|segment| !segment.is_empty())
                .map(str::to_owned)
                .collect(),
            provider: binding.provider.clone(),
            effect_row: binding.effect_row.clone(),
            action_row: binding.action_row.clone(),
        })
        .collect()
}

pub(super) fn external_effect_summaries(
    input: &ProjectInput,
    types: &TypeOutput,
    anchors: &ExternalImportAnchorIndex,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<etas_effects::AnchoredExternalMetadata<etas_effects::ExternalEffectSummaryMetadata>> {
    let mut summaries = Vec::new();
    let mut seen = HashSet::<(Option<String>, Vec<String>)>::new();
    for metadata in &input.environment.external_public_metadata {
        if anchors.package_span(metadata.package).is_none() {
            continue;
        }
        let package = input
            .environment
            .external_packages
            .iter()
            .find(|package| package.id == metadata.package);
        let package_identity = package.map(|package| {
            format!(
                "{}@{}#{}",
                package.name, package.version, package.import_root
            )
        });
        let import_root = package.map(|package| package.import_root.clone());
        let param_names_by_item = external_param_names_by_item(metadata);
        for summary in &metadata.effect_summaries {
            let Some(summary_span) = anchors.item_span(metadata.package, &summary.item) else {
                continue;
            };
            seen.insert((package_identity.clone(), summary.item.clone()));
            match external_effect_summary_metadata(
                summary,
                package_identity.clone(),
                import_root.clone(),
                &param_names_by_item,
                types,
            ) {
                Ok(summary) => summaries.push(etas_effects::AnchoredExternalMetadata {
                    package: metadata.package.0,
                    span: summary_span,
                    metadata: summary,
                }),
                Err(message) => {
                    push_external_metadata_diagnostic(diagnostics, summary_span, message)
                }
            }
        }

        for (item, row) in metadata
            .flows
            .iter()
            .map(|signature| (&signature.path, signature.effects.as_ref()))
            .chain(
                metadata
                    .agents
                    .iter()
                    .map(|signature| (&signature.path, signature.effects.as_ref())),
            )
            .chain(
                metadata
                    .tools
                    .iter()
                    .map(|signature| (&signature.path, signature.effects.as_ref())),
            )
        {
            let Some(item_span) = anchors.item_span(metadata.package, item) else {
                continue;
            };
            if !seen.insert((package_identity.clone(), item.clone())) {
                continue;
            }
            if row.is_some_and(|row| !row.effects.is_empty()) {
                diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    item_span,
                    format!(
                        "external callable `{}` declares effects but its package metadata does not provide a solved effect summary",
                        item.join(".")
                    ),
                ));
                continue;
            }
            let public_effects = match row {
                Some(row) => match external_effect_row(row, types) {
                    Ok(row) => row,
                    Err(message) => {
                        push_external_metadata_diagnostic(diagnostics, item_span, message);
                        continue;
                    }
                },
                None => Default::default(),
            };
            summaries.push(etas_effects::AnchoredExternalMetadata {
                package: metadata.package.0,
                span: item_span,
                metadata: etas_effects::ExternalEffectSummaryMetadata {
                    package: package_identity.clone(),
                    import_root: import_root.clone(),
                    item: item.clone(),
                    param_names: param_names_by_item.get(item).cloned().unwrap_or_default(),
                    public_effects,
                    requested_actions: Default::default(),
                    handled_requested_actions: Default::default(),
                    latent_flows: Vec::new(),
                },
            });
        }
    }
    summaries
}

fn external_effect_summary_metadata(
    summary: &crate::ProjectExternalEffectSummaryInput,
    package: Option<String>,
    import_root: Option<String>,
    param_names_by_item: &HashMap<Vec<String>, Vec<String>>,
    types: &TypeOutput,
) -> Result<etas_effects::ExternalEffectSummaryMetadata, String> {
    let latent_flows = summary
        .latent_flows
        .iter()
        .map(|latent| {
            Ok(etas_effects::ExternalLatentFlowSummaryMetadata {
                declared_bound: external_effect_row(&latent.declared_bound, types)?,
                inferred_effects: external_effect_row(&latent.inferred_effects, types)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(etas_effects::ExternalEffectSummaryMetadata {
        package,
        import_root,
        item: summary.item.clone(),
        param_names: param_names_by_item
            .get(&summary.item)
            .cloned()
            .unwrap_or_default(),
        public_effects: external_effect_row(&summary.public_effects, types)?,
        requested_actions: external_effect_row(&summary.requested_actions, types)?,
        handled_requested_actions: external_effect_row(&summary.handled_requested_actions, types)?,
        latent_flows,
    })
}

pub(super) fn external_trace_spec_summaries(
    input: &ProjectInput,
    types: &TypeOutput,
    anchors: &ExternalImportAnchorIndex,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<etas_effects::AnchoredExternalMetadata<etas_effects::ExternalTraceSpecSummaryMetadata>> {
    let mut summaries = Vec::new();
    for metadata in &input.environment.external_public_metadata {
        if anchors.package_span(metadata.package).is_none() {
            continue;
        }
        for summary in &metadata.trace_spec_summaries {
            let Some(summary_span) = anchors.item_span(metadata.package, &summary.trace_spec)
            else {
                continue;
            };
            match external_trace_spec_summary_metadata(summary, types) {
                Ok(summary) => summaries.push(etas_effects::AnchoredExternalMetadata {
                    package: metadata.package.0,
                    span: summary_span,
                    metadata: summary,
                }),
                Err(message) => {
                    push_external_metadata_diagnostic(diagnostics, summary_span, message)
                }
            }
        }
    }
    summaries
}

fn external_trace_spec_summary_metadata(
    summary: &crate::ProjectExternalTraceSpecSummaryInput,
    types: &TypeOutput,
) -> Result<etas_effects::ExternalTraceSpecSummaryMetadata, String> {
    let clauses = summary
        .clauses
        .iter()
        .map(|clause| {
            Ok(etas_effects::ExternalTraceSpecClauseMetadata {
                kind: external_trace_spec_clause_kind(clause.kind),
                pattern: clause
                    .pattern
                    .as_ref()
                    .map(|row| external_trace_spec_row(row, types))
                    .transpose()?,
                guard: clause
                    .guard
                    .as_ref()
                    .map(|row| external_trace_spec_row(row, types))
                    .transpose()?,
                target: clause
                    .target
                    .as_ref()
                    .map(|row| external_trace_spec_row(row, types))
                    .transpose()?,
                obligation: clause
                    .obligation
                    .as_ref()
                    .map(|row| external_trace_spec_row(row, types))
                    .transpose()?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(etas_effects::ExternalTraceSpecSummaryMetadata {
        trace_spec: summary.trace_spec.clone(),
        clauses,
    })
}

fn external_trace_spec_clause_kind(
    kind: ProjectExternalTraceSpecClauseKindInput,
) -> etas_effects::ExternalTraceSpecClauseKind {
    match kind {
        ProjectExternalTraceSpecClauseKindInput::Allow => {
            etas_effects::ExternalTraceSpecClauseKind::Allow
        }
        ProjectExternalTraceSpecClauseKindInput::Deny => {
            etas_effects::ExternalTraceSpecClauseKind::Deny
        }
        ProjectExternalTraceSpecClauseKindInput::RequireBefore => {
            etas_effects::ExternalTraceSpecClauseKind::RequireBefore
        }
        ProjectExternalTraceSpecClauseKindInput::RequireAfter => {
            etas_effects::ExternalTraceSpecClauseKind::RequireAfter
        }
    }
}

fn external_trace_spec_row(
    row: &ProjectExternalEffectRowInput,
    types: &TypeOutput,
) -> Result<etas_effects::ExternalTraceSpecEffectRowMetadata, String> {
    Ok(etas_effects::ExternalTraceSpecEffectRowMetadata {
        effects: row
            .effects
            .iter()
            .map(|effect| {
                Ok(etas_effects::ExternalTraceSpecEffectMetadata {
                    path: effect.path.clone(),
                    args: external_effect_args(&effect.args, types)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
    })
}

fn external_effect_row(
    row: &ProjectExternalEffectRowInput,
    types: &TypeOutput,
) -> Result<etas_effects::ExternalEffectRowMetadata, String> {
    Ok(etas_effects::ExternalEffectRowMetadata {
        effects: row
            .effects
            .iter()
            .map(|effect| {
                Ok(etas_effects::ExternalEffectMetadata {
                    path: effect.path.clone(),
                    args: external_effect_args(&effect.args, types)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
    })
}

fn external_effect_args(
    args: &[ProjectExternalEffectArgInput],
    types: &TypeOutput,
) -> Result<Vec<EffectArgRef>, String> {
    args.iter()
        .map(|arg| external_effect_arg(arg, types))
        .collect()
}

fn external_param_names_by_item(
    metadata: &ProjectExternalPublicMetadataInput,
) -> HashMap<Vec<String>, Vec<String>> {
    metadata
        .flows
        .iter()
        .map(|signature| (signature.path.clone(), signature.param_names.clone()))
        .chain(
            metadata
                .agents
                .iter()
                .map(|signature| (signature.path.clone(), signature.param_names.clone())),
        )
        .chain(
            metadata
                .tools
                .iter()
                .map(|signature| (signature.path.clone(), signature.param_names.clone())),
        )
        .collect()
}
