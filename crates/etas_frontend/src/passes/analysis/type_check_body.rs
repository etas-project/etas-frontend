use etas_utils::{
    ArtifactRef, ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{
    HIR_OUTPUT, TYPE_ENVIRONMENT, TYPE_OUTPUT, unit_kind_with_diagnostics,
};
use crate::{BODY_UNIT_KIND, ProjectContext};

use super::type_check_common::{body_unit_id, hir_item_for_body_unit};

pub struct TypeCheckBodyPass;

impl Pass<ProjectContext> for TypeCheckBodyPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("TypeCheckBodyPass", PassKind::Transform)
            .scope(etas_utils::PassScope::Unit(BODY_UNIT_KIND))
            .requires(ArtifactSet::from([HIR_OUTPUT, TYPE_ENVIRONMENT]))
            .produces(unit_kind_with_diagnostics(TYPE_OUTPUT, BODY_UNIT_KIND))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let unit = match body_unit_id(pass_context) {
            Ok(unit) => unit,
            Err(failure) => {
                return PassResult {
                    control: etas_utils::PassControl::Failed(failure),
                    changed: false,
                    preserved: PreservedArtifacts::All,
                    produced: ArtifactSet::new(),
                };
            }
        };
        let item = match hir_item_for_body_unit(context, pass_context) {
            Ok(item) => item,
            Err(failure) => {
                return PassResult {
                    control: etas_utils::PassControl::Failed(failure),
                    changed: false,
                    preserved: PreservedArtifacts::All,
                    produced: ArtifactSet::new(),
                };
            }
        };
        if context.type_body_outputs.contains_key(&unit) {
            return PassResult::unchanged();
        }
        let (Some(hir), Some(base)) = (&context.hir, &context.type_environment) else {
            return PassResult::failed("body type check requires prepared signature environment");
        };
        let output =
            etas_types::check_body_delta(&hir.hir, base, item, context.std_registry.clone());
        context
            .diagnostics
            .extend(output.diagnostics.iter().cloned());
        context
            .type_body_outputs
            .insert(unit, std::sync::Arc::new(output));
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::from_iter([ArtifactRef::unit_kind(TYPE_OUTPUT, BODY_UNIT_KIND)]),
        )
    }
}
