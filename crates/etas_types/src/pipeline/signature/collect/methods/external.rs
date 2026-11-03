use crate::{
    method::{MethodEvidence, MethodOwner, MethodSignatureFact, MethodTarget},
    pipeline::context::TypePipelineContext,
};

pub(super) fn collect(ctx: &TypePipelineContext<'_>) -> Vec<MethodSignatureFact> {
    let mut methods = ctx
        .signature_facts
        .external_methods
        .iter()
        .collect::<Vec<_>>();
    methods.sort_by_key(|method| &method.identity);
    methods
        .into_iter()
        .map(|method| MethodSignatureFact {
            name: method.identity.name().into(),
            owner: match &method.evidence {
                MethodEvidence::Implementation { spec, .. } => MethodOwner::Spec(spec.clone()),
                _ => MethodOwner::Inherent(method.self_type),
            },
            receiver: method.receiver,
            receiver_type: method.self_type,
            owner_params: method.owner_params.clone(),
            signature: method.signature.clone(),
            parameter_names: method.parameter_names.clone(),
            target: MethodTarget::External {
                identity: method.identity.clone(),
            },
            spec_member: method.spec_member.clone(),
            evidence: method.evidence.clone(),
            defining_module: None,
            public: true,
        })
        .collect()
}
