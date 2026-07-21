use std::collections::HashMap;

use etas_core::Span;

use crate::{AstImportRef, ModuleId, ModulePartId, ModulePath};

#[derive(Clone, Debug, Default)]
pub struct ImportGraph {
    pub edges: Vec<ImportEdge>,
    pub reverse_edges: HashMap<ModuleId, Vec<usize>>,
}

#[derive(Clone, Debug)]
pub struct ImportEdge {
    pub from: ModuleId,
    pub from_part: ModulePartId,
    pub candidate: ModulePath,
    pub resolved: Option<ModuleId>,
    pub import: AstImportRef,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct ModuleTopoOrder {
    pub modules: Vec<ModuleId>,
    pub cyclic_modules: Vec<ModuleId>,
}
