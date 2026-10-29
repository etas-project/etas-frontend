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
        let original = self.resolve(definition)?;
        self.syntax
            .resolve(&body)
            .map_err(DefinitionOperationError::Syntax)?;
        let data = original.0.data.with_body(body);
        let method_snapshots =
            original
                .revised_method_snapshots(data.revision())
                .map_err(|error| DefinitionOperationError::BackendUnavailable {
                    reason: error.to_string(),
                })?;
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
}

impl NativeDefinitionStore {
    pub fn body_overlay(
        &self,
        definition: &DefinitionRef,
    ) -> Result<crate::code::draft::DefaultBodyOverlay, super::NativeDefinitionError> {
        self.resolve(definition)?.body_overlay(&self.syntax)
    }
}
