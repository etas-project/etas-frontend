use crate::{CallableGenericParam, ConstraintOrigin, EffectRowRef, TypeId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallableGenericArg {
    Type(TypeId),
    EffectRow(EffectRowRef),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericLiteralKind {
    Integer,
    Float,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssignabilityReason {
    Annotation,
    Constructor,
    Return,
    Argument,
    Branch,
    Pattern,
    HandlerPattern,
    HandlerResume,
    Finish,
    Assignment,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeConstraint {
    HandlerInstantiation {
        parameters: Vec<(String, TypeId, TypeId)>,
        requests: Vec<etas_hir::HirExprId>,
        deferred: Option<(etas_hir::HirHandlerArmId, etas_hir::HirExprId)>,
        origin: ConstraintOrigin,
    },
    Constructible {
        target: TypeId,
        origin: ConstraintOrigin,
    },
    ReturnType {
        output: TypeId,
        candidates: Vec<(TypeId, ConstraintOrigin)>,
        empty: TypeId,
        origin: ConstraintOrigin,
    },
    AgentInference {
        expr: etas_hir::HirExprId,
        owner: etas_hir::HirItemId,
        prompt: etas_hir::HirExprId,
        prompt_type: TypeId,
        output: TypeId,
        schema: TypeId,
        action_symbol: etas_hir::SymbolId,
        origin: ConstraintOrigin,
    },
    Reflection {
        expr: etas_hir::HirExprId,
        access_module: Option<etas_hir::HirModuleId>,
        binding: super::reflection::ReflectionBinding,
        members: Vec<String>,
        output: TypeId,
        origin: ConstraintOrigin,
    },
    NumericLiteral {
        expr: Option<etas_hir::HirExprId>,
        text: String,
        kind: NumericLiteralKind,
        ty: TypeId,
        origin: ConstraintOrigin,
    },
    Equal {
        lhs: TypeId,
        rhs: TypeId,
        origin: ConstraintOrigin,
    },
    Assignable {
        from: TypeId,
        to: TypeId,
        origin: ConstraintOrigin,
        reason: AssignabilityReason,
    },
    Callable {
        call: Option<etas_hir::HirExprId>,
        callee: TypeId,
        generic_params: Vec<CallableGenericParam>,
        generic_args: Vec<CallableGenericArg>,
        arg_exprs: Vec<Option<etas_hir::HirExprId>>,
        args: Vec<TypeId>,
        output: TypeId,
        origin: ConstraintOrigin,
    },
    CallableReference {
        expr: etas_hir::HirExprId,
        signature: TypeId,
        generic_params: Vec<CallableGenericParam>,
        expected: Option<TypeId>,
        lexical_effect_params: Vec<String>,
        origin: ConstraintOrigin,
    },
    MethodCall {
        expr: etas_hir::HirExprId,
        method: String,
        access_module: Option<etas_hir::HirModuleId>,
        candidates: Vec<CallableCandidate>,
        generic_args: Vec<CallableGenericArg>,
        arg_exprs: Vec<Option<etas_hir::HirExprId>>,
        args: Vec<TypeId>,
        output: TypeId,
        origin: ConstraintOrigin,
    },
    FieldAccess {
        base: TypeId,
        field: String,
        output: TypeId,
        origin: ConstraintOrigin,
    },
    IndexAccess {
        expr: etas_hir::HirExprId,
        base: TypeId,
        index: TypeId,
        output: TypeId,
        index_error: Option<TypeId>,
        origin: ConstraintOrigin,
    },
    SliceAccess {
        expr: etas_hir::HirExprId,
        base: TypeId,
        start: TypeId,
        end: TypeId,
        output: TypeId,
        origin: ConstraintOrigin,
    },
    Iterable {
        iter: TypeId,
        item: TypeId,
        // A fresh (key, value) shape for entry iterables; solved, not guessed by collect.
        entry_pair: TypeId,
        origin: ConstraintOrigin,
    },
    Unary {
        op: etas_hir::HirUnaryOp,
        operand: TypeId,
        output: TypeId,
        origin: ConstraintOrigin,
    },
    TryOperand {
        source: etas_hir::HirExprId,
        operand: TypeId,
        output: TypeId,
        error: TypeId,
        origin: ConstraintOrigin,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallableCandidate {
    pub ty: TypeId,
    pub generic_params: Vec<CallableGenericParam>,
    pub operation: CallableCandidateOperation,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CallableCandidateOperation {
    #[default]
    Call,
    AgentMethod {
        symbol: etas_hir::SymbolId,
        callable: TypeId,
    },
    CheckedIndex {
        error: Option<TypeId>,
    },
    StdIntrinsic {
        qualified_path: Vec<String>,
        intrinsic: u32,
        has_receiver: bool,
    },
}
