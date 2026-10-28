use etas_code::descriptor::*;
use etas_types::{CheckedSpecRef, EffectArgRef, EffectRowRef};

use super::{DefinitionDescriptorProjector, ProjectionError};
use crate::code::identity::BoundDeclaration;

impl DefinitionDescriptorProjector<'_> {
    pub(super) fn project_row(
        &mut self,
        source: EffectRowRef,
        scope: usize,
    ) -> Result<EffectRowInfo, ProjectionError> {
        let tail = source
            .tail
            .map(|name| {
                let reference = self.scopes[scope]
                    .binders
                    .get(&name)
                    .copied()
                    .ok_or(ProjectionError::UnresolvedBinder(name))?;
                if self.binders[reference.0 as usize].kind != GenericParameterKind::EffectRow {
                    return Err(ProjectionError::Invalid(
                        "effect row tail refers to a value-type binder".into(),
                    ));
                }
                Ok(reference)
            })
            .transpose()?;
        let mut effects = Vec::with_capacity(source.effects.len());
        for effect in source.effects {
            let target = self.identities.effect_target(&self.checked, &effect.name)?;
            let mut arguments = Vec::with_capacity(effect.args.len());
            for argument in effect.args {
                arguments.push(self.project_static_arg(&argument, scope)?);
            }
            effects.push(EffectInfo { target, arguments });
        }
        Ok(EffectRowInfo { effects, tail })
    }

    pub(super) fn project_static_arg(
        &mut self,
        argument: &EffectArgRef,
        scope: usize,
    ) -> Result<StaticEffectArgInfo, ProjectionError> {
        self.edge()?;
        Ok(match argument {
            EffectArgRef::Type(ty) => StaticEffectArgInfo::Type(self.type_ref(*ty, scope)?),
            EffectArgRef::Path(path) => match self.identities.callable_path(path)? {
                Some(id) => StaticEffectArgInfo::Callable(id.clone()),
                None => StaticEffectArgInfo::Resource(
                    self.identities
                        .path(path)
                        .ok_or_else(|| ProjectionError::MissingIdentity(path.join(".")))?
                        .clone(),
                ),
            },
            EffectArgRef::String(value) => {
                self.text(value)?;
                StaticEffectArgInfo::String(value.clone())
            }
            EffectArgRef::Int(text) => {
                self.text(text)?;
                StaticEffectArgInfo::Integer(text.clone())
            }
            EffectArgRef::Wildcard => StaticEffectArgInfo::Wildcard,
        })
    }

    pub(super) fn project_bounds(
        &mut self,
        declarations: Vec<BoundDeclaration>,
        scope: usize,
    ) -> Result<Vec<SpecInfo>, ProjectionError> {
        let mut bounds = Vec::with_capacity(declarations.len());
        for declaration in declarations {
            let (definition, arguments) = match declaration {
                BoundDeclaration::Checked(bound) => {
                    let definition = match bound.spec {
                        CheckedSpecRef::Source(symbol) => self
                            .identities
                            .symbol(symbol)
                            .ok_or(ProjectionError::MissingDeclaration(symbol))?
                            .clone(),
                        CheckedSpecRef::Std(path)
                        | CheckedSpecRef::External(
                            etas_types::facts::specs::ExternalSpecIdentity { path, .. },
                        ) => self
                            .identities
                            .path(&path)
                            .ok_or_else(|| ProjectionError::MissingIdentity(path.join(".")))?
                            .clone(),
                    };
                    let arguments = bound
                        .args
                        .into_iter()
                        .map(|ty| self.type_ref(ty, scope))
                        .collect::<Result<_, _>>()?;
                    (definition, arguments)
                }
                BoundDeclaration::Standard(bound) => {
                    let definition = self
                        .identities
                        .path(&bound.path)
                        .ok_or_else(|| ProjectionError::MissingIdentity(bound.path.join(".")))?
                        .clone();
                    let module = match &self.scopes[scope].owner {
                        GenericOwner::Declaration(owner) => owner.module(),
                        GenericOwner::Method(method) => method.implementation().module(),
                    }
                    .to_vec();
                    let arguments = bound
                        .args
                        .into_iter()
                        .map(|ty| self.standard_ref(ty, scope, &module))
                        .collect::<Result<_, _>>()?;
                    (definition, arguments)
                }
            };
            bounds.push(SpecInfo {
                definition,
                arguments,
            });
        }
        Ok(bounds)
    }
}
