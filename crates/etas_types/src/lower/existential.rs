use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_hir::{HirTypeParamBound, ResolveResult, SymbolKind};

use crate::{
    CallableGenericParam, CheckedSpecBound, CheckedSpecRef, SpecObligation, ValidationRequest,
    pipeline::context::TypePipelineContext,
};

pub(super) fn take_obligations(
    ctx: &mut TypePipelineContext<'_>,
) -> (Vec<SpecObligation>, Vec<ValidationRequest>) {
    let mut obligations = Vec::new();
    let mut validations = Vec::new();
    for (ty, span) in std::mem::take(&mut ctx.existential_annotations) {
        let crate::Type::Existential { bounds } = ctx.interner.store().get(ty).unwrap().clone()
        else {
            unreachable!("existential annotation queue contains only existential types");
        };
        let facts = ctx
            .signature_base
            .map_or(&ctx.signature_facts, |base| &base.facts);
        if let Err(message) = crate::solver::spec_solver::object_safety::check(
            ctx.interner.store(),
            facts,
            &ctx.std_registry,
            &bounds,
        ) {
            validations.push(ValidationRequest::Diagnostic {
                code: TypeDiagnosticCode::TypeMismatch,
                span,
                message,
            });
            continue;
        }
        for bound in bounds {
            let Some(parameters) = application_parameters(ctx, &bound.spec) else {
                validations.push(ValidationRequest::Diagnostic {
                    code: TypeDiagnosticCode::IncompleteTypeFacts,
                    span,
                    message: "existential bound is missing its checked generic declaration".into(),
                });
                continue;
            };
            match crate::constraint::generic_bounds::instantiate(
                &mut ctx.interner,
                &parameters,
                &bound.args,
                span,
            ) {
                Ok(bounds) => obligations.extend(bounds),
                Err(error) => validations.push(error),
            }
        }
    }
    (obligations, validations)
}

fn application_parameters(
    ctx: &mut TypePipelineContext<'_>,
    spec: &CheckedSpecRef,
) -> Option<Vec<CallableGenericParam>> {
    let facts = ctx
        .signature_base
        .map_or(&ctx.signature_facts, |base| &base.facts);
    let path = match spec {
        CheckedSpecRef::Source(symbol) => {
            if let Some(identity) = facts.external_specs.aliases.get(symbol) {
                return Some(
                    facts
                        .external_specs
                        .signatures
                        .get(identity)?
                        .generic_params
                        .clone(),
                );
            }
            if let Some(path) = facts.std_spec_aliases.get(symbol) {
                path.clone()
            } else {
                return facts
                    .spec_signatures
                    .get(symbol)?
                    .generic_parameters_in(facts, ctx.hir);
            }
        }
        CheckedSpecRef::External(identity) => {
            return Some(
                facts
                    .external_specs
                    .signatures
                    .get(identity)?
                    .generic_params
                    .clone(),
            );
        }
        CheckedSpecRef::Std(path) => path.clone(),
        CheckedSpecRef::Canonical(_) => return None,
    };
    let registry = ctx.std_registry.clone();
    let etas_std::StdDecl::Type(declaration) = &registry.lookup_qualified(&path)?.decl else {
        return None;
    };
    Some(super::std::lower_std_generic_params(
        ctx,
        &registry,
        &declaration.params,
    ))
}

pub(super) fn bounds(
    ctx: &mut TypePipelineContext<'_>,
    bounds: &[HirTypeParamBound],
) -> Option<Vec<CheckedSpecBound>> {
    bounds
        .iter()
        .map(|bound| {
            let ResolveResult::Resolved(symbol) = bound.path.resolution else {
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::UnknownType,
                    bound.span,
                    "existential bound must resolve to a type spec",
                ));
                return None;
            };
            let symbol = ctx
                .symbols
                .canonical_symbol(ctx.hir, symbol)
                .unwrap_or(symbol);
            if !matches!(
                ctx.symbol_fact(symbol),
                Some(crate::SymbolTypeFact::Spec { .. })
            ) && ctx
                .hir
                .symbols
                .get(symbol)
                .is_none_or(|symbol| symbol.kind != SymbolKind::Spec)
            {
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::TypeMismatch,
                    bound.span,
                    "existential bound must name a type spec",
                ));
                return None;
            }
            let args = bound
                .args
                .iter()
                .map(|arg| super::type_ref::lower_type_ref(ctx, *arg))
                .collect::<Option<Vec<_>>>()?;
            Some(CheckedSpecBound {
                spec: CheckedSpecRef::Source(symbol),
                args,
            })
        })
        .collect()
}
