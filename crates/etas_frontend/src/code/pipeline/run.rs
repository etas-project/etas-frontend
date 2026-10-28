use super::CandidatePipelineContext;
use crate::code::{definition::FrozenDefinitionEnvironment, draft::DefaultBodyOverlay};
use crate::{
    CheckScope, FrontendMemoryProbe, PassMemorySample, ProjectContext, ProjectInput, ProjectOutput,
};
use etas_code::{CodeFingerprint, RevisionInfo};
use etas_utils::{ArtifactKey, PassControl, PassRunRecord};
use std::sync::Arc;

#[derive(Debug)]
pub enum CandidateAnalysisError {
    Interrupted(etas_utils::PipelineInterruption),
    Pipeline {
        reason: String,
        missing_artifact: Option<ArtifactKey>,
    },
    StoppedWithoutDiagnostic,
}

/// Normal compiler analysis of a body replacement. It is not a compile permit:
/// frozen behavioral compatibility and backend publication are separate stages.
pub struct CandidateBodyAnalysis {
    frozen: Arc<FrozenDefinitionEnvironment>,
    input: CodeFingerprint,
    output: ProjectOutput,
    records: Vec<PassRunRecord>,
    memory_samples: Vec<PassMemorySample>,
    contract: Option<crate::code::contract::CandidateContractAnalysis>,
    revision: RevisionInfo,
    source: etas_core::SourceId,
    syntax_identity: etas_code::syntax::SyntaxIdentity,
    emitted: Option<crate::code::service::CheckedCandidate>,
    descriptors: Option<super::passes::CandidateDescriptors>,
}

impl CandidateBodyAnalysis {
    pub fn frozen(&self) -> &Arc<FrozenDefinitionEnvironment> {
        &self.frozen
    }
    pub fn fingerprint(&self) -> CodeFingerprint {
        self.input
    }
    pub fn output(&self) -> &ProjectOutput {
        &self.output
    }
    pub fn pipeline_records(&self) -> &[PassRunRecord] {
        &self.records
    }
    pub fn memory_samples(&self) -> &[PassMemorySample] {
        &self.memory_samples
    }
    pub fn contract_analysis(&self) -> Option<&crate::code::contract::CandidateContractAnalysis> {
        self.contract.as_ref()
    }
    pub fn revision(&self) -> &RevisionInfo {
        &self.revision
    }
    pub(crate) fn take_emitted(&mut self) -> Option<crate::code::service::CheckedCandidate> {
        self.emitted.take()
    }
    pub(crate) fn validate_emission(&self) -> Result<bool, crate::code::service::CheckReportError> {
        use crate::code::service::CheckReportError as Error;
        let Some(emitted) = &self.emitted else {
            return Ok(false);
        };
        let analysis = self.contract.as_ref().ok_or(Error::IncompleteAnalysis(
            "emitted candidate has no contract artifact",
        ))?;
        let proven = analysis
            .validate_for(self.input, self.frozen.contract())
            .map_err(|error| Error::InvalidContract(error.to_string()))?;
        let checked = self
            .output
            .checked
            .as_ref()
            .ok_or(Error::IncompleteAnalysis(
                "emitted candidate has no checked program",
            ))?;
        let catalog = self
            .descriptors
            .as_ref()
            .and_then(|artifact| artifact.catalog.as_ref())
            .ok_or(Error::IncompleteAnalysis(
                "emitted candidate has no descriptor artifact",
            ))?;
        if !proven
            || !Arc::ptr_eq(&emitted.descriptors, catalog)
            || self
                .descriptors
                .as_ref()
                .is_none_or(|artifact| emitted.descriptor_limits != artifact.limits)
            || !Arc::ptr_eq(&emitted.program, checked)
            || !Arc::ptr_eq(&emitted.frozen, &self.frozen)
            || emitted.revision != self.revision
            || emitted.snapshot != self.input
            || Some(emitted.entry) != checked.entry
            || self
                .output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error)
        {
            return Err(Error::IncompleteAnalysis(
                "candidate emission does not match its validated native inputs",
            ));
        }
        Ok(true)
    }
    pub(crate) fn candidate_source(
        &self,
    ) -> (etas_core::SourceId, etas_code::syntax::SyntaxIdentity) {
        (self.source, self.syntax_identity)
    }
    pub fn definition_catalog(
        &self,
        limits: etas_code::descriptor::DescriptorLimits,
    ) -> Result<
        Option<Arc<crate::code::definition::DefinitionCatalog>>,
        crate::code::descriptor::ProjectionError,
    > {
        let Some(checked) = self.output.checked.as_ref() else {
            if self
                .descriptors
                .as_ref()
                .is_some_and(|artifact| artifact.catalog.is_some())
            {
                return Err(crate::code::descriptor::ProjectionError::Invalid(
                    "rejected source retained a checked descriptor catalog".into(),
                ));
            }
            return Ok(None);
        };
        let artifact = self.descriptors.as_ref().ok_or_else(|| {
            crate::code::descriptor::ProjectionError::Invalid(
                "checked candidate has no descriptor artifact".into(),
            )
        })?;
        let catalog = artifact
            .catalog
            .as_ref()
            .filter(|catalog| Arc::ptr_eq(catalog.checked_program(), checked))
            .ok_or_else(|| {
                crate::code::descriptor::ProjectionError::Invalid(
                    "candidate descriptor artifact has a different checked program".into(),
                )
            })?;
        if limits == artifact.limits {
            return Ok(Some(catalog.clone()));
        }
        // A new caller budget requires its own bounded projection, not a
        // replacement for a missing or foreign pipeline artifact.
        let origins = self.frozen.origins_for(checked)?;
        crate::code::definition::DefinitionCatalog::build(checked.clone(), &origins, limits)
            .map(|catalog| Some(Arc::new(catalog)))
    }
}

