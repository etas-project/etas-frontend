use std::collections::HashMap;

use etas_core::Diagnostic;

use crate::{SpecFacts, ValidationRequest, pipeline::context::TypePipelineContext};

pub(in crate::pipeline::signature) fn run(ctx: &mut TypePipelineContext<'_>) {
    let (mut obligations, mut validations) = crate::lower::type_application::take_obligations(ctx);
    for implementation in &ctx.signature_facts.spec_impls {
        let parameters = if let Some(identity) = ctx
            .signature_facts
            .external_specs
            .aliases
            .get(&implementation.spec_symbol)
        {
            ctx.signature_facts
                .external_specs
                .signatures
                .get(identity)
                .map(|signature| signature.generic_params.clone())
        } else {
            source_spec_parameters(ctx, implementation.spec_symbol)
        };
        let Some(parameters) = parameters else {
            validations.push(ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: implementation.span,
                message: "spec implementation is missing its checked generic declaration".into(),
            });
            continue;
        };
        match crate::constraint::generic_bounds::instantiate(
            &mut ctx.interner,
            &parameters,
            &implementation.args,
            implementation.span,
        ) {
            Ok(bounds) => obligations.extend(bounds),
            Err(error) => validations.push(error),
        }
    }
    for (_, item) in ctx.hir.items.iter() {
        let etas_hir::HirItem::Spec(declaration) = item else {
            continue;
        };
        let Some(signature) = ctx.signature_facts.spec_signatures.get(&declaration.symbol) else {
            validations.push(ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: declaration.span,
                message: "spec declaration is missing its checked signature".into(),
            });
            continue;
        };
        for bound in &signature.super_specs {
            let parameters = match &bound.super_spec {
                crate::CheckedSpecRef::External(identity) => ctx
                    .signature_facts
                    .external_specs
                    .signatures
                    .get(identity)
                    .map(|signature| signature.generic_params.clone()),
                crate::CheckedSpecRef::Source(symbol) => {
                    if let Some(identity) = ctx.signature_facts.external_specs.aliases.get(symbol) {
                        ctx.signature_facts
                            .external_specs
                            .signatures
                            .get(identity)
                            .map(|signature| signature.generic_params.clone())
                    } else {
                        source_spec_parameters(ctx, *symbol)
                    }
                }
                crate::CheckedSpecRef::Std(_) | crate::CheckedSpecRef::Canonical(_) => continue,
            };
            let Some(parameters) = parameters else {
                validations.push(ValidationRequest::Diagnostic {
                    code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    span: declaration.span,
                    message: "super-spec is missing its checked generic declaration".into(),
                });
                continue;
            };
            match crate::constraint::generic_bounds::instantiate(
                &mut ctx.interner,
                &parameters,
                &bound.args,
                declaration.span,
            ) {
                Ok(bounds) => obligations.extend(bounds),
                Err(error) => validations.push(error),
            }
        }
    }
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

fn source_spec_parameters(
    ctx: &TypePipelineContext<'_>,
    spec: etas_hir::SymbolId,
) -> Option<Vec<crate::CallableGenericParam>> {
    ctx.signature_facts
        .spec_signatures
        .get(&spec)?
        .generic_parameters_in(&ctx.signature_facts, ctx.hir)
}
