use std::collections::{BTreeSet, HashMap};

use crate::{Type, TypeId, TypeStore};

use super::{MethodEvidence, MethodSignatureFact, MethodTarget};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct MethodIndex {
    pub(super) source_origins: HashMap<etas_hir::SymbolId, etas_code::MethodId>,
    entries: Vec<MethodSignatureFact>,
    members: HashMap<String, Vec<u32>>,
    nominal: HashMap<TypeId, HashMap<String, Vec<u32>>>,
    families: HashMap<u8, HashMap<String, Vec<u32>>>,
    witness_patterns: HashMap<ReceiverHead, HashMap<crate::SpecMethodIdentity, Vec<u32>>>,
    implementations: HashMap<crate::CheckedSpecRef, Vec<u32>>,
    targets: HashMap<TargetKey, Vec<u32>>,
}

impl MethodIndex {
    pub fn target_identity<'a>(
        &'a self,
        target: &'a MethodTarget,
    ) -> Option<&'a etas_code::MethodId> {
        match target {
            MethodTarget::Source { symbol, .. } | MethodTarget::Agent { symbol, .. } => {
                self.source_origins.get(symbol)
            }
            MethodTarget::External { identity } => Some(identity),
            _ => None,
        }
    }

    pub(crate) fn with_remapped_types(
        &self,
        store: &TypeStore,
        entries: Vec<MethodSignatureFact>,
    ) -> Self {
        let mut index = Self::build(store, entries);
        index.source_origins = self.source_origins.clone();
        index
    }

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
            result
                .targets
                .entry(TargetKey::from(&entry.target))
                .or_default()
                .push(index);
            let bucket = match receiver_head(store, entry.receiver_type) {
                ReceiverHead::Nominal(owner) => result.nominal.entry(owner).or_default(),
                ReceiverHead::Family(kind) => result.families.entry(kind).or_default(),
            };
            bucket.entry(entry.name.clone()).or_default().push(index);
            if let super::MethodEvidence::Implementation { spec, .. } = &entry.evidence {
                if let Some(member) = &entry.spec_member {
                    result
                        .witness_patterns
                        .entry(receiver_head(store, entry.receiver_type))
                        .or_default()
                        .entry(member.clone())
                        .or_default()
                        .push(index);
                }
                result
                    .implementations
                    .entry(spec.clone())
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

    pub fn declaration(
        &self,
        target: &MethodTarget,
        evidence: &MethodEvidence,
    ) -> Option<&MethodSignatureFact> {
        let mut matches = self
            .targets
            .get(&TargetKey::from(target))?
            .iter()
            .map(|index| self.get(*index))
            .filter(|entry| entry.target == *target && same_evidence(&entry.evidence, evidence));
        let declaration = matches.next()?;
        matches.next().is_none().then_some(declaration)
    }

    pub fn resolve_witness(
        &self,
        store: &TypeStore,
        facts: &crate::SpecFacts,
        receiver: TypeId,
        args: &[TypeId],
        member: &crate::SpecMethodIdentity,
    ) -> Result<Option<MethodWitness<'_>>, String> {
        let Some(candidates) = self
            .witness_patterns
            .get(&receiver_head(store, receiver))
            .and_then(|bucket| bucket.get(member))
        else {
            return Ok(None);
        };
        let mut selected: Option<MethodWitness<'_>> = None;
        for index in candidates {
            let declaration = self.get(*index);
            let MethodEvidence::Implementation {
                args: parameters, ..
            } = &declaration.evidence
            else {
                unreachable!()
            };
            let Some(bindings) = crate::solver::spec_solver::match_implementation_subject(
                store,
                &declaration.owner_params,
                declaration.receiver_type,
                receiver,
            ) else {
                continue;
            };
            if !crate::solver::spec_solver::implementation_bounds_satisfied(
                store,
                facts,
                &declaration.owner_params,
                &bindings,
            ) {
                continue;
            }
            let Some(actual) = crate::solver::spec_solver::implementation::substitute(
                store, &bindings, parameters,
            ) else {
                continue;
            };
            if actual != args {
                continue;
            }
            if let Some(previous) = &selected {
                if previous.declaration.target == declaration.target
                    && previous.owner_bindings == bindings
                {
                    continue;
                }
                return Err(
                    "method has multiple applicable checked implementation witnesses".into(),
                );
            }
            selected = Some(MethodWitness {
                declaration,
                owner_bindings: bindings,
                spec_args: actual,
            });
        }
        Ok(selected)
    }

    pub fn witnesses_for_receiver(
        &self,
        store: &TypeStore,
        facts: &crate::SpecFacts,
        receiver: TypeId,
    ) -> Result<Vec<MethodWitness<'_>>, String> {
        let Some(members) = self.witness_patterns.get(&receiver_head(store, receiver)) else {
            return Ok(Vec::new());
        };
        let mut result = Vec::new();
        let mut visited = std::collections::HashSet::new();
        for (member, candidates) in members {
            for index in candidates {
                let entry = self.get(*index);
                let MethodEvidence::Implementation { args, .. } = &entry.evidence else {
                    unreachable!()
                };
                let Some(bindings) = crate::solver::spec_solver::match_implementation_subject(
                    store,
                    &entry.owner_params,
                    entry.receiver_type,
                    receiver,
                ) else {
                    continue;
                };
                let Some(args) =
                    crate::solver::spec_solver::implementation::substitute(store, &bindings, args)
                else {
                    continue;
                };
                if visited.insert((member, args.clone())) {
                    if let Some(witness) =
                        self.resolve_witness(store, facts, receiver, &args, member)?
                    {
                        result.push(witness);
                    }
                }
            }
        }
        Ok(result)
    }

    pub fn member(&self, name: &str) -> impl Iterator<Item = (u32, &MethodSignatureFact)> {
        self.members
            .get(name)
            .into_iter()
            .flatten()
            .map(|id| (*id, self.get(*id)))
    }

    pub fn implementations_for_spec(
        &self,
        spec: &crate::CheckedSpecRef,
    ) -> impl Iterator<Item = &MethodSignatureFact> {
        self.implementations
            .get(spec)
            .into_iter()
            .flatten()
            .map(|index| self.get(*index))
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

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
enum TargetKey {
    Source(etas_hir::SymbolId),
    Agent(etas_hir::SymbolId),
    Standard(Vec<String>, usize),
    External(etas_code::MethodId),
    Bound(crate::CheckedSpecRef, crate::SpecMethodIdentity),
}

impl From<&MethodTarget> for TargetKey {
    fn from(target: &MethodTarget) -> Self {
        match target {
            MethodTarget::Source { symbol, .. } => Self::Source(*symbol),
            MethodTarget::Agent { symbol, .. } => Self::Agent(*symbol),
            MethodTarget::Standard {
                qualified_path,
                overload,
                ..
            } => Self::Standard(qualified_path.clone(), *overload),
            MethodTarget::External { identity } => Self::External(identity.clone()),
            MethodTarget::BoundSpec { spec, member, .. } => {
                Self::Bound(spec.clone(), member.clone())
            }
        }
    }
}

fn same_evidence(left: &MethodEvidence, right: &MethodEvidence) -> bool {
    match (left, right) {
        (MethodEvidence::Inherent, MethodEvidence::Inherent) => true,
        (
            MethodEvidence::Implementation { spec: left, .. },
            MethodEvidence::Implementation { spec: right, .. },
        )
        | (MethodEvidence::Bound { spec: left, .. }, MethodEvidence::Bound { spec: right, .. }) => {
            left == right
        }
        _ => false,
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
enum ReceiverHead {
    Nominal(TypeId),
    Family(u8),
}

pub struct MethodWitness<'a> {
    pub declaration: &'a MethodSignatureFact,
    pub owner_bindings: HashMap<TypeId, TypeId>,
    pub spec_args: Vec<TypeId>,
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
        Type::Existential { .. } => 24,
        Type::Refined { base, .. } => return receiver_head(store, *base),
    };
    ReceiverHead::Family(family)
}
