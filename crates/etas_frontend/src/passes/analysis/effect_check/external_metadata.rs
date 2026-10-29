use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use etas_core::{Diagnostic, EffectDiagnosticCode, Span};
use etas_types::{EffectArgRef, TypeOutput};

use super::anchors::ExternalImportAnchorIndex;
use crate::{
    ProjectEnvironmentInput, ProjectExternalActionArgKindInput,
    ProjectExternalActionTraceEventSourceInput, ProjectExternalActionTraceInput,
    ProjectExternalCallableGenericParamKindInput, ProjectExternalEffectArgInput,
    ProjectExternalEffectRefInput, ProjectExternalEffectRowInput,
    ProjectExternalPublicMetadataInput, ProjectExternalTraceSpecClauseKindInput,
};

pub(super) fn external_effect_metadata(
    environment: &ProjectEnvironmentInput,
    types: &TypeOutput,
    anchors: &ExternalImportAnchorIndex,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<etas_effects::DependencyEffectMetadata, String> {
    let mut metadata = environment.external_effect_metadata.clone();
    for public_metadata in &environment.external_public_metadata {
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
                Ok(action) => {
                    if let Some(existing) = metadata
                        .actions
                        .iter()
                        .find(|existing| existing.path == action.path)
                    {
                        if existing != &action {
                            return Err(format!(
                                "conflicting external metadata for action `{}`",
                                action.path.join(".")
                            ));
                        }
                        continue;
                    }
                    metadata.actions.push(action);
                }
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
            ProjectExternalEffectArgInput::String(_)
                | ProjectExternalEffectArgInput::Int(_)
                | ProjectExternalEffectArgInput::Path(_)
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
    environment: &ProjectEnvironmentInput,
) -> Vec<etas_effects::ToolProviderBindingMetadata> {
    environment
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
    environment: &ProjectEnvironmentInput,
    types: &TypeOutput,
    std_registry: &etas_std::StdRegistry,
    anchors: &ExternalImportAnchorIndex,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<
    Vec<etas_effects::AnchoredExternalMetadata<etas_effects::ExternalEffectSummaryMetadata>>,
    String,
> {
    let mut summaries = Vec::new();
    let mut seen = HashSet::<(Option<String>, Vec<String>)>::new();
    let mut contracts = ExternalContractIndex::new(environment, std_registry)?;
    for metadata in &environment.external_public_metadata {
        if anchors.package_span(metadata.package).is_none() {
            continue;
        }
        let package = environment
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
        let generic_params_by_item = external_generic_params_by_item(metadata);
        for summary in &metadata.effect_summaries {
            let Some(summary_span) = anchors.item_span(metadata.package, &summary.item) else {
                continue;
            };
            seen.insert((package_identity.clone(), summary.item.clone()));
            let contract = contracts.get(metadata.package, &summary.item);
            match contract.and_then(|contract| {
                external_effect_summary_metadata(
                    summary,
                    package_identity.clone(),
                    import_root.clone(),
                    &param_names_by_item,
                    &generic_params_by_item,
                    types,
                    contract,
                )
            }) {
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

        for item in metadata
            .flows
            .iter()
            .map(|signature| &signature.path)
            .chain(
                metadata
                    .agents
                    .iter()
                    .filter(|agent| agent.output.is_some())
                    .map(|signature| &signature.path),
            )
            .chain(metadata.tools.iter().map(|signature| &signature.path))
            .chain(metadata.values.iter().filter_map(|value| {
                matches!(
                    value.ty.as_ref(),
                    Some(crate::ProjectExternalTypeInput::Function { .. })
                        | Some(crate::ProjectExternalTypeInput::Handler { .. })
                )
                .then_some(&value.path)
            }))
        {
            let Some(item_span) = anchors.item_span(metadata.package, item) else {
                continue;
            };
            if !seen.insert((package_identity.clone(), item.clone())) {
                continue;
            }
            diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                item_span,
                format!(
                    "external callable `{}` does not provide a solved effect summary",
                    item.join(".")
                ),
            ));
        }
    }
    Ok(summaries)
}

type ContractCandidate<'a> = (
    usize,
    &'a etas_code::descriptor::DefinitionDescriptorSet,
    &'a etas_code::descriptor::CallableHeaderInfo,
);

struct ExternalContractIndex<'a> {
    paths: HashMap<crate::ExternalPackageId, HashMap<Vec<String>, &'a etas_code::DefinitionKey>>,
    candidates: BTreeMap<&'a etas_code::DefinitionKey, Vec<ContractCandidate<'a>>>,
    graphs: HashMap<usize, Arc<etas_code::descriptor::DescriptorGraphInput>>,
    fingerprints: HashMap<usize, etas_code::CodeFingerprint>,
    definition_paths: Arc<BTreeMap<etas_code::DefinitionKey, Vec<Vec<String>>>>,
}

impl<'a> ExternalContractIndex<'a> {
    fn new(
        environment: &'a ProjectEnvironmentInput,
        std_registry: &etas_std::StdRegistry,
    ) -> Result<Self, String> {
        let mut paths = HashMap::new();
        let mut candidates = BTreeMap::<_, Vec<_>>::new();
        let mut definition_paths = BTreeMap::<etas_code::DefinitionKey, Vec<Vec<String>>>::new();
        let package = crate::code::identity::standard_package_key(std_registry)
            .map_err(|error| error.to_string())?;
        for symbol in std_registry.symbols() {
            if let Some(identity) =
                crate::code::identity::standard_definition_key(std_registry, package, symbol)
                    .map_err(|error| error.to_string())?
            {
                definition_paths
                    .entry(identity)
                    .or_default()
                    .push(symbol.qualified_path.clone());
            }
        }
        for (index, origin) in environment.external_definition_origins.iter().enumerate() {
            for binding in &origin.origins.declarations {
                definition_paths
                    .entry(binding.definition.clone())
                    .or_default()
                    .push(binding.path());
            }
            paths.insert(
                origin.package,
                origin
                    .origins
                    .declarations
                    .iter()
                    .map(|binding| (binding.path(), &binding.definition))
                    .collect(),
            );
            if let Some(set) = &origin.descriptors
                && let Some(headers) = &set.headers
            {
                for header in &headers.callables {
                    if let Some(identity) = header.id.as_declaration() {
                        candidates
                            .entry(identity)
                            .or_default()
                            .push((index, set, header));
                    }
                }
            }
        }
        Ok(Self {
            paths,
            candidates,
            graphs: HashMap::new(),
            fingerprints: HashMap::new(),
            definition_paths: Arc::new(definition_paths),
        })
    }

    fn get(
        &mut self,
        package: crate::ExternalPackageId,
        path: &[String],
    ) -> Result<Option<etas_effects::ExternalCallableContract>, String> {
        let Some(identity) = self.paths.get(&package).and_then(|paths| paths.get(path)) else {
            return Ok(None);
        };
        let Some(candidates) = self.candidates.get(identity) else {
            return Ok(None);
        };
        let Some((index, set, header)) = candidates.first().copied() else {
            return Ok(None);
        };
        if candidates
            .iter()
            .any(|(_, other_set, other_header)| *other_set != set || *other_header != header)
        {
            return Err(
                "conflicting external callable contracts for one declaration identity".into(),
            );
        }
        Ok(Some(etas_effects::ExternalCallableContract {
            definition_paths: self.definition_paths.clone(),
            descriptor_fingerprint: *self
                .fingerprints
                .entry(index)
                .or_insert_with(|| etas_package_metadata::definition_descriptors_fingerprint(set)),
            header: Arc::new(header.clone()),
            graph: self
                .graphs
                .entry(index)
                .or_insert_with(|| Arc::new(set.graph.clone()))
                .clone(),
        }))
    }
}

fn external_effect_summary_metadata(
    summary: &crate::ProjectExternalEffectSummaryInput,
    package: Option<String>,
    import_root: Option<String>,
    param_names_by_item: &HashMap<Vec<String>, Vec<String>>,
    generic_params_by_item: &HashMap<Vec<String>, ExternalGenericParams>,
    types: &TypeOutput,
    callable_contract: Option<etas_effects::ExternalCallableContract>,
) -> Result<etas_effects::ExternalEffectSummaryMetadata, String> {
    if let Some(contract) = &callable_contract {
        let Some(etas_code::descriptor::TypeInfo::Function(signature)) = contract
            .graph
            .types
            .get(contract.header.signature.0 as usize)
        else {
            return Err("external callable contract has no checked function signature".into());
        };
        let params = param_names_by_item.get(&summary.item).map_or(0, Vec::len);
        if signature.parameters.len() != params {
            return Err(
                "external callable contract and effect summary have different parameter arity"
                    .into(),
            );
        }
    }
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
        callable_contract,
        behavior: summary.behavior.clone(),
        handler_requests: summary.handler_requests.as_ref().map(|template| {
            Ok::<_, String>(etas_effects::ExternalRequestInstantiationMetadata {
                actions: template.actions.iter().map(|instance| Ok(etas_effects::ExternalActionRequestMetadata {
                    action: lower_external_effect_metadata(&instance.action, types)?,
                    signature: instance.signature.as_ref().map(|signature| {
                        let lower = |ty| etas_types::lower::external::external_type_id_from_output(types, &super::super::type_check_bridge::convert_external_type(ty)).map_err(|error| error.message);
                        Ok::<_, String>(etas_types::ActionRequestSignature {
                            params: signature.params.iter().map(lower).collect::<Result<_, _>>()?,
                            output: lower(&signature.output)?,
                        })
                    }).transpose()?,
                    type_bindings: instance.type_bindings.as_ref().map(|bindings| bindings.iter().map(|(name, ty)| {
                        Ok((name.clone(), etas_types::lower::external::external_type_id_from_output(types, &super::super::type_check_bridge::convert_external_type(ty)).map_err(|error| error.message)?))
                    }).collect::<Result<_, String>>()).transpose()?,
                })).collect::<Result<_, String>>()?,
                parameters: template.parameters.iter().map(|parameter| Ok(etas_effects::ExternalParameterRequestMetadata {
                    parameter: parameter.parameter.clone(),
                    excluded: parameter.excluded.iter().map(|effect| lower_external_effect_metadata(effect, types)).collect::<Result<_, _>>()?,
                })).collect::<Result<_, String>>()?,
            })
        }).transpose()?,
        callable_graph_key: summary
            .returned_callables
            .as_ref()
            .map(|graph| {
                let graph_key = etas_package_metadata::callable_value_graph_fingerprint(graph)
                    .map_err(|error| error.to_string())?;
                let package = package.as_deref().ok_or_else(|| {
                    "returned callable metadata has no checked package identity".to_owned()
                })?;
                Ok::<_, String>(etas_code::CodeFingerprint::of_parts(
                    "etas.effects.external-callable.v1",
                    std::iter::once(package.as_bytes())
                        .chain(summary.item.iter().map(|segment| segment.as_bytes()))
                        .chain(std::iter::once(graph_key.as_bytes().as_slice())),
                ))
            })
            .transpose()?,
        returned_callables: summary.returned_callables.clone(),
        callable_values: summary
            .returned_callables
            .as_ref()
            .map(|graph| crate::project::lower_callable_graph(graph, types))
            .transpose()?
            .unwrap_or_default(),
        default_handler_effects: summary
            .default_handler_effects
            .as_ref()
            .map(|effects| {
                Ok::<_, String>(etas_effects::ExternalDefaultHandlerEffectsMetadata {
                    direct_effects: external_effect_row(&effects.direct_effects, types)?,
                    default_actions: external_effect_row(&effects.default_actions, types)?,
                    handlers: effects
                        .handlers
                        .iter()
                        .map(|handler| {
                            Ok(etas_effects::ExternalDefaultHandlerContributionMetadata {
                                action: lower_external_effect_metadata(&handler.action, types)?,
                                produced: external_effect_row(&handler.produced, types)?,
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?,
                })
            })
            .transpose()?,
        package,
        import_root,
        item: summary.item.clone(),
        param_names: param_names_by_item
            .get(&summary.item)
            .cloned()
            .unwrap_or_default(),
        generic_params: generic_params_by_item
            .get(&summary.item)
            .map(|params| params.ordered.clone())
            .unwrap_or_default(),
        type_param_names: generic_params_by_item
            .get(&summary.item)
            .map(|params| params.type_params.clone())
            .unwrap_or_default(),
        effect_param_names: generic_params_by_item
            .get(&summary.item)
            .map(|params| params.effect_params.clone())
            .unwrap_or_default(),
        public_effects: external_effect_row(&summary.public_effects, types)?,
        requested_actions: external_effect_row(&summary.requested_actions, types)?,
        handled_requested_actions: external_effect_row(&summary.handled_requested_actions, types)?,
        latent_flows,
        action_trace: external_action_trace(&summary.action_trace, types)?,
    })
}

fn external_action_trace(
    trace: &ProjectExternalActionTraceInput,
    types: &TypeOutput,
) -> Result<etas_effects::ExternalActionTraceMetadata, String> {
    Ok(match trace {
        ProjectExternalActionTraceInput::Empty => etas_effects::ExternalActionTraceMetadata::Empty,
        ProjectExternalActionTraceInput::Event { action, source } => {
            etas_effects::ExternalActionTraceMetadata::Event {
                action: lower_external_effect_metadata(action, types)?,
                source: match source {
                    ProjectExternalActionTraceEventSourceInput::Perform => {
                        etas_effects::ActionEventSource::Perform
                    }
                    ProjectExternalActionTraceEventSourceInput::StdIntrinsic => {
                        etas_effects::ActionEventSource::StdIntrinsic
                    }
                    ProjectExternalActionTraceEventSourceInput::AgentCall => {
                        etas_effects::ActionEventSource::AgentCall
                    }
                    ProjectExternalActionTraceEventSourceInput::ExternalMetadata => {
                        etas_effects::ActionEventSource::ExternalMetadata
                    }
                    ProjectExternalActionTraceEventSourceInput::Transfer => {
                        etas_effects::ActionEventSource::Transfer
                    }
                    ProjectExternalActionTraceEventSourceInput::Unknown => {
                        etas_effects::ActionEventSource::Unknown
                    }
                },
            }
        }
        ProjectExternalActionTraceInput::ParameterCall { parameter } => {
            etas_effects::ExternalActionTraceMetadata::ParameterCall {
                parameter: parameter.clone(),
            }
        }
        ProjectExternalActionTraceInput::Seq(children) => {
            etas_effects::ExternalActionTraceMetadata::Seq(
                children
                    .iter()
                    .map(|child| external_action_trace(child, types))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        }
        ProjectExternalActionTraceInput::Choice(children) => {
            etas_effects::ExternalActionTraceMetadata::Choice(
                children
                    .iter()
                    .map(|child| external_action_trace(child, types))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        }
        ProjectExternalActionTraceInput::Repeat(child) => {
            etas_effects::ExternalActionTraceMetadata::Repeat(Box::new(external_action_trace(
                child, types,
            )?))
        }
        ProjectExternalActionTraceInput::UnknownOrder(actions) => {
            etas_effects::ExternalActionTraceMetadata::UnknownOrder(
                actions
                    .iter()
                    .map(|action| lower_external_effect_metadata(action, types))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        }
        ProjectExternalActionTraceInput::Widened {
            actions,
            parameter_calls,
        } => etas_effects::ExternalActionTraceMetadata::Widened {
            actions: actions
                .iter()
                .map(|action| lower_external_effect_metadata(action, types))
                .collect::<Result<Vec<_>, _>>()?,
            parameter_calls: parameter_calls.clone(),
        },
    })
}

fn lower_external_effect_metadata(
    effect: &ProjectExternalEffectRefInput,
    types: &TypeOutput,
) -> Result<etas_effects::ExternalEffectMetadata, String> {
    Ok(etas_effects::ExternalEffectMetadata {
        path: effect.path.clone(),
        args: external_effect_args(&effect.args, types)?,
    })
}

pub(super) fn external_trace_spec_summaries(
    environment: &ProjectEnvironmentInput,
    types: &TypeOutput,
    anchors: &ExternalImportAnchorIndex,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<etas_effects::AnchoredExternalMetadata<etas_effects::ExternalTraceSpecSummaryMetadata>> {
    let mut summaries = Vec::new();
    for metadata in &environment.external_public_metadata {
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
        tail: row.tail.clone(),
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

#[derive(Clone, Debug, Default)]
struct ExternalGenericParams {
    ordered: Vec<(String, etas_types::CallableGenericParamKind)>,
    type_params: Vec<String>,
    effect_params: Vec<String>,
}

fn external_generic_params_by_item(
    metadata: &ProjectExternalPublicMetadataInput,
) -> HashMap<Vec<String>, ExternalGenericParams> {
    metadata
        .flows
        .iter()
        .map(|signature| {
            (
                signature.path.clone(),
                split_external_generic_params(&signature.generic_params),
            )
        })
        .chain(metadata.agents.iter().map(|signature| {
            (
                signature.path.clone(),
                split_external_generic_params(&signature.generic_params),
            )
        }))
        .chain(metadata.tools.iter().map(|signature| {
            (
                signature.path.clone(),
                split_external_generic_params(&signature.generic_params),
            )
        }))
        .collect()
}

fn split_external_generic_params(
    params: &[crate::ProjectExternalCallableGenericParamInput],
) -> ExternalGenericParams {
    let mut result = ExternalGenericParams::default();
    for param in params {
        match param.kind {
            ProjectExternalCallableGenericParamKindInput::Type => {
                result.ordered.push((
                    param.name.clone(),
                    etas_types::CallableGenericParamKind::Type,
                ));
                result.type_params.push(param.name.clone());
            }
            ProjectExternalCallableGenericParamKindInput::Effect => {
                result.ordered.push((
                    param.name.clone(),
                    etas_types::CallableGenericParamKind::Effect,
                ));
                result.effect_params.push(param.name.clone());
            }
        }
    }
    result
}
