use etas_core::{Span, TypeDiagnosticCode};
use etas_hir::SymbolId;

use crate::{
    SpecObligation, SymbolTypeFact, TypeId, ValidationRequest,
    pipeline::context::TypePipelineContext,
};

pub(crate) struct TypeApplication {
    pub symbol: SymbolId,
    pub arguments: Vec<TypeId>,
    pub span: Span,
}

pub(crate) fn take_obligations(
    ctx: &mut TypePipelineContext<'_>,
) -> (Vec<SpecObligation>, Vec<ValidationRequest>) {
    let mut obligations = Vec::new();
    let mut validations = Vec::new();
    for application in std::mem::take(&mut ctx.type_applications) {
        let parameters = match ctx.symbol_fact(application.symbol) {
            Some(SymbolTypeFact::TypeAlias { params, .. }) => params.clone(),
            Some(
                SymbolTypeFact::NominalType { constructor, .. }
                | SymbolTypeFact::Type { constructor },
            ) => {
                let Some(parameters) = ctx.type_generics(TypeId(constructor.0)) else {
                    validations.push(ValidationRequest::Diagnostic {
                        code: TypeDiagnosticCode::IncompleteTypeFacts,
                        span: application.span,
                        message: "type application is missing its checked generic declaration"
                            .into(),
                    });
                    continue;
                };
                parameters.clone()
            }
            _ => {
                validations.push(ValidationRequest::Diagnostic {
                    code: TypeDiagnosticCode::IncompleteTypeFacts,
                    span: application.span,
                    message: "type application is missing its checked declaration".into(),
                });
                continue;
            }
        };
        match crate::constraint::generic_bounds::instantiate(
            &mut ctx.interner,
            &parameters,
            &application.arguments,
            application.span,
        ) {
            Ok(bounds) => obligations.extend(bounds),
            Err(error) => validations.push(error),
        }
    }
    (obligations, validations)
}
