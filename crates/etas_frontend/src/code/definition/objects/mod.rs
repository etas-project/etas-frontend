mod bindings;
mod checked;
mod diff;
mod edit;
mod external;
mod reflect;
mod source;
mod store;
mod view;

use std::{collections::BTreeMap, fmt, sync::Arc};

use etas_code::{
    CallableId, CodeFingerprint,
    definition::{
        AgentDefinition, AgentDefinitionLease, DefinitionAnalysisState, DefinitionError,
        DefinitionOperationError,
    },
    descriptor::CallableAnalysisInfo,
};

use super::{FrozenAgentContract, FrozenDefinitionEnvironment};

pub use bindings::{FrozenReflectionBindings, NativeCallableReflection, NativeSourceReflection};
pub use store::NativeDefinitionStore;
use store::SourceReflectionKey;
pub use view::{NativeAgentDefinitionView, NativeAgentMethodView};

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
    MissingBodySource,
    SourceBundle(etas_code::definition::SourceBundleError),
    Overlay(crate::code::draft::OverlayError),
}

impl fmt::Display for NativeDefinitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "native code definition operation failed: {self:?}")
    }
}
impl std::error::Error for NativeDefinitionError {}
impl From<DefinitionOperationError> for NativeDefinitionError {
    fn from(error: DefinitionOperationError) -> Self {
        Self::Operation(error)
    }
}

#[derive(etas_core::RetainedSize)]
struct NativeAgentDefinition {
    data: AgentDefinition<FrozenAgentContract, CallableAnalysisInfo>,
    environment: DefinitionEnvironment,
    auxiliary: DefinitionAnalysisState<BTreeMap<CallableId, CallableAnalysisInfo>>,
    analysis_configuration: CodeFingerprint,
    analysis_graph: etas_code::descriptor::DescriptorGraph,
    reflection_body_access: bool,
    reflection_access: Option<etas_hir::HirModuleId>,
    provenance: DefinitionProvenance,
    implementation: Option<Arc<crate::code::service::CheckedCandidate>>,
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

impl fmt::Debug for NativeAgentDefinition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeAgentDefinition")
            .field("identity", &self.data.base().identity())
            .field("revision", self.data.revision())
            .finish_non_exhaustive()
    }
}

impl AgentDefinitionLease for NativeAgentDefinition {
    fn identity(&self) -> &etas_code::DefinitionKey {
        self.data.base().identity()
    }
    fn revision(&self) -> &etas_code::RevisionInfo {
        self.data.revision()
    }
}
