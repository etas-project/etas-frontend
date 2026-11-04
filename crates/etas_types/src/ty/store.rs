use std::{
    collections::{BTreeMap, HashMap},
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
    nominal_identities: BTreeMap<TypeId, etas_code::DefinitionKey>,
    parameter_identities: BTreeMap<TypeId, CanonicalTypeParameterIdentity>,
    parameter_ids: HashMap<CanonicalTypeParameterIdentity, TypeId>,
    identity: TypeBaseId,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct TypeBaseId(pub [u8; 32]);

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct CanonicalTypeParameterIdentity {
    owner: etas_code::descriptor::GenericOwner,
    index: u32,
}

impl CanonicalTypeParameterIdentity {
    pub fn new(owner: etas_code::DefinitionKey, index: u32) -> Result<Self, &'static str> {
        if !matches!(
            owner.kind(),
            etas_code::DefinitionKind::Type
                | etas_code::DefinitionKind::Spec
                | etas_code::DefinitionKind::Agent
        ) {
            return Err("type parameter owner is not a type, spec or agent declaration");
        }
        Self::new_scoped(
            etas_code::descriptor::GenericOwner::Declaration(owner),
            index,
        )
    }

    pub fn new_scoped(
        owner: etas_code::descriptor::GenericOwner,
        index: u32,
    ) -> Result<Self, &'static str> {
        let identity = Self { owner, index };
        if !identity.valid_owner() {
            return Err("invalid canonical generic owner");
        }
        Ok(identity)
    }

    pub fn owner(&self) -> &etas_code::descriptor::GenericOwner {
        &self.owner
    }
    pub fn index(&self) -> u32 {
        self.index
    }

    fn valid_owner(&self) -> bool {
        self.owner.is_portable_parameter_owner()
    }
}

