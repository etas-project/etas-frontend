pub mod api;
pub mod constraint;
pub mod diagnostic;
pub mod facts;
pub mod lower;
pub mod pipeline;
pub mod solver;
pub mod ty;

pub use api::{
    CheckBodyRequest, CheckProjectRequest, CheckSignaturesRequest, ExternalActionArgKindInput,
    ExternalActionBindingInput, ExternalActionGenericParamInput, ExternalActionSignatureInput,
    ExternalAgentSignatureInput, ExternalCallableGenericParamInput,
    ExternalCallableGenericParamKindInput, ExternalCallableSpecSatisfactionInput,
    ExternalEffectArgInput, ExternalEffectRefInput, ExternalEffectRowInput,
    ExternalFlowSignatureInput, ExternalNamedSignatureInput, ExternalPackageKey,
    ExternalPublicMetadataInput, ExternalRecordFieldInput, ExternalSignatureInput,
    ExternalSpecBoundInput, ExternalSpecImplInput, ExternalSpecKindInput, ExternalSpecMethodInput,
    ExternalSpecSignatureInput, ExternalSymbolBindingInput, ExternalToolSignatureInput,
    ExternalTraceSpecConformanceInput, ExternalTraceSpecConformanceTargetInput, ExternalTypeInput,
    ExternalTypeSpecSatisfactionInput, SignaturePipelineInput, SourceSignatureInput,
    SourceSymbolBindingInput, StdSignatureInput, StdSymbolBindingInput, TypeOutput, check_body,
    check_project, check_signatures,
};
pub use constraint::{
    AssignabilityReason, CallableCandidate, CallableGenericArg, ConstraintOrigin,
    NumericLiteralKind, SpecObligation, TypeConstraint, ValidationRequest,
};
pub use facts::{
    AgentSignature, CallableGenericParam, CallableGenericParamKind, CallableSignature,
    CallableSpecSatisfactionFact, CheckedIndexKind, CheckedSliceKind, CheckedSpecBound,
    CheckedSpecRef, CheckedStdSpecImplFact, DeferredEffectRowObligation, EffectActionArgKind,
    EffectActionSignature, EnumLayoutFact, EnumVariantLayoutFact,
    ExternalCallableSpecSatisfactionFact, ExternalTraceSpecConformanceFact,
    ExternalTraceSpecConformanceTarget, FlowSignature, GenericInstantiationFact, ItemSignature,
    KnownStdTypes, ResourceHandleFact, SpecFacts, SpecImplFact, SpecImplMethodFact, SpecKind,
    SpecMethodFact, SpecMethodIdentity, SpecSignature, SpecSuperBoundFact, SymbolTypeFact,
    ToolSignature, TopLevelLetSignature, TraceSpecConformanceFact, TraceSpecConformanceTarget,
    TryExprTypeFact, TypeFacts, TypeParamBoundFact, TypeSpecSatisfactionFact,
};
pub use solver::{
    Assignable, SolverFailure, SolverReport, Substitution, TypeRelation, TypeSolver, TypeUnifier,
    UnifyError,
};
pub use ty::{
    EffectArgRef, EffectRef, EffectRowRef, EnumTypeRef, FieldType, FlowType,
    HandlerProducedEffects, HandlerType, MemoryPlaceType, NamedTypeRef, NominalTypeRef,
    PrimitiveType, RecordType, RefinementId, ResourceHandleType, TrustWrapper, Type,
    TypeConstructorId, TypeId, TypeInterner, TypeScheme, TypeStore, TypeSubstitutionEngine,
    TypeSubstitutionError, TypeVarId, applied_representation, nominal_representation_parts,
    record_fields_with_applied_params, substitute_effect_row_params, substitute_named_params,
    substitute_named_params_in_store, substitute_type_params, type_contains_named_param,
};

pub fn check_program(hir: &etas_hir::HirProgram) -> TypeOutput {
    check_project(CheckProjectRequest { hir })
}

pub fn check_top_level_items(hir: &etas_hir::HirProgram, output: TypeOutput) -> TypeOutput {
    let body_outputs = hir
        .items
        .iter()
        .filter_map(|(item, data)| {
            matches!(data, etas_hir::HirItem::TopLevelLet(_)).then_some(item)
        })
        .map(|item| pipeline::check_body_with_seed(hir, output.clone(), item))
        .collect();
    pipeline::finalize_type_outputs(output, body_outputs)
}

pub fn build_signature_facts(hir: &etas_hir::HirProgram) -> TypeOutput {
    check_signatures(CheckSignaturesRequest { hir })
}

pub fn run_signature_pipeline(input: SignaturePipelineInput<'_>) -> TypeOutput {
    pipeline::run_signature_pipeline(input)
}

pub fn check_body_item(
    hir: &etas_hir::HirProgram,
    seed: TypeOutput,
    item: etas_hir::HirItemId,
) -> TypeOutput {
    pipeline::check_body_with_seed(hir, seed, item)
}

pub fn check_body_item_with_std_registry(
    hir: &etas_hir::HirProgram,
    seed: TypeOutput,
    item: etas_hir::HirItemId,
    std_registry: std::sync::Arc<etas_std::StdRegistry>,
) -> TypeOutput {
    pipeline::check_body_with_seed_and_std_registry(hir, seed, item, std_registry)
}

pub fn finalize_type_outputs(signatures: TypeOutput, body_outputs: Vec<TypeOutput>) -> TypeOutput {
    pipeline::finalize_type_outputs(signatures, body_outputs)
}
