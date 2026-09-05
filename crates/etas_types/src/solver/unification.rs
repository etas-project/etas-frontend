use std::collections::HashMap;

use crate::{EffectArgRef, EffectRef, EffectRowRef, Type, TypeId, TypeStore, TypeVarId};

#[derive(Clone, Debug, Default)]
pub struct Substitution {
    vars: HashMap<TypeVarId, TypeId>,
}

impl Substitution {
    pub fn insert(&mut self, var: TypeVarId, ty: TypeId) {
        self.vars.insert(var, ty);
    }

    pub fn get(&self, var: TypeVarId) -> Option<TypeId> {
        self.vars.get(&var).copied()
    }

    pub fn extend(&mut self, other: &Substitution) {
        self.vars
            .extend(other.vars.iter().map(|(var, ty)| (*var, *ty)));
    }

    pub fn iter(&self) -> impl Iterator<Item = (TypeVarId, TypeId)> + '_ {
        self.vars.iter().map(|(var, ty)| (*var, *ty))
    }
}

pub struct TypeUnifier<'a> {
    store: &'a TypeStore,
    substitution: Substitution,
}

impl<'a> TypeUnifier<'a> {
    pub fn new(store: &'a TypeStore) -> Self {
        Self {
            store,
            substitution: Substitution::default(),
        }
    }

    pub fn with_substitution(store: &'a TypeStore, substitution: Substitution) -> Self {
        Self {
            store,
            substitution,
        }
    }

    pub fn substitution(&self) -> &Substitution {
        &self.substitution
    }

    pub fn unify(&mut self, lhs: TypeId, rhs: TypeId) -> Result<(), UnifyError> {
        let lhs = self.resolve(lhs);
        let rhs = self.resolve(rhs);
        if lhs == rhs {
            return Ok(());
        }
        match (self.store.get(lhs), self.store.get(rhs)) {
            (Some(Type::Var(var)), _) => self.bind(*var, rhs),
            (_, Some(Type::Var(var))) => self.bind(*var, lhs),
            (Some(Type::Primitive(a)), Some(Type::Primitive(b))) if a == b => Ok(()),
            (Some(Type::IntegerLiteral { .. }), Some(Type::Primitive(p)))
                if is_integer_primitive(*p) =>
            {
                Ok(())
            }
            (Some(Type::Primitive(p)), Some(Type::IntegerLiteral { .. }))
                if is_integer_primitive(*p) =>
            {
                Ok(())
            }
            (Some(Type::IntegerLiteral { .. }), Some(Type::IntegerLiteral { .. })) => Ok(()),
            (Some(Type::Array(a)), Some(Type::Array(b)))
            | (Some(Type::List(a)), Some(Type::List(b)))
            | (Some(Type::Set(a)), Some(Type::Set(b)))
            | (Some(Type::Slice(a)), Some(Type::Slice(b)))
            | (Some(Type::Option(a)), Some(Type::Option(b)))
            | (Some(Type::Message(a)), Some(Type::Message(b)))
            | (Some(Type::Schema(a)), Some(Type::Schema(b)))
            | (Some(Type::MemorySelection(a)), Some(Type::MemorySelection(b)))
            | (Some(Type::MemoryRegion(a)), Some(Type::MemoryRegion(b))) => self.unify(*a, *b),
            (
                Some(Type::Trust {
                    wrapper: aw,
                    inner: a,
                }),
                Some(Type::Trust {
                    wrapper: bw,
                    inner: b,
                }),
            ) if aw == bw => self.unify(*a, *b),
            (Some(Type::Range { index: a }), Some(Type::Range { index: b })) => self.unify(*a, *b),
            (Some(Type::Map { key: ak, value: av }), Some(Type::Map { key: bk, value: bv })) => {
                self.unify(*ak, *bk)?;
                self.unify(*av, *bv)
            }
            (
                Some(Type::Store { key: ak, value: av }),
                Some(Type::Store { key: bk, value: bv }),
            ) => {
                self.unify(*ak, *bk)?;
                self.unify(*av, *bv)
            }
            (
                Some(Type::ResourceHandle(crate::ResourceHandleType::MemoryRegion {
                    schema: aschema,
                })),
                Some(Type::ResourceHandle(crate::ResourceHandleType::MemoryRegion {
                    schema: bschema,
                })),
            ) => self.unify(*aschema, *bschema),
            (
                Some(Type::ResourceHandle(crate::ResourceHandleType::ExternalTool {
                    signature: asig,
                })),
                Some(Type::ResourceHandle(crate::ResourceHandleType::ExternalTool {
                    signature: bsig,
                })),
            ) => self.unify(*asig, *bsig),
            (
                Some(Type::ResourceHandle(crate::ResourceHandleType::Other { name: an, args: aa })),
                Some(Type::ResourceHandle(crate::ResourceHandleType::Other { name: bn, args: ba })),
            ) if an == bn && aa.len() == ba.len() => {
                for (a, b) in aa.iter().copied().zip(ba.iter().copied()) {
                    self.unify(a, b)?;
                }
                Ok(())
            }
            (Some(Type::Result { ok: aok, err: ae }), Some(Type::Result { ok: bok, err: be })) => {
                self.unify(*aok, *bok)?;
                self.unify(*ae, *be)
            }
            (Some(Type::Tuple(a)), Some(Type::Tuple(b))) if a.len() == b.len() => {
                for (a, b) in a.iter().copied().zip(b.iter().copied()) {
                    self.unify(a, b)?;
                }
                Ok(())
            }
            (Some(Type::Record(a)), Some(Type::Record(b))) if a.fields.len() == b.fields.len() => {
                for field in &a.fields {
                    let Some(other) = b
                        .fields
                        .iter()
                        .find(|candidate| candidate.name == field.name)
                    else {
                        return Err(UnifyError::Mismatch { lhs, rhs });
                    };
                    self.unify(field.ty, other.ty)?;
                }
                Ok(())
            }
            (Some(Type::Function(a)), Some(Type::Function(b)))
                if a.input.len() == b.input.len() =>
            {
                for (a, b) in a.input.iter().copied().zip(b.input.iter().copied()) {
                    self.unify(a, b)?;
                }
                self.unify(a.output, b.output)
            }
            (Some(Type::Handler(a)), Some(Type::Handler(b))) if a.handled == b.handled => {
                match (a.result, b.result) {
                    (Some(a), Some(b)) => self.unify(a, b),
                    (None, None) | (None, Some(_)) | (Some(_), None) => Ok(()),
                }
            }
            (Some(Type::Named(a)), Some(Type::Named(b))) if a == b => Ok(()),
            (Some(Type::Nominal(a)), Some(Type::Nominal(b))) if a.name == b.name => Ok(()),
            (
                Some(Type::Applied {
                    constructor: ac,
                    args: aa,
                }),
                Some(Type::Applied {
                    constructor: bc,
                    args: ba,
                }),
            ) if aa.len() == ba.len() => {
                self.unify(TypeId(ac.0), TypeId(bc.0))?;
                for (a, b) in aa.iter().copied().zip(ba.iter().copied()) {
                    self.unify(a, b)?;
                }
                Ok(())
            }
            _ => Err(UnifyError::Mismatch { lhs, rhs }),
        }
    }

