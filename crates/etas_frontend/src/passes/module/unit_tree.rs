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

                    let ast = &context
                        .parsed_sources
                        .iter()
                        .find(|parsed| parsed.source == item.source)
                        .expect("module item has parsed source")
                        .parse
                        .value
                        .items[item.index]
                        .item;
                    let has_body = match ast {
                        etas_syntax::ast::Item::Flow(flow) => {
                            !matches!(flow.body, etas_syntax::ast::FlowBody::Decl { .. }) || flow.params.iter().any(|param| param.default.is_some())
                        }
                        etas_syntax::ast::Item::Agent(agent) => {
                            matches!(agent.body, etas_syntax::ast::AgentBody::Source(_)) || agent.params.iter().any(|param| param.default.is_some())
                        }
                        etas_syntax::ast::Item::Tool(tool) => {
                            matches!(tool.body, etas_syntax::ast::ToolBody::Source(_)) || tool.params.iter().any(|param| param.default.is_some())
                        }
                        etas_syntax::ast::Item::Effect(effect) => effect.body.actions().iter().any(|action| action.params.iter().any(|param| param.default.is_some())),
                        etas_syntax::ast::Item::Spec(spec) => spec.items.iter().any(|member| matches!(member, etas_syntax::ast::SpecItem::FlowSignature(signature) if signature.params.iter().any(|param| param.default.is_some()))),
                        etas_syntax::ast::Item::Impl(decl) => decl.items.iter().any(|member| matches!(member,
                            etas_syntax::ast::ImplItem::Action(action) if action.params.iter().any(|param| param.default.is_some()))),
                        _ => false,
                    };
                    let mut bodies = Vec::new();
                    if has_body {
                        let body = AstBodyRef {
                            source: item.source,
                            item: item.clone(),
                            impl_member: None,
                            span: item.span,
                        };
                        bodies.push(body);
                    }
                    if item.kind == AstItemKind::Impl {
                        let Some(parsed) = context
                            .parsed_sources
                            .iter()
                            .find(|parsed| parsed.source == item.source)
                        else {
                            return PassResult::failed("impl body unit requires its parsed source");
                        };
                        let Some(etas_syntax::ast::AnnotatedItem {
                            item: etas_syntax::ast::Item::Impl(decl),
                            ..
                        }) = parsed.parse.value.items.get(item.index)
                        else {
                            return PassResult::failed(
                                "impl body unit requires its AST declaration",
                            );
                        };
                        for (index, member) in decl.items.iter().enumerate() {
                            let span = match member {
                                etas_syntax::ast::ImplItem::Flow(flow)
                                    if !matches!(
                                        flow.body,
                                        etas_syntax::ast::FlowBody::Decl { .. }
                                    ) || flow
                                        .params
                                        .iter()
                                        .any(|param| param.default.is_some()) =>
                                {
                                    flow.span
                                }
                                etas_syntax::ast::ImplItem::Tool(tool)
                                    if matches!(
                                        tool.body,
                                        etas_syntax::ast::ToolBody::Source(_)
                                    ) =>
                                {
                                    tool.span
                                }
                                _ => continue,
                            };
                            bodies.push(AstBodyRef {
                                source: item.source,
                                item: item.clone(),
                                impl_member: Some(index),
                                span,
                            });
                        }
                    }
                    for body in bodies {
                        let body_unit = nodes.alloc_with_id(|id| UnitNode {
                            id,
                            kind: UnitKind::Body,
                            parent: Some(item_unit),
                            children: Vec::new(),
                            target: UnitTarget::AstBody(body.clone()),
                            source: Some(item.source),
                            span: Some(body.span),
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

        context.units = Some(std::sync::Arc::new(UnitTree {
            root,
            nodes,
            by_target,
        }));
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(UNIT_TREE))
    }
}
