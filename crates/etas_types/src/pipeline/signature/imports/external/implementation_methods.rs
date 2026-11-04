use super::*;

pub(super) fn owner_parameters(
    ctx: &TypePipelineContext<'_>,
    methods: &[etas_code::MethodId],
) -> Result<Vec<crate::CallableGenericParam>, String> {
    let mut parameters = None;
    for identity in methods {
        let method = ctx
            .signature_facts
            .external_methods
            .iter()
            .find(|method| &method.identity == identity)
            .ok_or("external implementation has no checked owner binder")?;
        if let Some(previous) = &parameters {
            if previous != &method.owner_params {
                return Err(
                    "external implementation methods have inconsistent owner binders".into(),
                );
            }
        } else {
            parameters = Some(method.owner_params.clone());
        }
    }
    Ok(parameters.unwrap_or_default())
}

pub(super) fn apply(
    ctx: &mut TypePipelineContext<'_>,
    _package: ExternalPackageKey,
    span: etas_core::Span,
    self_type: TypeId,
    spec: &crate::CheckedSpecRef,
    args: &[TypeId],
    methods: &[etas_code::MethodId],
) -> bool {
    let declaration = match spec {
        crate::CheckedSpecRef::Source(symbol) => ctx
            .signature_facts
            .spec_signatures
            .get(symbol)
            .map(|spec| (spec.param_names.clone(), spec.methods.clone())),
        crate::CheckedSpecRef::External(identity) => ctx
            .signature_facts
            .external_specs
            .signatures
            .get(identity)
            .map(|spec| (spec.param_names.clone(), spec.methods.clone())),
        crate::CheckedSpecRef::Std(_) if methods.is_empty() => return true,
        crate::CheckedSpecRef::Std(_) => None,
        crate::CheckedSpecRef::Canonical(identity) => ctx
            .signature_facts
            .canonical_specs
            .signatures
            .get(identity)
            .filter(|_| methods.is_empty())
            .map(|spec| (spec.param_names.clone(), Vec::new())),
    };
    let Some((parameters, declared)) = declaration else {
        incomplete_external_type_facts(
            ctx,
            span,
            "external implementation has no checked spec method declaration".into(),
        );
        return false;
    };
    let mut pending = Vec::new();
    let evidence = crate::SpecFacts::from(&ctx.signature_facts);
    let result = (|| -> Result<(), String> {
        if parameters.len() != args.len() || methods.len() != declared.len() {
            return Err(
                "external implementation spec arguments or method set is incomplete".into(),
            );
        }
        let mut names = std::collections::BTreeSet::new();
        for identity in methods {
            let (index, provided) = ctx
                .signature_facts
                .external_methods
                .iter()
                .enumerate()
                .find(|(_, method)| &method.identity == identity)
                .ok_or("external implementation references an unavailable method")?;
            let name = identity.name();
            if provided.self_type != self_type {
                return Err(
                    "external method receiver differs from its implementation subject".into(),
                );
            }
            if !names.insert(name) {
                return Err("external implementation has duplicate method evidence".into());
            }
            let method = declared
                .iter()
                .find(|method| method.name == name)
                .ok_or("external implementation method is not declared by the spec")?;
            let expected = method
                .signature_in(&ctx.signature_facts)
                .cloned()
                .ok_or("external spec method signature is missing")?;
            let mut actual = provided.signature.clone();
            if provided.receiver == crate::method::MethodReceiver::Component {
                actual.params.remove(0);
            }
            let expected = crate::lower::spec_method::specialize(
                &mut ctx.interner,
                &expected,
                method.receiver_param,
                parameters
                    .iter()
                    .cloned()
                    .zip(args.iter().copied())
                    .collect(),
                self_type,
            )?;
            crate::pipeline::signature::validate::spec_methods::compare(
                ctx, &evidence, &expected, &actual, span,
            )?;
            pending.push((index, method.identity.clone()));
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            for (index, member) in pending {
                let method = &mut ctx.signature_facts.external_methods[index];
                method.implementations.push(
                    crate::facts::methods::ExternalMethodImplementationFact {
                        member,
                        spec: spec.clone(),
                        args: args.to_vec(),
                    },
                );
            }
            true
        }
        Err(reason) => {
            incomplete_external_type_facts(
                ctx,
                span,
                format!("invalid external spec implementation: {reason}"),
            );
            false
        }
    }
}
