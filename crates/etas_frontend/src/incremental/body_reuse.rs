use etas_cache::ArtifactKey;
use etas_core::{SourceId, Span};
use etas_types::TypeOutput;

use crate::artifact::{FrontendArtifactDependency, FrontendArtifactManifest};
use crate::{AstItemKind, SourceSet, UnitId, UnitKind, UnitTarget, UnitTree};

#[derive(Clone, Debug, Eq)]
pub(crate) struct BodyArtifactIdentity {
    pub source: SourceId,
    pub item_index: usize,
    pub item_kind: AstItemKind,
    pub text: String,
}

impl PartialEq for BodyArtifactIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
            && self.item_index == other.item_index
            && self.item_kind == other.item_kind
            && self.text == other.text
    }
}

impl BodyArtifactIdentity {
    pub(crate) fn for_unit(unit: UnitId, units: &UnitTree, sources: &SourceSet) -> Option<Self> {
        let node = units.nodes.get(unit)?;
        if node.kind != UnitKind::Body {
            return None;
        }
        let UnitTarget::AstBody(body) = &node.target else {
            return None;
        };
        let source = sources
            .files
            .iter()
            .find(|source| source.id == body.source)?;
        let text = span_text(source, body.span)?.to_owned();
        Some(Self {
            source: body.source,
            item_index: body.item.index,
            item_kind: body.item.kind,
            text,
        })
    }

    pub(crate) fn fingerprint_parts(&self, unit: UnitId) -> Vec<String> {
        vec![
            format!("unit:{}", unit.0),
            format!("source:{}", self.source.0),
            format!("item_index:{}", self.item_index),
            format!("item_kind:{:?}", self.item_kind),
            self.text.clone(),
        ]
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CachedTypeBodyArtifact {
    pub unit: UnitId,
    pub cache_key: ArtifactKey,
    pub identity: BodyArtifactIdentity,
    pub dependencies: Vec<FrontendArtifactDependency>,
    pub output: TypeOutput,
}

#[derive(Clone, Debug)]
pub(crate) struct CachedEffectBodyArtifact {
    pub unit: UnitId,
    pub cache_key: ArtifactKey,
    pub identity: BodyArtifactIdentity,
    pub dependencies: Vec<FrontendArtifactDependency>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BodyArtifactReuseInput {
    pub type_outputs: Vec<CachedTypeBodyArtifact>,
    pub effect_outputs: Vec<CachedEffectBodyArtifact>,
}

impl BodyArtifactReuseInput {
    pub(crate) fn is_empty(&self) -> bool {
        self.type_outputs.is_empty() && self.effect_outputs.is_empty()
    }
}

pub(crate) fn dependency_fingerprints_match(
    dependencies: &[FrontendArtifactDependency],
    current_manifest: &FrontendArtifactManifest,
) -> bool {
    dependencies.iter().all(|dependency| {
        let Some(current) = current_manifest.get(&dependency.key) else {
            return false;
        };
        dependency
            .fingerprint
            .is_none_or(|fingerprint| current.fingerprint == fingerprint)
    })
}

fn span_text(source: &crate::SourceFile, span: Span) -> Option<&str> {
    let start = span.range.start.to_usize();
    let end = span.range.end.to_usize();
    source.text.get(start..end)
}
