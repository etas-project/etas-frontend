use crate::{ProjectContext, ProjectOutput};

pub(crate) fn project_output_from_context(context: ProjectContext) -> ProjectOutput {
    context.into_project_output()
}
