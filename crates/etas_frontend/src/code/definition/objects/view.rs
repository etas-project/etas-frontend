use std::sync::Arc;

use etas_code::{
    CallableId, RevisionInfo,
    definition::{AnalysisFingerprint, AnalysisUnavailableReason},
    descriptor::{CallableAnalysisInfo, SignatureInfo},
};

use super::{NativeAgentDefinition, NativeDefinitionError};
use crate::code::{
    definition::FrozenDefinitionEnvironment, draft::DefaultBodyOverlay,
    service::NativeCodeSyntaxOperations,
};

#[derive(Clone, etas_core::RetainedSize)]
pub struct NativeAgentDefinitionView(pub(super) Arc<NativeAgentDefinition>);

pub struct NativeAgentMethodView {
    pub header: etas_code::descriptor::CallableHeaderInfo,
    pub signature_descriptors: etas_code::descriptor::DescriptorGraph,
    pub analysis: Option<(etas_code::descriptor::DescriptorGraph, CallableAnalysisInfo)>,
    pub body: Option<etas_code::syntax::SyntaxBlockRef>,
}

impl NativeAgentDefinitionView {
    pub fn identity(&self) -> &etas_code::DefinitionKey {
        self.0.data.base().identity()
    }
    pub fn revision(&self) -> &RevisionInfo {
        self.0.data.revision()
    }
    pub fn signature(&self) -> Option<&SignatureInfo> {
        self.0.data.base().signature()
    }
    pub fn body(&self) -> Option<&etas_code::syntax::SyntaxBlockRef> {
        self.0.data.body()
    }
    pub fn environment(&self) -> Result<&Arc<FrozenDefinitionEnvironment>, NativeDefinitionError> {
        match &self.0.environment {
            super::DefinitionEnvironment::Native(environment) => Ok(environment),
            super::DefinitionEnvironment::Metadata(_) => {
                Err(NativeDefinitionError::MissingSourceEnvironment)
            }
        }
    }
    pub fn dependency_info(
        &self,
    ) -> Result<etas_code::report::DependencyInfo, crate::code::descriptor::ProjectionError> {
        match &self.0.environment {
            super::DefinitionEnvironment::Native(environment) => environment.dependency_info(),
            super::DefinitionEnvironment::Metadata(dependencies) => {
                Ok(dependencies.as_ref().clone())
            }
        }
    }
    pub fn current_analysis(&self) -> Result<&CallableAnalysisInfo, AnalysisUnavailableReason> {
        self.0.data.current_analysis()
    }
    pub fn signature_descriptors(&self) -> &etas_code::descriptor::DescriptorGraph {
        self.0.data.base().descriptors()
    }
    pub fn declared_tools(&self) -> &[etas_code::DefinitionKey] {
        &self.0.data.base().contract().header().tools
    }
    pub fn annotations(&self) -> &[etas_code::descriptor::AnnotationInfo] {
        &self.0.data.base().contract().header().annotations
    }
    pub fn specs(&self) -> &[etas_code::descriptor::AttachedSpecInfo] {
        &self.0.data.base().contract().header().specs
    }
    pub fn current_analysis_descriptors(
        &self,
    ) -> Result<&etas_code::descriptor::DescriptorGraph, AnalysisUnavailableReason> {
        self.current_analysis().map(|_| &self.0.analysis_graph)
    }
    pub fn current_method_descriptors(
        &self,
    ) -> Result<&etas_code::descriptor::DescriptorGraph, AnalysisUnavailableReason> {
        let expected =
            AnalysisFingerprint::new(self.0.data.revision().key(), self.0.analysis_configuration);
        self.0
            .auxiliary
            .current(expected)
            .map(|_| &self.0.analysis_graph)
    }
    pub fn checked_implementation(&self) -> Option<&Arc<crate::code::service::CheckedCandidate>> {
        self.0.implementation.as_ref()
    }
    pub fn current_method_analysis(
        &self,
        method: &CallableId,
    ) -> Result<Option<&CallableAnalysisInfo>, AnalysisUnavailableReason> {
        let expected =
            AnalysisFingerprint::new(self.0.data.revision().key(), self.0.analysis_configuration);
        self.0
            .auxiliary
            .current(expected)
            .map(|summaries| summaries.get(method))
    }

