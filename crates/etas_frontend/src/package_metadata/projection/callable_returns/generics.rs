use super::*;
use etas_package_metadata::CallableValueGenericBinding;
use etas_types::CallableGenericParam;

pub(super) fn bindings(
    parameters: &[CallableGenericParam],
    scope: Option<CallableValueScope>,
) -> Result<Vec<CallableValueGenericBinding>, PackageMetadataError> {
    parameters
        .iter()
        .enumerate()
        .map(|(position, parameter)| {
            Ok(CallableValueGenericBinding {
                label: parameter.name.clone(),
                scope: scope.ok_or_else(|| {
                    invalid("exported generic callable has no lexical binder scope")
                })?,
                position: u32::try_from(position)
                    .map_err(|_| invalid("callable generic position exceeds u32"))?,
            })
        })
        .collect()
}
