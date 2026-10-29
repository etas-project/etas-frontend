use etas_code::descriptor::DefinitionSnapshotInfo;

use crate::code::descriptor::ProjectionError;

impl super::DefinitionCatalog {
    pub(super) fn source_method_snapshots(
        &self,
        environment: &super::FrozenDefinitionEnvironment,
        syntax: &crate::code::service::NativeCodeSyntaxOperations,
    ) -> Result<Vec<DefinitionSnapshotInfo>, ProjectionError> {
        let owner = DefinitionSnapshotInfo {
            identity: environment.identity().clone(),
            revision: self.source_revision(environment, syntax)?,
            dependencies: environment.dependency_info()?,
        };
        environment
            .contract()
            .callables()
            .map(|(id, _)| {
                owner
                    .method_snapshot(id, self.body_content(id, syntax)?)
                    .map_err(ProjectionError::from)
            })
            .collect()
    }
}
