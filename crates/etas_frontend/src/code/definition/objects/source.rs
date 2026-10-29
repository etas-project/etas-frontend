use etas_code::{
    definition::{
        BaseRevisionInput, DEFINITION_SOURCE_BUNDLE_SCHEMA, DefinitionAnalysisState,
        DefinitionOperationError, DefinitionRef, DefinitionSnapshot, DefinitionSourceBundle,
        SourceBundleError, SourceBundleLimits,
    },
    syntax::SyntaxOperationError,
};

use super::{NativeDefinition, NativeDefinitionError, NativeDefinitionStore};

impl NativeDefinitionStore {
    /// Exports only source already available through this definition's view.
    /// It never obtains a hidden body from the compiler's private environment.
    pub fn save_source(
        &self,
        definition: &DefinitionRef,
        limits: SourceBundleLimits,
    ) -> Result<DefinitionSourceBundle, NativeDefinitionError> {
        let view = self.resolve(definition)?;
        view.environment()?;
        let body = view
            .body()
            .ok_or(NativeDefinitionError::MissingBodySource)?;
        let source = self
            .syntax
            .export_body_source_bounded(body, limits.max_source_bytes)
            .map_err(|error| {
                NativeDefinitionError::Operation(DefinitionOperationError::Syntax(error))
            })?;
        let bundle = DefinitionSourceBundle {
            schema: DEFINITION_SOURCE_BUNDLE_SCHEMA,
            target: view.identity().clone(),
            revision_key: view.revision().key(),
            revision: view.revision().inputs().clone(),
            source,
            dependencies: view
                .dependency_info()
                .map_err(NativeDefinitionError::Projection)?,
        };
        bundle
            .validate(limits)
            .map_err(NativeDefinitionError::SourceBundle)?;
        Ok(bundle)
    }

    pub fn restore_source_draft(
        &self,
        expected: &DefinitionRef,
        source: &DefinitionSourceBundle,
        limits: SourceBundleLimits,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        self.restore_source_draft_impl(expected, source, limits, None)
    }

    pub fn restore_source_draft_controlled(
        &self,
        expected: &DefinitionRef,
        source: &DefinitionSourceBundle,
        limits: SourceBundleLimits,
        control: &dyn etas_utils::PipelineExecutionControl,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        self.restore_source_draft_impl(expected, source, limits, Some(control))
    }

    fn restore_source_draft_impl(
        &self,
        expected: &DefinitionRef,
        source: &DefinitionSourceBundle,
        limits: SourceBundleLimits,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let checkpoint = || -> Result<(), NativeDefinitionError> {
            if let Some(control) = control {
                control.checkpoint().map_err(|error| {
                    NativeDefinitionError::Operation(DefinitionOperationError::Syntax(
                        SyntaxOperationError::Interrupted(error),
                    ))
                })?;
            }
            Ok(())
        };
        checkpoint()?;
        source
            .validate(limits)
            .map_err(NativeDefinitionError::SourceBundle)?;
        let original = self.resolve(expected)?;
        let environment = original.environment()?;
        let base = original.0.data.base();
        let fail = NativeDefinitionError::SourceBundle;
        if original.identity() != &source.target
            || base.default_callable() != Some(&source.revision.definition)
        {
            return Err(fail(SourceBundleError::TargetMismatch));
        }
        if base.contract_fingerprint() != source.revision.contract {
            return Err(fail(SourceBundleError::ContractMismatch));
        }
        if environment.compilation_fingerprint() != source.revision.compilation {
            return Err(fail(SourceBundleError::ConfigurationMismatch));
        }
        if base.environment() != source.revision.environment {
            return Err(fail(SourceBundleError::EnvironmentMismatch));
        }
        if original
            .dependency_info()
            .map_err(NativeDefinitionError::Projection)?
            != source.dependencies
        {
            return Err(fail(SourceBundleError::DependencyMismatch));
        }
        checkpoint()?;
        let body = match control {
            Some(control) => self.syntax.import_retained_body_controlled(
                &source.revision.definition,
                &source.source,
                control,
            ),
            None => self
                .syntax
                .import_retained_body(&source.revision.definition, &source.source),
        }
        .map_err(|error| {
            NativeDefinitionError::Operation(DefinitionOperationError::Syntax(error))
        })?;
        let data = DefinitionSnapshot::from_base(
            original.0.data.shared_base(),
            BaseRevisionInput {
                body: Some(body),
                content: source.revision.content,
                parent: source.revision.parent,
                analysis_configuration: original.0.analysis_configuration,
                summaries: None,
            },
        )
        .map_err(NativeDefinitionError::Definition)?;
        if data.revision().inputs() != &source.revision {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        }
        checkpoint()?;
        let method_snapshots = original
            .revised_method_snapshots(data.revision(), control)
            .map_err(NativeDefinitionError::Projection)?;
        checkpoint()?;
        self.publish(NativeDefinition {
            data,
            method_snapshots,
            environment: original.0.environment.clone(),
            auxiliary: DefinitionAnalysisState::Unavailable {
                reason: etas_code::definition::AnalysisUnavailableReason::NotAnalyzed,
            },
            analysis_configuration: original.0.analysis_configuration,
            analysis_graph: original.0.analysis_graph.clone(),
            reflection_body_access: original.0.reflection_body_access,
            reflection_access: original.0.reflection_access,
            provenance: super::DefinitionProvenance::Draft,
            implementation: None,
        })
        .map_err(Into::into)
    }
}