    fn bind(&mut self, var: TypeVarId, ty: TypeId) -> Result<(), UnifyError> {
        if self.occurs(var, ty) {
            return Err(UnifyError::OccursCheck { var, ty });
        }
        self.substitution.insert(var, ty);
        Ok(())
    }

    fn resolve(&self, ty: TypeId) -> TypeId {
        match self.store.get(ty) {
            Some(Type::Var(var)) => self.substitution.get(*var).unwrap_or(ty),
            _ => ty,
        }
    }

    fn occurs(&self, needle: TypeVarId, ty: TypeId) -> bool {
        match self.store.get(ty) {
            Some(Type::Var(var)) => *var == needle,
            Some(Type::Array(inner))
            | Some(Type::List(inner))
            | Some(Type::Set(inner))
            | Some(Type::Slice(inner))
            | Some(Type::Option(inner))
            | Some(Type::Message(inner))
            | Some(Type::Schema(inner))
            | Some(Type::MemorySelection(inner))
            | Some(Type::MemoryRegion(inner))
            | Some(Type::Range { index: inner }) => self.occurs(needle, *inner),
            Some(Type::Trust { inner, .. }) => self.occurs(needle, *inner),
            Some(Type::Map { key, value }) | Some(Type::Store { key, value }) => {
                self.occurs(needle, *key) || self.occurs(needle, *value)
            }
            Some(Type::ResourceHandle(crate::ResourceHandleType::MemoryRegion { schema })) => {
                self.occurs(needle, *schema)
            }
            Some(Type::ResourceHandle(crate::ResourceHandleType::ExternalTool { signature })) => {
                self.occurs(needle, *signature)
            }
            Some(Type::ResourceHandle(crate::ResourceHandleType::Other { args, .. })) => {
                args.iter().any(|arg| self.occurs(needle, *arg))
            }
            Some(Type::Result { ok, err }) => self.occurs(needle, *ok) || self.occurs(needle, *err),
            Some(Type::Tuple(elems)) => elems.iter().any(|elem| self.occurs(needle, *elem)),
            Some(Type::Record(record)) => record
                .fields
                .iter()
                .any(|field| self.occurs(needle, field.ty)),
            Some(Type::Function(flow)) => {
                flow.input.iter().any(|input| self.occurs(needle, *input))
                    || self.occurs(needle, flow.output)
                    || flow
                        .effects
                        .as_ref()
                        .is_some_and(|row| effect_row_occurs(self.store, needle, row))
            }
            Some(Type::Handler(handler)) => {
                handler
                    .result
                    .is_some_and(|result| self.occurs(needle, result))
                    || effect_row_occurs(self.store, needle, &handler.handled)
                    || match &handler.produced {
                        crate::HandlerProducedEffects::Infer => false,
                        crate::HandlerProducedEffects::Explicit(row) => {
                            effect_row_occurs(self.store, needle, row)
                        }
                    }
            }
            _ => false,
        }
    }
}

fn is_integer_primitive(primitive: crate::PrimitiveType) -> bool {
    matches!(
        primitive,
        crate::PrimitiveType::I8
            | crate::PrimitiveType::I16
            | crate::PrimitiveType::I32
            | crate::PrimitiveType::I64
            | crate::PrimitiveType::I128
            | crate::PrimitiveType::ISize
            | crate::PrimitiveType::U8
            | crate::PrimitiveType::U16
            | crate::PrimitiveType::U32
            | crate::PrimitiveType::U64
            | crate::PrimitiveType::U128
            | crate::PrimitiveType::USize
    )
}

fn effect_row_occurs(store: &TypeStore, needle: TypeVarId, row: &EffectRowRef) -> bool {
    row.effects
        .iter()
        .any(|effect| effect_occurs(store, needle, effect))
}

fn effect_occurs(store: &TypeStore, needle: TypeVarId, effect: &EffectRef) -> bool {
    effect.args.iter().any(|arg| match arg {
        EffectArgRef::Type(ty) => match store.get(*ty) {
            Some(Type::Var(var)) => *var == needle,
            _ => false,
        },
        _ => false,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnifyError {
    Mismatch { lhs: TypeId, rhs: TypeId },
    OccursCheck { var: TypeVarId, ty: TypeId },
}
