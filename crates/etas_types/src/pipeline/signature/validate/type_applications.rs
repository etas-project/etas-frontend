use std::collections::HashMap;

use etas_core::Diagnostic;

use crate::{SpecFacts, ValidationRequest, pipeline::context::TypePipelineContext};

pub(in crate::pipeline::signature) fn run(ctx: &mut TypePipelineContext<'_>) {
    let (obligations, validations) = crate::lower::type_application::take_obligations(ctx);
    let facts = SpecFacts::from(&ctx.signature_facts);
    let report = crate::solver::spec_solver::solve_spec_obligations(
        ctx.interner.store(),
        &facts,
        &obligations,
        &HashMap::new(),
    );
    for failure in report.failures {
        ctx.diagnostics.push(Diagnostic::type_check(
            failure.code,
            failure.span,
            failure.message,
        ));
    }
    for validation in validations {
        if let ValidationRequest::Diagnostic {
            code,
            span,
            message,
        } = validation
        {
            ctx.diagnostics
                .push(Diagnostic::type_check(code, span, message));
        }
    }
}
