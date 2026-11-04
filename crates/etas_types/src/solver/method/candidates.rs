use super::*;
use crate::method::{MethodEvidence, MethodSelection};

pub(super) fn same_spec(
    facts: &SpecFacts,
    left: &crate::CheckedSpecRef,
    right: &crate::CheckedSpecRef,
) -> bool {
    let canonical = |spec: &crate::CheckedSpecRef| match spec {
        crate::CheckedSpecRef::Source(symbol) => facts
            .external_aliases
            .get(symbol)
            .map(|identity| crate::CheckedSpecRef::External(identity.clone()))
            .or_else(|| {
                facts
                    .std_spec_aliases
                    .get(symbol)
                    .map(|path| crate::CheckedSpecRef::Std(path.clone()))
            })
            .unwrap_or_else(|| spec.clone()),
        _ => spec.clone(),
    };
    canonical(left) == canonical(right)
}

pub(super) fn eligible(
    input: &TypeSolveInput<'_>,
    call: Input<'_>,
    candidate: &crate::CallableCandidate,
    receiver: TypeId,
    applicable: &std::collections::BTreeSet<u32>,
) -> bool {
    let crate::CallableCandidateOperation::Method {
        index, evidence, ..
    } = &candidate.operation
    else {
        return false;
    };
    if !applicable.contains(index) {
        return false;
    }
    if let MethodSelection::ExplicitSpec { spec, .. } = call.selection {
        let selected = match evidence {
            MethodEvidence::Implementation { spec, .. } | MethodEvidence::Bound { spec, .. } => {
                spec
            }
            MethodEvidence::Inherent => return false,
        };
        if !same_spec(input.spec_facts, selected, spec) {
            return false;
        }
    }
    if matches!(evidence, MethodEvidence::Bound { .. })
        && !matches!(input.store.get(receiver), Some(Type::Existential { .. }))
        && !matches!(input.store.get(receiver), Some(Type::Named(name)) if name.source_parameter.is_some())
    {
        return false;
    }
    true
}
