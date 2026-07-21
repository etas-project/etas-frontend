use crate::{ConstraintOrigin, TypeId};

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
        callee: TypeId,
        generic_param_names: Vec<String>,
        generic_args: Vec<TypeId>,
        args: Vec<TypeId>,
        output: TypeId,
        origin: ConstraintOrigin,
    },
    MethodCall {
        method: String,
        candidates: Vec<TypeId>,
        generic_args: Vec<TypeId>,
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
        origin: ConstraintOrigin,
    },
    Unary {
        op: etas_hir::HirUnaryOp,
        operand: TypeId,
        output: TypeId,
        origin: ConstraintOrigin,
    },
    TryOperand {
        operand: TypeId,
        output: TypeId,
        error: TypeId,
        origin: ConstraintOrigin,
    },
}
