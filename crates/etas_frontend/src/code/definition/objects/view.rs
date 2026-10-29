use std::sync::Arc;

use etas_code::{
    CallableId, RevisionInfo,
    definition::{AnalysisFingerprint, AnalysisUnavailableReason},
    descriptor::{CallableAnalysisInfo, SignatureInfo},
};

use super::{NativeDefinition, NativeDefinitionError};
use crate::code::{
    definition::FrozenDefinitionEnvironment, draft::DefaultBodyOverlay,
    service::NativeCodeSyntaxOperations,
};

#[derive(Clone, etas_core::RetainedSize)]
pub struct NativeDefinitionView(pub(super) Arc<NativeDefinition>);

pub struct NativeAgentMethodView {
    pub header: etas_code::descriptor::CallableHeaderInfo,
    pub signature_descriptors: etas_code::descriptor::DescriptorGraph,
    pub analysis: Option<(etas_code::descriptor::DescriptorGraph, CallableAnalysisInfo)>,
    pub body: Option<etas_code::syntax::SyntaxBlockRef>,
    pub snapshot: NativeMethodSnapshot,
}

#[derive(Clone)]
pub struct NativeMethodSnapshot {
    pub coordinates: Arc<etas_code::descriptor::DefinitionSnapshotInfo>,
    pub owner: etas_code::definition::DefinitionRef,
}

impl NativeDefinitionView {
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
    pub fn declared_tools(&self) -> Result<&[etas_code::DefinitionKey], NativeDefinitionError> {
        Ok(&self.agent_header()?.tools)
    }
    pub fn annotations(
        &self,
    ) -> Result<&[etas_code::descriptor::AnnotationInfo], NativeDefinitionError> {
        Ok(&self.agent_header()?.annotations)
    }
    pub fn specs(
        &self,
    ) -> Result<&[etas_code::descriptor::AttachedSpecInfo], NativeDefinitionError> {
        Ok(&self.agent_header()?.specs)
    }
    fn agent_header(
        &self,
    ) -> Result<&etas_code::descriptor::AgentHeaderInfo, NativeDefinitionError> {
        self.0
            .data
            .base()
            .contract()
            .agent_header()
            .ok_or(NativeDefinitionError::WrongDefinitionKind)
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

    pub fn method_snapshot(
        &self,
        id: &CallableId,
    ) -> Result<NativeMethodSnapshot, NativeDefinitionError> {
        if id.as_method().is_none() || self.0.data.base().contract().callable(id).is_none() {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        }
        let coordinates = self
            .0
            .method_snapshots
            .get(id)
            .cloned()
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
        Ok(NativeMethodSnapshot {
            coordinates,
            owner: etas_code::definition::DefinitionRef::from_backend(self.0.clone()),
        })
    }

    pub(super) fn revised_method_snapshots(
        &self,
        revision: &RevisionInfo,
    ) -> Result<
        std::collections::BTreeMap<CallableId, Arc<etas_code::descriptor::DefinitionSnapshotInfo>>,
        crate::code::descriptor::ProjectionError,
    > {
        if self.0.data.base().contract().agent_header().is_none() {
            return Ok(std::collections::BTreeMap::new());
        }
        let owner = etas_code::descriptor::DefinitionSnapshotInfo {
            identity: self.identity().clone(),
            revision: revision.inputs().clone(),
            dependencies: self.dependency_info()?,
        };
        self.0
            .method_snapshots
            .iter()
            .map(|(id, previous)| {
                let content = if id == &owner.revision.definition {
                    owner.revision.content
                } else {
                    previous.revision.content
                };
                Ok((id.clone(), Arc::new(owner.method_snapshot(id, content)?)))
            })
            .collect()
    }

    pub(super) fn methods(
        &self,
        backend: &super::NativeDefinitionStore,
    ) -> Result<Vec<NativeAgentMethodView>, NativeDefinitionError> {
        self.agent_header()?;
        let mut methods = Vec::new();
        for id in self.0.data.base().contract().methods() {
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
                snapshot: self.method_snapshot(id)?,
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
