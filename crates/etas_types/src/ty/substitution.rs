use std::collections::{HashMap, HashSet};

use crate::solver::Substitution;

use super::{
    EffectArgRef, EffectRowRef, FieldType, FlowType, HandlerProducedEffects, HandlerType,
    RecordType, ResourceHandleType, Type, TypeId, TypeInterner, TypeStore,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeSubstitutionError {
    MissingType(TypeId),
    CyclicType(TypeId),
    UnmaterializedType(Type),
}

impl std::fmt::Display for TypeSubstitutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingType(ty) => write!(f, "type store is missing {ty:?}"),
            Self::CyclicType(ty) => write!(f, "type graph contains a cycle at {ty:?}"),
            Self::UnmaterializedType(ty) => {
                write!(f, "specialized type was not materialized: {ty:?}")
            }
        }
    }
}

impl std::error::Error for TypeSubstitutionError {}

enum SubstitutionTarget<'a> {
    Materialize(&'a mut TypeInterner),
    Existing(&'a TypeStore),
}

pub struct TypeSubstitutionEngine<'a> {
    target: SubstitutionTarget<'a>,
    named: &'a HashMap<String, TypeId>,
    variables: Option<&'a Substitution>,
    active: HashSet<TypeId>,
    memo: HashMap<TypeId, TypeId>,
}

impl<'a> TypeSubstitutionEngine<'a> {
    pub fn materializing(
        interner: &'a mut TypeInterner,
        named: &'a HashMap<String, TypeId>,
        variables: Option<&'a Substitution>,
    ) -> Self {
        Self {
            target: SubstitutionTarget::Materialize(interner),
            named,
            variables,
            active: HashSet::new(),
            memo: HashMap::new(),
        }
    }

    pub fn existing(store: &'a TypeStore, named: &'a HashMap<String, TypeId>) -> Self {
        Self {
            target: SubstitutionTarget::Existing(store),
            named,
            variables: None,
            active: HashSet::new(),
            memo: HashMap::new(),
        }
    }

    pub fn substitute(&mut self, ty: TypeId) -> Result<TypeId, TypeSubstitutionError> {
        if let Some(specialized) = self.memo.get(&ty).copied() {
            return Ok(specialized);
        }
        if !self.active.insert(ty) {
            return Err(TypeSubstitutionError::CyclicType(ty));
        }
        let result = self.substitute_active(ty);
        self.active.remove(&ty);
        let specialized = result?;
        self.memo.insert(ty, specialized);
        Ok(specialized)
    }

