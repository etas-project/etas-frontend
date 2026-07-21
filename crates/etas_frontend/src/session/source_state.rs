use crate::{ProjectInput, SourceFile, SourceInput, SourceSet};

pub(super) fn source_set_from_input(input: &ProjectInput) -> SourceSet {
    SourceSet {
        project_root: input.project_root.clone(),
        source_root: crate::project::source_root_for_project_input(input),
        environment_fingerprint: input.environment.canonical_environment_fingerprint(),
        external_modules_fingerprint: input.environment.canonical_external_modules_fingerprint(),
        files: input.sources.iter().map(source_file_from_input).collect(),
    }
}

pub(super) fn source_file_from_input(input: &SourceInput) -> SourceFile {
    SourceFile::from_input(input)
}
