use super::*;
use etas_effects::facts::{CallableEnvironmentId, CallableGenericParameter, CallableGenericValue};
use etas_types::{CallableGenericParam, CallableGenericParamKind};

mod expansion;

#[derive(Default)]
pub(super) struct CapturedProjection {
    pub bindings: Vec<etas_package_metadata::CallableValueGenericBinding>,
    pub type_names: BTreeSet<String>,
    pub effect_names: BTreeSet<String>,
    pub owner_type_names: BTreeSet<String>,
    pub types: BTreeMap<String, MetadataType>,
    pub rows: BTreeMap<String, MetadataEffectRow>,
}

impl Builder<'_, '_> {
    pub(super) fn captured_projection(
        &self,
        owner: HirItemId,
        parameters: &[CallableGenericParam],
        environment: CallableEnvironmentId,
    ) -> Result<CapturedProjection, PackageMetadataError> {
        let snapshot = self
            .projection
            .checked
            .effects
            .callable_environments
            .get(&environment)
            .ok_or_else(|| invalid("captured descriptor has no checked environment"))?;
        let mut projection = self.generic_context(Some(snapshot.owner))?;
        for (position, parameter) in parameters.iter().enumerate() {
            let value = snapshot
                .generics
                .get(&CallableGenericParameter::Native {
                    owner,
                    position: position as u32,
                })
                .ok_or_else(|| {
                    invalid("captured descriptor is missing its checked generic argument")
                })?;
            match (parameter.kind, value) {
                (CallableGenericParamKind::Type, CallableGenericValue::Type(ty)) => {
                    projection.owner_type_names.insert(parameter.name.clone());
                    let mut ty = self.projection.metadata_type(*ty)?;
                    normalize_generic_type(&mut ty, &projection.type_names);
                    projection.types.insert(parameter.name.clone(), ty);
                }
                (CallableGenericParamKind::Effect, CallableGenericValue::EffectRow(row)) => {
                    projection.rows.insert(
                        parameter.name.clone(),
                        self.projection.effect_row_metadata(
                            row,
                            &projection.type_names,
                            &projection.effect_names,
                        )?,
                    );
                }
                _ => {
                    return Err(invalid(
                        "captured generic argument has the wrong checked kind",
                    ));
                }
            }
        }
        Ok(projection)
    }

    pub(super) fn generic_context(
        &self,
        owner: Option<HirItemId>,
    ) -> Result<CapturedProjection, PackageMetadataError> {
        let Some(owner) = owner else {
            return Ok(CapturedProjection::default());
        };
        let free_parameters = match self.projection.checked.types.item_signatures.get(&owner) {
            Some(ItemSignature::Flow(signature) | ItemSignature::Tool(signature)) => {
                signature.generic_params.as_slice()
            }
            Some(ItemSignature::Agent(signature)) => signature
                .callable()
                .map_or(&[][..], |signature| signature.generic_params.as_slice()),
            Some(ItemSignature::TopLevelLet(_)) => &[],
            None => {
                return Err(invalid(
                    "captured descriptor has no lexical signature facts",
                ));
            }
        };
        let type_names = free_parameters
            .iter()
            .filter(|parameter| parameter.kind == CallableGenericParamKind::Type)
            .map(|parameter| parameter.name.clone())
            .collect::<BTreeSet<_>>();
        let effect_names = free_parameters
            .iter()
            .filter(|parameter| parameter.kind == CallableGenericParamKind::Effect)
            .map(|parameter| parameter.name.clone())
            .collect::<BTreeSet<_>>();
        let scope = if owner == self.root {
            Some(CallableValueScope::Root)
        } else {
            let mut matches = self
                .units
                .iter()
                .filter(|(instance, _)| instance.unit == EffectUnit::Item(owner));
            match (matches.next(), matches.next()) {
                (Some((_, index)), None) => Some(CallableValueScope::Value(*index)),
                _ => None,
            }
        };
        let mut bindings = Vec::new();
        for (position, parameter) in free_parameters.iter().enumerate() {
            bindings.push(etas_package_metadata::CallableValueGenericBinding {
                label: parameter.name.clone(),
                scope: scope.ok_or_else(|| {
                    invalid("captured generic has no unique exported lexical scope")
                })?,
                position: u32::try_from(position)
                    .map_err(|_| invalid("captured generic position exceeds u32"))?,
            });
        }
        Ok(CapturedProjection {
            bindings,
            owner_type_names: type_names.clone(),
            type_names,
            effect_names,
            ..Default::default()
        })
    }
}

enum Part<'a> {
    Type(&'a mut MetadataType),
    Row(&'a mut MetadataEffectRow),
}

impl CapturedProjection {
    pub fn metadata_type(
        &self,
        projection: &ProjectMetadataProjection<'_>,
        ty: TypeId,
    ) -> Result<MetadataType, PackageMetadataError> {
        let mut ty = projection.metadata_type(ty)?;
        normalize_generic_type(&mut ty, &self.owner_type_names);
        expansion::validate(&ty, &self.types, &self.rows)?;
        let mut pending = vec![Part::Type(&mut ty)];
        while let Some(part) = pending.pop() {
            match part {
                Part::Type(ty) => {
                    if ty.kind == MetadataTypeKind::Var
                        && let Some(argument) = self.types.get(&ty.name)
                    {
                        // Arguments already use the caller's binder namespace.
                        *ty = argument.clone();
                        continue;
                    }
                    pending.extend(ty.children.iter_mut().map(Part::Type));
                    pending.extend(ty.fields.iter_mut().map(|field| Part::Type(&mut field.ty)));
                    pending.extend(
                        ty.effects
                            .iter_mut()
                            .chain(&mut ty.produced_effects)
                            .map(Part::Row),
                    );
                }
                Part::Row(row) => {
                    let original_members = row.effects.len();
                    if let Some(argument) = row.tail.as_ref().and_then(|tail| self.rows.get(tail)) {
                        row.effects.extend(argument.effects.iter().cloned());
                        row.tail = argument.tail.clone();
                    }
                    pending.extend(
                        row.effects
                            .iter_mut()
                            .take(original_members)
                            .flat_map(|effect| &mut effect.args)
                            .filter_map(|arg| arg.ty.as_mut())
                            .map(Part::Type),
                    );
                }
            }
        }
        Ok(ty)
    }
}
