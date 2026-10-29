use crate::code::draft::{DefaultBodyOverlay, NativeBodyRevision};
use etas_core::Span;
use etas_hir::HirItemId;
use std::{collections::HashMap, sync::Arc};

pub(crate) struct CandidatePipelineContext {
    pub overlay: DefaultBodyOverlay,
    pub revisions: HashMap<Span, Arc<NativeBodyRevision>>,
    pub target: Option<HirItemId>,
    pub bound_application: Option<BoundCandidateApplication>,
    pub bound_body: Option<etas_types::InstantiatedBodyOutput>,
    pub descriptors: Option<super::passes::CandidateDescriptors>,
    pub descriptor_limits: etas_code::descriptor::DescriptorLimits,
    pub contract_analysis: Option<crate::code::contract::CandidateContractAnalysis>,
    pub emission: Option<crate::code::service::CandidateEmission>,
    pub dependency_sources: Vec<crate::SourceInput>,
    pub dependency_source_ids: Vec<etas_core::SourceId>,
    pub snapshot: etas_code::CodeFingerprint,
    pub baseline: Option<super::baseline::CompletedBaseline>,
}

impl CandidatePipelineContext {
    pub fn new(overlay: DefaultBodyOverlay) -> Self {
        let mut revisions = overlay
            .frozen()
            .checked()
            .definition_sources
            .revisions()
            .clone();
        revisions.insert(overlay.declaration(), overlay.native_revision().clone());
        let snapshot = overlay.fingerprint();
        Self {
            overlay,
            revisions,
            target: None,
            bound_application: None,
            bound_body: None,
            descriptors: None,
            descriptor_limits: Default::default(),
            contract_analysis: None,
            emission: None,
            dependency_sources: Vec::new(),
            dependency_source_ids: Vec::new(),
            snapshot,
            baseline: None,
        }
    }

    pub fn set_dependency_sources(
        &mut self,
        mut sources: Vec<crate::SourceInput>,
    ) -> Result<(), String> {
        sources.sort_by(|left, right| left.path.cmp(&right.path));
        let mut fingerprints = vec![self.overlay.fingerprint()];
        for source in &sources {
            let crate::SourceKind::DependencySourceOverlay {
                package,
                import_root,
            } = &source.kind
            else {
                return Err("candidate source completion accepts only dependency overlays".into());
            };
            let path = source
                .path
                .as_ref()
                .ok_or("candidate dependency source has no file identity")?;
            let content = etas_code::CodeFingerprint::of_bytes(
                "etas.code.dependency-source-content.v1",
                source.text.as_bytes(),
            );
            fingerprints.push(etas_code::CodeFingerprint::of_parts(
                "etas.code.dependency-source-input.v1",
                [
                    package.0.to_le_bytes().as_slice(),
                    import_root.as_bytes(),
                    path.as_os_str().as_encoded_bytes(),
                    content.as_bytes().as_slice(),
                ],
            ));
        }
        if !sources.is_empty() {
            self.snapshot = etas_code::CodeFingerprint::of_parts(
                "etas.code.candidate-with-dependencies.v1",
                fingerprints
                    .iter()
                    .map(|fingerprint| fingerprint.as_bytes().as_slice()),
            );
        }
        self.dependency_sources = sources;
        Ok(())
    }
}

pub(crate) struct BoundCandidateApplication {
    pub owner: HirItemId,
    pub arguments: Vec<etas_types::CallableGenericArg>,
}
