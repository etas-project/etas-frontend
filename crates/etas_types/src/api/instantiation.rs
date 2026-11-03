use crate::{BodyTypeOutput, CallableGenericArg, CallableSignature};

/// A monomorphic body result is not an ordinary, universally checked body delta.
/// Keep its application attached so caches cannot silently erase the bindings.
#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct InstantiatedBodyOutput {
    pub(crate) body: BodyTypeOutput,
    pub(crate) arguments: Vec<CallableGenericArg>,
    pub(crate) signature: CallableSignature,
}

impl InstantiatedBodyOutput {
    pub fn owner(&self) -> etas_hir::HirItemId {
        self.body.owner
    }

    pub fn diagnostics(&self) -> &[etas_core::Diagnostic] {
        &self.body.diagnostics
    }

    pub fn facts(&self) -> &crate::TypeFacts {
        &self.body.facts
    }

    pub fn type_base(&self) -> crate::ty::TypeBaseId {
        self.body.base
    }

    pub fn base_len(&self) -> usize {
        self.body.base_len
    }

    pub fn local_types(&self) -> &[crate::Type] {
        &self.body.types
    }

    pub fn arguments(&self) -> &[CallableGenericArg] {
        &self.arguments
    }

    pub fn signature(&self) -> &CallableSignature {
        &self.signature
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodyInstantiationError {
    MissingSignature(etas_hir::HirItemId),
    InvalidSignature(etas_hir::HirItemId),
    UnsupportedOwner(etas_hir::HirItemId),
    ArgumentCount { expected: usize, actual: usize },
    ArgumentKind { position: usize },
    InvalidBinder { position: usize },
    MissingType(crate::TypeId),
    Substitution(crate::TypeSubstitutionError),
    Interrupted(etas_utils::PipelineInterruption),
}

impl std::fmt::Display for BodyInstantiationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSignature(item) => {
                write!(f, "bound body has no checked signature: {item:?}")
            }
            Self::InvalidSignature(item) => write!(
                f,
                "bound body signature disagrees with its declaration: {item:?}"
            ),
            Self::UnsupportedOwner(item) => {
                write!(f, "bound body has no ordinary-flow owner: {item:?}")
            }
            Self::ArgumentCount { expected, actual } => write!(
                f,
                "bound body expects {expected} generic arguments, got {actual}"
            ),
            Self::ArgumentKind { position } => write!(
                f,
                "bound body generic argument {position} has the wrong kind"
            ),
            Self::InvalidBinder { position } => write!(
                f,
                "bound body generic parameter {position} has no matching declaration identity"
            ),
            Self::MissingType(ty) => write!(
                f,
                "bound body argument is outside its signature type universe: {ty:?}"
            ),
            Self::Substitution(error) => write!(f, "bound body type substitution failed: {error}"),
            Self::Interrupted(interruption) => write!(
                f,
                "bound body checking interrupted: {}",
                interruption.reason
            ),
        }
    }
}

impl std::error::Error for BodyInstantiationError {}

impl From<crate::TypeSubstitutionError> for BodyInstantiationError {
    fn from(error: crate::TypeSubstitutionError) -> Self {
        Self::Substitution(error)
    }
}

impl InstantiatedBodyOutput {
    pub(crate) fn new(
        body: BodyTypeOutput,
        arguments: Vec<CallableGenericArg>,
        signature: CallableSignature,
    ) -> Self {
        Self {
            body,
            arguments,
            signature,
        }
    }
}
