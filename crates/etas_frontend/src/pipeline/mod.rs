mod adapters;
mod passes;
mod scheduler;

#[cfg(test)]
pub(crate) use passes::project_pipeline;
pub(crate) use scheduler::{FrontendPipelineRun, run_check_pipeline};