pub(crate) enum TypeBaseBuildError {
    TypeLimit { limit: usize },
    Interrupted(etas_utils::PipelineInterruption),
}

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
        let nominal_identities = store.nominal_identities();
        let parameter_identities = store.parameter_identities();
        let types = if store.base.is_none() {
            store.types
        } else {
            store.iter().map(|(_, ty)| ty.clone()).collect()
        };
        let mut ids = HashMap::with_capacity(types.len());
        for (index, ty) in types.iter().enumerate() {
            let id = TypeId(index as u32);
            if !nominal_identities.contains_key(&id) && !parameter_identities.contains_key(&id) {
                ids.entry(ty.clone()).or_insert(id);
            }
        }
        let mut hash = TypeHash(blake3::Hasher::new_derive_key("etas.type-base.v1"));
        types.hash(&mut hash);
        nominal_identities.hash(&mut hash);
        parameter_identities.hash(&mut hash);
        let identity = TypeBaseId(*hash.0.finalize().as_bytes());
        let parameter_ids = parameter_identities
            .iter()
            .map(|(id, key)| (key.clone(), *id))
            .collect();
        Self(Arc::new(FrozenTypes {
            types,
            ids,
            nominal_identities,
            parameter_identities,
            parameter_ids,
            identity,
        }))
    }

    pub(crate) fn new_controlled(
        store: TypeStore,
        max_types: usize,
        control: &dyn etas_utils::PipelineExecutionControl,
    ) -> Result<Self, TypeBaseBuildError> {
        control
            .checkpoint()
            .map_err(TypeBaseBuildError::Interrupted)?;
        if store.len() > max_types {
            return Err(TypeBaseBuildError::TypeLimit { limit: max_types });
        }
        let mut types = Vec::with_capacity(store.len());
        let mut nominal_identities = BTreeMap::new();
        let mut parameter_identities = BTreeMap::new();
        for (id, ty) in store.iter() {
            control
                .checkpoint()
                .map_err(TypeBaseBuildError::Interrupted)?;
            types.push(ty.clone());
            if let Some(identity) = store.nominal_identity(id) {
                nominal_identities.insert(id, identity.clone());
            }
            if let Some(identity) = store.parameter_identity(id) {
                parameter_identities.insert(id, identity.clone());
            }
        }
        let mut ids = HashMap::with_capacity(types.len());
        for (index, ty) in types.iter().enumerate() {
            control
                .checkpoint()
                .map_err(TypeBaseBuildError::Interrupted)?;
            let id = TypeId(index as u32);
            if !nominal_identities.contains_key(&id) && !parameter_identities.contains_key(&id) {
                ids.entry(ty.clone()).or_insert(id);
            }
        }
        let mut hash = TypeHash(blake3::Hasher::new_derive_key("etas.type-base.v1"));
        types.len().hash(&mut hash);
        for ty in &types {
            control
                .checkpoint()
                .map_err(TypeBaseBuildError::Interrupted)?;
            ty.hash(&mut hash);
        }
        nominal_identities.len().hash(&mut hash);
        for entry in &nominal_identities {
            control
                .checkpoint()
                .map_err(TypeBaseBuildError::Interrupted)?;
            entry.hash(&mut hash);
        }
        parameter_identities.len().hash(&mut hash);
        for entry in &parameter_identities {
            control
                .checkpoint()
                .map_err(TypeBaseBuildError::Interrupted)?;
            entry.hash(&mut hash);
        }
        control
            .checkpoint()
            .map_err(TypeBaseBuildError::Interrupted)?;
        let identity = TypeBaseId(*hash.0.finalize().as_bytes());
        let parameter_ids = parameter_identities
            .iter()
            .map(|(id, key)| (key.clone(), *id))
            .collect();
        Ok(Self(Arc::new(FrozenTypes {
            types,
            ids,
            nominal_identities,
            parameter_identities,
            parameter_ids,
            identity,
        })))
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
    nominal_identities: BTreeMap<TypeId, etas_code::DefinitionKey>,
    parameter_identities: BTreeMap<TypeId, CanonicalTypeParameterIdentity>,
    parameter_ids: HashMap<CanonicalTypeParameterIdentity, TypeId>,
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
            if store.nominal_identity(id).is_some() || store.parameter_identity(id).is_some() {
                continue;
            }
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

fn identity_matches_type(identity: &etas_code::DefinitionKey, ty: &Type) -> bool {
    match ty {
        Type::Nominal(_) => matches!(
            identity.kind(),
            etas_code::DefinitionKind::Type | etas_code::DefinitionKind::Agent
        ),
        Type::Enum(_) => identity.kind() == etas_code::DefinitionKind::Type,
        _ => false,
    }
}

impl TypeStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_base(base: SharedTypeBase) -> Self {
        Self {
            base: Some(base),
            types: Vec::new(),
            nominal_identities: BTreeMap::new(),
            parameter_identities: BTreeMap::new(),
            parameter_ids: HashMap::new(),
        }
    }

    pub fn base(&self) -> Option<&SharedTypeBase> {
        self.base.as_ref()
    }

    /// Canonical identities are descriptive checked-type data, not execution
    /// or visibility authority. The caller must separately retain its proof.
    pub fn nominal_identity(&self, id: TypeId) -> Option<&etas_code::DefinitionKey> {
        self.nominal_identities
            .get(&id)
            .or_else(|| self.base.as_ref()?.0.nominal_identities.get(&id))
    }

    fn nominal_identities(&self) -> BTreeMap<TypeId, etas_code::DefinitionKey> {
        let mut identities = self
            .base
            .as_ref()
            .map(|base| base.0.nominal_identities.clone())
            .unwrap_or_default();
        identities.extend(self.nominal_identities.clone());
        identities
    }

    pub fn parameter_identity(&self, id: TypeId) -> Option<&CanonicalTypeParameterIdentity> {
        self.parameter_identities
            .get(&id)
            .or_else(|| self.base.as_ref()?.0.parameter_identities.get(&id))
    }

    pub fn canonical_parameter(&self, identity: &CanonicalTypeParameterIdentity) -> Option<TypeId> {
        self.parameter_ids
            .get(identity)
            .copied()
            .or_else(|| self.base.as_ref()?.0.parameter_ids.get(identity).copied())
    }

    fn parameter_identities(&self) -> BTreeMap<TypeId, CanonicalTypeParameterIdentity> {
        let mut identities = self
            .base
            .as_ref()
            .map(|base| base.0.parameter_identities.clone())
            .unwrap_or_default();
        identities.extend(self.parameter_identities.clone());
        identities
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
        self.types.push(canonical_type(ty));
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

    pub fn into_local_parts(
        self,
    ) -> (
        Vec<Type>,
        BTreeMap<TypeId, etas_code::DefinitionKey>,
        BTreeMap<TypeId, CanonicalTypeParameterIdentity>,
    ) {
        (
            self.types,
            self.nominal_identities,
            self.parameter_identities,
        )
    }

    pub fn with_local_parts(
        base: SharedTypeBase,
        types: Vec<Type>,
        nominal_identities: BTreeMap<TypeId, etas_code::DefinitionKey>,
        parameter_identities: BTreeMap<TypeId, CanonicalTypeParameterIdentity>,
    ) -> Result<Self, &'static str> {
        let base_len = base.len();
        let mut keys: std::collections::BTreeSet<_> = base.0.nominal_identities.values().collect();
        for (id, key) in &nominal_identities {
            let local = (id.0 as usize).checked_sub(base_len);
            if !local
                .and_then(|index| types.get(index))
                .is_some_and(|ty| identity_matches_type(key, ty))
                || !keys.insert(key)
            {
                return Err("invalid or duplicate body nominal identity");
            }
        }
        let mut parameter_keys: std::collections::BTreeSet<_> =
            base.0.parameter_identities.values().collect();
        for (id, key) in &parameter_identities {
            let local = (id.0 as usize).checked_sub(base_len);
            if !key.valid_owner()
                || !matches!(local.and_then(|index| types.get(index)), Some(Type::Named(name)) if name.source_parameter.is_none())
                || !parameter_keys.insert(key)
                || nominal_identities.contains_key(id)
            {
                return Err("invalid or duplicate body parameter identity");
            }
        }
        Ok(Self {
            base: Some(base),
            types: types.into_iter().map(canonical_type).collect(),
            nominal_identities,
            parameter_ids: parameter_identities
                .iter()
                .map(|(id, key)| (key.clone(), *id))
                .collect(),
            parameter_identities,
        })
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
        let mut record = serializer.serialize_struct("TypeStore", 3)?;
        record.serialize_field("types", &Types(self))?;
        record.serialize_field("nominal_identities", &self.nominal_identities())?;
        record.serialize_field("parameter_identities", &self.parameter_identities())?;
        record.end()
    }
}

