use crate::{PrimitiveType, Type, TypeId, TypeStore, TypeUnifier, UnifyError};

pub struct TypeRelation<'a> {
    store: &'a TypeStore,
}

impl<'a> TypeRelation<'a> {
    pub fn new(store: &'a TypeStore) -> Self {
        Self { store }
    }
}

pub trait Assignable {
    fn assignable(&self, from: TypeId, to: TypeId) -> Result<(), UnifyError>;
}

impl Assignable for TypeRelation<'_> {
    fn assignable(&self, from: TypeId, to: TypeId) -> Result<(), UnifyError> {
        if matches!(
            self.store.get(from),
            Some(Type::Primitive(PrimitiveType::Never))
        ) {
            return Ok(());
        }
        if support_constraint_assignable(self.store, from, to) {
            return Ok(());
        }
        let mut unifier = TypeUnifier::new(self.store);
        unifier.unify(from, to)
    }
}

pub fn is_assignable(store: &TypeStore, from: TypeId, to: TypeId) -> bool {
    TypeRelation::new(store).assignable(from, to).is_ok()
}

fn support_constraint_assignable(store: &TypeStore, from: TypeId, to: TypeId) -> bool {
    let Some(Type::Named(name)) = store.get(to) else {
        return false;
    };
    match name.name.as_str() {
        "Index" => is_index_input(store, from),
        "LengthInput" => is_length_input(store, from),
        "EmptinessInput" => is_emptiness_input(store, from),
        _ => false,
    }
}

fn is_index_input(store: &TypeStore, ty: TypeId) -> bool {
    matches!(
        store.get(ty),
        Some(Type::IntegerLiteral { .. })
            | Some(Type::Primitive(
                PrimitiveType::I8
                    | PrimitiveType::I16
                    | PrimitiveType::I32
                    | PrimitiveType::I64
                    | PrimitiveType::I128
                    | PrimitiveType::ISize
                    | PrimitiveType::U8
                    | PrimitiveType::U16
                    | PrimitiveType::U32
                    | PrimitiveType::U64
                    | PrimitiveType::U128
                    | PrimitiveType::USize
            ))
    )
}

fn is_length_input(store: &TypeStore, ty: TypeId) -> bool {
    matches!(
        store.get(ty),
        Some(Type::Array(_))
            | Some(Type::List(_))
            | Some(Type::Slice(_))
            | Some(Type::Map { .. })
            | Some(Type::Primitive(PrimitiveType::String))
    )
}

fn is_emptiness_input(store: &TypeStore, ty: TypeId) -> bool {
    is_length_input(store, ty)
}
