use etas_hir::{HirArg, HirExprId, HirGenericArg};

use crate::{
    AssignabilityReason, CallableCandidate, CallableCandidateOperation, ConstraintOrigin, FlowType,
    Type, TypeConstraint, TypeId,
    method::{MethodEvidence, MethodReceiver, MethodSelection},
    pipeline::context::BodyCollectContext,
};

pub(super) struct SelectedMethodCall<'a> {
    pub expr: HirExprId,
    pub receiver: HirExprId,
    pub method: &'a str,
    pub generic_args: &'a [HirGenericArg],
    pub args: &'a [HirArg],
    pub selection: MethodSelection,
    pub expected: Option<TypeId>,
}

pub fn collect_method_call<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    expr: HirExprId,
    receiver: HirExprId,
    method: &str,
    generic_args: &[HirGenericArg],
    args: &[HirArg],
    expected: Option<TypeId>,
) -> Result<TypeId, E> {
    let associated = match &ctx.ctx.hir.exprs[receiver] {
        etas_hir::HirExpr::Path(path) => match path.resolution {
            etas_hir::ResolveResult::Resolved(symbol) => matches!(
                ctx.ctx.symbol_fact(symbol),
                Some(
                    crate::SymbolTypeFact::Type { .. }
                        | crate::SymbolTypeFact::NominalType { .. }
                        | crate::SymbolTypeFact::TypeAlias { .. }
                )
            ),
            _ => false,
        },
        _ => false,
    };
    collect_selected(
        ctx,
        SelectedMethodCall {
            expr,
            receiver,
            method,
            generic_args,
            args,
            selection: if associated {
                MethodSelection::Associated
            } else {
                MethodSelection::Unqualified
            },
            expected,
        },
    )
}

