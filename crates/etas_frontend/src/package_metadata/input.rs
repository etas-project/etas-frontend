use etas_package_metadata::{
    MetadataArtifactHeader, ResolvedDependency, ToolBinding as MetadataToolBinding,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageMetadataBuildInput {
    pub package_id: String,
    pub package_version: String,
    pub package_edition: String,
    pub compiler_version: String,
    pub source_payload_hash: String,
    pub manifest_hash: String,
    pub dependency_lock_hash: String,
    pub bins: Vec<PackageMetadataBinInput>,
    pub dependencies: Vec<PackageMetadataDependencyInput>,
    pub tool_bindings: Vec<PackageMetadataToolBindingInput>,
}

pub type PackageMetadataBinInput = etas_package_metadata::BinTarget;
pub type PackageMetadataDependencyInput = ResolvedDependency;
pub type PackageMetadataToolBindingInput = MetadataToolBinding;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageMetadataArtifact {
    pub header: PackageMetadataHeader,
    pub bytes: Vec<u8>,
    pub artifact_hash: String,
}

pub type PackageMetadataHeader = MetadataArtifactHeader;
