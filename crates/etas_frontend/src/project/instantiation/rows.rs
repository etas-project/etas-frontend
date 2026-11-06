use etas_effects::{EffectRegistry, EffectRow};
use etas_types::{CallableGenericParamKind, CallableSignature, EffectRowRef};

pub(crate) fn checked_row(
    row: &EffectRow,
    registry: &EffectRegistry,
    owner: Option<&CallableSignature>,
    scoped_parameters: impl IntoIterator<Item = String>,
) -> Result<EffectRowRef, String> {
    let mut parameters = owner
        .into_iter()
        .flat_map(|signature| &signature.generic_params)
        .filter(|parameter| parameter.kind == CallableGenericParamKind::Effect)
        .map(|parameter| parameter.name.clone())
        .collect::<Vec<_>>();
    for parameter in scoped_parameters {
        if !parameters.contains(&parameter) {
            parameters.push(parameter);
        }
    }
    etas_effects::facts::checked_type_effect_row(row, registry, &parameters)
}
