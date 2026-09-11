use etas_utils::ArtifactKey;

pub(crate) const EFFECT_REGISTRY: ArtifactKey = ArtifactKey::new("effects", "registry");
pub(crate) const EFFECT_UNITS: ArtifactKey = ArtifactKey::new("effects", "units");
pub(crate) const MEMORY_PROVENANCE: ArtifactKey = ArtifactKey::new("effects", "memory_provenance");
pub(crate) const EFFECT_SUMMARIES: ArtifactKey = ArtifactKey::new("effects", "summaries");
pub(crate) const EFFECT_FACTS: ArtifactKey = ArtifactKey::new("effects", "facts");
pub(crate) const VALIDATED_EFFECT_FACTS: ArtifactKey =
    ArtifactKey::new("effects", "validated_facts");
pub(crate) const TRACE_SPEC_MODELS: ArtifactKey = ArtifactKey::new("effects", "trace_spec_models");
pub(crate) const TRACE_SPEC_ANALYSIS: ArtifactKey =
    ArtifactKey::new("effects", "trace_spec_analysis");
pub(crate) const VALIDATED_TRACE_SPEC_FACTS: ArtifactKey =
    ArtifactKey::new("effects", "validated_trace_spec_facts");
pub(crate) const EFFECT_OUTPUT: ArtifactKey = ArtifactKey::new("effects", "output");
