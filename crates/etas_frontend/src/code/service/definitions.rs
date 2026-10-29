use std::sync::Arc;

use etas_code::{
    CodeFingerprint,
    definition::DefinitionRef,
    descriptor::{DescriptorLimits, EffectSummaryInfo},
    diagnostic::{
        DiagnosticDetails, DiagnosticLocation, DiagnosticRecord, DiagnosticSeverity,
        ViolatedConstraint,
    },
    report::{CheckReport, CheckStatus},
};

use super::{CandidateAnalysisError, CandidateCompileError, CheckReportError, CheckedCandidate};
use crate::{
    FrontendSession,
    code::{
        definition::{NativeDefinitionError, NativeDefinitionStore, NativeDefinitionView},
        diagnostic::DiagnosticProjector,
    },
};

#[derive(Debug)]
pub enum NativeCompilerError {
    Definition(NativeDefinitionError),
    Analysis(CandidateAnalysisError),
    Report(CheckReportError),
    Rejected(Box<CheckReport>),
}

impl std::fmt::Display for NativeCompilerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(report) => write!(
                f,
                "candidate rejected with {} diagnostics",
                report.diagnostics.len()
            ),
            error => write!(f, "native compiler service failed: {error:?}"),
        }
    }
}
impl std::error::Error for NativeCompilerError {}
impl From<CandidateCompileError> for NativeCompilerError {
    fn from(error: CandidateCompileError) -> Self {
        match error {
            CandidateCompileError::Analysis(error) => Self::Analysis(error),
            CandidateCompileError::Report(error) => Self::Report(error),
            CandidateCompileError::Rejected(report) => Self::Rejected(report),
        }
    }
}

