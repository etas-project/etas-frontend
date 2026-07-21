use etas_core::Span;

use crate::{
    HirExprId, HirItemId, HirModuleId, HirPatId, HirStmtId, HirTypeId, HirTypeParamBound, SymbolId,
    Visibility,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub visibility: Visibility,
    pub defining_module: HirModuleId,
    pub defining_item: Option<HirItemId>,
    pub def: SymbolDef,
    pub declared_type: Option<HirTypeId>,
    pub definition_span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolData {
    pub name: String,
    pub kind: SymbolKind,
    pub visibility: Visibility,
    pub defining_module: HirModuleId,
    pub defining_item: Option<HirItemId>,
    pub def: SymbolDef,
    pub declared_type: Option<HirTypeId>,
    pub definition_span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Module,
    TypeAlias,
    Type,
    Enum,
    Spec,
    EnumVariant,
    Flow,
    Agent,
    Tool,
    TopLevelLet,
    Protocol,
    Effect,
    EffectAction,
    Param,
    Local,
    Field,
    TypeParam,
    EffectParam,
    Import,
    StdPreludeAlias,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SymbolDef {
    Module {
        module: HirModuleId,
    },
    Item {
        item: HirItemId,
    },
    TopLevelLet {
        item: HirItemId,
        ty: Option<HirTypeId>,
        initializer: HirExprId,
        classification: TopLevelLetClassification,
    },
    Param {
        owner: HirItemId,
        param_index: u32,
        pattern: Option<HirPatId>,
        ty: Option<HirTypeId>,
    },
    Local {
        binding: HirStmtId,
        pattern: HirPatId,
        ty: Option<HirTypeId>,
        initializer: Option<HirExprId>,
    },
    PatternBinding {
        owner: PatternBindingOwner,
        pattern: HirPatId,
        ty: Option<HirTypeId>,
        initializer: Option<HirExprId>,
    },
    Field {
        owner: HirItemId,
        field_index: u32,
        ty: HirTypeId,
    },
    EffectAction {
        declaring_item: HirItemId,
        owner_effect: Option<SymbolId>,
        action_index: u32,
    },
    TypeParam {
        owner: HirItemId,
        param_index: u32,
        bounds: Vec<HirTypeParamBound>,
    },
    EffectParam {
        owner: HirItemId,
        param_index: u32,
    },
    EnumVariant {
        enum_item: HirItemId,
        variant_index: u32,
    },
    ImportAlias {
        path: Vec<String>,
        origin: ImportAliasOrigin,
    },
    Synthetic {
        reason: SyntheticSymbolReason,
    },
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportAliasOrigin {
    SourceImport,
    StdPrelude,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternBindingOwner {
    MatchArm { expr: HirExprId },
    Handler { expr: HirExprId },
    For { stmt: HirStmtId },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TopLevelLetClassification {
    Unknown,
    Const,
    Handler,
    ResourceHandle(ResourceKind),
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ResourceKind {
    MemoryRegion,
    ExternalTool,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyntheticSymbolReason {
    ImportAlias,
    QualifiedEffectAction,
    ExternalEffectAction,
    Recovery,
}