pub(crate) fn analyze_body(
    overlay: DefaultBodyOverlay,
    memory_probe: Option<FrontendMemoryProbe>,
) -> Result<CandidateBodyAnalysis, CandidateAnalysisError> {
    analyze_controlled_body(overlay, memory_probe, None)
}

pub(crate) fn analyze_controlled_body(
    overlay: DefaultBodyOverlay,
    memory_probe: Option<FrontendMemoryProbe>,
    control: Option<Arc<dyn etas_utils::PipelineExecutionControl>>,
) -> Result<CandidateBodyAnalysis, CandidateAnalysisError> {
    analyze_body_with_dependencies(
        overlay,
        Vec::new(),
        Default::default(),
        memory_probe,
        control,
    )
}

pub(crate) fn analyze_body_with_dependencies(
    overlay: DefaultBodyOverlay,
    dependency_sources: Vec<crate::SourceInput>,
    descriptor_limits: etas_code::descriptor::DescriptorLimits,
    memory_probe: Option<FrontendMemoryProbe>,
    control: Option<Arc<dyn etas_utils::PipelineExecutionControl>>,
) -> Result<CandidateBodyAnalysis, CandidateAnalysisError> {
    let mut baseline_requirements = None;
    let baseline = if !dependency_sources.is_empty() && super::baseline::needs_completion(&overlay)
    {
        let original = super::baseline::original_overlay(&overlay)?;
        let analysis = analyze_body_inputs(
            original.clone(),
            dependency_sources.clone(),
            None,
            descriptor_limits,
            memory_probe,
            control.clone(),
        )?;
        baseline_requirements = analysis.output.runtime_source_requirements.clone();
        match &analysis.output.checked {
            Some(checked) => Some(super::baseline::CompletedBaseline::verify(
                &original,
                checked,
                analysis
                    .definition_catalog(descriptor_limits)
                    .map_err(|error| CandidateAnalysisError::Pipeline {
                        reason: error.to_string(),
                        missing_artifact: Some(super::passes::CANDIDATE_DESCRIPTORS),
                    })?
                    .ok_or_else(|| CandidateAnalysisError::Pipeline {
                        reason: "checked original baseline has no descriptor artifact".into(),
                        missing_artifact: Some(super::passes::CANDIDATE_DESCRIPTORS),
                    })?,
            )?),
            None => return Err(CandidateAnalysisError::Pipeline {
                reason:
                    "original frozen body could not be checked with the locked dependency inputs"
                        .into(),
                missing_artifact: None,
            }),
        }
    } else {
        None
    };
    let mut analysis = analyze_body_inputs(
        overlay,
        dependency_sources,
        baseline,
        descriptor_limits,
        memory_probe,
        control,
    )?;
    if let Some(requirements) = baseline_requirements {
        let combined = analysis
            .output
            .runtime_source_requirements
            .get_or_insert_with(Default::default);
        for (package, requirement) in requirements.dependencies {
            match combined.dependencies.entry(package) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(requirement);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let target = entry.get_mut();
                    if target.import_root != requirement.import_root {
                        return Err(CandidateAnalysisError::Pipeline {
                            reason: "baseline and candidate dependency bindings disagree".into(),
                            missing_artifact: None,
                        });
                    }
                    target.seed_modules.extend(requirement.seed_modules);
                    target.required_modules.extend(requirement.required_modules);
                    target.reasons.extend(requirement.reasons);
                }
            }
        }
    }
    Ok(analysis)
}

