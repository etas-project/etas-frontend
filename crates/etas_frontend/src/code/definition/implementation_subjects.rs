use etas_types::{CallableGenericParam, Type, TypeId, TypeStore};
use std::{collections::HashMap, mem::Discriminant};

/// A coarse index for the checked implementation unifier. Generic subjects
/// can match any type. Candidate selection never decides a witness itself.
pub(crate) struct ImplementationSubjects<'a> {
    store: &'a TypeStore,
    all: &'a [TypeId],
    groups: HashMap<Head<'a>, Vec<TypeId>>,
}

#[derive(Hash, Eq, PartialEq)]
enum Head<'a> {
    Nominal(&'a etas_code::DefinitionKey, bool),
    UnqualifiedNominal(&'a str, bool),
    Enum(TypeId, bool),
    // Integer literals unify with integer primitives.
    Primitive,
    Shape(Discriminant<Type>),
}

impl<'a> ImplementationSubjects<'a> {
    pub(crate) fn new(store: &'a TypeStore, all: &'a [TypeId]) -> Self {
        let mut groups = HashMap::<_, Vec<_>>::new();
        for &ty in all {
            groups.entry(head(store, ty)).or_default().push(ty);
        }
        Self { store, all, groups }
    }

    pub(crate) fn candidates(&self, ty: TypeId, parameters: &[CallableGenericParam]) -> &[TypeId] {
        if parameters.iter().any(|p| p.subject == ty)
            || matches!(self.store.get(ty), Some(Type::Applied { constructor, .. })
                if parameters.iter().any(|p| p.subject == TypeId(constructor.0)))
        {
            return self.all;
        }
        self.groups
            .get(&head(self.store, ty))
            .map_or(&[], Vec::as_slice)
    }
}

fn head(store: &TypeStore, ty: TypeId) -> Head<'_> {
    let data = store.get(ty).expect("checked implementation type");
    let (owner, applied) = match data {
        Type::Applied { constructor, .. } => (TypeId(constructor.0), true),
        _ => (ty, false),
    };
    match store.get(owner) {
        Some(Type::Nominal(decl)) => match store.nominal_identity(owner) {
            Some(identity) => Head::Nominal(identity, applied),
            None => Head::UnqualifiedNominal(&decl.name, applied),
        },
        Some(Type::Enum(_)) => Head::Enum(owner, applied),
        Some(Type::Primitive(_) | Type::IntegerLiteral { .. }) if !applied => Head::Primitive,
        _ => Head::Shape(std::mem::discriminant(data)),
    }
}
