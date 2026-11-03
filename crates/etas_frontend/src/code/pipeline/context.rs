use crate::code::draft::{DefaultBodyOverlay, NativeBodyRevision};
use etas_core::Span;
use etas_hir::HirItemId;
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Debug)]
pub struct CandidateDependencySource {
    pub source: crate::SourceInput,
    pub stable_key: etas_code::StableSourceKey,
}

pub(crate) struct CandidatePipelineContext {
    pub overlay: DefaultBodyOverlay,
    pub revisions: HashMap<Span, Arc<NativeBodyRevision>>,
    pub target: Option<HirItemId>,
    pub bound_application: Option<BoundCandidateApplication>,
    pub bound_body: Option<etas_types::InstantiatedBodyOutput>,
    pub frozen_application_program: Option<crate::CheckedProgram>,
    pub descriptors: Option<super::passes::CandidateDescriptors>,
    pub descriptor_limits: etas_code::descriptor::DescriptorLimits,
    pub contract_analysis: Option<crate::code::contract::CandidateContractAnalysis>,
    pub emission: Option<crate::code::service::CandidateEmission>,
    pub dependency_sources: Vec<CandidateDependencySource>,
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
            frozen_application_program: None,
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
        mut sources: Vec<CandidateDependencySource>,
    ) -> Result<(), String> {
        sources.sort_by_key(|source| source.stable_key);
        if sources
            .windows(2)
            .any(|pair| pair[0].stable_key == pair[1].stable_key)
        {
            return Err("candidate dependency sources have duplicate stable identities".into());
        }
        let limits = self.descriptor_limits;
        let encoded_limits = [
            limits.max_nodes,
            limits.max_edges,
            limits.max_expansion_depth,
            limits.max_text_bytes,
        ]
        .into_iter()
        .map(|value| {
            u64::try_from(value)
                .map(u64::to_le_bytes)
                .map_err(|_| "candidate descriptor limit cannot be represented".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
        let configuration = etas_code::CodeFingerprint::of_parts(
            "etas.code.candidate-descriptor-limits.v1",
            encoded_limits.iter().map(|value| value.as_slice()),
        );
        let mut fingerprints = vec![self.overlay.fingerprint(), configuration];
        for entry in &sources {
            let source = &entry.source;
            let crate::SourceKind::DependencySourceOverlay {
                package,
                import_root,
            } = &source.kind
            else {
                return Err("candidate source completion accepts only dependency overlays".into());
            };
            source
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
                    entry.stable_key.fingerprint().as_bytes().as_slice(),
                    content.as_bytes().as_slice(),
                ],
            ));
        }
        self.snapshot = etas_code::CodeFingerprint::of_parts(
            "etas.code.candidate-analysis-inputs.v1",
            fingerprints
                .iter()
                .map(|fingerprint| fingerprint.as_bytes().as_slice()),
        );
        self.dependency_sources = sources;
        Ok(())
    }
}

pub(crate) struct BoundCandidateApplication {
    pub owner: HirItemId,
    pub arguments: Vec<etas_types::CallableGenericArg>,
    pub presented_type: Option<etas_types::TypeId>,
    pub foreign_type_bindings: Arc<crate::code::definition::ForeignTypeBindings>,
}