impl<'de> Deserialize<'de> for TypeStore {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct FlatStore {
            types: Vec<Type>,
            #[serde(default)]
            nominal_identities: BTreeMap<TypeId, etas_code::DefinitionKey>,
            #[serde(default)]
            parameter_identities: BTreeMap<TypeId, CanonicalTypeParameterIdentity>,
        }
        let flat = FlatStore::deserialize(deserializer)?;
        let mut keys = std::collections::BTreeSet::new();
        for (id, identity) in &flat.nominal_identities {
            if !flat
                .types
                .get(id.0 as usize)
                .is_some_and(|ty| identity_matches_type(identity, ty))
                || !keys.insert(identity)
            {
                return Err(serde::de::Error::custom(
                    "invalid or duplicate canonical nominal identity",
                ));
            }
        }
        let mut parameter_keys = std::collections::BTreeSet::new();
        for (id, identity) in &flat.parameter_identities {
            if !identity.valid_owner()
                || !matches!(flat.types.get(id.0 as usize), Some(Type::Named(name)) if name.source_parameter.is_none())
                || !parameter_keys.insert(identity)
                || flat.nominal_identities.contains_key(id)
            {
                return Err(serde::de::Error::custom(
                    "invalid or duplicate canonical parameter identity",
                ));
            }
        }
        Ok(Self {
            base: None,
            types: flat.types.into_iter().map(canonical_type).collect(),
            nominal_identities: flat.nominal_identities,
            parameter_ids: flat
                .parameter_identities
                .iter()
                .map(|(id, key)| (key.clone(), *id))
                .collect(),
            parameter_identities: flat.parameter_identities,
        })
    }
}

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
pub struct TypeInterner {
    store: TypeStore,
    types: HashMap<Type, TypeId>,
    canonical_nominals: HashMap<etas_code::DefinitionKey, TypeId>,
}

