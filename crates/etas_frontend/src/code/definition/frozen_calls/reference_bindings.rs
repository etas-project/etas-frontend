use super::{projection, relocate, *};
use etas_code::{CallableId, descriptor::DescriptorLimits};
use etas_effects::{CallableReferenceTarget, CallableReturnReference};
use etas_hir::HirItemId;
use etas_types::{EffectRowRef, TypeId};

/// A checked application in one native type universe. Creation expression IDs
/// are deliberately excluded: moving an uncaptured reference does not change
/// its declaration, generic arguments or creation scope.
#[derive(PartialEq, Eq)]
pub(crate) struct ReferenceBindings {
    pub(super) owner: HirItemId,
    pub(super) target: CallableReferenceTarget,
    pub(super) signature: TypeId,
    pub(super) types: Vec<(String, TypeId)>,
    pub(super) rows: Vec<(String, EffectRowRef)>,
}

impl ReferenceBindings {
    pub(crate) fn checked(reference: &CallableReturnReference) -> Option<Self> {
        reference.environment.is_none().then(|| Self {
            owner: reference.owner,
            target: reference.target.clone(),
            signature: reference.signature,
            types: reference.type_bindings.clone(),
            rows: reference.effect_row_bindings.clone(),
        })
    }
}

pub(crate) fn relocate_reference_bindings(
    overlay: &crate::code::draft::DefaultBodyOverlay,
    candidate: &crate::CheckedProgram,
    owner: &CallableId,
    references: &[&CallableReturnReference],
    limits: DescriptorLimits,
) -> Result<Vec<ReferenceBindings>, ProjectionError> {
    let frozen = overlay.frozen();
    let original = frozen.checked();
    let mut source_input: DescriptorInput<'_> = original.as_ref().into();
    source_input.effects = None;
    let mut target_input: DescriptorInput<'_> = candidate.as_ref().into();
    target_input.effects = None;
    let from = projection::Projection::build(source_input, &frozen.origins_for(original)?, limits)?;
    let to = projection::Projection::build(target_input, &frozen.origins_for(candidate)?, limits)?;
    let relocation = relocate::Relocation::new(original, target_input, &from, &to, overlay)?;
    let scope = projection::owner(owner)?;
    references
        .iter()
        .map(|reference| relocation.reference_bindings(reference, &scope))
        .collect()
}
