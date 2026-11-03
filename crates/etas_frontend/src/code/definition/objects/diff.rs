use etas_code::{
    CodeFingerprint,
    definition::{
        AnalysisUnavailableReason, ChangeReport, DefinitionDiffSnapshot, DefinitionOperationError,
    },
    syntax::{CodeSyntaxOperations, SyntaxOperationError},
};
use etas_utils::PipelineExecutionControl;

use super::{NativeDefinitionError, NativeDefinitionView};
use crate::code::descriptor::ProjectionError;

impl NativeDefinitionView {
    pub fn diff(
        &self,
        other: &Self,
        old_syntax: &dyn CodeSyntaxOperations,
        new_syntax: &dyn CodeSyntaxOperations,
        max_body_bytes: usize,
        control: &dyn PipelineExecutionControl,
    ) -> Result<ChangeReport, NativeDefinitionError> {
        if self.identity() != other.identity() {
            return Err(DefinitionOperationError::IncompatibleDefinitions.into());
        }
        let old = self.diff_snapshot(old_syntax, max_body_bytes, control)?;
        let new = other.diff_snapshot(new_syntax, max_body_bytes, control)?;
        ChangeReport::compare(old, new).map_err(Into::into)
    }

    fn diff_snapshot(
        &self,
        syntax: &dyn CodeSyntaxOperations,
        max_body_bytes: usize,
        control: &dyn PipelineExecutionControl,
    ) -> Result<DefinitionDiffSnapshot<'_>, NativeDefinitionError> {
        let checkpoint = || {
            control.checkpoint().map_err(|error| {
                NativeDefinitionError::Operation(DefinitionOperationError::Syntax(
                    SyntaxOperationError::Interrupted(error),
                ))
            })
        };
        checkpoint()?;
        let signature_graph = self.signature_descriptors();
        let signature = self
            .0
            .data
            .base()
            .contract()
            .entry()
            .map(|id| {
                let callable = self
                    .0
                    .data
                    .base()
                    .contract()
                    .callable(id)
                    .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
                signature_graph
                    .type_fingerprint(callable.signature)
                    .map_err(ProjectionError::from)
                    .map_err(NativeDefinitionError::Projection)
            })
            .transpose()?;
        let body = self
            .body()
            .map(|body| {
                syntax
                    .format_bounded_controlled(body, max_body_bytes, control)
                    .map(|text| {
                        CodeFingerprint::of_bytes("etas.code.diff-body.v1", text.as_bytes())
                    })
                    .map_err(DefinitionOperationError::Syntax)
            })
            .transpose()?;
        checkpoint()?;
        let declared_bound = self
            .signature()
            .and_then(|signature| signature.effects)
            .map(|row| signature_graph.row_fingerprint(row))
            .transpose()
            .map_err(ProjectionError::from)
            .map_err(NativeDefinitionError::Projection)?;
        let current = match self.current_analysis() {
            Ok(analysis) => Some(analysis),
            Err(AnalysisUnavailableReason::InputMismatch) => {
                return Err(NativeDefinitionError::CandidateInputMismatch);
            }
            Err(_) => None,
        };
        let graph = &self.0.analysis_graph;
        let row = |row: Option<etas_code::descriptor::EffectRowInfoRef>| {
            checkpoint()?;
            row.map(|row| graph.row_fingerprint(row))
                .transpose()
                .map_err(ProjectionError::from)
                .map_err(NativeDefinitionError::Projection)
        };
        let escaping = row(current.and_then(|analysis| analysis.effects.escaping))?;
        let requested = row(current.and_then(|analysis| analysis.effects.requested))?;
        let default_handled = row(current.and_then(|analysis| analysis.effects.default_handled))?;
        let trace = current
            .and_then(|analysis| analysis.trace)
            .map(|trace| graph.trace_fingerprint(trace))
            .transpose()
            .map_err(ProjectionError::from)
            .map_err(NativeDefinitionError::Projection)?;
        checkpoint()?;
        Ok(DefinitionDiffSnapshot {
            identity: self.identity(),
            revision: self.revision(),
            signature,
            body,
            declared_bound,
            escaping,
            requested,
            default_handled,
            determinism: current.and_then(|analysis| analysis.determinism),
            trace,
        })
    }
}
