use std::collections::HashSet;

use etas_hir::{BlockView, BodyView, ExprView, HirTreeView, HirVisitor, walk_body};

use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{HIR_OUTPUT, PREDECLARED_PROJECT_SYMBOLS, global_with_diagnostics};
use crate::{HirBodyBindings, HirOutput, ProjectContext, UnitId, UnitKind, UnitNode, UnitTarget};

pub struct FinalizeProjectHirPass;

impl Pass<ProjectContext> for FinalizeProjectHirPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("FinalizeProjectHirPass", PassKind::Transform)
            .requires(ArtifactSet::one(PREDECLARED_PROJECT_SYMBOLS))
            .produces(global_with_diagnostics([HIR_OUTPUT]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let lowering = context
            .hir_lowering
            .as_ref()
            .expect("project HIR lowering state should exist");
        if lowering.lowered_parts.len() != lowering.total_parts {
            return PassResult::failed(format!(
                "cannot finalize project HIR before all module parts are lowered: {}/{} lowered",
                lowering.lowered_parts.len(),
                lowering.total_parts
            ));
        }
        let lowering = context
            .hir_lowering
            .take()
            .expect("project HIR lowering state should exist");
        let hir = lowering.lowering.finish();
        let tree_index = match etas_hir::HirTreeIndex::try_build(&hir) {
            Ok(index) => index,
            Err(error) => {
                return PassResult::failed(format!(
                    "cannot finalize project HIR with invalid tree invariants: {error}"
                ));
            }
        };
        let hir_view = HirTreeView::from_validated_index(&hir, tree_index.clone());
        context.hir_item_bindings = Some(lowering.item_bindings);
        context.hir_body_bindings = Some(build_hir_body_bindings(context, &hir_view));
        context.diagnostics.extend(hir.diagnostics.clone());
        attach_hir_units(context, &hir_view);
        context.hir = Some(HirOutput { hir, tree_index });
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(HIR_OUTPUT))
    }
}

fn build_hir_body_bindings(
    context: &ProjectContext,
    hir_view: &HirTreeView<'_>,
) -> HirBodyBindings {
    let item_bindings = context
        .hir_item_bindings
        .as_ref()
        .expect("HIR item bindings should exist");
    let tree = context.units.as_ref().expect("unit tree should exist");
    let mut bindings = HirBodyBindings::default();

    for (_, node) in tree.nodes.iter() {
        let UnitTarget::AstBody(body) = &node.target else {
            continue;
        };
        let Some(item) = item_bindings.ast_to_hir.get(&body.item).copied() else {
            continue;
        };
        let Some(root_block) = item_body_block(hir_view, item) else {
            continue;
        };
        bindings.ast_to_root_block.insert(body.clone(), root_block);
        bindings.root_block_to_ast.insert(root_block, body.clone());

        let (blocks, exprs) = collect_item_hir_nodes(hir_view, item);
        for block in &blocks {
            bindings.block_to_ast.insert(*block, body.clone());
        }
        for expr in &exprs {
            bindings.expr_to_ast.insert(*expr, body.clone());
        }
        bindings.body_blocks.insert(body.clone(), blocks);
        bindings.body_exprs.insert(body.clone(), exprs);
    }

    bindings
}

fn attach_hir_units(context: &mut ProjectContext, hir_view: &HirTreeView<'_>) {
    let item_bindings = context
        .hir_item_bindings
        .as_ref()
        .expect("HIR item bindings should exist");
    let tree = context.units.as_mut().expect("unit tree should exist");
    let body_units = tree
        .nodes
        .iter()
        .filter_map(|(body_unit, node)| {
            let UnitTarget::AstBody(body) = &node.target else {
                return None;
            };
            Some((body_unit, body.clone()))
        })
        .collect::<Vec<_>>();

    for (body_unit, ast_body) in body_units {
        let Some(item) = item_bindings.ast_to_hir.get(&ast_body.item).copied() else {
            continue;
        };
        let item = hir_view
            .item(item)
            .expect("AST item binding should point at an existing HIR item");
        let mut visitor = HirUnitAttachVisitor::new(tree, body_unit);
        for body in item.bodies() {
            walk_body(hir_view, body.id(), &mut visitor);
        }
    }
}

fn item_body_block(
    hir_view: &HirTreeView<'_>,
    item: etas_hir::HirItemId,
) -> Option<etas_hir::HirBlockId> {
    hir_view
        .item(item)?
        .body()?
        .blocks()
        .next()
        .map(|block| block.id())
}

