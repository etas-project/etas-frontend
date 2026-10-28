use etas_code::{
    definition::{AgentDefinitionRef, CodeDefinitionOperations, DefinitionOperationError},
    syntax::SyntaxBlockRef,
};

use super::{NativeAgentDefinition, NativeDefinitionStore};

impl CodeDefinitionOperations for NativeDefinitionStore {
    fn with_body(
        &self,
        definition: &AgentDefinitionRef,
        body: SyntaxBlockRef,
    ) -> Result<AgentDefinitionRef, DefinitionOperationError> {
        let original = self.resolve(definition)?;
        self.syntax
            .resolve(&body)
            .map_err(DefinitionOperationError::Syntax)?;
        self.publish(NativeAgentDefinition {
            data: original.0.data.with_body(body),
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
        definition: &AgentDefinitionRef,
    ) -> Result<crate::code::draft::DefaultBodyOverlay, super::NativeDefinitionError> {
        self.resolve(definition)?.body_overlay(&self.syntax)
    }
}