impl TypeInterner {
    pub(crate) fn define_nominal(&mut self, id: TypeId, representation: Option<TypeId>) {
        let index = (id.0 as usize)
            .checked_sub(self.store.base_len())
            .expect("shared signature types are immutable");
        let Type::Nominal(mut nominal) = self.store.types[index].clone() else {
            unreachable!("only nominal declarations can be completed")
        };
        let canonical = self.store.nominal_identity(id).is_some();
        if !canonical {
            self.types.remove(&Type::Nominal(nominal.clone()));
        }
        nominal.representation = representation;
        let ty = Type::Nominal(nominal);
        self.store.types[index] = ty.clone();
        if !canonical {
            self.types.insert(ty, id);
        }
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_store(store: TypeStore) -> Self {
        let mut types = HashMap::with_capacity(store.types.len());
        for (index, ty) in store.types.iter().enumerate() {
            let id = TypeId((store.base_len() + index) as u32);
            if store.nominal_identity(id).is_some() || store.parameter_identity(id).is_some() {
                continue;
            }
            types.entry(ty.clone()).or_insert(id);
        }
        let canonical_nominals = store
            .nominal_identities()
            .into_iter()
            .map(|(id, key)| (key, id))
            .collect();
        Self {
            store,
            types,
            canonical_nominals,
        }
    }

    pub fn primitive(&mut self, primitive: PrimitiveType) -> TypeId {
        self.intern(Type::Primitive(primitive))
    }

    pub fn intern(&mut self, ty: Type) -> TypeId {
        let ty = canonical_type(ty);
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

    /// Reserves a distinct local representation for a canonical declaration.
    /// Possession of a key does not establish that it came from checked code.
    pub fn intern_canonical_nominal(
        &mut self,
        identity: etas_code::DefinitionKey,
        ty: Type,
    ) -> Result<TypeId, &'static str> {
        if !identity_matches_type(&identity, &ty) {
            return Err("canonical nominal identity has an incompatible declaration kind");
        }
        if let Some(id) = self.canonical_nominals.get(&identity).copied() {
            return if self.store.get(id) == Some(&ty) {
                Ok(id)
            } else {
                Err("canonical nominal identity has conflicting layouts")
            };
        }
        let id = self.store.intern(ty);
        self.store.nominal_identities.insert(id, identity.clone());
        self.canonical_nominals.insert(identity, id);
        Ok(id)
    }

    pub fn canonical_nominal(&self, identity: &etas_code::DefinitionKey) -> Option<TypeId> {
        self.canonical_nominals.get(identity).copied()
    }

    /// Reserve a binder by its checked owner and position, never by spelling.
    pub fn intern_canonical_parameter(
        &mut self,
        identity: CanonicalTypeParameterIdentity,
        name: String,
    ) -> Result<TypeId, &'static str> {
        if !identity.valid_owner() || name.is_empty() {
            return Err("invalid canonical type parameter");
        }
        let ty = Type::Named(super::NamedTypeRef {
            name,
            source_parameter: None,
        });
        if let Some(id) = self.store.canonical_parameter(&identity) {
            return if self.store.get(id) == Some(&ty)
                || matches!(
                    identity.owner,
                    etas_code::descriptor::GenericOwner::Implementation(_)
                ) {
                Ok(id)
            } else {
                Err("canonical parameter has conflicting declaration")
            };
        }
        let id = self.store.intern(ty);
        self.store.parameter_identities.insert(id, identity.clone());
        self.store.parameter_ids.insert(identity, id);
        Ok(id)
    }

