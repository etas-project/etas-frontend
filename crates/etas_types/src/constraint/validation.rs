use etas_core::{Span, TypeDiagnosticCode};

use crate::TypeId;

#[derive(Clone, Debug)]
pub enum ValidationRequest {
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
