use std::path::PathBuf;

use etas_package_metadata::MetadataArtifactError;
use etas_types::TypeId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageMetadataError {
    UncheckedProject,
    MissingExportSignature { path: Vec<String> },
    InvalidToolSchema { path: Vec<String>, reason: String },
    UnsupportedType { ty: TypeId, reason: String },
    InvalidMetadataType { reason: String },
    UnresolvedEffectTag { id: u32 },
    UnresolvedEffectAction { tag: u32, action: u32 },
    UnresolvedEffectPath { path: Vec<String> },
    Io { path: PathBuf, message: String },
    Artifact(String),
}

impl std::fmt::Display for PackageMetadataError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UncheckedProject => formatter.write_str(
                "package metadata artifact requires a checked project; no metadata was emitted",
            ),
            Self::MissingExportSignature { path } => write!(
                formatter,
                "public export `{}` is missing checked signature facts",
                path.join(".")
            ),
            Self::InvalidToolSchema { path, reason } => write!(
                formatter,
                "public tool `{}` cannot be emitted in package metadata: {reason}",
                path.join(".")
            ),
            Self::UnsupportedType { ty, reason } => {
                write!(
                    formatter,
                    "type {ty:?} cannot be emitted in package metadata: {reason}"
                )
            }
            Self::InvalidMetadataType { reason } => {
                write!(formatter, "package metadata type is invalid: {reason}")
            }
            Self::UnresolvedEffectTag { id } => write!(
                formatter,
                "effect tag {id} cannot be emitted in package metadata without a canonical name"
            ),
            Self::UnresolvedEffectAction { tag, action } => write!(
                formatter,
                "effect action {tag}.{action} cannot be emitted in package metadata without a canonical name"
            ),
            Self::UnresolvedEffectPath { path } => write!(
                formatter,
                "effect path `{}` cannot be emitted in package metadata without a canonical name",
                path.join(".")
            ),
            Self::Io { path, message } => write!(formatter, "{}: {message}", path.display()),
            Self::Artifact(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for PackageMetadataError {}

impl From<MetadataArtifactError> for PackageMetadataError {
    fn from(error: MetadataArtifactError) -> Self {
        Self::Artifact(error.to_string())
    }
}