fn collect_item_hir_nodes(
    hir_view: &HirTreeView<'_>,
    item: etas_hir::HirItemId,
) -> (Vec<etas_hir::HirBlockId>, Vec<etas_hir::HirExprId>) {
    let mut collector = BodyBindingCollector::default();
    let item = hir_view
        .item(item)
        .expect("AST item binding should point at an existing HIR item");
    for body in item.bodies() {
        walk_body(hir_view, body.id(), &mut collector);
    }
    (collector.blocks, collector.exprs)
}

#[derive(Default)]
struct BodyBindingCollector {
    seen_blocks: HashSet<etas_hir::HirBlockId>,
    seen_exprs: HashSet<etas_hir::HirExprId>,
    blocks: Vec<etas_hir::HirBlockId>,
    exprs: Vec<etas_hir::HirExprId>,
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for BodyBindingCollector {
    fn enter_block(&mut self, block: BlockView<'view, 'hir>) {
        if self.seen_blocks.insert(block.id()) {
            self.blocks.push(block.id());
        }
    }

    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        if self.seen_exprs.insert(expr.id()) {
            self.exprs.push(expr.id());
        }
    }
}

struct HirUnitAttachVisitor<'tree> {
    tree: &'tree mut crate::UnitTree,
    body_unit: UnitId,
    block_stack: Vec<UnitId>,
    seen_blocks: HashSet<etas_hir::HirBlockId>,
    seen_exprs: HashSet<etas_hir::HirExprId>,
}

impl<'tree> HirUnitAttachVisitor<'tree> {
    fn new(tree: &'tree mut crate::UnitTree, body_unit: UnitId) -> Self {
        Self {
            tree,
            body_unit,
            block_stack: Vec::new(),
            seen_blocks: HashSet::new(),
            seen_exprs: HashSet::new(),
        }
    }

    fn current_parent(&self) -> UnitId {
        self.block_stack.last().copied().unwrap_or(self.body_unit)
    }

    fn attach_block(&mut self, block: BlockView<'_, '_>) -> UnitId {
        let block_id = block.id();
        if let Some(existing) = self
            .tree
            .by_target
            .get(&UnitTarget::HirBlock(block_id))
            .copied()
        {
            return existing;
        }

        let block_data = block.data();
        let parent = self.current_parent();
        let unit = self.tree.nodes.alloc_with_id(|id| UnitNode {
            id,
            kind: UnitKind::Block,
            parent: Some(parent),
            children: Vec::new(),
            target: UnitTarget::HirBlock(block_id),
            source: Some(block_data.span.source),
            span: Some(block_data.span),
        });
        self.tree
            .nodes
            .get_mut(parent)
            .expect("HIR unit parent should exist")
            .children
            .push(unit);
        self.tree
            .by_target
            .insert(UnitTarget::HirBlock(block_id), unit);
        unit
    }

    fn attach_expr(&mut self, expr: ExprView<'_, '_>) {
        let expr_id = expr.id();
        if self
            .tree
            .by_target
            .contains_key(&UnitTarget::HirExpr(expr_id))
        {
            return;
        }

        let span = expr.source_span();
        let parent = self.current_parent();
        let unit = self.tree.nodes.alloc_with_id(|id| UnitNode {
            id,
            kind: UnitKind::Expression,
            parent: Some(parent),
            children: Vec::new(),
            target: UnitTarget::HirExpr(expr_id),
            source: Some(span.source),
            span: Some(span),
        });
        self.tree
            .nodes
            .get_mut(parent)
            .expect("HIR unit parent should exist")
            .children
            .push(unit);
        self.tree
            .by_target
            .insert(UnitTarget::HirExpr(expr_id), unit);
    }
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for HirUnitAttachVisitor<'_> {
    fn enter_body(&mut self, _body: BodyView<'view, 'hir>) {
        self.block_stack.clear();
    }

    fn enter_block(&mut self, block: BlockView<'view, 'hir>) {
        if self.seen_blocks.insert(block.id()) {
            let unit = self.attach_block(block);
            self.block_stack.push(unit);
            return;
        }
        let unit = self
            .tree
            .by_target
            .get(&UnitTarget::HirBlock(block.id()))
            .copied()
            .expect("seen HIR block should have an attached unit");
        self.block_stack.push(unit);
    }

    fn exit_block(&mut self, _block: BlockView<'view, 'hir>) {
        self.block_stack
            .pop()
            .expect("HIR block exit should match an entered block");
    }

    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        if self.seen_exprs.insert(expr.id()) {
            self.attach_expr(expr);
        }
    }
}
