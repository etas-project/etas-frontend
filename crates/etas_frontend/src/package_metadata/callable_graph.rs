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

pub(crate) fn checked_callable_graph(
    checked: &CheckedProject,
    item: etas_hir::HirItemId,
    summary: &etas_effects::EffectSummary,
) -> Result<Option<CheckedCallableGraph>, PackageMetadataError> {
    ProjectMetadataProjection::new(checked).returned_callable_projection(item, summary)
}

pub(crate) fn checked_request_template(
    checked: &CheckedProject,
    item: etas_hir::HirItemId,
    summary: &etas_effects::EffectSummary,
) -> Result<Option<etas_package_metadata::RequestInstantiationTemplate>, PackageMetadataError> {
    ProjectMetadataProjection::new(checked).item_request_template(item, summary)
}
