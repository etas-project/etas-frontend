use super::ExternalMetadataLoweringError;
use crate::{
    ExternalEffectArgInput as Arg, ExternalEffectRefInput as Effect, ExternalEffectRowInput as Row,
    ExternalRecordFieldInput as Field, ExternalTypeInput as Ty,
};
use etas_package_metadata::{EffectArgKind, TypeKind};

pub fn package_type_input(
    ty: &etas_package_metadata::Type,
) -> Result<Ty, ExternalMetadataLoweringError> {
    Projection::default().ty(ty, 0)
}

pub fn package_effect_row_input(
    row: &etas_package_metadata::EffectRow,
) -> Result<Row, ExternalMetadataLoweringError> {
    Projection::default().row(row, 0)
}

pub fn package_effect_arg_input(
    arg: &etas_package_metadata::EffectArg,
) -> Result<Arg, ExternalMetadataLoweringError> {
    Projection::default().arg(arg, 0)
}

#[derive(Default)]
struct Projection {
    nodes: usize,
}

impl Projection {
    fn enter(&mut self, depth: usize) -> Result<(), ExternalMetadataLoweringError> {
        self.nodes += 1;
        if depth > etas_package_metadata::MAX_METADATA_GRAPH_DEPTH
            || self.nodes > etas_package_metadata::MAX_METADATA_GRAPH_NODES
        {
            return Err(error(
                "external type projection exceeds metadata resource bounds",
            ));
        }
        Ok(())
    }

    fn ty(
        &mut self,
        ty: &etas_package_metadata::Type,
        depth: usize,
    ) -> Result<Ty, ExternalMetadataLoweringError> {
        self.enter(depth)?;
        let child = |index: usize| {
            ty.children
                .get(index)
                .ok_or_else(|| error("external type projection is missing a required child"))
        };
        let single = |this: &mut Self| this.ty(child(0)?, depth + 1).map(Box::new);
        Ok(match ty.kind {
            TypeKind::Primitive => Ty::Primitive(ty.name.clone()),
            TypeKind::Var => Ty::Var(ty.name.clone()),
            TypeKind::Named => Ty::Named(ty.path.clone()),
            TypeKind::Enum => Ty::Enum {
                path: ty.path.clone(),
                variants: ty
                    .fields
                    .iter()
                    .map(|variant| {
                        Ok(Field {
                            name: variant.name.clone(),
                            ty: self.ty(&variant.ty, depth + 1)?,
                        })
                    })
                    .collect::<Result<_, ExternalMetadataLoweringError>>()?,
            },
            TypeKind::Applied => Ty::Applied {
                path: ty.path.clone(),
                args: ty
                    .children
                    .iter()
                    .map(|ty| self.ty(ty, depth + 1))
                    .collect::<Result<_, _>>()?,
            },
            TypeKind::Alias => Ty::Alias {
                path: ty.path.clone(),
                target: single(self)?,
            },
            TypeKind::Nominal => Ty::Nominal {
                path: ty.path.clone(),
                representation: ty
                    .children
                    .first()
                    .map(|ty| self.ty(ty, depth + 1).map(Box::new))
                    .transpose()?,
            },
            TypeKind::Array => Ty::Array(single(self)?),
            TypeKind::List => Ty::List(single(self)?),
            TypeKind::Set => Ty::Set(single(self)?),
            TypeKind::Range => Ty::Range(single(self)?),
            TypeKind::Slice => Ty::Slice(single(self)?),
            TypeKind::Option => Ty::Option(single(self)?),
            TypeKind::Message => Ty::Message(single(self)?),
            TypeKind::MemorySelection => Ty::MemorySelection(single(self)?),
            TypeKind::MemoryRegion => Ty::MemoryRegion(single(self)?),
            TypeKind::Map => Ty::Map {
                key: single(self)?,
                value: Box::new(self.ty(child(1)?, depth + 1)?),
            },
            TypeKind::Store => Ty::Store {
                key: single(self)?,
                value: Box::new(self.ty(child(1)?, depth + 1)?),
            },
            TypeKind::Result => Ty::Result {
                ok: single(self)?,
                err: Box::new(self.ty(child(1)?, depth + 1)?),
            },
            TypeKind::Record => Ty::Record {
                fields: ty
                    .fields
                    .iter()
                    .map(|field| {
                        Ok(Field {
                            name: field.name.clone(),
                            ty: self.ty(&field.ty, depth + 1)?,
                        })
                    })
                    .collect::<Result<_, ExternalMetadataLoweringError>>()?,
            },
            TypeKind::Tuple => Ty::Tuple(
                ty.children
                    .iter()
                    .map(|ty| self.ty(ty, depth + 1))
                    .collect::<Result<_, _>>()?,
            ),
            TypeKind::Function => {
                let (output, input) = ty
                    .children
                    .split_last()
                    .ok_or_else(|| error("external function type is missing output"))?;
                Ty::Function {
                    input: input
                        .iter()
                        .map(|ty| self.ty(ty, depth + 1))
                        .collect::<Result<_, _>>()?,
                    output: Box::new(self.ty(output, depth + 1)?),
                    effects: ty
                        .effects
                        .as_ref()
                        .map(|row| self.row(row, depth + 1))
                        .transpose()?,
                }
            }
            TypeKind::Handler => Ty::Handler {
                handled: self.row(
                    ty.effects
                        .as_ref()
                        .ok_or_else(|| error("external handler type is missing handled row"))?,
                    depth + 1,
                )?,
                produced: ty
                    .produced_effects
                    .as_ref()
                    .map(|row| self.row(row, depth + 1))
                    .transpose()?,
                result: ty
                    .children
                    .first()
                    .map(|ty| self.ty(ty, depth + 1).map(Box::new))
                    .transpose()?,
            },
            TypeKind::Trusted
            | TypeKind::Untrusted
            | TypeKind::Secret
            | TypeKind::Public
            | TypeKind::Sanitized => Ty::Trust {
                wrapper: match ty.kind {
                    TypeKind::Trusted => "trusted",
                    TypeKind::Untrusted => "untrusted",
                    TypeKind::Secret => "secret",
                    TypeKind::Public => "public",
                    TypeKind::Sanitized => "sanitized",
                    _ => return Err(error("invalid external trust wrapper")),
                }
                .into(),
                inner: single(self)?,
            },
            TypeKind::Prompt => Ty::Prompt,
            TypeKind::PromptPart => Ty::PromptPart,
            TypeKind::ResourceHandle => Ty::ResourceHandle {
                name: ty.name.clone(),
                args: ty
                    .children
                    .iter()
                    .map(|ty| self.ty(ty, depth + 1))
                    .collect::<Result<_, _>>()?,
            },
        })
    }

