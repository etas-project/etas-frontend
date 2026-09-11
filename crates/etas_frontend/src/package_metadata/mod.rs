use std::path::Path;

use etas_package_metadata::{
    ARTIFACT_SCHEMA_VERSION, EncodedMetadataSection, blake3_hash, encode_metadata_artifact,
    write_metadata_artifact_file,
};

use crate::{CheckedProject, ProjectOutput};

mod error;
mod input;
mod projection;
mod type_graph;

use projection::ProjectMetadataProjection;

pub use error::PackageMetadataError;
pub use input::{
    PackageMetadataArtifact, PackageMetadataBinInput, PackageMetadataBuildInput,
    PackageMetadataDependencyInput, PackageMetadataHeader, PackageMetadataToolBindingInput,
};

const CREATED_TARGET: &str = "etas-frontend";

pub fn build_package_metadata_artifact(
    input: PackageMetadataBuildInput,
    output: &ProjectOutput,
) -> Result<PackageMetadataArtifact, PackageMetadataError> {
    let checked = output
        .checked
        .as_ref()
        .ok_or(PackageMetadataError::UncheckedProject)?;
    build_package_metadata_artifact_from_checked(input, checked)
}

pub fn build_package_metadata_artifact_from_checked(
    input: PackageMetadataBuildInput,
    checked: &CheckedProject,
) -> Result<PackageMetadataArtifact, PackageMetadataError> {
    let header = PackageMetadataHeader {
        artifact_schema_version: ARTIFACT_SCHEMA_VERSION,
        compiler_version: input.compiler_version.clone(),
        package_id: input.package_id.clone(),
        package_version: input.package_version.clone(),
        source_payload_hash: input.source_payload_hash.clone(),
        manifest_hash: input.manifest_hash.clone(),
        dependency_lock_hash: input.dependency_lock_hash.clone(),
        created_target: CREATED_TARGET.to_owned(),
    };
    let sections = ProjectMetadataProjection::new(checked).build_sections(&input, &header)?;
    let bytes = encode_artifact(&header, sections)?;
    let artifact_hash = blake3_hash(&bytes);
    Ok(PackageMetadataArtifact {
        header,
        bytes,
        artifact_hash,
    })
}

pub fn emit_package_metadata_artifact(
    path: impl AsRef<Path>,
    input: PackageMetadataBuildInput,
    output: &ProjectOutput,
) -> Result<PackageMetadataArtifact, PackageMetadataError> {
    let artifact = build_package_metadata_artifact(input, output)?;
    write_metadata_artifact_file(path.as_ref(), &artifact.bytes)?;
    Ok(artifact)
}

fn encode_artifact(
    header: &PackageMetadataHeader,
    sections: Vec<EncodedMetadataSection>,
) -> Result<Vec<u8>, PackageMetadataError> {
    Ok(encode_metadata_artifact(header, sections)?)
}