    fn substitute_active(&mut self, ty: TypeId) -> Result<TypeId, TypeSubstitutionError> {
        let ty_data = self.require_type(ty)?;
        if let Some(replacement) = self
            .replacement(&ty_data)
            .filter(|replacement| *replacement != ty)
        {
            return self.substitute(replacement);
        }
        let specialized = match ty_data {
            Type::Array(inner) => Type::Array(self.substitute(inner)?),
            Type::List(inner) => Type::List(self.substitute(inner)?),
            Type::Map { key, value } => Type::Map {
                key: self.substitute(key)?,
                value: self.substitute(value)?,
            },
            Type::Set(inner) => Type::Set(self.substitute(inner)?),
            Type::Range { index } => Type::Range {
                index: self.substitute(index)?,
            },
            Type::Slice(inner) => Type::Slice(self.substitute(inner)?),
            Type::Option(inner) => Type::Option(self.substitute(inner)?),
            Type::Result { ok, err } => Type::Result {
                ok: self.substitute(ok)?,
                err: self.substitute(err)?,
            },
            Type::Record(record) => Type::Record(RecordType {
                fields: record
                    .fields
                    .into_iter()
                    .map(|field| {
                        Ok(FieldType {
                            name: field.name,
                            ty: self.substitute(field.ty)?,
                        })
                    })
                    .collect::<Result<Vec<_>, TypeSubstitutionError>>()?,
            }),
            Type::Tuple(elements) => Type::Tuple(
                elements
                    .into_iter()
                    .map(|element| self.substitute(element))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Type::Function(flow) => Type::Function(FlowType {
                input: flow
                    .input
                    .into_iter()
                    .map(|input| self.substitute(input))
                    .collect::<Result<Vec<_>, _>>()?,
                output: self.substitute(flow.output)?,
                effects: flow
                    .effects
                    .map(|row| self.substitute_effect_row(row))
                    .transpose()?,
            }),
            Type::Handler(handler) => Type::Handler(HandlerType {
                handled: self.substitute_effect_row(handler.handled)?,
                produced: match handler.produced {
                    HandlerProducedEffects::Infer => HandlerProducedEffects::Infer,
                    HandlerProducedEffects::Explicit(row) => {
                        HandlerProducedEffects::Explicit(self.substitute_effect_row(row)?)
                    }
                },
                result: handler
                    .result
                    .map(|result| self.substitute(result))
                    .transpose()?,
            }),
            Type::Applied { constructor, args } => Type::Applied {
                constructor,
                args: args
                    .into_iter()
                    .map(|arg| self.substitute(arg))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Type::Refined { base, predicate } => Type::Refined {
                base: self.substitute(base)?,
                predicate,
            },
            Type::Trust { wrapper, inner } => Type::Trust {
                wrapper,
                inner: self.substitute(inner)?,
            },
            Type::Schema(inner) => Type::Schema(self.substitute(inner)?),
            Type::Message(inner) => Type::Message(self.substitute(inner)?),
            Type::MemorySelection(inner) => Type::MemorySelection(self.substitute(inner)?),
            Type::Store { key, value } => Type::Store {
                key: self.substitute(key)?,
                value: self.substitute(value)?,
            },
            Type::MemoryRegion(inner) => Type::MemoryRegion(self.substitute(inner)?),
            Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => {
                Type::ResourceHandle(ResourceHandleType::MemoryRegion {
                    schema: self.substitute(schema)?,
                })
            }
            Type::ResourceHandle(ResourceHandleType::ExternalTool { signature }) => {
                Type::ResourceHandle(ResourceHandleType::ExternalTool {
                    signature: self.substitute(signature)?,
                })
            }
            Type::ResourceHandle(ResourceHandleType::Other { name, args }) => {
                Type::ResourceHandle(ResourceHandleType::Other {
                    name,
                    args: args
                        .into_iter()
                        .map(|arg| self.substitute(arg))
                        .collect::<Result<Vec<_>, _>>()?,
                })
            }
            Type::Primitive(_)
            | Type::IntegerLiteral { .. }
            | Type::Var(_)
            | Type::Enum(_)
            | Type::Named(_)
            | Type::Nominal(_)
            | Type::Prompt
            | Type::PromptPart
            | Type::MemoryPlace(_) => return Ok(ty),
        };
        self.materialize(ty, specialized)
    }

    pub fn substitute_effect_row(
        &mut self,
        mut row: EffectRowRef,
    ) -> Result<EffectRowRef, TypeSubstitutionError> {
        for effect in &mut row.effects {
            for arg in &mut effect.args {
                if let EffectArgRef::Type(ty) = arg {
                    *ty = self.substitute(*ty)?;
                }
            }
        }
        Ok(row)
    }

    fn replacement(&self, ty: &Type) -> Option<TypeId> {
        match ty {
            Type::Named(name) => self.named.get(&name.name).copied(),
            Type::Var(var) => self.variables.and_then(|variables| variables.get(*var)),
            _ => None,
        }
    }

    fn require_type(&self, ty: TypeId) -> Result<Type, TypeSubstitutionError> {
        self.type_data(ty)
            .ok_or(TypeSubstitutionError::MissingType(ty))
    }

    fn type_data(&self, ty: TypeId) -> Option<Type> {
        match &self.target {
            SubstitutionTarget::Materialize(interner) => interner.store().get(ty).cloned(),
            SubstitutionTarget::Existing(store) => store.get(ty).cloned(),
        }
    }

    fn materialize(
        &mut self,
        original: TypeId,
        specialized: Type,
    ) -> Result<TypeId, TypeSubstitutionError> {
        if self.require_type(original)? == specialized {
            return Ok(original);
        }
        match &mut self.target {
            SubstitutionTarget::Materialize(interner) => Ok(interner.intern(specialized)),
            SubstitutionTarget::Existing(store) => store
                .iter()
                .find_map(|(id, candidate)| (candidate == &specialized).then_some(id))
                .ok_or(TypeSubstitutionError::UnmaterializedType(specialized)),
        }
    }
}

pub fn substitute_named_params(
    interner: &mut TypeInterner,
    ty: TypeId,
    substitutions: &HashMap<String, TypeId>,
) -> Result<TypeId, TypeSubstitutionError> {
    TypeSubstitutionEngine::materializing(interner, substitutions, None).substitute(ty)
}

pub fn substitute_type_params(
    interner: &mut TypeInterner,
    ty: TypeId,
    named: &HashMap<String, TypeId>,
    variables: &Substitution,
) -> Result<TypeId, TypeSubstitutionError> {
    TypeSubstitutionEngine::materializing(interner, named, Some(variables)).substitute(ty)
}

pub fn substitute_effect_row_params(
    interner: &mut TypeInterner,
    row: EffectRowRef,
    named: &HashMap<String, TypeId>,
    variables: &Substitution,
) -> Result<EffectRowRef, TypeSubstitutionError> {
    TypeSubstitutionEngine::materializing(interner, named, Some(variables))
        .substitute_effect_row(row)
}

pub fn substitute_named_params_in_store(
    store: &TypeStore,
    ty: TypeId,
    substitutions: &HashMap<String, TypeId>,
) -> Result<TypeId, TypeSubstitutionError> {
    TypeSubstitutionEngine::existing(store, substitutions).substitute(ty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectRef, NamedTypeRef, NominalTypeRef, PrimitiveType, TypeConstructorId};

    fn typed_row(ty: TypeId) -> EffectRowRef {
        EffectRowRef {
            effects: vec![EffectRef {
                name: "Fs.read".to_owned(),
                args: vec![EffectArgRef::Type(ty)],
            }],
            tail: None,
        }
    }

    #[test]
    fn substitution_specializes_function_and_handler_effect_rows() {
        let mut interner = TypeInterner::new();
        let param = interner.intern(Type::Named(NamedTypeRef {
            name: "T".to_owned(),
        }));
        let concrete = interner.primitive(PrimitiveType::String);
        let function = interner.intern(Type::Function(FlowType {
            input: vec![param],
            output: param,
            effects: Some(typed_row(param)),
        }));
        let handler = interner.intern(Type::Handler(HandlerType {
            handled: typed_row(param),
            produced: HandlerProducedEffects::Explicit(typed_row(param)),
            result: Some(function),
        }));

        let substitutions = HashMap::from([("T".to_owned(), concrete)]);
        let specialized = substitute_named_params(&mut interner, handler, &substitutions)
            .expect("substitution should materialize");
        let Type::Handler(handler) = interner.store().get(specialized).unwrap() else {
            panic!("expected handler")
        };
        assert_eq!(
            handler.handled.effects[0].args,
            vec![EffectArgRef::Type(concrete)]
        );
        let HandlerProducedEffects::Explicit(produced) = &handler.produced else {
            panic!("expected explicit produced row")
        };
        assert_eq!(produced.effects[0].args, vec![EffectArgRef::Type(concrete)]);
        let Type::Function(flow) = interner.store().get(handler.result.unwrap()).unwrap() else {
            panic!("expected function result")
        };
        assert_eq!(
            flow.effects.as_ref().unwrap().effects[0].args,
            vec![EffectArgRef::Type(concrete)]
        );
    }

    #[test]
    fn readonly_substitution_specializes_nested_applied_types() {
        let mut interner = TypeInterner::new();
        let param = interner.intern(Type::Named(NamedTypeRef {
            name: "T".to_owned(),
        }));
        let concrete_arg = interner.primitive(PrimitiveType::I32);
        let wrapper = interner.intern(Type::Nominal(NominalTypeRef {
            name: "Wrapper".to_owned(),
            params: vec!["T".to_owned()],
            representation: None,
        }));
        let generic = interner.intern(Type::Applied {
            constructor: TypeConstructorId(wrapper.0),
            args: vec![param],
        });
        let without_specialization = interner.store().clone();
        let concrete = interner.intern(Type::Applied {
            constructor: TypeConstructorId(wrapper.0),
            args: vec![concrete_arg],
        });
        let store = interner.into_store();
        let substitutions = HashMap::from([("T".to_owned(), concrete_arg)]);

        assert_eq!(
            substitute_named_params_in_store(&store, generic, &substitutions),
            Ok(concrete)
        );
        assert!(matches!(
            substitute_named_params_in_store(&without_specialization, generic, &substitutions),
            Err(TypeSubstitutionError::UnmaterializedType(Type::Applied { args, .. }))
                if args == vec![concrete_arg]
        ));
    }
}
