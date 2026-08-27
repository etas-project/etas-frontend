mod output;
mod request;

pub use output::TypeOutput;
pub use request::{
    CheckBodyRequest, CheckProjectRequest, CheckSignaturesRequest, ExternalActionArgKindInput,
    ExternalActionBindingInput, ExternalActionGenericParamInput, ExternalActionSignatureInput,
    ExternalAgentSignatureInput, ExternalCallableGenericParamInput,
    ExternalCallableSpecSatisfactionInput, ExternalEffectArgInput, ExternalEffectRefInput,
    ExternalEffectRowInput, ExternalFlowSignatureInput, ExternalNamedSignatureInput,
    ExternalPackageKey, ExternalPublicMetadataInput, ExternalRecordFieldInput,
    ExternalSignatureInput, ExternalSpecBoundInput, ExternalSpecImplInput, ExternalSpecKindInput,
    ExternalSpecMethodInput, ExternalSpecSignatureInput, ExternalSymbolBindingInput,
    ExternalToolSignatureInput, ExternalTraceSpecConformanceInput,
    ExternalTraceSpecConformanceTargetInput, ExternalTypeInput, ExternalTypeSpecSatisfactionInput,
    SignaturePipelineInput, SourceSignatureInput, SourceSymbolBindingInput, StdSignatureInput,
    StdSymbolBindingInput,
};

pub fn check_project(request: CheckProjectRequest<'_>) -> TypeOutput {
    crate::pipeline::check_project(request.hir)
}

pub fn check_signatures(request: CheckSignaturesRequest<'_>) -> TypeOutput {
    crate::pipeline::check_signatures(request.hir)
}

pub fn check_body(request: CheckBodyRequest<'_>) -> TypeOutput {
    crate::pipeline::check_body(request)
}
