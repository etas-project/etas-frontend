use etas_core::{Diagnostic, TypeDiagnosticCode};

pub fn type_error(
    code: TypeDiagnosticCode,
    span: etas_core::Span,
    message: impl Into<String>,
) -> Diagnostic {
    Diagnostic::type_check(code, span, message)
}
