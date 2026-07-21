use etas_core::Span;
use etas_hir::{HirExprId, HirModuleId, PartialResolutionReason, SymbolId};

use crate::{
    ExternalModuleId, ExternalPackageId, ExternalSymbolId, ModuleId, ModuleKey, ModulePartId,
    ModulePath,
};

use super::module_index::{AstImportRef, AstItemRef, ReExport};

#[derive(Clone, Debug, Default)]
pub struct ResolvedImports {
    pub imports: Vec<ResolvedImport>,
    pub wildcard_imports: Vec<ResolvedWildcardImport>,
    pub re_exports: Vec<ReExport>,
}

#[derive(Clone, Debug)]
pub struct ResolvedImport {
    pub from: ModuleId,
    pub from_part: ModulePartId,
    pub import: AstImportRef,
    pub target: ImportTarget,
    pub local_name: String,
    pub visibility: etas_hir::Visibility,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ResolvedWildcardImport {
    pub from: ModuleId,
    pub from_part: ModulePartId,
    pub import: AstImportRef,
    pub target_module: ResolvedModuleTarget,
    pub exported_names: Vec<String>,
    pub visibility: etas_hir::Visibility,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    pub fn module_key(&self) -> ModuleKey {
        match self {
            Self::Module(module) => module.key(),
            Self::SourceItem { module_key, .. }
            | Self::StdItem { module_key, .. }
            | Self::ExternalItem { module_key, .. } => *module_key,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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

#[derive(Clone, Debug, Default)]
pub struct ResolvedPaths {
    pub module_names: Vec<ResolvedModulePath>,
    pub import_targets: Vec<etas_hir::ResolvedPath>,
    pub expr_paths: Vec<HirExprPathResolution>,
}

#[derive(Clone, Debug)]
pub struct ResolvedModulePath {
    pub module: HirModuleId,
    pub path: etas_hir::ResolvedPath,
}

#[derive(Clone, Debug)]
pub struct HirExprPathResolution {
    pub expr: HirExprId,
    pub path: etas_hir::ResolvedPath,
    pub result: HirPathResolution,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