impl NativeDefinitionStore {
    pub fn analyze_definition_with_dependencies(
        &self,
        definition: &DefinitionRef,
        session: &mut FrontendSession,
        dependency_sources: Vec<crate::SourceInput>,
        limits: DescriptorLimits,
        control: Arc<dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<super::CandidateBodyAnalysis, NativeCompilerError> {
        check_interruption(Some(control.as_ref()))?;
        let view = self
            .resolve(definition)
            .map_err(|error| NativeCompilerError::Definition(error.into()))?;
        if view.signature().is_none() {
            return Err(NativeCompilerError::Rejected(Box::new(
                self.missing_entry_report(&view, limits)?,
            )));
        }
        let overlay = view
            .body_overlay(self.syntax_operations())
            .map_err(NativeCompilerError::Definition)?;
        session
            .analyze_body_overlay_with_dependencies(
                overlay,
                dependency_sources,
                limits,
                Some(control),
            )
            .map_err(NativeCompilerError::Analysis)
    }

    /// Native service entry. The Host adapter supplies authorization, resource
    /// accounting and operation ownership; this runs the common frontend passes.
    pub fn check_definition(
        &self,
        definition: &DefinitionRef,
        session: &mut FrontendSession,
        limits: DescriptorLimits,
    ) -> Result<CheckReport, NativeCompilerError> {
        self.check_definition_inner(definition, session, limits, None)
    }

    pub fn check_definition_controlled(
        &self,
        definition: &DefinitionRef,
        session: &mut FrontendSession,
        limits: DescriptorLimits,
        control: Arc<dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<CheckReport, NativeCompilerError> {
        self.check_definition_inner(definition, session, limits, Some(control))
    }

    fn check_definition_inner(
        &self,
        definition: &DefinitionRef,
        session: &mut FrontendSession,
        limits: DescriptorLimits,
        control: Option<Arc<dyn etas_utils::PipelineExecutionControl>>,
    ) -> Result<CheckReport, NativeCompilerError> {
        check_interruption(control.as_deref())?;
        let view = self
            .resolve(definition)
            .map_err(|error| NativeCompilerError::Definition(error.into()))?;
        if view.signature().is_none() {
            let report = self.missing_entry_report(&view, limits)?;
            check_interruption(control.as_deref())?;
            return Ok(report);
        }
        let overlay = view
            .body_overlay(self.syntax_operations())
            .map_err(NativeCompilerError::Definition)?;
        let report = analyze(session, overlay, limits, control.clone())
            .map_err(NativeCompilerError::Analysis)?
            .check_report(limits)
            .map_err(NativeCompilerError::Report)?;
        check_interruption(control.as_deref())?;
        Ok(report)
    }

    pub fn compile_definition(
        &self,
        definition: &DefinitionRef,
        session: &mut FrontendSession,
        limits: DescriptorLimits,
    ) -> Result<CheckedCandidate, NativeCompilerError> {
        self.compile_definition_inner(definition, session, limits, None)
    }

    pub fn compile_definition_controlled(
        &self,
        definition: &DefinitionRef,
        session: &mut FrontendSession,
        limits: DescriptorLimits,
        control: Arc<dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<CheckedCandidate, NativeCompilerError> {
        self.compile_definition_inner(definition, session, limits, Some(control))
    }

    fn compile_definition_inner(
        &self,
        definition: &DefinitionRef,
        session: &mut FrontendSession,
        limits: DescriptorLimits,
        control: Option<Arc<dyn etas_utils::PipelineExecutionControl>>,
    ) -> Result<CheckedCandidate, NativeCompilerError> {
        check_interruption(control.as_deref())?;
        let view = self
            .resolve(definition)
            .map_err(|error| NativeCompilerError::Definition(error.into()))?;
        if view.signature().is_none() {
            let report = self.missing_entry_report(&view, limits)?;
            check_interruption(control.as_deref())?;
            return Err(NativeCompilerError::Rejected(Box::new(report)));
        }
        let overlay = view
            .body_overlay(self.syntax_operations())
            .map_err(NativeCompilerError::Definition)?;
        let candidate = analyze(session, overlay, limits, control.clone())
            .map_err(NativeCompilerError::Analysis)?
            .into_checked_candidate(limits)
            .map_err(NativeCompilerError::from)?;
        check_interruption(control.as_deref())?;
        Ok(candidate)
    }

    fn missing_entry_report(
        &self,
        view: &NativeDefinitionView,
        limits: DescriptorLimits,
    ) -> Result<CheckReport, NativeCompilerError> {
        let graph = view.signature_descriptors();
        graph.validate(limits).map_err(|error| {
            NativeCompilerError::Report(CheckReportError::Projection(error.into()))
        })?;
        let location = match view.body() {
            Some(body) => {
                let native = self.syntax_operations().resolve(body).map_err(|error| {
                    NativeCompilerError::Definition(NativeDefinitionError::Overlay(
                        crate::code::draft::OverlayError::Syntax(error),
                    ))
                })?;
                let identity = body.identity();
                Some(DiagnosticLocation {
                    source: identity.source(),
                    revision: identity.content(),
                    byte_start: 0,
                    byte_end: native.source().text().len(),
                })
            }
            None => match view.environment() {
                Ok(frozen) => {
                    let declaration = frozen.checked().hir.items.get(frozen.declaration()).ok_or(
                        NativeCompilerError::Report(CheckReportError::IncompleteAnalysis(
                            "bare agent declaration is missing",
                        )),
                    )?;
                    Some(
                        DiagnosticProjector::frozen(frozen)
                            .map_err(NativeCompilerError::Report)?
                            .location(declaration.span())
                            .map_err(NativeCompilerError::Report)?,
                    )
                }
                Err(NativeDefinitionError::MissingSourceEnvironment) => None,
                Err(error) => return Err(NativeCompilerError::Definition(error)),
            },
        };
        let revision = view.revision();
        let syntax = match view.body() {
            Some(body) => CodeFingerprint::of_parts(
                "etas.code.present-draft-syntax.v1",
                [body.identity().fingerprint().as_bytes().as_slice()],
            ),
            None => CodeFingerprint::of_bytes("etas.code.absent-draft-syntax.v1", b""),
        };
        let snapshot = CodeFingerprint::of_parts(
            "etas.code.missing-default-entry.v1",
            [
                revision.key().fingerprint().as_bytes().as_slice(),
                syntax.as_bytes().as_slice(),
            ],
        );
        Ok(CheckReport {
            status: CheckStatus::Rejected,
            diagnostics: vec![DiagnosticRecord { code: "code::MissingDefaultEntry".into(), severity: DiagnosticSeverity::Error,
                message: "agent definition has no default-entry signature; replacing its body cannot invent a callable contract".into(),
                location, details: Some(Arc::new(DiagnosticDetails { constraint: Some(ViolatedConstraint {
                    kind: "default_entry_contract".into(), owner: Some(view.identity().clone()), message: "a declared default callable interface is required".into(),
                }), ..Default::default() })) }],
            revision: revision.clone(), dependencies: view.dependency_info().map_err(|error| NativeCompilerError::Report(error.into()))?,
            snapshot_fingerprint: snapshot, descriptors: graph.clone(), effects: EffectSummaryInfo::default(), determinism: None, trace: None, runtime_checks: vec![],
        })
    }
}

fn analyze(
    session: &mut FrontendSession,
    overlay: crate::code::draft::DefaultBodyOverlay,
    limits: DescriptorLimits,
    control: Option<Arc<dyn etas_utils::PipelineExecutionControl>>,
) -> Result<super::CandidateBodyAnalysis, CandidateAnalysisError> {
    session.analyze_body_overlay_with_dependencies(overlay, Vec::new(), limits, control)
}

fn check_interruption(
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<(), NativeCompilerError> {
    if let Some(control) = control {
        control.checkpoint().map_err(|error| {
            NativeCompilerError::Analysis(CandidateAnalysisError::Interrupted(error))
        })?;
    }
    Ok(())
}
