use etas_cache::{ArtifactFingerprint, ArtifactMeta, ProjectRevision};

use super::{FRONTEND_ARTIFACT_SCHEMA_VERSION, fingerprint_text};

pub const FRONTEND_COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) fn frontend_std_version() -> String {
    let version = etas_std::standard_registry().version().contract.clone();
    format!("etas_std:{version}")
}

pub(crate) fn changed_compiler_options_hash(
    current: &str,
    revision: ProjectRevision,
    changed_options: &[String],
) -> String {
    let mut options = changed_options.to_vec();
    options.sort();
    options.dedup();

    let revision = revision.0.to_string();
    let mut parts = vec![
        "frontend-options",
        "v1",
        "previous",
        current,
        "revision",
        revision.as_str(),
    ];
    parts.extend(options.iter().map(String::as_str));
    fingerprint_text(&parts).to_string()
}

pub(crate) fn frontend_artifact_meta(
    revision: ProjectRevision,
    fingerprint: ArtifactFingerprint,
    std_version: &str,
    options_hash: &str,
) -> ArtifactMeta {
    ArtifactMeta::new(
        revision,
        fingerprint,
        FRONTEND_COMPILER_VERSION,
        FRONTEND_ARTIFACT_SCHEMA_VERSION,
    )
    .with_std_version(std_version)
    .with_options_hash(options_hash)
}
