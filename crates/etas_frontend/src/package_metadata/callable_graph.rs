use super::{PackageMetadataError, ProjectMetadataProjection};
use crate::CheckedProject;

#[derive(Clone, etas_core::RetainedSize)]
pub(crate) struct CheckedCallableGraph {
    pub graph: std::sync::Arc<etas_package_metadata::CallableValueGraph>,
    pub origins: Vec<CallableGraphOrigin>,
}

#[derive(Clone, etas_core::RetainedSize)]
pub(crate) enum CallableGraphOrigin {
    Native {
        unit: etas_effects::EffectUnit,
        environment: Option<etas_effects::facts::CallableEnvironmentId>,
    },
    Reference(etas_effects::CallableReturnReference),
    Metadata(etas_effects::MetadataCallableSource),
}

pub(crate) struct GraphResidualFacts<'a> {
    pub solved: &'a etas_effects::ResidualCheckSet,
    pub latent: Option<&'a etas_effects::ResidualCheckSet>,
}

/// Resolves graph nodes through the same checked facts used by projection.
/// Missing or unavailable analysis is not an empty residual set.
pub(crate) fn graph_residual_facts<'a>(
    checked: &'a CheckedProject,
    graph: &CheckedCallableGraph,
) -> Option<Vec<GraphResidualFacts<'a>>> {
    use etas_effects::facts::{
        CallableReferenceAnalysis, CapturedCallableAnalysis, CapturedCallableId,
        MetadataCallableAnalysis,
    };
    if graph.origins.len() != graph.graph.values.len() {
        return None;
    }
    graph
        .origins
        .iter()
        .map(|origin| {
            let summary = match origin {
                CallableGraphOrigin::Native {
                    unit,
                    environment: Some(environment),
                } => {
                    match checked
                        .effects
                        .captured_callables
                        .get(&CapturedCallableId {
                            unit: *unit,
                            environment: *environment,
                        })? {
                        CapturedCallableAnalysis::Solved(summary) => summary.as_ref(),
                        CapturedCallableAnalysis::Unavailable { .. } => return None,
                    }
                }
                CallableGraphOrigin::Native {
                    unit,
                    environment: None,
                } => checked.effects.unit_effects.get(unit)?,
                CallableGraphOrigin::Reference(reference) => {
                    match checked.effects.callable_references.get(reference)? {
                        CallableReferenceAnalysis::Solved { summary, .. } => summary.as_ref(),
                        CallableReferenceAnalysis::Unavailable { .. } => return None,
                    }
                }
                CallableGraphOrigin::Metadata(source) => {
                    match checked.effects.metadata_callables.get(source)? {
                        MetadataCallableAnalysis::Solved { summary, .. } => summary.as_ref(),
                        MetadataCallableAnalysis::Unavailable { .. } => return None,
                    }
                }
            };
            let latent = match origin {
                CallableGraphOrigin::Native {
                    unit: etas_effects::EffectUnit::AnonymousFlow { value, .. },
                    ..
                } => Some(
                    &checked
                        .effects
                        .latent_effects
                        .get(value)?
                        .summary
                        .residual_checks,
                ),
                _ => None,
            };
            Some(GraphResidualFacts {
                solved: &summary.residual_checks,
                latent,
            })
        })
        .collect()
}

pub(crate) fn checked_callable_graph(
    checked: &CheckedProject,
    item: etas_hir::HirItemId,
    summary: &etas_effects::EffectSummary,
    identities: Option<&crate::code::identity::DefinitionIdentityIndex>,
) -> Result<Option<CheckedCallableGraph>, PackageMetadataError> {
    ProjectMetadataProjection::new(checked).returned_callable_projection(item, summary, identities)
}

pub(crate) fn checked_request_template(
    checked: &CheckedProject,
    item: etas_hir::HirItemId,
    summary: &etas_effects::EffectSummary,
    identities: &crate::code::identity::DefinitionIdentityIndex,
) -> Result<Option<etas_package_metadata::RequestInstantiationTemplate>, PackageMetadataError> {
    ProjectMetadataProjection::new(checked)
        .with_identities(Some(identities))
        .item_request_template(item, summary)
}
