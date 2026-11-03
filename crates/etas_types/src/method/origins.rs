use std::collections::{BTreeSet, HashMap};

use etas_code::MethodId;
use etas_hir::SymbolId;

use super::{MethodEvidence, MethodIndex, MethodOwner, MethodTarget};
use crate::{CheckedSpecRef, SpecMethodIdentity, TypeFacts, TypeStore};

impl MethodIndex {
    /// Bind loaded implementations to their published declaration identities.
    pub fn bind_source_origins(
        &self,
        store: &TypeStore,
        facts: &TypeFacts,
        origins: &HashMap<SymbolId, MethodId>,
    ) -> Result<Self, String> {
        let mut members = HashMap::new();
        for (symbol, identity) in &facts.external_specs.aliases {
            let Some(source) = facts.spec_signatures.get(symbol) else {
                continue;
            };
            let external = facts
                .external_specs
                .signatures
                .get(identity)
                .ok_or("loaded spec has no published declaration")?;
            for member in &source.methods {
                let published = external
                    .methods
                    .iter()
                    .find(|item| item.name == member.name)
                    .ok_or("loaded spec member has no published declaration")?;
                if let SpecMethodIdentity::Source(symbol) = member.identity {
                    members.insert(symbol, published.identity.clone());
                }
            }
        }
        let canonical_spec = |spec: &mut CheckedSpecRef| {
            if let CheckedSpecRef::Source(symbol) = spec
                && let Some(identity) = facts.external_specs.aliases.get(symbol)
            {
                *spec = CheckedSpecRef::External(identity.clone());
            }
        };
        let canonical_member = |member: &mut SpecMethodIdentity| {
            if let SpecMethodIdentity::Source(symbol) = member
                && let Some(identity) = members.get(symbol)
            {
                *member = identity.clone();
            }
        };
        let native = self
            .entries()
            .filter_map(|entry| match &entry.target {
                MethodTarget::Source { symbol, .. } | MethodTarget::Agent { symbol, .. } => {
                    origins.get(symbol)
                }
                _ => None,
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        let source_bounds = self
            .entries()
            .filter_map(|entry| match &entry.target {
                MethodTarget::BoundSpec {
                    member: SpecMethodIdentity::Source(symbol),
                    ..
                } => members.get(symbol),
                _ => None,
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut entries = Vec::new();
        for entry in self.entries() {
            match &entry.target {
                MethodTarget::External { identity } if native.contains(identity) => continue,
                MethodTarget::BoundSpec {
                    member: member @ SpecMethodIdentity::External { .. },
                    ..
                } if source_bounds.contains(member) => {
                    continue;
                }
                _ => {}
            }
            let mut entry = entry.clone();
            if let MethodOwner::Spec(spec) = &mut entry.owner {
                canonical_spec(spec);
            }
            match &mut entry.evidence {
                MethodEvidence::Implementation { spec, .. }
                | MethodEvidence::Bound { spec, .. } => canonical_spec(spec),
                MethodEvidence::Inherent => {}
            }
            if let Some(member) = &mut entry.spec_member {
                canonical_member(member);
            }
            if let MethodTarget::BoundSpec { spec, member, .. } = &mut entry.target {
                canonical_spec(spec);
                canonical_member(member);
            }
            for parameter in entry
                .owner_params
                .iter_mut()
                .chain(&mut entry.signature.generic_params)
            {
                for bound in &mut parameter.bounds {
                    canonical_spec(&mut bound.spec);
                }
            }
            entries.push(entry);
        }
        Ok(Self::build(store, entries))
    }
}
