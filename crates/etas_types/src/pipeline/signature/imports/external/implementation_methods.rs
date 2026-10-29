use super::*;

pub(super) fn apply(
    ctx: &mut TypePipelineContext<'_>,
    package: ExternalPackageKey,
    span: etas_core::Span,
    self_type: TypeId,
    spec: &crate::CheckedSpecRef,
    args: &[TypeId],
    methods: &[crate::ExternalSpecImplMethodInput],
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
        for provided in methods {
            let name = provided.identity.name();
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
            let generic_params = crate::lower::external::lower_external_generic_params(
                ctx,
                span,
                package,
                &format!("{:?}", provided.identity),
                &provided.generic_params,
            )
            .ok_or("external implementation generic signature is inconsistent")?;
            let actual = CallableSignature {
                generic_params,
                params: provided
                    .params
                    .iter()
                    .map(|ty| lower_external_type(ctx, span, ty))
                    .collect(),
                output: lower_external_type(ctx, span, &provided.output),
                effects: Some(lower_external_effect_row(ctx, span, &provided.effects)),
                requested_actions: None,
            };
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
            pending.push(crate::facts::specs::ExternalSpecImplMethodFact {
                span,
                identity: provided.identity.clone(),
                self_type,
                spec: spec.clone(),
                args: args.to_vec(),
                signature: actual,
            });
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            ctx.signature_facts.external_specs.methods.extend(pending);
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
