use etas_cache::ArtifactKey;
use etas_core::SourceId;
use etas_hir::HirItemId;
use etas_utils::{UnitKey, UnitKindKey};

use crate::{
    BLOCK_UNIT_KIND, BODY_UNIT_KIND, EXPRESSION_UNIT_KIND, ITEM_UNIT_KIND, MODULE_PART_UNIT_KIND,
    MODULE_UNIT_KIND, ModuleId, ModulePartId, PROJECT_UNIT_KIND, SOURCE_FILE_UNIT_KIND,
};

use super::kind::{FRONTEND_CACHE_NAMESPACE, FrontendArtifactKind};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FrontendUnitKey {
    Project,
    Source(SourceId),
    Module(ModuleId),
    ModulePart(ModulePartId),
    Item(HirItemId),
    Unit(UnitKey),
}

impl FrontendUnitKey {
    pub fn cache_unit(&self) -> String {
        match self {
            Self::Project => "project".to_owned(),
            Self::Source(source) => format!("source:{}", source.0),
            Self::Module(module) => format!("module:{}", module.0),
            Self::ModulePart(part) => format!("module_part:{}", part.0),
            Self::Item(item) => format!("item:{}", item.0),
            Self::Unit(unit) => format!(
                "unit:{}:{}:{}",
                unit.kind.namespace, unit.kind.name, unit.id
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FrontendArtifactKey {
    pub kind: FrontendArtifactKind,
    pub unit: FrontendUnitKey,
}

impl FrontendArtifactKey {
    pub fn project(kind: FrontendArtifactKind) -> Self {
        Self {
            kind,
            unit: FrontendUnitKey::Project,
        }
    }

    pub fn source(kind: FrontendArtifactKind, source: SourceId) -> Self {
        Self {
            kind,
            unit: FrontendUnitKey::Source(source),
        }
    }

    pub fn module(kind: FrontendArtifactKind, module: ModuleId) -> Self {
        Self {
            kind,
            unit: FrontendUnitKey::Module(module),
        }
    }

    pub fn module_part(kind: FrontendArtifactKind, part: ModulePartId) -> Self {
        Self {
            kind,
            unit: FrontendUnitKey::ModulePart(part),
        }
    }

    pub fn item(kind: FrontendArtifactKind, item: HirItemId) -> Self {
        Self {
            kind,
            unit: FrontendUnitKey::Item(item),
        }
    }

    pub fn unit(kind: FrontendArtifactKind, unit: UnitKey) -> Self {
        Self {
            kind,
            unit: FrontendUnitKey::Unit(unit),
        }
    }

    pub fn to_cache_key(&self) -> ArtifactKey {
        ArtifactKey::new(
            FRONTEND_CACHE_NAMESPACE,
            self.kind.key(),
            self.unit.cache_unit(),
        )
    }

    pub fn from_cache_key(key: &ArtifactKey) -> Option<Self> {
        if key.namespace.as_str() != FRONTEND_CACHE_NAMESPACE {
            return None;
        }
        Some(Self {
            kind: FrontendArtifactKind::from_key(key.kind.as_str())?,
            unit: FrontendUnitKey::from_cache_unit(key.unit.as_str())?,
        })
    }

    pub fn source_from_cache_key(
        key: &ArtifactKey,
        kind: FrontendArtifactKind,
    ) -> Option<SourceId> {
        if key.namespace.as_str() != FRONTEND_CACHE_NAMESPACE || key.kind.as_str() != kind.key() {
            return None;
        }
        let source = key.unit.as_str().strip_prefix("source:")?;
        source.parse::<u32>().ok().map(SourceId)
    }
}

impl FrontendUnitKey {
    pub fn from_cache_unit(unit: &str) -> Option<Self> {
        if unit == "project" {
            return Some(Self::Project);
        }
        if let Some(source) = unit.strip_prefix("source:") {
            return source.parse::<u32>().ok().map(SourceId).map(Self::Source);
        }
        if let Some(module) = unit.strip_prefix("module:") {
            return module.parse::<u32>().ok().map(ModuleId).map(Self::Module);
        }
        if let Some(part) = unit.strip_prefix("module_part:") {
            return part
                .parse::<u32>()
                .ok()
                .map(ModulePartId)
                .map(Self::ModulePart);
        }
        if let Some(item) = unit.strip_prefix("item:") {
            return item.parse::<u32>().ok().map(HirItemId).map(Self::Item);
        }
        let unit = unit.strip_prefix("unit:")?;
        let mut pieces = unit.split(':');
        let namespace = pieces.next()?;
        let name = pieces.next()?;
        let id = pieces.next()?.parse::<u64>().ok()?;
        if pieces.next().is_some() {
            return None;
        }
        let kind = frontend_unit_kind(namespace, name)?;
        Some(Self::Unit(UnitKey::new(kind, id)))
    }
}

fn frontend_unit_kind(namespace: &str, name: &str) -> Option<UnitKindKey> {
    match (namespace, name) {
        ("frontend", "project") => Some(PROJECT_UNIT_KIND),
        ("frontend", "source_file") => Some(SOURCE_FILE_UNIT_KIND),
        ("frontend", "module") => Some(MODULE_UNIT_KIND),
        ("frontend", "module_part") => Some(MODULE_PART_UNIT_KIND),
        ("frontend", "item") => Some(ITEM_UNIT_KIND),
        ("frontend", "body") => Some(BODY_UNIT_KIND),
        ("frontend", "block") => Some(BLOCK_UNIT_KIND),
        ("frontend", "expression") => Some(EXPRESSION_UNIT_KIND),
        _ => None,
    }
}
