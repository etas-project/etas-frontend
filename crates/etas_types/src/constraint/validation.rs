use etas_core::{Span, TypeDiagnosticCode};

use crate::TypeId;

#[derive(Clone, Debug)]
pub enum ValidationRequest {
    HandlerNeverCompletion {
        ty: TypeId,
        span: Span,
    },
    WritablePlace {
        target: etas_hir::HirExprId,
        span: Span,
    },
    MatchCoverage {
        scrutinee: TypeId,
        arms: Vec<etas_hir::HirPatId>,
        span: Span,
    },
    CallableArity {
        callee_ty: TypeId,
        arg_count: usize,
        span: Span,
    },
    Diagnostic {
        code: TypeDiagnosticCode,
        span: Span,
        message: String,
    },
}
