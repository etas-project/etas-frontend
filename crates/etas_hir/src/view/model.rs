use std::collections::HashMap;

use crate::{
    HirBlockId, HirExprId, HirHandlerArmId, HirItemId, HirModuleId, HirPatId, HirStmtId, ScopeId,
};

#[derive(Clone, Debug, Default)]
pub struct HirTreeIndex {
    pub module_items: HashMap<HirModuleId, Vec<HirItemId>>,
    pub item_module: HashMap<HirItemId, HirModuleId>,
    pub item_body: HashMap<HirItemId, HirBodyRef>,
    pub item_bodies: HashMap<HirItemId, Vec<HirBodyRef>>,
    pub body_owner: HashMap<HirBodyRef, HirOwner>,
    pub body_blocks: HashMap<HirBodyRef, Vec<HirBlockId>>,
    pub body_exprs: HashMap<HirBodyRef, Vec<HirExprId>>,
    pub block_owner: HashMap<HirBlockId, HirOwner>,
    pub stmt_block: HashMap<HirStmtId, HirBlockId>,
    pub expr_owner: HashMap<HirExprId, HirOwner>,
    pub handler_arm_owner: HashMap<HirHandlerArmId, HirOwner>,
    pub pat_owner: HashMap<HirPatId, HirOwner>,
    pub scope_children: HashMap<ScopeId, Vec<ScopeId>>,
    pub scope_by_owner: HashMap<HirOwner, Vec<ScopeId>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirTreeIndexError {
    pub diagnostics: Vec<HirTreeIndexDiagnostic>,
}

impl std::fmt::Display for HirTreeIndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid HIR tree index")?;
        for diagnostic in &self.diagnostics {
            write!(f, "\n  - {diagnostic:?}")?;
        }
        Ok(())
    }
}

impl std::error::Error for HirTreeIndexError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirTreeIndexDiagnostic {
    MissingNode {
        kind: HirTreeNodeKind,
        id: u32,
        owner: Option<HirOwner>,
    },
    DuplicateOwner {
        kind: HirTreeNodeKind,
        id: u32,
        previous: HirOwner,
        next: HirOwner,
    },
    OrphanNode {
        kind: HirTreeNodeKind,
        id: u32,
    },
    MissingScope {
        scope: ScopeId,
        owner: Option<HirOwner>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HirTreeNodeKind {
    Module,
    Item,
    Body,
    Block,
    Statement,
    Expr,
    HandlerArm,
    Pattern,
    Scope,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HirBodyRef {
    pub item: HirItemId,
    pub kind: HirBodyKind,
}

impl HirBodyRef {
    pub fn new(item: HirItemId, kind: HirBodyKind) -> Self {
        Self { item, kind }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HirBodyKind {
    Flow,
    Tool,
    Agent,
    TopLevelLet,
    ImplFlow(u32),
    AnnotationArg {
        annotation: u32,
        arg: u32,
    },
    Lambda(HirExprId),
    HandlerArm(HirHandlerArmId),
    MatchArm {
        expr: HirExprId,
        arm: u32,
    },
    StageLimit {
        expr: HirExprId,
        stage: u32,
        limit: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HirOwner {
    Module(HirModuleId),
    Item(HirItemId),
    Body(HirBodyRef),
    Block(HirBlockId),
    Stmt(HirStmtId),
    Expr(HirExprId),
    HandlerArm(HirHandlerArmId),
    Pattern(HirPatId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HirTreeChild {
    Expr(HirExprId),
    Block(HirBlockId),
    Stmt(HirStmtId),
    HandlerArm(HirHandlerArmId),
    Pattern(HirPatId),
}
