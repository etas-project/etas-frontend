mod body;
mod instantiation;
mod output;
mod request;

pub use body::{BodyTypeOutput, SignatureBaseExtensionError, SignatureEnvironment};
pub use instantiation::{BodyInstantiationError, InstantiatedBodyOutput};
pub use output::TypeOutput;
pub use request::{
    AnnotationValidationMode, CheckBodyRequest, CheckProjectRequest, CheckSignaturesRequest,
    ExternalActionArgKindInput, ExternalActionBindingInput, ExternalActionGenericParamInput,
    ExternalActionSignatureInput, ExternalAgentSignatureInput, ExternalCallableGenericParamInput,
    ExternalCallableGenericParamKindInput, ExternalCallableSpecSatisfactionInput,
    ExternalEffectArgInput, ExternalEffectRefInput, ExternalEffectRowInput,
    ExternalFlowSignatureInput, ExternalNamedSignatureInput, ExternalPackageKey,
    ExternalPublicMetadataInput, ExternalRecordFieldInput, ExternalSignatureInput,
    ExternalSpecBoundInput, ExternalSpecImplInput, ExternalSpecKindInput, ExternalSpecMethodInput,
    ExternalSpecSignatureInput, ExternalSymbolBindingInput, ExternalToolSignatureInput,
    ExternalTraceSpecConformanceInput, ExternalTraceSpecConformanceTargetInput, ExternalTypeInput,
    ExternalTypeSpecSatisfactionInput, ReflectionSignatureInput, SignaturePipelineInput,
    SourceSignatureInput, SourceSymbolBindingInput, StdSignatureInput, StdSymbolBindingInput,
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
