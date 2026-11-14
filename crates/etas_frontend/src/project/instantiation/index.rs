use std::hash::{Hash, Hasher};

use etas_types::{CallableGenericArg, EffectRowRef, TypeId};

use super::{NativeGenericArgument, NativeInstantiation, NativeInstantiationPlan};

impl NativeInstantiationPlan {
    pub(super) fn push(&mut self, instance: NativeInstantiation) {
        let hash = application_hash(
            instance.owner,
            instance.arguments.len(),
            instance.arguments.iter().map(|argument| match argument {
                NativeGenericArgument::Type(ty) => Argument::Type(*ty),
                NativeGenericArgument::EffectRow(row) => Argument::Row(row),
            }),
        );
        self.by_hash
            .entry(hash)
            .or_default()
            .push(self.instances.len());
        self.instances.push(instance);
    }

    pub(crate) fn checked_instance(
        &self,
        owner: etas_hir::HirItemId,
        arguments: &[CallableGenericArg],
    ) -> Result<Option<&NativeInstantiation>, String> {
        if !self.instances.is_empty() && self.by_hash.is_empty() {
            return Err("native application index is missing".into());
        }
        let hash = application_hash(
            owner,
            arguments.len(),
            arguments.iter().map(|argument| match argument {
                CallableGenericArg::Type(ty) => Argument::Type(*ty),
                CallableGenericArg::EffectRow(row) => Argument::Row(row),
            }),
        );
        let mut selected: Option<&NativeInstantiation> = None;
        for position in self.by_hash.get(&hash).into_iter().flatten() {
            let instance = self
                .instances
                .get(*position)
                .ok_or("native application index references a missing instance")?;
            if instance.owner != owner || instance.arguments.len() != arguments.len() {
                continue;
            }
            let equal = instance
                .arguments
                .iter()
                .zip(arguments)
                .all(|(prepared, actual)| match (prepared, actual) {
                    (NativeGenericArgument::Type(prepared), CallableGenericArg::Type(actual)) => {
                        prepared == actual
                    }
                    (
                        NativeGenericArgument::EffectRow(prepared),
                        CallableGenericArg::EffectRow(actual),
                    ) => normalized_row(prepared) == normalized_row(actual),
                    _ => false,
                });
            if equal {
                if selected
                    .is_some_and(|previous| previous.owner_bindings != instance.owner_bindings)
                {
                    return Err(
                        "callable application requires distinct implementation owner bindings"
                            .into(),
                    );
                }
                selected = Some(instance);
            }
        }
        Ok(selected)
    }
}

// Only native application keys cross the bundle boundary. Rebuild the lookup
// table in the receiving process; its hashes are never portable identities.
impl serde::Serialize for NativeInstantiationPlan {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&self.instances, serializer)
    }
}

impl<'de> serde::Deserialize<'de> for NativeInstantiationPlan {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let instances =
            <Vec<NativeInstantiation> as serde::Deserialize>::deserialize(deserializer)?;
        let mut plan = Self::default();
        for instance in instances {
            plan.push(instance);
        }
        Ok(plan)
    }
}

enum Argument<'a> {
    Type(TypeId),
    Row(&'a EffectRowRef),
}

// Session-local acceleration only. Every hit checks the complete native key;
// these hashes are not a portable identity or executable authorization.
fn application_hash<'a>(
    owner: etas_hir::HirItemId,
    count: usize,
    arguments: impl Iterator<Item = Argument<'a>>,
) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    owner.hash(&mut hash);
    count.hash(&mut hash);
    for argument in arguments {
        match argument {
            Argument::Type(ty) => {
                0_u8.hash(&mut hash);
                ty.hash(&mut hash);
            }
            Argument::Row(row) => {
                1_u8.hash(&mut hash);
                normalized_row(row).hash(&mut hash);
            }
        }
    }
    hash.finish()
}

pub(crate) fn normalized_row(row: &EffectRowRef) -> EffectRowRef {
    let mut row = row.clone();
    row.effects.sort();
    row.effects.dedup();
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_round_trip_rebuilds_generic_application_lookup() {
        let owner = etas_hir::HirItemId(7);
        let arguments = [CallableGenericArg::Type(TypeId(3))];
        let mut plan = NativeInstantiationPlan::default();
        plan.push(NativeInstantiation {
            owner,
            arguments: vec![NativeGenericArgument::Type(TypeId(3))],
            owner_bindings: vec![(TypeId(2), TypeId(3))],
        });
        let bytes = bincode::serde::encode_to_vec(&plan, bincode::config::standard()).unwrap();
        let (restored, consumed): (NativeInstantiationPlan, _) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        assert_eq!(consumed, bytes.len());
        assert_eq!(
            restored.checked_instance(owner, &arguments).unwrap(),
            plan.checked_instance(owner, &arguments).unwrap()
        );
    }

    #[test]
    fn a_hash_hit_does_not_replace_full_application_identity() {
        let owner = etas_hir::HirItemId(7);
        let arguments = [CallableGenericArg::Type(TypeId(3))];
        let mut plan = NativeInstantiationPlan::default();
        plan.push(NativeInstantiation {
            owner,
            arguments: vec![NativeGenericArgument::Type(TypeId(4))],
            owner_bindings: vec![],
        });
        let hash = application_hash(owner, 1, [Argument::Type(TypeId(3))].into_iter());
        plan.by_hash.insert(hash, vec![0]);
        assert!(plan.checked_instance(owner, &arguments).unwrap().is_none());
        plan.by_hash.insert(hash, vec![1]);
        assert!(plan.checked_instance(owner, &arguments).is_err());
        plan.by_hash.clear();
        assert!(plan.checked_instance(owner, &arguments).is_err());
    }
}
