use std::collections::HashMap;

use etas_hir::HirProjectModule;
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::ProjectContext;
use crate::passes::artifacts::{
    MODULE_INDEX, MODULE_TOPO_ORDER, PARSED_SOURCE_SET, PREDECLARED_PROJECT_SYMBOLS, UNIT_TREE,
};
use crate::project::ProjectHirLoweringState;

use super::common::parsed_by_source;

pub struct PredeclareProjectSymbolsPass;

impl Pass<ProjectContext> for PredeclareProjectSymbolsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("PredeclareProjectSymbolsPass", PassKind::Transform)
            .requires(ArtifactSet::from([
                PARSED_SOURCE_SET,
                MODULE_INDEX,
                UNIT_TREE,
                MODULE_TOPO_ORDER,
            ]))
            .produces(ArtifactSet::one(PREDECLARED_PROJECT_SYMBOLS))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let modules = context.modules.as_ref().expect("module index should exist");
        let topo_order = context
            .module_topo_order
            .as_ref()
            .expect("module topo order should exist");
        let parsed_by_source = parsed_by_source(context);

        let mut module_part_to_module_index = HashMap::new();
        let mut project_modules = Vec::new();
        let mut part_sets = Vec::new();
        for (module_index, module_id) in topo_order.modules.iter().enumerate() {
            let module = modules
                .modules
                .get(*module_id)
                .expect("topological module id should resolve");
            let parts = module
                .parts
                .iter()
                .map(|part_id| {
                    module_part_to_module_index.insert(*part_id, module_index);
                    let part = modules
                        .parts
                        .get(*part_id)
                        .expect("module part id should resolve");
                    &parsed_by_source
                        .get(&part.source)
                        .expect("module part source should be parsed")
                        .parse
                        .value
                })
                .collect::<Vec<_>>();
            part_sets.push(parts);
        }
        for parts in &part_sets {
            project_modules.push(HirProjectModule {
                parts: parts.as_slice(),
            });
        }

        context.hir_lowering = Some(ProjectHirLoweringState {
            lowering: etas_hir::HirProjectLowering::new_with_std_registry(
                &project_modules,
                context.std_registry.clone(),
            ),
            module_part_to_module_index,
            normalized_parts: Default::default(),
            lowered_parts: Default::default(),
            item_bindings: Default::default(),
            total_parts: modules.parts.len(),
        });
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(PREDECLARED_PROJECT_SYMBOLS),
        )
    }
}
