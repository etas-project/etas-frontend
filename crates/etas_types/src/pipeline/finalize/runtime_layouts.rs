use crate::{
    CallableSignature, ItemSignature, TypeFacts, TypeId, TypeInterner, TypeSubstitutionError,
};

pub(super) fn materialize(
    facts: &TypeFacts,
    interner: &mut TypeInterner,
) -> Result<(), TypeSubstitutionError> {
    let mut roots = facts
        .expr_types
        .values()
        .copied()
        .chain(facts.stmt_types.values().copied())
        .chain(facts.pattern_types.values().copied())
        .collect::<Vec<_>>();
    for signature in facts.item_signatures.values() {
        match signature {
            ItemSignature::Flow(signature) | ItemSignature::Tool(signature) => {
                callable_roots(signature, &mut roots)
            }
            ItemSignature::Agent(signature) => {
                if let Some(callable) = signature.callable() {
                    callable_roots(callable, &mut roots);
                }
            }
            ItemSignature::TopLevelLet(signature) => roots.push(signature.ty),
        }
    }
    for projections in facts.field_projections.values() {
        for projection in projections {
            roots.extend([projection.receiver, projection.output]);
        }
    }
    for call in facts.std_intrinsic_calls.values() {
        roots.push(call.callable_type);
        callable_roots(&call.signature, &mut roots);
    }
    roots.sort_unstable();
    roots.dedup();
    crate::ty::TypeRepresentationMaterializer::with_acyclic_enum_payloads(
        interner.store(),
        &facts.enum_layouts,
    )?
    .materialize(interner, roots)
}

fn callable_roots(signature: &CallableSignature, roots: &mut Vec<TypeId>) {
    roots.extend(signature.params.iter().copied());
    roots.push(signature.output);
}
