mod analysis;
mod effects;
mod handler_scopes;
mod input;
mod outcomes;
mod project;
mod specs;
mod standard;
mod types;

use etas_hir::{HirModuleId, SymbolId};
use etas_types::TypeId;

pub(crate) use input::DescriptorInput;
pub use project::{
    DefinitionDescriptorProjector, NativeDescriptorProjection, NativeTypeOrigin, ProjectedSignature,
};
pub(crate) use specs::AttachedSpecIndex;

#[derive(Debug)]
pub enum ProjectionError {
    MissingPackage(HirModuleId),
    MissingModule(HirModuleId),
    MissingSymbol(SymbolId),
    MissingDeclaration(SymbolId),
    MissingType(TypeId),
    MissingTypeReference(etas_hir::HirTypeId),
    MissingReflection(etas_hir::HirExprId),
    MissingCallable(SymbolId),
    RuntimeRevisionRequired,
    InaccessibleDefinition,
    UnresolvedType(TypeId),
    UnresolvedBinder(String),
    MissingIdentity(String),
    Invalid(String),
    InvalidAnnotation {
        span: etas_core::Span,
        reason: String,
    },
    Limit {
        resource: &'static str,
        limit: usize,
    },
    Identity(etas_code::IdentityError),
    Descriptor(etas_code::descriptor::DescriptorError),
    Syntax(etas_code::syntax::SyntaxOperationError),
}

impl ProjectionError {
    pub(crate) fn resource_limit(&self) -> Option<(&'static str, usize)> {
        match self {
            Self::Limit { resource, limit }
            | Self::Descriptor(etas_code::descriptor::DescriptorError::Limit { resource, limit }) => {
                Some((*resource, *limit))
            }
            _ => None,
        }
    }
}

impl std::fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "checked definition projection failed: {self:?}")
    }
}
impl std::error::Error for ProjectionError {}
impl From<etas_code::IdentityError> for ProjectionError {
    fn from(error: etas_code::IdentityError) -> Self {
        Self::Identity(error)
    }
}
impl From<etas_code::descriptor::DescriptorError> for ProjectionError {
    fn from(error: etas_code::descriptor::DescriptorError) -> Self {
        Self::Descriptor(error)
    }
}
