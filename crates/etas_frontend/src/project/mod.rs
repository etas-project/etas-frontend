mod context;
mod hir_bindings;
mod import_graph;
mod incremental;
mod loader;
mod module_catalog;
mod module_index;
mod reachability;
mod resolution_facts;
mod runtime_source;
mod source;
mod unit_tree;

pub use context::ProjectContext;
pub(crate) use context::{ENTRY_REACHABLE_UNIT_FILTER, ProjectHirLoweringState};
pub use hir_bindings::{HirBodyBindings, HirItemBindings};
pub use import_graph::{ImportEdge, ImportGraph, ModuleTopoOrder};
pub use incremental::AffectedModuleSet;
pub use loader::{
    FsSourceLoader, LoadedProjectInput, ProjectSourceLoadOptions, ProjectSourceLoadScope,
    ProjectSourceLoader, SourceLoader,
};
pub use module_catalog::{
    ModuleCatalog, ModuleExport, ModuleExportTable, ModuleExportTarget, ModuleKey,
    ModuleNamespaceNode, ModuleNamespaceTree, ModuleOrigin as CatalogModuleOrigin,
    ModuleOriginKind as CatalogModuleOriginKind, ModuleRecord,
};
pub use module_index::{
    AstBodyRef, AstImportRef, AstItemKind, AstItemRef, ExportTable, ExportedItem, ModuleIndex,
    ModuleInfo, ModuleOrigin, ModulePart, ReExport,
};
pub use reachability::ReachabilityFacts;
pub use resolution_facts::{
    HirExprPathResolution, HirPathResolution, ImportTarget, ResolvedImport, ResolvedImports,
    ResolvedModulePath, ResolvedModuleTarget, ResolvedPaths, ResolvedWildcardImport,
};
pub use runtime_source::{
    RuntimeSourceReason, RuntimeSourceReasonKind, RuntimeSourceRequirement,
    RuntimeSourceRequirements,
};
pub(crate) use source::source_root_for_project_input;
pub use source::{
    HirOutput, ParseOutput, ParsedSource, SourceBundle, SourceFile, SourceInput, SourceKind,
    SourceSet,
};
pub use unit_tree::{UnitKind, UnitNode, UnitTarget, UnitTree};
