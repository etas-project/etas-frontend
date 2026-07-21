use std::collections::HashMap;

use etas_core::Span;

use crate::{
    HirBlockId, HirExprId, HirHandlerArmId, HirItemId, HirPatId, HirStmtId, HirTypeId, SymbolId,
};

etas_core::id_type!(SyntaxNodeId);

#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    pub item_sources: HashMap<HirItemId, HirOrigin>,
    pub expr_sources: HashMap<HirExprId, HirOrigin>,
    pub handler_arm_sources: HashMap<HirHandlerArmId, HirOrigin>,
    pub stmt_sources: HashMap<HirStmtId, HirOrigin>,
    pub pat_sources: HashMap<HirPatId, HirOrigin>,
    pub type_sources: HashMap<HirTypeId, HirOrigin>,
    pub block_sources: HashMap<HirBlockId, HirOrigin>,
    pub symbol_sources: HashMap<SymbolId, HirOrigin>,
    pub syntax_nodes: HashMap<SyntaxNodeId, SyntaxNodeRef>,
    pub syntax_to_hir: HashMap<SyntaxNodeId, Vec<HirNodeRef>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntaxNodeRef {
    pub id: SyntaxNodeId,
    pub span: Span,
    pub kind: SyntaxNodeKind,
    pub parent: Option<SyntaxNodeId>,
    pub child_index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SyntaxNodeKind {
    Item,
    Expr,
    HandlerArm,
    Stmt,
    Pattern,
    Type,
    Block,
    Symbol,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirOrigin {
    Direct(SyntaxNodeRef),
    Desugared {
        primary: SyntaxNodeRef,
        related: Vec<SyntaxNodeRef>,
        desugaring: DesugaringKind,
    },
    Synthetic {
        span: Span,
        reason: SyntheticOriginReason,
    },
    Error {
        span: Span,
    },
}

impl HirOrigin {
    pub fn span(&self) -> Span {
        match self {
            Self::Direct(source) => source.span,
            Self::Desugared { primary, .. } => primary.span,
            Self::Synthetic { span, .. } | Self::Error { span } => *span,
        }
    }

    pub fn primary_syntax(&self) -> Option<SyntaxNodeRef> {
        match self {
            Self::Direct(source) => Some(*source),
            Self::Desugared { primary, .. } => Some(*primary),
            Self::Synthetic { .. } | Self::Error { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesugaringKind {
    MatchStatementExpr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntheticOriginReason {
    LoweringGenerated,
    Recovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HirNodeRef {
    Item(HirItemId),
    Expr(HirExprId),
    HandlerArm(HirHandlerArmId),
    Stmt(HirStmtId),
    Pat(HirPatId),
    Type(HirTypeId),
    Block(HirBlockId),
    Symbol(SymbolId),
}

#[derive(Clone, Debug, Default)]
pub struct SourceMapBuilder {
    source_map: SourceMap,
    syntax_node_ids: HashMap<SyntaxNodeKey, SyntaxNodeId>,
}

impl SourceMapBuilder {
    pub fn finish(self) -> SourceMap {
        self.source_map
    }

    pub fn map_item(&mut self, id: HirItemId, span: Span) {
        let origin = self.direct_origin(SyntaxNodeKind::Item, span);
        self.record(origin.clone(), HirNodeRef::Item(id));
        self.source_map.item_sources.insert(id, origin);
    }

    pub fn map_expr(&mut self, id: HirExprId, span: Span) {
        let origin = self.direct_origin(SyntaxNodeKind::Expr, span);
        self.record(origin.clone(), HirNodeRef::Expr(id));
        self.source_map.expr_sources.insert(id, origin);
    }

    pub fn map_handler_arm(&mut self, id: HirHandlerArmId, span: Span) {
        let origin = self.direct_origin(SyntaxNodeKind::HandlerArm, span);
        self.record(origin.clone(), HirNodeRef::HandlerArm(id));
        self.source_map.handler_arm_sources.insert(id, origin);
    }

    pub fn map_stmt(&mut self, id: HirStmtId, span: Span) {
        let origin = self.direct_origin(SyntaxNodeKind::Stmt, span);
        self.record(origin.clone(), HirNodeRef::Stmt(id));
        self.source_map.stmt_sources.insert(id, origin);
    }

    pub fn map_pat(&mut self, id: HirPatId, span: Span) {
        let origin = self.direct_origin(SyntaxNodeKind::Pattern, span);
        self.record(origin.clone(), HirNodeRef::Pat(id));
        self.source_map.pat_sources.insert(id, origin);
    }

    pub fn map_type(&mut self, id: HirTypeId, span: Span) {
        let origin = self.direct_origin(SyntaxNodeKind::Type, span);
        self.record(origin.clone(), HirNodeRef::Type(id));
        self.source_map.type_sources.insert(id, origin);
    }

    pub fn map_block(&mut self, id: HirBlockId, span: Span) {
        let origin = self.direct_origin(SyntaxNodeKind::Block, span);
        self.record(origin.clone(), HirNodeRef::Block(id));
        self.source_map.block_sources.insert(id, origin);
    }

    pub fn map_symbol(&mut self, id: SymbolId, span: Span) {
        let origin = self.direct_origin(SyntaxNodeKind::Symbol, span);
        self.record(origin.clone(), HirNodeRef::Symbol(id));
        self.source_map.symbol_sources.insert(id, origin);
    }

    fn direct_origin(&mut self, kind: SyntaxNodeKind, span: Span) -> HirOrigin {
        HirOrigin::Direct(self.syntax_ref(kind, span))
    }

    fn syntax_ref(&mut self, kind: SyntaxNodeKind, span: Span) -> SyntaxNodeRef {
        let key = SyntaxNodeKey { kind, span };
        if let Some(id) = self.syntax_node_ids.get(&key).copied() {
            return *self
                .source_map
                .syntax_nodes
                .get(&id)
                .expect("syntax node id should have a source ref");
        }

        let id = SyntaxNodeId(self.source_map.syntax_nodes.len().min(u32::MAX as usize) as u32);
        let source = SyntaxNodeRef {
            id,
            span,
            kind,
            parent: None,
            child_index: 0,
        };
        self.syntax_node_ids.insert(key, id);
        self.source_map.syntax_nodes.insert(id, source);
        source
    }

    fn record(&mut self, origin: HirOrigin, node: HirNodeRef) {
        if let Some(source) = origin.primary_syntax() {
            self.source_map
                .syntax_to_hir
                .entry(source.id)
                .or_default()
                .push(node);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SyntaxNodeKey {
    kind: SyntaxNodeKind,
    span: Span,
}
