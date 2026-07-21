use etas_utils::{
    ArtifactRef, ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PassScope, PreservedArtifacts,
};

use crate::passes::artifacts::{
    LOWERED_MODULE_ITEMS, NORMALIZED_MODULE_IMPORTS, PREDECLARED_PROJECT_SYMBOLS,
};
use crate::{MODULE_PART_UNIT_KIND, ProjectContext};

use super::common::{current_module_part, failed_pass_result, parsed_program_for_part};

pub struct NormalizeModuleImportsPass;

impl Pass<ProjectContext> for NormalizeModuleImportsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("NormalizeModuleImportsPass", PassKind::Transform)
            .scope(PassScope::Unit(MODULE_PART_UNIT_KIND))
            .requires(ArtifactSet::one(PREDECLARED_PROJECT_SYMBOLS))
            .produces(ArtifactSet::from_iter([ArtifactRef::unit_kind(
                NORMALIZED_MODULE_IMPORTS,
                MODULE_PART_UNIT_KIND,
            )]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let part_id = match current_module_part(pass_context) {
            Ok(part_id) => part_id,
            Err(failure) => return failed_pass_result(failure),
        };
        let parsed = parsed_program_for_part(context, part_id);
        let lowering = context
            .hir_lowering
            .as_mut()
            .expect("project HIR lowering state should exist");
        let module_index = *lowering
            .module_part_to_module_index
            .get(&part_id)
            .expect("module part should have a HIR module index");
        lowering.lowering.normalize_part(module_index, &parsed);
        lowering.normalized_parts.insert(part_id);
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::from_iter([ArtifactRef::unit_kind(
                NORMALIZED_MODULE_IMPORTS,
                MODULE_PART_UNIT_KIND,
            )]),
        )
    }
}

pub struct LowerModuleItemsPass;

impl Pass<ProjectContext> for LowerModuleItemsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("LowerModuleItemsPass", PassKind::Transform)
            .scope(PassScope::Unit(MODULE_PART_UNIT_KIND))
            .requires(ArtifactSet::from_iter([
                ArtifactRef::global(PREDECLARED_PROJECT_SYMBOLS),
                ArtifactRef::unit_kind(NORMALIZED_MODULE_IMPORTS, MODULE_PART_UNIT_KIND),
            ]))
            .produces(ArtifactSet::from_iter([ArtifactRef::unit_kind(
                LOWERED_MODULE_ITEMS,
                MODULE_PART_UNIT_KIND,
            )]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let part_id = match current_module_part(pass_context) {
            Ok(part_id) => part_id,
            Err(failure) => return failed_pass_result(failure),
        };
        let parsed = parsed_program_for_part(context, part_id);
        let part_items = context
            .modules
            .as_ref()
            .expect("module index should exist")
            .parts
            .get(part_id)
            .expect("module part should exist")
            .items
            .clone();
        let lowering = context
            .hir_lowering
            .as_mut()
            .expect("project HIR lowering state should exist");
        if !lowering.normalized_parts.contains(&part_id) {
            return PassResult::failed(format!(
                "module part {:?} imports must be normalized before lowering items",
                part_id
            ));
        }
        let module_index = *lowering
            .module_part_to_module_index
            .get(&part_id)
            .expect("module part should have a HIR module index");
        let item_ids = lowering.lowering.lower_part(module_index, &parsed);
        for (ast_item, hir_item) in part_items.into_iter().zip(item_ids) {
            lowering
                .item_bindings
                .hir_to_ast
                .insert(hir_item, ast_item.clone());
            lowering.item_bindings.ast_to_hir.insert(ast_item, hir_item);
        }
        lowering.lowered_parts.insert(part_id);

        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::from_iter([ArtifactRef::unit_kind(
                LOWERED_MODULE_ITEMS,
                MODULE_PART_UNIT_KIND,
            )]),
        )
    }
}
