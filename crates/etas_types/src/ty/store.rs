use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::Arc,
};

use serde::{Deserialize, Serialize, ser::SerializeSeq};

use super::{PrimitiveType, Type, TypeId};

/// A frozen, indexed type universe. Body stores append after this shared prefix.
#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct SharedTypeBase(Arc<FrozenTypes>);

#[derive(Debug, etas_core::RetainedSize)]
struct FrozenTypes {
    types: Vec<Type>,
    ids: HashMap<Type, TypeId>,
    identity: TypeBaseId,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct TypeBaseId(pub [u8; 32]);

struct TypeHash(blake3::Hasher);
impl Hasher for TypeHash {
    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
    fn finish(&self) -> u64 {
        u64::from_le_bytes(
            self.0.finalize().as_bytes()[..8]
                .try_into()
                .expect("eight bytes"),
        )
    }
}

impl SharedTypeBase {
    pub fn new(store: TypeStore) -> Self {
        let types = if store.base.is_none() {
            store.types
        } else {
            store.iter().map(|(_, ty)| ty.clone()).collect()
        };
        let mut ids = HashMap::with_capacity(types.len());
        for (index, ty) in types.iter().enumerate() {
            ids.entry(ty.clone()).or_insert(TypeId(index as u32));
        }
        let mut hash = TypeHash(blake3::Hasher::new_derive_key("etas.type-base.v1"));
        types.hash(&mut hash);
        let identity = TypeBaseId(*hash.0.finalize().as_bytes());
        Self(Arc::new(FrozenTypes {
            types,
            ids,
            identity,
        }))
    }

    pub fn len(&self) -> usize {
        self.0.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.types.is_empty()
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub fn identity(&self) -> TypeBaseId {
        self.0.identity
    }
}

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
pub struct TypeStore {
    base: Option<SharedTypeBase>,
    types: Vec<Type>,
}

/// Program-local lookup only. Hash collisions are resolved by full Type equality.
/// The index borrows no mutable interner and does not copy aggregate type nodes.
pub struct TypeLookupIndex {
    first: HashMap<u64, TypeId>,
    collisions: HashMap<u64, Vec<TypeId>>,
}

impl TypeLookupIndex {
    pub fn new(store: &TypeStore) -> Self {
        let mut first = HashMap::with_capacity(store.len());
        let mut collisions = HashMap::<u64, Vec<TypeId>>::new();
        for (id, ty) in store.iter() {
            let key = type_hash(ty);
            match first.entry(key) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(id);
                }
                std::collections::hash_map::Entry::Occupied(_) => {
                    collisions.entry(key).or_default().push(id);
                }
            }
        }
        Self { first, collisions }
    }

    pub fn get(&self, store: &TypeStore, ty: &Type) -> Option<TypeId> {
        let key = type_hash(ty);
        let first = *self.first.get(&key)?;
        if store.get(first) == Some(ty) {
            return Some(first);
        }
        self.collisions
            .get(&key)?
            .iter()
            .copied()
            .find(|id| store.get(*id) == Some(ty))
    }
}

fn type_hash(ty: &Type) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    ty.hash(&mut hash);
    hash.finish()
}

impl TypeStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_base(base: SharedTypeBase) -> Self {
        Self {
            base: Some(base),
            types: Vec::new(),
        }
    }

    pub fn base(&self) -> Option<&SharedTypeBase> {
        self.base.as_ref()
    }

    pub fn base_len(&self) -> usize {
        self.base.as_ref().map_or(0, SharedTypeBase::len)
    }

    pub fn len(&self) -> usize {
        self.base_len() + self.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn intern(&mut self, ty: Type) -> TypeId {
        let id = TypeId(self.len() as u32);
        self.types.push(ty);
        id
    }

    pub fn get(&self, id: TypeId) -> Option<&Type> {
        let index = id.0 as usize;
        if index < self.base_len() {
            self.base.as_ref()?.0.types.get(index)
        } else {
            self.types.get(index - self.base_len())
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (TypeId, &Type)> {
        self.base
            .iter()
            .flat_map(|base| base.0.types.iter())
            .chain(self.types.iter())
            .enumerate()
            .map(|(index, ty)| (TypeId(index as u32), ty))
    }

    pub fn local_types(&self) -> &[Type] {
        &self.types
    }

    pub fn into_local_types(self) -> Vec<Type> {
        self.types
    }

    pub fn with_local_types(base: SharedTypeBase, types: Vec<Type>) -> Self {
        Self {
            base: Some(base),
            types,
        }
    }
}

// Complete outputs retain the flat wire format. Body artifacts serialize only
// their local segment and an explicit signature-base identity separately.
impl Serialize for TypeStore {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        struct Types<'a>(&'a TypeStore);
        impl Serialize for Types<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
                for (_, ty) in self.0.iter() {
                    seq.serialize_element(ty)?;
                }
                seq.end()
            }
        }
        let mut record = serializer.serialize_struct("TypeStore", 1)?;
        record.serialize_field("types", &Types(self))?;
        record.end()
    }
}

impl<'de> Deserialize<'de> for TypeStore {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct FlatStore {
            types: Vec<Type>,
        }
        Ok(Self {
            base: None,
            types: FlatStore::deserialize(deserializer)?.types,
        })
    }
}

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
pub struct TypeInterner {
    store: TypeStore,
    types: HashMap<Type, TypeId>,
}

impl TypeInterner {
    pub(crate) fn define_nominal(&mut self, id: TypeId, representation: Option<TypeId>) {
        let index = (id.0 as usize)
            .checked_sub(self.store.base_len())
            .expect("shared signature types are immutable");
        let Type::Nominal(mut nominal) = self.store.types[index].clone() else {
            unreachable!("only nominal declarations can be completed")
        };
        self.types.remove(&Type::Nominal(nominal.clone()));
        nominal.representation = representation;
        let ty = Type::Nominal(nominal);
        self.store.types[index] = ty.clone();
        self.types.insert(ty, id);
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_store(store: TypeStore) -> Self {
        let mut types = HashMap::with_capacity(store.types.len());
        for (index, ty) in store.types.iter().enumerate() {
            types
                .entry(ty.clone())
                .or_insert(TypeId((store.base_len() + index) as u32));
        }
        Self { store, types }
    }

    pub fn primitive(&mut self, primitive: PrimitiveType) -> TypeId {
        self.intern(Type::Primitive(primitive))
    }

    pub fn intern(&mut self, ty: Type) -> TypeId {
        if let Some(id) = self
            .types
            .get(&ty)
            .or_else(|| self.store.base.as_ref()?.0.ids.get(&ty))
        {
            return *id;
        }
        let id = self.store.intern(ty.clone());
        self.types.insert(ty, id);
        id
    }

    pub fn store(&self) -> &TypeStore {
        &self.store
    }
    pub fn into_store(self) -> TypeStore {
        self.store
    }
}
