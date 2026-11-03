mod applications;
mod bindings;
mod checked;
mod diff;
mod edit;
mod external;
mod external_application;
mod methods;
mod reflect;
mod restore_external;
mod source;
mod store;
mod view;

use std::{collections::BTreeMap, fmt, sync::Arc};

use etas_code::{
    CallableId, CodeFingerprint,
    definition::{
        DefinitionAnalysisState, DefinitionError, DefinitionLease, DefinitionOperationError,
        DefinitionSnapshot,
    },
    descriptor::CallableAnalysisInfo,
};

use super::{FrozenDefinitionContract, FrozenDefinitionEnvironment};

pub use bindings::{FrozenReflectionBindings, NativeCallableReflection, NativeSourceReflection};
pub use external_application::CheckedExternalCallableApplication;
pub use store::NativeDefinitionStore;
use store::SourceReflectionKey;
pub use view::{NativeAgentMethodView, NativeDefinitionView, NativeMethodSnapshot};

#[derive(Clone, Copy, Debug)]
pub struct NativeDefinitionLimits {
    pub max_retained_objects: usize,
    pub max_retained_bytes: u64,
}

impl Default for NativeDefinitionLimits {
    fn default() -> Self {
        Self {
            max_retained_objects: 256,
            max_retained_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub enum NativeDefinitionError {
    Operation(DefinitionOperationError),
    Projection(crate::code::descriptor::ProjectionError),
    Definition(DefinitionError),
    MissingRevisionContext,
    MissingSourceEnvironment,
    CandidateInputMismatch,
    TypeRelocation(etas_types::TypeSubstitutionError),
    MissingBodySource,
    WrongDefinitionKind,
    SourceBundle(etas_code::definition::SourceBundleError),
    Overlay(crate::code::draft::OverlayError),
}

impl fmt::Display for NativeDefinitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TypeRelocation(error) => write!(f, "native code type relocation failed: {error}"),
            _ => write!(f, "native code definition operation failed: {self:?}"),
        }
    }
}
impl std::error::Error for NativeDefinitionError {}
impl From<DefinitionOperationError> for NativeDefinitionError {
    fn from(error: DefinitionOperationError) -> Self {
        Self::Operation(error)
    }
}

#[derive(etas_core::RetainedSize)]
struct NativeDefinition {
    data: DefinitionSnapshot<FrozenDefinitionContract, CallableAnalysisInfo>,
    environment: DefinitionEnvironment,
    auxiliary: DefinitionAnalysisState<BTreeMap<CallableId, CallableAnalysisInfo>>,
    method_snapshots: BTreeMap<CallableId, Arc<etas_code::descriptor::DefinitionSnapshotInfo>>,
    analysis_configuration: CodeFingerprint,
    analysis_graph: etas_code::descriptor::DescriptorGraph,
    reflection_body_access: bool,
    reflection_access: Option<etas_hir::HirModuleId>,
    provenance: DefinitionProvenance,
    implementation: Option<Arc<crate::code::service::CheckedCandidate>>,
    external_application: Option<Arc<ExternalApplicationEvidence>>,
    external_restoration: Option<Arc<ExternalRestorationReceipt>>,
}

#[derive(etas_core::RetainedSize)]
pub(crate) struct ExternalRestorationReceipt {
    metadata_base: Arc<etas_code::definition::FrozenDefinition<FrozenDefinitionContract>>,
    native_environment: Arc<FrozenDefinitionEnvironment>,
}

#[derive(etas_core::RetainedSize)]
struct ExternalApplicationEvidence {
    application: CheckedExternalCallableApplication,
    catalog: Arc<super::DefinitionCatalog>,
}

#[derive(Clone, etas_core::RetainedSize)]
enum DefinitionEnvironment {
    Native(Arc<FrozenDefinitionEnvironment>),
    Metadata(Arc<etas_code::report::DependencyInfo>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
enum DefinitionProvenance {
    Source,
    Metadata,
    Draft,
    Checked { snapshot: CodeFingerprint },
}

impl fmt::Debug for NativeDefinition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeDefinition")
            .field("identity", &self.data.base().identity())
            .field("revision", self.data.revision())
            .finish_non_exhaustive()
    }
}

impl DefinitionLease for NativeDefinition {
    fn identity(&self) -> &etas_code::DefinitionKey {
        self.data.base().identity()
    }
    fn revision(&self) -> &etas_code::RevisionInfo {
        self.data.revision()
    }
}
