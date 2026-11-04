use crate::{
    method::{MethodEvidence, MethodOwner, MethodSignatureFact, MethodTarget},
    pipeline::context::TypePipelineContext,
};

pub(super) fn collect(ctx: &TypePipelineContext<'_>) -> Vec<MethodSignatureFact> {
    let members = ctx
        .signature_facts
        .spec_signatures
        .values()
        .flat_map(|spec| &spec.methods)
        .chain(
            ctx.signature_facts
                .external_specs
                .signatures
                .values()
                .flat_map(|spec| &spec.methods),
        )
        .map(|member| (&member.identity, member))
        .collect::<std::collections::HashMap<_, _>>();
    let mut methods = ctx
        .signature_facts
        .external_methods
        .iter()
        .collect::<Vec<_>>();
    methods.sort_by_key(|method| &method.identity);
    let mut result = Vec::new();
    for method in methods {
        let mut fact = MethodSignatureFact {
            name: method.identity.name().into(),
            owner: MethodOwner::Inherent(method.self_type),
            receiver: method.receiver,
            receiver_type: method.self_type,
            owner_params: method.owner_params.clone(),
            signature: method.signature.clone(),
            parameter_names: method.parameter_names.clone(),
            target: MethodTarget::External {
                identity: method.identity.clone(),
            },
            spec_member: None,
            evidence: MethodEvidence::Inherent,
            defining_module: None,
            public: true,
        };
        if method.implementations.is_empty() {
            result.push(fact);
        } else {
            for implementation in &method.implementations {
                let member = members
                    .get(&implementation.member)
                    .expect("validated external method has a declared spec member");
                fact.parameter_names = member.parameter_names.iter().cloned().map(Some).collect();
                fact.owner = MethodOwner::Spec(implementation.spec.clone());
                fact.spec_member = Some(implementation.member.clone());
                fact.evidence = MethodEvidence::Implementation {
                    spec: implementation.spec.clone(),
                    subject: method.self_type,
                    args: implementation.args.clone(),
                };
                result.push(fact.clone());
            }
        }
    }
    let mut specs = ctx
        .signature_facts
        .external_specs
        .signatures
        .iter()
        .collect::<Vec<_>>();
    specs.sort_by(|(left, _), (right, _)| {
        left.package
            .cmp(&right.package)
            .then_with(|| left.path.cmp(&right.path))
    });
    for (identity, spec) in specs {
        if spec.kind != crate::SpecKind::TypeSpec {
            continue;
        }
        for member in &spec.methods {
            let Some(mut signature) = member.signature_in(&ctx.signature_facts).cloned() else {
                continue;
            };
            let Some(receiver_type) = signature.params.first().copied() else {
                continue;
            };
            let mut owner_params = spec.generic_params.clone();
            let args = owner_params
                .iter()
                .map(|parameter| parameter.subject)
                .collect();
            if let Some(position) = member.receiver_param {
                owner_params.push(signature.generic_params.remove(position as usize));
            }
            let spec_ref = crate::CheckedSpecRef::External(identity.clone());
            result.push(MethodSignatureFact {
                name: member.name.clone(),
                owner: MethodOwner::Spec(spec_ref.clone()),
                receiver: crate::method::MethodReceiver::Value { slot: 0 },
                receiver_type,
                owner_params,
                parameter_names: member.parameter_names.iter().cloned().map(Some).collect(),
                signature,
                target: MethodTarget::BoundSpec {
                    spec: spec_ref.clone(),
                    member: member.identity.clone(),
                    receiver_parameter: member.receiver_param,
                },
                spec_member: Some(member.identity.clone()),
                evidence: MethodEvidence::Bound {
                    spec: spec_ref,
                    subject: receiver_type,
                    args,
                },
                defining_module: None,
                public: true,
            });
        }
    }
    result
}