    pub fn canonical_parameter(&self, identity: &CanonicalTypeParameterIdentity) -> Option<TypeId> {
        self.store.canonical_parameter(identity)
    }

    pub fn store(&self) -> &TypeStore {
        &self.store
    }
    pub fn into_store(self) -> TypeStore {
        self.store
    }
}

fn canonical_type(ty: Type) -> Type {
    match ty {
        Type::Existential { mut bounds } => {
            bounds.sort_by(|left, right| {
                left.spec.cmp(&right.spec).then_with(|| {
                    left.args
                        .iter()
                        .map(|ty| ty.0)
                        .cmp(right.args.iter().map(|ty| ty.0))
                })
            });
            bounds.dedup();
            Type::Existential { bounds }
        }
        Type::Tuple(elements) if elements.is_empty() => Type::Primitive(PrimitiveType::Unit),
        ty => ty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NominalTypeRef, solver::unification::TypeUnifier};
    use etas_code::{DefinitionKey, DefinitionKind, PackageKey};

    fn identity(package: &str) -> DefinitionKey {
        DefinitionKey::new(
            PackageKey::from_name_and_version(package, "1.0.0").unwrap(),
            vec!["app".into()],
            vec!["Token".into()],
            DefinitionKind::Type,
        )
        .unwrap()
    }

    fn nominal() -> Type {
        Type::Nominal(NominalTypeRef {
            name: "Token".into(),
            params: Vec::new(),
            representation: None,
            read_only_erased_view: false,
        })
    }

    #[test]
    fn canonical_nominals_do_not_alias_by_name_or_shape() {
        let mut interner = TypeInterner::new();
        let first = interner
            .intern_canonical_nominal(identity("first"), nominal())
            .unwrap();
        let second = interner
            .intern_canonical_nominal(identity("second"), nominal())
            .unwrap();
        let ordinary = interner.intern(nominal());
        assert_ne!(first, second);
        assert_ne!(first, ordinary);
        assert_eq!(
            interner
                .intern_canonical_nominal(identity("first"), nominal())
                .unwrap(),
            first
        );
        assert!(
            TypeUnifier::new(interner.store())
                .unify(first, second)
                .is_err()
        );
        assert!(
            TypeUnifier::new(interner.store())
                .unify(first, ordinary)
                .is_err()
        );
        assert_eq!(
            TypeLookupIndex::new(interner.store()).get(interner.store(), &nominal()),
            Some(ordinary)
        );
        let flow_identity = DefinitionKey::new(
            PackageKey::from_name_and_version("first", "1.0.0").unwrap(),
            vec!["app".into()],
            vec!["Token".into()],
            DefinitionKind::Flow,
        )
        .unwrap();
        assert!(
            interner
                .intern_canonical_nominal(flow_identity, nominal())
                .is_err()
        );
    }

    #[test]
    fn canonical_identities_survive_shared_base_and_body_delta() {
        let mut interner = TypeInterner::new();
        let base_id = interner
            .intern_canonical_nominal(identity("base"), nominal())
            .unwrap();
        let base = SharedTypeBase::new(interner.into_store());
        let mut body = TypeInterner::from_store(TypeStore::with_base(base.clone()));
        assert_eq!(body.canonical_nominal(&identity("base")), Some(base_id));
        let local_id = body
            .intern_canonical_nominal(identity("local"), nominal())
            .unwrap();
        let (types, identities, parameters) = body.into_store().into_local_parts();
        let restored = TypeStore::with_local_parts(
            base.clone(),
            types.clone(),
            identities.clone(),
            parameters.clone(),
        )
        .unwrap();
        assert_eq!(
            restored.nominal_identity(local_id),
            Some(&identity("local"))
        );
        assert!(
            TypeUnifier::new(&restored)
                .unify(base_id, local_id)
                .is_err()
        );
        let mut invalid = identities;
        invalid.insert(base_id, identity("forged"));
        assert!(TypeStore::with_local_parts(base, types, invalid, parameters).is_err());
    }

    #[test]
    fn serialized_store_preserves_and_validates_canonical_identities() {
        let mut interner = TypeInterner::new();
        let id = interner
            .intern_canonical_nominal(identity("package"), nominal())
            .unwrap();
        let data = serde_json::to_value(interner.into_store()).unwrap();
        let store: TypeStore = serde_json::from_value(data.clone()).unwrap();
        assert_eq!(store.nominal_identity(id), Some(&identity("package")));

        let mut invalid = data;
        invalid["types"] = serde_json::json!([]);
        assert!(serde_json::from_value::<TypeStore>(invalid).is_err());
    }

    #[test]
    fn binary_type_store_roundtrip_keeps_canonical_identity() {
        let mut interner = TypeInterner::new();
        let id = interner
            .intern_canonical_nominal(identity("package"), nominal())
            .unwrap();
        let bytes =
            bincode::serde::encode_to_vec(interner.into_store(), bincode::config::standard())
                .unwrap();
        let (store, consumed): (TypeStore, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        assert_eq!(consumed, bytes.len());
        assert_eq!(store.nominal_identity(id), Some(&identity("package")));
    }

    #[test]
    fn controlled_base_has_the_same_identity_as_uncontrolled_base() {
        struct Control;
        impl etas_utils::PipelineExecutionControl for Control {
            fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
                Ok(())
            }
        }
        let mut interner = TypeInterner::new();
        interner
            .intern_canonical_nominal(identity("package"), nominal())
            .unwrap();
        interner
            .intern_canonical_parameter(
                CanonicalTypeParameterIdentity::new(identity("package"), 0).unwrap(),
                "T".into(),
            )
            .unwrap();
        let store = interner.into_store();
        let expected = SharedTypeBase::new(store.clone()).identity();
        let actual = SharedTypeBase::new_controlled(store, 100, &Control)
            .map_err(|_| "controlled type base failed")
            .unwrap()
            .identity();
        assert_eq!(expected, actual);
    }

    #[test]
    fn same_spelled_parameters_are_scoped_by_canonical_owner() {
        let mut types = TypeInterner::new();
        let first_key = CanonicalTypeParameterIdentity::new(identity("first"), 0).unwrap();
        let second_key = CanonicalTypeParameterIdentity::new(identity("second"), 0).unwrap();
        let first = types
            .intern_canonical_parameter(first_key.clone(), "T".into())
            .unwrap();
        let second = types
            .intern_canonical_parameter(second_key, "T".into())
            .unwrap();
        let ordinary = types.intern(Type::Named(super::super::NamedTypeRef {
            name: "T".into(),
            source_parameter: None,
        }));
        assert_ne!(first, second);
        assert_ne!(first, ordinary);
        assert!(
            TypeUnifier::new(types.store())
                .unify(first, second)
                .is_err()
        );
        assert!(
            TypeUnifier::new(types.store())
                .unify(first, ordinary)
                .is_err()
        );
        let bytes =
            bincode::serde::encode_to_vec(types.into_store(), bincode::config::standard()).unwrap();
        let (restored, _): (TypeStore, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        assert_eq!(restored.parameter_identity(first), Some(&first_key));
        assert!(TypeUnifier::new(&restored).unify(first, second).is_err());
    }
}
