use crate::{
    method::{MethodEvidence, MethodOwner, MethodReceiver, MethodSignatureFact, MethodTarget},
    pipeline::context::TypePipelineContext,
};

pub(super) fn collect(ctx: &TypePipelineContext<'_>) -> Vec<MethodSignatureFact> {
    let mut methods = ctx
        .signature_facts
        .external_specs
        .methods
        .iter()
        .collect::<Vec<_>>();
    methods.sort_by_key(|method| &method.identity);
    methods
        .into_iter()
        .map(|method| {
            let spec = match &method.spec {
                crate::CheckedSpecRef::External(identity) => Some(identity),
                crate::CheckedSpecRef::Source(symbol) => {
                    ctx.signature_facts.external_specs.aliases.get(symbol)
                }
                _ => None,
            };
            let spec_member = spec
                .and_then(|spec| ctx.signature_facts.external_specs.signatures.get(spec))
                .and_then(|spec| {
                    spec.methods
                        .iter()
                        .find(|member| member.name == method.identity.name())
                })
                .map(|member| member.identity.clone());
            MethodSignatureFact {
                name: method.identity.name().into(),
                owner: MethodOwner::Spec(method.spec.clone()),
                receiver: MethodReceiver::Value { slot: 0 },
                receiver_type: method.self_type,
                owner_params: Vec::new(),
                signature: method.signature.clone(),
                parameter_names: vec![None; method.signature.params.len()],
                target: MethodTarget::External {
                    identity: method.identity.clone(),
                },
                spec_member,
                evidence: MethodEvidence::Implementation {
                    spec: method.spec.clone(),
                    subject: method.self_type,
                    args: method.args.clone(),
                },
                defining_module: None,
                public: true,
            }
        })
        .collect()
}
