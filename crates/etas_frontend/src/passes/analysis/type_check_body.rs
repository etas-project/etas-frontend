use etas_utils::{
    ArtifactRef, ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{
    HIR_OUTPUT, SIGNATURE_FACTS, TYPE_OUTPUT, unit_kind_with_diagnostics,
};
use crate::{BODY_UNIT_KIND, ProjectContext};

use super::type_check_common::{body_unit_id, hir_item_for_body_unit};

pub struct TypeCheckBodyPass;

impl Pass<ProjectContext> for TypeCheckBodyPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("TypeCheckBodyPass", PassKind::Transform)
            .scope(etas_utils::PassScope::Unit(BODY_UNIT_KIND))
            .requires(ArtifactSet::from([HIR_OUTPUT, SIGNATURE_FACTS]))
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
        let hir = context.hir.as_ref().expect("HIR output should exist");
        let mut seed = context
            .signature_types
            .as_ref()
            .cloned()
            .expect("signature facts should exist before body type checking");
        seed.diagnostics.clear();
        let output = etas_types::check_body_item(&hir.hir, seed, item);
        context
            .diagnostics
            .extend(output.diagnostics.iter().cloned());
        context.type_body_outputs.insert(unit, output);
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::from_iter([ArtifactRef::unit_kind(TYPE_OUTPUT, BODY_UNIT_KIND)]),
        )
    }
}