fn analyze_body_inputs(
    overlay: DefaultBodyOverlay,
    dependency_sources: Vec<crate::SourceInput>,
    baseline: Option<super::baseline::CompletedBaseline>,
    descriptor_limits: etas_code::descriptor::DescriptorLimits,
    memory_probe: Option<FrontendMemoryProbe>,
    control: Option<Arc<dyn etas_utils::PipelineExecutionControl>>,
) -> Result<CandidateBodyAnalysis, CandidateAnalysisError> {
    let frozen = overlay.frozen().clone();
    let checked = frozen.checked();
    let sources = checked.definition_sources.source_set();
    let input = ProjectInput {
        project_root: sources.project_root.clone(),
        source_root: Some(sources.source_root.clone()),
        options: checked.compile_options.clone(),
        environment: checked.definition_environment.environment().clone(),
        sources: Vec::new(),
        entry: checked.entry_fact.requested.clone(),
    };
    let mut context = ProjectContext::new_with_std_registry(input, checked.std_registry.clone())
        .with_check_scope(CheckScope::EntryReachable);
    context.incremental = true;
    context.changed_sources = vec![overlay.slot().source];
    let syntax_identity = overlay.body().identity();
    let source = overlay.relocated().source().id;
    let revision = overlay.revision().clone();
    let mut candidate = CandidatePipelineContext::new(overlay);
    candidate.descriptor_limits = descriptor_limits;
    candidate.baseline = baseline;
    candidate
        .set_dependency_sources(dependency_sources)
        .map_err(|reason| CandidateAnalysisError::Pipeline {
            reason,
            missing_artifact: None,
        })?;
    let fingerprint = candidate.snapshot;
    context.candidate = Some(candidate);
    let run = crate::pipeline::run_controlled_context_pipeline(
        context,
        crate::pipeline::candidate_body_pipeline(),
        true,
        memory_probe,
        control,
    );
    let completed = matches!(run.control, PassControl::Continue);
    match run.control {
        PassControl::Failed(failure) => {
            if let Some(interruption) = failure.interruption {
                return Err(CandidateAnalysisError::Interrupted(interruption));
            }
            return Err(CandidateAnalysisError::Pipeline {
                reason: failure.message,
                missing_artifact: failure.missing_artifact,
            });
        }
        PassControl::Stop if run.output.diagnostics.is_empty() => {
            return Err(CandidateAnalysisError::StoppedWithoutDiagnostic);
        }
        PassControl::Continue | PassControl::Stop => {}
    }
    if completed && run.candidate_contract.is_none() {
        return Err(CandidateAnalysisError::Pipeline {
            reason: "candidate pipeline omitted contract analysis".into(),
            missing_artifact: Some(crate::code::pipeline::passes::CANDIDATE_CONTRACT_ANALYSIS),
        });
    }
    if completed && run.candidate_descriptors.is_none() {
        return Err(CandidateAnalysisError::Pipeline {
            reason: "candidate pipeline omitted definition descriptors".into(),
            missing_artifact: Some(crate::code::pipeline::passes::CANDIDATE_DESCRIPTORS),
        });
    }
    if completed && let Some(analysis) = &run.candidate_contract {
        let proven = analysis
            .validate_for(fingerprint, frozen.contract())
            .map_err(|error| CandidateAnalysisError::Pipeline {
                reason: error.to_string(),
                missing_artifact: None,
            })?;
        if run.candidate_emission.is_none() {
            return Err(CandidateAnalysisError::Pipeline {
                reason: "candidate pipeline omitted native emission".into(),
                missing_artifact: Some(crate::code::pipeline::passes::CANDIDATE_EMISSION),
            });
        }
        if proven
            != matches!(
                run.candidate_emission,
                Some(crate::code::service::CandidateEmission::Ready(_))
            )
        {
            return Err(CandidateAnalysisError::Pipeline {
                reason: "candidate emission contradicts its frozen contract result".into(),
                missing_artifact: None,
            });
        }
    }
    Ok(CandidateBodyAnalysis {
        frozen,
        input: fingerprint,
        output: run.output,
        records: run.records,
        memory_samples: run.memory_samples,
        contract: run.candidate_contract,
        revision,
        descriptors: run.candidate_descriptors,
        source,
        syntax_identity,
        emitted: match run.candidate_emission {
            Some(crate::code::service::CandidateEmission::Ready(candidate)) => Some(*candidate),
            Some(crate::code::service::CandidateEmission::Rejected) | None => None,
        },
    })
}

#[cfg(test)]
mod tests;
