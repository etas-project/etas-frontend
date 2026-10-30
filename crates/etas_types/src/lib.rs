pub mod api;
pub mod constraint;
pub mod diagnostic;
pub mod facts;
pub mod lower;
pub mod pipeline;
pub mod solver;
pub mod ty;

pub use api::{
    AnnotationValidationMode, BodyInstantiationError, BodyTypeOutput, CheckBodyRequest,
    CheckProjectRequest, CheckSignaturesRequest, ExternalActionArgKindInput,
    ExternalActionBindingInput, ExternalActionGenericParamInput, ExternalActionSignatureInput,
    ExternalAgentSignatureInput, ExternalCallableGenericParamInput,
    ExternalCallableGenericParamKindInput, ExternalCallableSpecSatisfactionInput,
    ExternalEffectArgInput, ExternalEffectRefInput, ExternalEffectRowInput,
    ExternalFlowSignatureInput, ExternalNamedSignatureInput, ExternalPackageKey,
    ExternalPublicMetadataInput, ExternalRecordFieldInput, ExternalSignatureInput,
    ExternalSpecBoundInput, ExternalSpecImplInput, ExternalSpecKindInput, ExternalSpecMethodInput,
    ExternalSpecSignatureInput, ExternalSymbolBindingInput, ExternalToolSignatureInput,
    ExternalTraceSpecConformanceInput, ExternalTraceSpecConformanceTargetInput, ExternalTypeInput,
    ExternalTypeSpecSatisfactionInput, InstantiatedBodyOutput, ReflectionSignatureInput,
    SignatureBaseExtensionError, SignatureEnvironment, SignaturePipelineInput,
    SourceSignatureInput, SourceSymbolBindingInput, StdSignatureInput, StdSymbolBindingInput,
    TypeOutput, check_body, check_project, check_signatures,
};
pub use constraint::{
    AssignabilityReason, CallableCandidate, CallableCandidateOperation, CallableGenericArg,
    ConstraintOrigin, NumericLiteralKind, SpecObligation, TypeConstraint, ValidationRequest,
};
pub use facts::{
    ActionRequestSignature, AgentEntrySignature, AgentSignature, CallableGenericParam,
    CallableGenericParamKind, CallableSignature, CallableSpecSatisfactionFact,
    CheckedAgentInferenceFact, CheckedHandlerActionFact, CheckedIndexKind, CheckedReflectionFact,
    CheckedReflectionTarget, CheckedSliceKind, CheckedSpecBound, CheckedSpecRef,
    CheckedStdSpecImplFact, DeferredEffectRowObligation, DeferredHandlerInstantiation,
    DeferredHandlerOverload, EffectActionArgKind, EffectActionSignature, EnumLayoutFact,
    EnumVariantLayoutFact, ExternalCallableSpecSatisfactionFact, ExternalTraceSpecConformanceFact,
    ExternalTraceSpecConformanceTarget, FieldProjectionFact, FlowSignature,
    GenericInstantiationFact, HandlerTypeBinding, ItemSignature, KnownStdTypes, ResourceHandleFact,
    SpecFacts, SpecImplFact, SpecImplMethodFact, SpecKind, SpecMethodFact, SpecMethodIdentity,
    SpecSignature, SpecSuperBoundFact, SymbolTypeFact, ToolSignature, TopLevelLetSignature,
    TraceSpecConformanceFact, TraceSpecConformanceTarget, TryExprTypeFact, TypeFacts,
    TypeParamBoundFact, TypeSpecSatisfactionFact,
};
pub use pipeline::finalize::{
    EffectRowMaterializationError, HandlerInstantiationProof, HandlerRequestProof,
    ResolvedEffectRowInstantiation, complete_handler_instantiations,
    materialize_effect_row_instantiations,
};
pub use pipeline::signature::validate::annotations::validate_annotations as validate_item_annotations;
pub use solver::{
    Assignable, CheckedIntegerLiteral, ExternalDescriptorIdentities, ExternalDescriptorMatcher,
    SolverFailure, SolverReport, Substitution, TypeRelation, TypeSolver, TypeUnifier, UnifyError,
    checked_descriptor_type_matches, checked_descriptor_type_matches_with_bindings,
    checked_integer_literal,
};
pub use ty::{
    EffectArgRef, EffectRef, EffectRowRef, EnumTypeRef, FieldType, FlowType,
    HandlerProducedEffects, HandlerType, MemoryPlaceType, NamedTypeRef, NominalTypeRef,
    PrimitiveType, RecordType, RefinementId, ResourceHandleType, TrustWrapper, Type,
    TypeConstructorId, TypeId, TypeInterner, TypeScheme, TypeStore, TypeSubstitutionBudget,
    TypeSubstitutionEngine, TypeSubstitutionError, TypeVarId, applied_representation,
    nominal_representation_parts, record_fields_with_applied_params, substitute_effect_row_params,
    substitute_named_params, substitute_named_params_in_store, substitute_type_params,
    type_contains_named_param,
};

pub fn check_program(hir: &etas_hir::HirProgram) -> TypeOutput {
    check_project(CheckProjectRequest { hir })
}

pub fn check_top_level_items(hir: &etas_hir::HirProgram, output: TypeOutput) -> TypeOutput {
    pipeline::check_top_level_items(
        hir,
        output,
        std::sync::Arc::new(etas_std::standard_registry()),
    )
}

pub fn build_signature_facts(hir: &etas_hir::HirProgram) -> TypeOutput {
    check_signatures(CheckSignaturesRequest { hir })
}

pub fn run_signature_pipeline(input: SignaturePipelineInput<'_>) -> TypeOutput {
    pipeline::run_signature_pipeline(input)
}

pub use pipeline::body::instantiation::{
    check_instantiated_body_delta, check_instantiated_body_delta_controlled,
};
pub use pipeline::{
    check_body_delta, check_body_delta_controlled, finalize::finalize_body_deltas,
    finalize::finalize_body_deltas_with_application, infer_callable_outputs,
    infer_callable_outputs_controlled,
};
