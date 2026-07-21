use std::collections::HashMap;

use etas_core::{Arena, Span, TextSize};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{MODULE_INDEX, PARSED_SOURCE_SET, SOURCE_SET, UNIT_TREE};
use crate::{
    AstBodyRef, AstItemKind, ProjectContext, ProjectId, UnitKind, UnitNode, UnitTarget, UnitTree,
};

pub struct BuildUnitTreePass;

impl Pass<ProjectContext> for BuildUnitTreePass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("BuildUnitTreePass", PassKind::Transform)
            .requires(ArtifactSet::from([
                SOURCE_SET,
                PARSED_SOURCE_SET,
                MODULE_INDEX,
            ]))
            .produces(ArtifactSet::one(UNIT_TREE))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let sources = context.sources.as_ref().expect("source set should exist");
        let modules = context.modules.as_ref().expect("module index should exist");
        let mut nodes = Arena::default();
        let mut by_target = HashMap::new();
        let root = nodes.alloc_with_id(|id| UnitNode {
            id,
            kind: UnitKind::Project,
            parent: None,
            children: Vec::new(),
            target: UnitTarget::Project(ProjectId(0)),
            source: None,
            span: None,
        });
        by_target.insert(UnitTarget::Project(ProjectId(0)), root);

        for source in &sources.files {
            let source_unit = nodes.alloc_with_id(|id| UnitNode {
                id,
                kind: UnitKind::SourceFile,
                parent: Some(root),
                children: Vec::new(),
                target: UnitTarget::Source(source.id),
                source: Some(source.id),
                span: Some(Span::empty(source.id, TextSize::ZERO)),
            });
            nodes
                .get_mut(root)
                .expect("root should exist")
                .children
                .push(source_unit);
            by_target.insert(UnitTarget::Source(source.id), source_unit);
        }

        for (module_id, module) in modules.modules.iter() {
            let module_unit = nodes.alloc_with_id(|id| UnitNode {
                id,
                kind: UnitKind::Module,
                parent: Some(root),
                children: Vec::new(),
                target: UnitTarget::Module(module_id),
                source: None,
                span: Some(module.span),
            });
            nodes
                .get_mut(root)
                .expect("root should exist")
                .children
                .push(module_unit);
            by_target.insert(UnitTarget::Module(module_id), module_unit);

            for part_id in &module.parts {
                let part = &modules.parts[*part_id];
                let part_unit = nodes.alloc_with_id(|id| UnitNode {
                    id,
                    kind: UnitKind::ModulePart,
                    parent: Some(module_unit),
                    children: Vec::new(),
                    target: UnitTarget::ModulePart(*part_id),
                    source: Some(part.source),
                    span: part.declared_module_span,
                });
                nodes
                    .get_mut(module_unit)
                    .expect("module unit should exist")
                    .children
                    .push(part_unit);
                by_target.insert(UnitTarget::ModulePart(*part_id), part_unit);

                for item in &part.items {
                    let item_unit = nodes.alloc_with_id(|id| UnitNode {
                        id,
                        kind: UnitKind::Item,
                        parent: Some(part_unit),
                        children: Vec::new(),
                        target: UnitTarget::AstItem(item.clone()),
                        source: Some(item.source),
                        span: Some(item.span),
                    });
                    nodes
                        .get_mut(part_unit)
                        .expect("module-part unit should exist")
                        .children
                        .push(item_unit);
                    by_target.insert(UnitTarget::AstItem(item.clone()), item_unit);

                    if item_has_body(item.kind) {
                        let body = AstBodyRef {
                            source: item.source,
                            item: item.clone(),
                            span: item.span,
                        };
                        let body_unit = nodes.alloc_with_id(|id| UnitNode {
                            id,
                            kind: UnitKind::Body,
                            parent: Some(item_unit),
                            children: Vec::new(),
                            target: UnitTarget::AstBody(body.clone()),
                            source: Some(item.source),
                            span: Some(item.span),
                        });
                        nodes
                            .get_mut(item_unit)
                            .expect("item unit should exist")
                            .children
                            .push(body_unit);
                        by_target.insert(UnitTarget::AstBody(body), body_unit);
                    }
                }
            }
        }

        context.units = Some(UnitTree {
            root,
            nodes,
            by_target,
        });
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(UNIT_TREE))
    }
}

fn item_has_body(kind: AstItemKind) -> bool {
    matches!(
        kind,
        AstItemKind::Flow | AstItemKind::Agent | AstItemKind::Tool
    )
}
