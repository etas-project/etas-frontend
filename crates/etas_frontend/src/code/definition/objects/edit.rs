use etas_code::{
    definition::{CodeDefinitionOperations, DefinitionOperationError, DefinitionRef},
    syntax::SyntaxBlockRef,
};

use super::{NativeDefinition, NativeDefinitionStore};

impl CodeDefinitionOperations for NativeDefinitionStore {
    fn with_body(
        &self,
        definition: &DefinitionRef,
        body: SyntaxBlockRef,
    ) -> Result<DefinitionRef, DefinitionOperationError> {
        self.with_body_impl(definition, body, None)
    }
}

impl NativeDefinitionStore {
    pub fn with_body_controlled(
        &self,
        definition: &DefinitionRef,
        body: SyntaxBlockRef,
        control: &dyn etas_utils::PipelineExecutionControl,
    ) -> Result<DefinitionRef, DefinitionOperationError> {
        self.with_body_impl(definition, body, Some(control))
    }

    fn with_body_impl(
        &self,
        definition: &DefinitionRef,
        body: SyntaxBlockRef,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<DefinitionRef, DefinitionOperationError> {
        let checkpoint = || -> Result<(), DefinitionOperationError> {
            if let Some(control) = control {
                control.checkpoint().map_err(|error| {
                    DefinitionOperationError::Syntax(
                        etas_code::syntax::SyntaxOperationError::Interrupted(error),
                    )
                })?;
            }
            Ok(())
        };
        checkpoint()?;
        let original = self.resolve(definition)?;
        self.syntax
            .resolve(&body)
            .map_err(DefinitionOperationError::Syntax)?;
        let data = original.0.data.with_body(body);
        let method_snapshots = original
            .revised_method_snapshots(data.revision(), control)
            .map_err(|error| match error {
                crate::code::descriptor::ProjectionError::Syntax(error) => {
                    DefinitionOperationError::Syntax(error)
                }
                error => DefinitionOperationError::BackendUnavailable {
                    reason: error.to_string(),
                },
            })?;
        checkpoint()?;
        self.publish(NativeDefinition {
            data,
            method_snapshots,
            environment: original.0.environment.clone(),
            auxiliary: original.0.auxiliary.after_body_edit(),
            analysis_configuration: original.0.analysis_configuration,
            analysis_graph: original.0.analysis_graph.clone(),
            reflection_body_access: original.0.reflection_body_access,
            reflection_access: original.0.reflection_access,
            provenance: super::DefinitionProvenance::Draft,
            implementation: None,
        })
    }
    pub fn body_overlay(
        &self,
        definition: &DefinitionRef,
    ) -> Result<crate::code::draft::DefaultBodyOverlay, super::NativeDefinitionError> {
        self.resolve(definition)?.body_overlay(&self.syntax)
    }
}
