use std::collections::HashMap;

use etas_core::{Arena, SourceId, Span};
use etas_hir::{HirBlockId, HirExprId, HirItemId, HirModuleId};

use crate::{AstBodyRef, AstItemRef, ModuleId, ModulePartId, ProjectId, UnitId};

#[derive(Clone, Debug)]
pub struct UnitTree {
    pub root: UnitId,
    pub nodes: Arena<UnitId, UnitNode>,
    pub by_target: HashMap<UnitTarget, UnitId>,
}

#[derive(Clone, Debug)]
pub struct UnitNode {
    pub id: UnitId,
    pub kind: UnitKind,
    pub parent: Option<UnitId>,
    pub children: Vec<UnitId>,
    pub target: UnitTarget,
    pub source: Option<SourceId>,
    pub span: Option<Span>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnitKind {
    Project,
    SourceFile,
    Module,
    ModulePart,
    Item,
    Body,
    Block,
    Expression,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum UnitTarget {
    Project(ProjectId),
    Source(SourceId),
    Module(ModuleId),
    ModulePart(ModulePartId),
    AstItem(AstItemRef),
    AstBody(AstBodyRef),
    HirModule(HirModuleId),
    HirItem(HirItemId),
    HirBlock(HirBlockId),
    HirExpr(HirExprId),
}
