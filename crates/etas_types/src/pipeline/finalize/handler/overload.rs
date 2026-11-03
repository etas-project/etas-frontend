use super::HandlerRequestProof;
use crate::{DeferredHandlerOverload, Substitution, TypeStore, TypeUnifier};
use etas_core::TypeDiagnosticCode;

pub(super) fn solve(
    store: &TypeStore,
    overload: &DeferredHandlerOverload,
    requests: &[HandlerRequestProof],
    known: &Substitution,
) -> Result<Substitution, (TypeDiagnosticCode, String)> {
    let invalid = |message: &str| (TypeDiagnosticCode::IncompleteTypeFacts, message.to_owned());
    let mismatch = |message: &str| (TypeDiagnosticCode::TypeMismatch, message.to_owned());
    if requests.is_empty() || overload.candidates.is_empty() {
        return Err(invalid(
            "handler overload has no complete checked request signature",
        ));
    }
    let mut substitutions = known.clone();
    let mut selected = None;
    for request in requests {
        let signature = request
            .signature
            .as_ref()
            .ok_or_else(|| invalid("handled request is missing its selected payload ABI"))?;
        let matching = overload
            .candidates
            .iter()
            .enumerate()
            .filter_map(|(index, candidate)| {
                let mut probe = TypeUnifier::with_known_substitution(store, &substitutions);
                (candidate.params.len() == signature.params.len()
                    && candidate
                        .params
                        .iter()
                        .zip(&signature.params)
                        .all(|(expected, actual)| probe.unify(*expected, *actual).is_ok()))
                .then_some(index)
            })
            .collect::<Vec<_>>();
        let [index] = matching.as_slice() else {
            return Err(mismatch(
                "handler request input does not identify exactly one registered action overload",
            ));
        };
        if selected.is_some_and(|selected| selected != *index) {
            return Err(mismatch(
                "handler requests require different action input overloads; one arm cannot assume a single payload ABI",
            ));
        }
        selected = Some(*index);
        let candidate = &overload.candidates[*index];
        let mut probe = TypeUnifier::with_known_substitution(store, &substitutions);
        if candidate.params.len() != overload.params.len()
            || candidate
                .params
                .iter()
                .zip(&signature.params)
                .any(|(expected, actual)| probe.unify(*expected, *actual).is_err())
            || probe.unify(candidate.output, signature.output).is_err()
            || candidate
                .params
                .iter()
                .zip(&overload.params)
                .any(|(expected, actual)| probe.unify(*expected, *actual).is_err())
            || probe.unify(candidate.output, overload.output).is_err()
        {
            return Err(mismatch(
                "handler payload or resume type does not match every reachable request signature",
            ));
        }
        substitutions.extend(&probe.into_substitution());
    }
    Ok(substitutions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ActionRequestSignature, PrimitiveType, Type, TypeInterner, TypeVarId};

    #[test]
    fn deferred_overload_uses_request_inputs_and_checks_every_resume_abi() {
        let mut types = TypeInterner::new();
        let integer = types.primitive(PrimitiveType::I32);
        let a = types.intern(Type::Var(TypeVarId(0)));
        let f = types.intern(Type::Var(TypeVarId(1)));
        let input = types.intern(Type::Var(TypeVarId(2)));
        let option = types.intern(Type::Option(a));
        let list = types.intern(Type::List(f));
        let list_int = types.intern(Type::List(integer));
        let option_int = types.intern(Type::Option(integer));
        let overload = DeferredHandlerOverload {
            candidates: vec![
                ActionRequestSignature {
                    params: vec![option],
                    output: a,
                },
                ActionRequestSignature {
                    params: vec![list],
                    output: f,
                },
            ],
            params: vec![input],
            output: integer,
        };
        let proof = |params| HandlerRequestProof {
            type_bindings: vec![],
            signature: Some(ActionRequestSignature {
                params: vec![params],
                output: integer,
            }),
        };
        let known = Substitution::default();
        assert!(solve(types.store(), &overload, &[proof(list_int)], &known).is_ok());
        let error = solve(
            types.store(),
            &overload,
            &[proof(list_int), proof(option_int)],
            &known,
        )
        .unwrap_err();
        assert_eq!(error.0, TypeDiagnosticCode::TypeMismatch);
        assert!(error.1.contains("different action input overloads"));
        let error = solve(types.store(), &overload, &[proof(integer)], &known).unwrap_err();
        assert_eq!(error.0, TypeDiagnosticCode::TypeMismatch);
        let missing = HandlerRequestProof {
            type_bindings: vec![],
            signature: None,
        };
        let error = solve(types.store(), &overload, &[missing], &known).unwrap_err();
        assert_eq!(error.0, TypeDiagnosticCode::IncompleteTypeFacts);
    }
}
