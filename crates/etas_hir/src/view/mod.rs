//! Derived structural HIR view for flat HIR arenas.
//!
//! `HirProgram` arenas are the storage layer. New analysis passes should prefer
//! entering HIR through `HirTreeView`, `ModuleView`, `ItemView`, `BodyView`, and
//! traversal helpers instead of globally scanning `program.items`,
//! `program.exprs`, or `program.blocks`. Direct arena scans are reserved for
//! dump/debug code, validation, and index construction.
//!
//! This module is not lexical scope storage. `crate::scope::ScopeTree` owns
//! scope parent links, namespaces, and symbol lookup. `HirTreeIndex` only builds
//! derived read indexes such as `scope_children` and `scope_by_owner`; any HIR
//! transform that mutates scopes must update `ScopeTree` through the lowering or
//! transform editor path and then rebuild this index.

mod helpers;
mod index;
mod model;
mod tree;
mod visit;

pub use model::{
    HirBodyKind, HirBodyRef, HirOwner, HirTreeChild, HirTreeIndex, HirTreeIndexDiagnostic,
    HirTreeIndexError, HirTreeNodeKind,
};
pub use tree::{
    BlockView, BodyView, ExprView, HandlerArmView, HirTreeView, ItemView, ModuleView, PatternView,
    ScopeView, StmtView,
};
pub use visit::{HirVisitor, walk_block, walk_body, walk_expr, walk_item, walk_module};