    fn row(
        &mut self,
        row: &etas_package_metadata::EffectRow,
        depth: usize,
    ) -> Result<Row, ExternalMetadataLoweringError> {
        self.enter(depth)?;
        Ok(Row {
            effects: row
                .effects
                .iter()
                .map(|effect| {
                    self.enter(depth + 1)?;
                    Ok(Effect {
                        path: effect.path.clone(),
                        args: effect
                            .args
                            .iter()
                            .map(|arg| self.arg(arg, depth + 2))
                            .collect::<Result<_, _>>()?,
                    })
                })
                .collect::<Result<_, ExternalMetadataLoweringError>>()?,
            tail: row.tail.clone(),
        })
    }

    fn arg(
        &mut self,
        arg: &etas_package_metadata::EffectArg,
        depth: usize,
    ) -> Result<Arg, ExternalMetadataLoweringError> {
        self.enter(depth)?;
        Ok(match arg.kind {
            EffectArgKind::Type => Arg::Type(
                self.ty(
                    arg.ty
                        .as_ref()
                        .ok_or_else(|| error("external effect type argument is missing"))?,
                    depth + 1,
                )?,
            ),
            EffectArgKind::Path => Arg::Path(arg.path.clone()),
            EffectArgKind::String => Arg::String(arg.value.clone()),
            EffectArgKind::Int => Arg::Int(arg.value.clone()),
            EffectArgKind::Wildcard => Arg::Wildcard,
        })
    }
}

fn error(message: &str) -> ExternalMetadataLoweringError {
    ExternalMetadataLoweringError {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests;
