use std::collections::HashMap;

use etas_core::{Arena, SourceId, Span};
use etas_syntax::ast;

use crate::{ExternalModuleId, ExternalPackageId, ModuleId, ModulePartId, ModulePath};

#[derive(Clone, Debug, Default)]
pub struct ModuleIndex {
    pub modules: Arena<ModuleId, ModuleInfo>,
    pub parts: Arena<ModulePartId, ModulePart>,
    pub by_path: HashMap<ModulePath, ModuleId>,
    pub by_source: HashMap<SourceId, ModuleId>,
}

#[derive(Clone, Debug)]
pub struct ModuleInfo {
    pub id: ModuleId,
    pub path: ModulePath,
    pub parts: Vec<ModulePartId>,
    pub origin: ModuleOrigin,
    pub visibility_exports: ExportTable,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ModuleOrigin {
    Source,
    DependencySourceOverlay {
        package: ExternalPackageId,
        import_root: String,
    },
    VirtualStd(ModulePath),
    External {
        module: ExternalModuleId,
        path: ModulePath,
    },
}

#[derive(Clone, Debug, Default)]
pub struct ExportTable {
    pub items: HashMap<String, ExportedItem>,
    pub re_exports: Vec<ReExport>,
}

#[derive(Clone, Debug)]
pub struct ExportedItem {
    pub name: String,
    pub module: ModuleId,
    pub part: ModulePartId,
    pub item: AstItemRef,
    pub visibility: etas_hir::Visibility,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ReExport {
    pub import: AstImportRef,
    pub target: ModulePath,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ModulePart {
    pub id: ModulePartId,
    pub module: ModuleId,
    pub source: SourceId,
    pub declared_module_span: Option<Span>,
    pub imports: Vec<AstImportRef>,
    pub items: Vec<AstItemRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AstItemRef {
    pub source: SourceId,
    pub module_part: Option<ModulePartId>,
    pub index: usize,
    pub kind: AstItemKind,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum AstItemKind {
    TypeAlias,
    Type,
    Enum,
    Spec,
    Impl,
    Effect,
    TopLevelLet,
    Tool,
    Agent,
    Protocol,
    Flow,
    Error,
}

impl AstItemKind {
    pub fn from_item(item: &ast::Item) -> Self {
        match item {
            ast::Item::Alias(_) => Self::TypeAlias,
            ast::Item::Type(_) => Self::Type,
            ast::Item::Enum(_) => Self::Enum,
            ast::Item::Spec(_) => Self::Spec,
            ast::Item::Impl(_) => Self::Impl,
            ast::Item::Effect(_) => Self::Effect,
            ast::Item::TopLevelLet(_) => Self::TopLevelLet,
            ast::Item::Tool(_) => Self::Tool,
            ast::Item::Agent(_) => Self::Agent,
            ast::Item::Protocol(_) => Self::Protocol,
            ast::Item::Flow(_) => Self::Flow,
            ast::Item::Error(_) => Self::Error,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AstImportRef {
    pub source: SourceId,
    pub module_part: Option<ModulePartId>,
    pub index: usize,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AstBodyRef {
    pub source: SourceId,
    pub item: AstItemRef,
    pub span: Span,
}
