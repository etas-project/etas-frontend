use etas_core::Span;
use etas_hir::{HirExprId, HirModuleId, PartialResolutionReason, SymbolId};

use crate::{
    ExternalModuleId, ExternalPackageId, ExternalSymbolId, ModuleId, ModuleKey, ModulePartId,
    ModulePath,
};

use super::module_index::{AstImportRef, AstItemRef, ReExport};

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
pub struct ResolvedImports {
    pub imports: Vec<ResolvedImport>,
    pub wildcard_imports: Vec<ResolvedWildcardImport>,
    pub re_exports: Vec<ReExport>,
    pub module_members:
        std::collections::BTreeMap<ModuleKey, std::collections::BTreeMap<String, ImportTarget>>,
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct ResolvedImport {
    pub from: ModuleId,
    pub from_part: ModulePartId,
    pub import: AstImportRef,
    pub target: ImportTarget,
    pub local_name: String,
    pub visibility: etas_hir::Visibility,
    pub span: Span,
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct ResolvedWildcardImport {
    pub from: ModuleId,
    pub from_part: ModulePartId,
    pub import: AstImportRef,
    pub target_module: ResolvedModuleTarget,
    pub exported_names: Vec<String>,
    pub targets: std::collections::BTreeMap<String, ImportTarget>,
    pub visibility: etas_hir::Visibility,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub enum ImportTarget {
    Module(ResolvedModuleTarget),
    SourceItem {
        module_key: ModuleKey,
        module: ModuleId,
        name: String,
        item: AstItemRef,
    },
    StdItem {
        module_key: ModuleKey,
        module: etas_std::StdModuleId,
        module_path: ModulePath,
        name: String,
        symbol: etas_std::StdSymbolId,
    },
    ExternalItem {
        module_key: ModuleKey,
        package: Option<ExternalPackageId>,
        module: ExternalModuleId,
        module_path: ModulePath,
        name: String,
        symbol: ExternalSymbolId,
    },
}

impl ImportTarget {
    pub(crate) fn catalog_path(&self, catalog: &crate::ModuleCatalog) -> Option<Vec<String>> {
        let mut path = catalog
            .modules
            .get(self.module_key())?
            .path
            .segments
            .clone();
        match self {
            Self::Module(_) => {}
            Self::SourceItem { name, .. }
            | Self::StdItem { name, .. }
            | Self::ExternalItem { name, .. } => path.push(name.clone()),
        }
        Some(path)
    }

    pub fn module_key(&self) -> ModuleKey {
        match self {
            Self::Module(module) => module.key(),
            Self::SourceItem { module_key, .. }
            | Self::StdItem { module_key, .. }
            | Self::ExternalItem { module_key, .. } => *module_key,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, etas_core::RetainedSize)]
pub enum ResolvedModuleTarget {
    Source {
        key: ModuleKey,
        module: ModuleId,
    },
    Std {
        key: ModuleKey,
        module: etas_std::StdModuleId,
        path: ModulePath,
    },
    External {
        key: ModuleKey,
        package: Option<ExternalPackageId>,
        module: ExternalModuleId,
        path: ModulePath,
    },
}

impl ResolvedModuleTarget {
    pub fn key(&self) -> ModuleKey {
        match self {
            Self::Source { key, .. } | Self::Std { key, .. } | Self::External { key, .. } => *key,
        }
    }
}

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
pub struct ResolvedPaths {
    pub module_names: Vec<ResolvedModulePath>,
    pub import_targets: Vec<etas_hir::ResolvedPath>,
    pub expr_paths: Vec<HirExprPathResolution>,
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct ResolvedModulePath {
    pub module: HirModuleId,
    pub path: etas_hir::ResolvedPath,
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct HirExprPathResolution {
    pub expr: HirExprId,
    pub path: etas_hir::ResolvedPath,
    pub result: HirPathResolution,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub enum HirPathResolution {
    DirectSymbol(SymbolId),
    ExplicitImport {
        target: ImportTarget,
    },
    PartialImport {
        target: ImportTarget,
        resolved_segments: u32,
        remaining: Vec<String>,
        reason: PartialResolutionReason,
    },
    WildcardImport {
        target: ResolvedModuleTarget,
        item: ImportTarget,
    },
    AmbiguousWildcard {
        targets: Vec<ResolvedModuleTarget>,
    },
    PartialSymbol {
        prefix: SymbolId,
        resolved_segments: u32,
        remaining: Vec<String>,
        reason: PartialResolutionReason,
    },
    PartiallyResolved {
        resolved_segments: u32,
        remaining: Vec<String>,
        reason: PartialResolutionReason,
    },
    Unresolved,
}
