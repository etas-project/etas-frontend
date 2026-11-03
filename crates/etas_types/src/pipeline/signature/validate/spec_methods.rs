use etas_core::{Diagnostic, TypeDiagnosticCode};

use crate::{
    CallableSignature, SymbolTypeFact, TypeUnifier,
    pipeline::{context::TypePipelineContext, signature::state::SignaturePipelineState},
};

pub(super) fn validate(ctx: &mut TypePipelineContext<'_>, state: &SignaturePipelineState) {
    let mut evidence = crate::SpecFacts::from(&ctx.signature_facts);
    evidence.signatures.extend(state.spec_signatures.clone());
    evidence.impls.extend(state.spec_impls.iter().cloned());
    evidence
        .type_param_bounds
        .extend(state.type_param_bounds.clone());
    for implementation in &state.spec_impls {
        let Some(spec) = state
            .spec_signatures
            .get(&implementation.spec_symbol)
            .or_else(|| {
                ctx.signature_facts
                    .spec_signatures
                    .get(&implementation.spec_symbol)
            })
            .cloned()
        else {
            continue;
        };
        for method in &spec.methods {
            let Some(provided) = implementation
                .methods
                .iter()
                .find(|candidate| candidate.name == method.name)
            else {
                continue;
            };
            let span = ctx
                .hir
                .symbols
                .get(provided.symbol)
                .map_or(implementation.span, |symbol| symbol.definition_span);
            let expected = method
                .signature
                .as_ref()
                .or_else(|| {
                    method
                        .source_symbol()
                        .and_then(|symbol| callable(ctx, state, symbol))
                })
                .cloned();
            let actual = callable(ctx, state, provided.symbol).cloned();
            let (Some(expected), Some(actual)) = (expected, actual) else {
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::IncompleteTypeFacts,
                    span,
                    "spec method is missing a checked callable signature",
                ));
                continue;
            };
            let bindings = spec
                .param_names
                .iter()
                .cloned()
                .zip(implementation.args.iter().copied())
                .collect();
            let result = crate::lower::spec_method::specialize(
                &mut ctx.interner,
                &expected,
                method.receiver_param,
                bindings,
                implementation.self_type,
            )
            .and_then(|expected| compare(ctx, &evidence, &expected, &actual, span));
            if let Err(reason) = result {
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::TypeMismatch,
                    span,
                    format!(
                        "method `{}` does not satisfy its spec signature: {reason}",
                        method.name
                    ),
                ));
            }
        }
    }
}

fn callable<'a>(
    ctx: &'a TypePipelineContext<'_>,
    state: &'a SignaturePipelineState,
    symbol: etas_hir::SymbolId,
) -> Option<&'a CallableSignature> {
    match state
        .symbol_types
        .get(&symbol)
        .or_else(|| ctx.symbol_fact(symbol))
    {
        Some(SymbolTypeFact::Flow { signature }) => Some(signature),
        _ => None,
    }
}

pub(in crate::pipeline::signature) fn compare(
    ctx: &mut TypePipelineContext<'_>,
    evidence: &crate::SpecFacts,
    expected: &CallableSignature,
    actual: &CallableSignature,
    span: etas_core::Span,
) -> Result<(), String> {
    if expected.params.len() != actual.params.len() {
        return Err("parameter arity differs".into());
    }
    if expected.generic_params.len() != actual.generic_params.len() {
        return Err("generic parameter arity differs".into());
    }
    let mut bindings = std::collections::HashMap::new();
    for (expected, actual) in expected.generic_params.iter().zip(&actual.generic_params) {
        if expected.kind != actual.kind {
            return Err("generic parameter kind differs".into());
        }
        bindings.insert(expected.name.clone(), actual.subject);
    }
    validate_generic_bounds(ctx, evidence, expected, actual, span)?;
    let mut engine =
        crate::TypeSubstitutionEngine::materializing(&mut ctx.interner, &bindings, None);
    let inputs = expected
        .params
        .iter()
        .map(|ty| engine.substitute(*ty))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let output = engine
        .substitute(expected.output)
        .map_err(|error| error.to_string())?;
    let mut unifier = TypeUnifier::new(ctx.interner.store());
    for (expected, actual) in inputs.into_iter().zip(&actual.params) {
        unifier
            .unify(expected, *actual)
            .map_err(|_| "input type differs".to_owned())?;
    }
    unifier
        .unify(output, actual.output)
        .map_err(|_| "output type differs".to_owned())?;
    Ok(())
}

fn validate_generic_bounds(
    ctx: &mut TypePipelineContext<'_>,
    evidence: &crate::SpecFacts,
    expected: &CallableSignature,
    actual: &CallableSignature,
    span: etas_core::Span,
) -> Result<(), String> {
    let inverse = actual
        .generic_params
        .iter()
        .zip(&expected.generic_params)
        .map(|(actual, expected)| (actual.name.clone(), expected.subject))
        .collect();
    let mut substitution =
        crate::TypeSubstitutionEngine::materializing(&mut ctx.interner, &inverse, None);
    let mut obligations = Vec::new();
    for (actual, expected) in actual.generic_params.iter().zip(&expected.generic_params) {
        for bound in &actual.bounds {
            let args = bound
                .args
                .iter()
                .map(|ty| substitution.substitute(*ty))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            obligations.push(crate::SpecObligation {
                ty: expected.subject,
                spec: bound.spec.clone(),
                args,
                span,
            });
        }
    }
    let report = crate::solver::spec_solver::solve_spec_obligations(
        ctx.interner.store(),
        evidence,
        &obligations,
        &std::collections::HashMap::new(),
    );
    if let Some(failure) = report.failures.first() {
        return Err(format!(
            "implementation strengthens the spec's generic bounds: {}",
            failure.message
        ));
    }
    Ok(())
}