pub(super) fn collect_selected<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    call: SelectedMethodCall<'_>,
) -> Result<TypeId, E> {
    let SelectedMethodCall {
        expr,
        receiver,
        method,
        generic_args,
        args,
        selection,
        expected,
    } = call;
    ctx.checkpoint()?;
    let span = ctx.ctx.hir.exprs[expr].span(&ctx.ctx.hir.blocks);
    if generic_args
        .iter()
        .any(|arg| matches!(arg, HirGenericArg::Wildcard { .. }))
    {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span,
            message: "method generic arguments cannot be wildcard selectors".into(),
        });
    }
    let checked_generic_args = super::call::lower_callable_generic_args(ctx, generic_args);
    let argument_names = std::iter::once(None)
        .chain(args.iter().map(|arg| match arg {
            HirArg::Positional(_) => None,
            HirArg::Named { name, .. } => Some(name.clone()),
        }))
        .collect::<Vec<_>>();
    let associated_receiver = if matches!(selection, MethodSelection::Associated) {
        let etas_hir::HirExpr::Path(path) = &ctx.ctx.hir.exprs[receiver] else {
            unreachable!("associated method receiver is a type path")
        };
        let etas_hir::ResolveResult::Resolved(symbol) = path.resolution else {
            unreachable!("associated method has a resolved owner")
        };
        ctx.ctx.type_fact_as_type_id(symbol)
    } else {
        None
    };
    let entries = ctx
        .ctx
        .methods()
        .member(method)
        .filter(|(_, entry)| {
            matches!(
                (&selection, entry.receiver),
                (MethodSelection::Associated, MethodReceiver::Associated)
            ) || (!matches!(selection, MethodSelection::Associated)
                && entry.receiver != MethodReceiver::Associated)
        })
        .map(|(id, entry)| (id, entry.clone()))
        .collect::<Vec<_>>();
    let mut candidates = Vec::with_capacity(entries.len());
    let mut candidate_hints = Vec::with_capacity(entries.len());
    for (index, entry) in entries {
        let owner_count = entry.owner_params.len();
        let mut signature = entry.signature.clone();
        signature
            .generic_params
            .splice(0..0, entry.owner_params.clone());
        if entry.receiver == MethodReceiver::Associated {
            signature.params.insert(
                0,
                associated_receiver.expect("associated owner has a checked type fact"),
            );
        }
        let original_parameters = signature.generic_params.clone();
        let Some(mut signature) =
            super::callable_instantiation::instantiate_signature(ctx, signature)
        else {
            continue;
        };
        let parameter_bindings = original_parameters
            .iter()
            .zip(&signature.generic_params)
            .map(|(old, new)| (old.subject, new.subject))
            .collect();
        let evidence = {
            let names = std::collections::HashMap::new();
            let mut engine =
                crate::TypeSubstitutionEngine::materializing(&mut ctx.ctx.interner, &names, None)
                    .with_type_parameters(&parameter_bindings);
            let result = match &entry.evidence {
                MethodEvidence::Inherent => Ok(MethodEvidence::Inherent),
                MethodEvidence::Implementation {
                    spec,
                    subject,
                    args,
                } => engine.substitute(*subject).and_then(|subject| {
                    args.iter()
                        .map(|arg| engine.substitute(*arg))
                        .collect::<Result<Vec<_>, _>>()
                        .map(|args| MethodEvidence::Implementation {
                            spec: spec.clone(),
                            subject,
                            args,
                        })
                }),
                MethodEvidence::Bound {
                    spec,
                    subject,
                    args,
                } => engine.substitute(*subject).and_then(|subject| {
                    args.iter()
                        .map(|arg| engine.substitute(*arg))
                        .collect::<Result<Vec<_>, _>>()
                        .map(|args| MethodEvidence::Bound {
                            spec: spec.clone(),
                            subject,
                            args,
                        })
                }),
            };
            match result {
                Ok(evidence) => evidence,
                Err(error) => {
                    ctx.validate(crate::ValidationRequest::Diagnostic {
                        code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                        span,
                        message: error.to_string(),
                    });
                    continue;
                }
            }
        };
        let owner_params = signature
            .generic_params
            .drain(..owner_count)
            .collect::<Vec<_>>();
        let owner_bindings = entry
            .owner_params
            .iter()
            .zip(&owner_params)
            .map(|(old, new)| (old.subject, new.subject))
            .collect();
        let flow = FlowType {
            input: signature.params.clone(),
            output: signature.output,
            effects: signature.effects.clone(),
        };
        let mut ty = ctx.ctx.interner.intern(Type::Function(flow.clone()));
        let mut inputs = signature.params.clone();
        if let Some(instantiated) = super::callable_instantiation::signature_hints(
            ctx,
            ty,
            &signature.generic_params,
            &checked_generic_args,
            span,
        ) {
            inputs = instantiated.input;
            // Callback rows stay schematic until their latent sources solve.
            // An explicit result is still available to the next chain link.
            ty = ctx.ctx.interner.intern(Type::Function(FlowType {
                output: instantiated.output,
                ..flow
            }));
        }
        let mut names = entry.parameter_names.clone();
        if entry.receiver == MethodReceiver::Associated {
            names.insert(0, None);
        }
        candidate_hints.push(
            crate::method::arguments::slots(&names, &argument_names)
                .ok()
                .and_then(|slots| {
                    slots
                        .into_iter()
                        .map(|slot| inputs.get(slot as usize).copied())
                        .collect()
                }),
        );
        candidates.push(CallableCandidate {
            ty,
            signature,
            operation: CallableCandidateOperation::Method {
                index,
                owner_params,
                owner_bindings,
                evidence,
            },
        });
    }
    let hints = common_inputs(&candidate_hints);
    let receiver_ty = match associated_receiver {
        Some(ty) => ctx.record_expr_type(receiver, ty),
        None => super::expr::collect_expr(
            ctx,
            receiver,
            hints.as_ref().and_then(|hints| hints.first()).copied(),
        )?,
    };
    let mut arg_types = vec![receiver_ty];
    for (index, arg) in args.iter().enumerate() {
        let value = match arg {
            HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
        };
        arg_types.push(super::expr::collect_expr(
            ctx,
            value,
            hints
                .as_ref()
                .and_then(|hints| hints.get(index + 1))
                .copied(),
        )?);
    }
    let output = ctx.fresh_type_var();
    let probes = candidates.iter().map(|_| ctx.fresh_type_var()).collect();
    ctx.emit(TypeConstraint::MethodCall {
        expr,
        method: method.into(),
        selection,
        probes,
        argument_names,
        access_module: ctx.ctx.hir.item_module(ctx.state.item),
        candidates,
        generic_args: checked_generic_args,
        arg_exprs: std::iter::once(Some(receiver))
            .chain(args.iter().map(|arg| {
                Some(match arg {
                    HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => *expr,
                })
            }))
            .collect(),
        args: arg_types,
        output,
        origin: ConstraintOrigin { span },
    });
    if let Some(expected) = expected {
        ctx.emit(TypeConstraint::Assignable {
            from: output,
            to: expected,
            origin: ConstraintOrigin { span },
            reason: AssignabilityReason::Other,
        });
    }
    Ok(output)
}

fn common_inputs(candidates: &[Option<Vec<TypeId>>]) -> Option<Vec<TypeId>> {
    let first = candidates.first()?.as_ref()?.clone();
    candidates
        .iter()
        .all(|candidate| candidate.as_ref() == Some(&first))
        .then_some(first)
}
