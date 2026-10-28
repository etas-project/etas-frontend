use etas_core::Span;

use crate::{
    Arena, HirItemId, HirModuleId, PathSegment, ResolvedPath, ScopeId, SymbolId, Visibility,
};

pub type HirModuleArena = Arena<HirModuleId, HirModule>;

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct HirModule {
    pub id: HirModuleId,
    pub name: Option<ResolvedPath>,
    pub imports: Vec<HirImport>,
    pub items: Vec<HirItemId>,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct HirImport {
    pub source: HirImportSource,
    pub kind: HirImportKind,
    pub target: ResolvedPath,
    pub binding: Option<HirImportBinding>,
    pub visibility: Visibility,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub enum HirImportKind {
    Single,
    GroupMember,
    Wildcard,
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub enum HirImportSource {
    Single {
        path_span: Span,
    },
    GroupMember {
        prefix: Vec<PathSegment>,
        member: PathSegment,
        group_span: Span,
    },
    Wildcard {
        prefix: Vec<PathSegment>,
        star_span: Span,
    },
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct HirImportBinding {
    pub symbol: SymbolId,
    pub local_name: String,
    pub local_name_span: Span,
    pub is_alias: bool,
}
