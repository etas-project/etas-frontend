use etas_std::FlowSourceMethodKind;

use crate::{
    method::{MethodEvidence, MethodOwner, MethodReceiver, MethodSignatureFact, MethodTarget},
    pipeline::context::TypePipelineContext,
};

pub(super) fn collect(ctx: &mut TypePipelineContext<'_>) -> Vec<MethodSignatureFact> {
    let registry = ctx.std_registry.clone();
    let mut result = Vec::new();
    for symbol in registry.symbols() {
        for (overload, flow) in registry.flow_declarations(symbol.id).enumerate() {
            let Some(method) = &flow.source_method else {
                continue;
            };
            let scope = symbol.qualified_path.split_last().map(|(_, scope)| scope);
            let mut signature =
                crate::lower::std::lower_std_flow_signature(ctx, &registry, flow, scope);
            let (owner_params, method_params) = signature
                .generic_params
                .into_iter()
                .partition(|parameter| method.owner_type_params.contains(&parameter.name));
            signature.generic_params = method_params;
            let receiver_type = match method.kind {
                FlowSourceMethodKind::ValueMethod => signature.params[0],
                FlowSourceMethodKind::TypeMember => crate::lower::std::lower_std_method_owner(
                    ctx,
                    &registry,
                    &method.receiver,
                    scope,
                )
                .expect("standard associated method has a declared type owner"),
            };
            result.push(MethodSignatureFact {
                name: method.name.clone(),
                owner: MethodOwner::Standard,
                receiver: match method.kind {
                    FlowSourceMethodKind::ValueMethod => MethodReceiver::Value { slot: 0 },
                    FlowSourceMethodKind::TypeMember => MethodReceiver::Associated,
                },
                receiver_type,
                owner_params,
                parameter_names: vec![None; signature.params.len()],
                signature,
                target: MethodTarget::Standard {
                    qualified_path: symbol.qualified_path.clone(),
                    intrinsic: symbol.intrinsic.as_ref().map(|descriptor| descriptor.id.0),
                    overload,
                    operation: method.operation,
                },
                spec_member: None,
                evidence: MethodEvidence::Inherent,
                defining_module: None,
                public: true,
            });
        }
    }
    result
}
