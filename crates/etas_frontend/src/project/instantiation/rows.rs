use etas_effects::{EffectRegistry, EffectRow};
use etas_types::{CallableGenericParamKind, CallableSignature, EffectRowRef};

pub(crate) fn checked_row(
    row: &EffectRow,
    registry: &EffectRegistry,
    owner: Option<&CallableSignature>,
) -> Result<EffectRowRef, String> {
    let parameters = owner
        .into_iter()
        .flat_map(|signature| &signature.generic_params)
        .filter(|parameter| parameter.kind == CallableGenericParamKind::Effect)
        .map(|parameter| parameter.name.clone())
        .collect::<Vec<_>>();
    etas_effects::facts::checked_type_effect_row(row, registry, &parameters)
}
