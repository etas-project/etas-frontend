mod adapters;
mod passes;
mod scheduler;

pub(crate) use passes::candidate_body_pipeline;
#[cfg(test)]
pub(crate) use passes::project_pipeline;
pub(crate) use scheduler::run_controlled_context_pipeline;
pub(crate) use scheduler::{FrontendPipelineRequest, FrontendPipelineRun, run_check_pipeline};
