use std::collections::{BTreeSet, HashMap};

use crate::{Type, TypeId, TypeStore};

use super::MethodSignatureFact;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct MethodIndex {
    entries: Vec<MethodSignatureFact>,
    members: HashMap<String, Vec<u32>>,
    nominal: HashMap<TypeId, HashMap<String, Vec<u32>>>,
    families: HashMap<u8, HashMap<String, Vec<u32>>>,
    witnesses: HashMap<TypeId, Vec<u32>>,
}

impl MethodIndex {
    pub fn build(store: &TypeStore, entries: Vec<MethodSignatureFact>) -> Self {
        let mut result = Self::default();
        for (index, entry) in entries.into_iter().enumerate() {
            let index =
                u32::try_from(index).expect("method index exceeds the HIR resource ceiling");
            result
                .members
                .entry(entry.name.clone())
                .or_default()
                .push(index);
            let bucket = match receiver_head(store, entry.receiver_type) {
                ReceiverHead::Nominal(owner) => result.nominal.entry(owner).or_default(),
                ReceiverHead::Family(kind) => result.families.entry(kind).or_default(),
            };
            bucket.entry(entry.name.clone()).or_default().push(index);
            if matches!(entry.evidence, super::MethodEvidence::Implementation { .. }) {
                result
                    .witnesses
                    .entry(entry.receiver_type)
                    .or_default()
                    .push(index);
            }
            result.entries.push(entry);
        }
        result
    }

    pub fn get(&self, index: u32) -> &MethodSignatureFact {
        &self.entries[index as usize]
    }

    pub fn entries(&self) -> impl Iterator<Item = &MethodSignatureFact> {
        self.entries.iter()
    }

    pub fn witness(
        &self,
        receiver: TypeId,
        args: &[TypeId],
        member: &crate::SpecMethodIdentity,
    ) -> Option<&MethodSignatureFact> {
        let mut matches = self.witnesses.get(&receiver)?.iter().map(|index| self.get(*index))
            .filter(|entry| entry.spec_member.as_ref() == Some(member)
                && matches!(&entry.evidence, super::MethodEvidence::Implementation { args: actual, .. } if actual == args));
        let selected = matches.next()?;
        matches.next().is_none().then_some(selected)
    }

    pub fn member(&self, name: &str) -> impl Iterator<Item = (u32, &MethodSignatureFact)> {
        self.members
            .get(name)
            .into_iter()
            .flatten()
            .map(|id| (*id, self.get(*id)))
    }

    pub fn applicable(&self, store: &TypeStore, receiver: TypeId, name: &str) -> BTreeSet<u32> {
        let mut result = BTreeSet::new();
        let bucket = match receiver_head(store, receiver) {
            ReceiverHead::Nominal(owner) => self.nominal.get(&owner),
            ReceiverHead::Family(kind) => self.families.get(&kind),
        };
        if let Some(entries) = bucket.and_then(|bucket| bucket.get(name)) {
            result.extend(entries);
        }
        if let Some(entries) = self.families.get(&0).and_then(|bucket| bucket.get(name)) {
            result.extend(entries);
        }
        result
    }
}

enum ReceiverHead {
    Nominal(TypeId),
    Family(u8),
}

fn receiver_head(store: &TypeStore, ty: TypeId) -> ReceiverHead {
    let family = match store
        .get(ty)
        .expect("method receiver belongs to the signature type store")
    {
        Type::Nominal(_) | Type::Enum(_) | Type::Primitive(_) => return ReceiverHead::Nominal(ty),
        Type::Applied { constructor, .. } => return ReceiverHead::Nominal(TypeId(constructor.0)),
        Type::Named(_) | Type::Var(_) => 0,
        Type::Array(_) => 1,
        Type::List(_) => 2,
        Type::Map { .. } => 3,
        Type::Set(_) => 4,
        Type::Range { .. } => 5,
        Type::Slice(_) => 6,
        Type::Option(_) => 7,
        Type::Result { .. } => 8,
        Type::Record(_) => 9,
        Type::Tuple(_) => 10,
        Type::Function(_) => 11,
        Type::Handler(_) => 12,
        Type::Trust { .. } => 13,
        Type::Schema(_) => 14,
        Type::Prompt => 15,
        Type::PromptPart => 16,
        Type::Message(_) => 17,
        Type::MemorySelection(_) => 18,
        Type::Store { .. } => 19,
        Type::MemoryPlace(_) => 20,
        Type::MemoryRegion(_) => 21,
        Type::ResourceHandle(_) => 22,
        Type::IntegerLiteral { .. } => 23,
        Type::Refined { base, .. } => return receiver_head(store, *base),
    };
    ReceiverHead::Family(family)
}