    pub(super) fn methods(
        &self,
        backend: &super::NativeDefinitionStore,
    ) -> Result<Vec<NativeAgentMethodView>, NativeDefinitionError> {
        let mut methods = Vec::new();
        for id in &self.0.data.base().contract().header().methods {
            if matches!(&self.0.environment, super::DefinitionEnvironment::Native(_)) {
                let callable = backend
                    .catalog
                    .callables
                    .get(id)
                    .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
                if super::super::store::check_access(
                    callable.native.private_to,
                    self.0.reflection_access,
                )
                .is_err()
                {
                    continue;
                }
            }
            let header = self
                .0
                .data
                .base()
                .contract()
                .callable(id)
                .ok_or(NativeDefinitionError::CandidateInputMismatch)?
                .clone();
            let analysis = match self.current_method_analysis(id) {
                Ok(Some(analysis)) => Some((
                    self.current_method_descriptors()
                        .map_err(|_| NativeDefinitionError::CandidateInputMismatch)?
                        .clone(),
                    analysis.clone(),
                )),
                Ok(None) | Err(AnalysisUnavailableReason::InputMismatch) => {
                    return Err(NativeDefinitionError::CandidateInputMismatch);
                }
                Err(_) => None,
            };
            let body = backend
                .catalog
                .retained_body_at_access(id, self.0.reflection_access, &backend.syntax)
                .map_err(NativeDefinitionError::Projection)?;
            methods.push(NativeAgentMethodView {
                header,
                signature_descriptors: self.signature_descriptors().clone(),
                analysis,
                body,
            });
        }
        Ok(methods)
    }

    pub fn body_overlay(
        &self,
        syntax: &NativeCodeSyntaxOperations,
    ) -> Result<DefaultBodyOverlay, NativeDefinitionError> {
        let environment = self.environment()?.clone();
        let body = match self.body() {
            Some(body) => body.clone(),
            None => self.private_body(syntax)?,
        };
        DefaultBodyOverlay::prepare_revision(environment, body, syntax, self.revision().clone())
            .map_err(NativeDefinitionError::Overlay)
    }

    fn private_body(
        &self,
        syntax: &NativeCodeSyntaxOperations,
    ) -> Result<etas_code::syntax::SyntaxBlockRef, NativeDefinitionError> {
        use crate::code::draft::OverlayError;
        if let Some(implementation) = &self.0.implementation {
            return implementation
                .program()
                .definition_sources
                .overlaid_body(implementation.entry())
                .map(|body| body.syntax.clone())
                .ok_or(NativeDefinitionError::Overlay(
                    OverlayError::InvalidFrozenSlot,
                ));
        }
        if self.0.provenance != super::DefinitionProvenance::Source {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        }
        let frozen = self.environment()?;
        let item = frozen.default_body().ok_or(NativeDefinitionError::Overlay(
            OverlayError::MissingDefaultEntry,
        ))?;
        let slot = frozen.checked().definition_sources.body(item).ok_or(
            NativeDefinitionError::Overlay(OverlayError::InvalidFrozenSlot),
        )?;
        let source = frozen
            .checked()
            .definition_sources
            .source(slot.source)
            .ok_or(NativeDefinitionError::Overlay(
                OverlayError::InvalidFrozenSlot,
            ))?;
        let callable =
            self.0
                .data
                .base()
                .default_callable()
                .ok_or(NativeDefinitionError::Overlay(
                    OverlayError::MissingDefaultEntry,
                ))?;
        syntax
            .retain_source_body(
                callable.fingerprint(),
                source.file.clone(),
                source.program.clone(),
                etas_syntax::ProgramBodySelection {
                    item: slot.item.index,
                    impl_member: slot.impl_member,
                },
            )
            .map_err(|error| NativeDefinitionError::Overlay(OverlayError::Syntax(error)))
    }
}
