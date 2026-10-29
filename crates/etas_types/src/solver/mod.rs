mod agent;
pub mod assignability;
pub mod callable;
mod callable_reference;
pub mod callable_spec;
mod construction;
pub mod effect_row;
#[cfg(test)]
mod execution_tests;
pub mod field;
mod handler;
pub mod index;
#[cfg(test)]
mod index_tests;
mod iterable;
mod method;
mod numeric_literal;
mod reflection;
pub mod report;
mod return_type;
pub mod spec_solver;
mod statement;
mod try_capture;
pub mod unification;

pub use assignability::{Assignable, TypeRelation};
pub use report::{SolverFailure, SolverReport};
pub use unification::{Substitution, TypeUnifier, UnifyError};

use std::collections::HashMap;
use std::collections::HashSet;

use crate::{
    ConstraintOrigin, NumericLiteralKind, PrimitiveType, SpecFacts, SpecObligation, Type,
    TypeConstraint, TypeId, TypeStore, ty::display_type,
};

pub struct TypeSolveInput<'a> {
    pub rigid_type_parameters: &'a [crate::CallableGenericParam],
    pub store: &'a TypeStore,
    pub known_std_types: &'a crate::KnownStdTypes,
    pub declaration_facts: &'a crate::TypeFacts,
    pub spec_facts: &'a SpecFacts,
    pub constraints: &'a [TypeConstraint],
    pub spec_obligations: &'a [SpecObligation],
}

pub struct TypeSolver;

impl TypeSolver {
    pub fn solve(input: TypeSolveInput<'_>) -> SolverReport {
        Self::solve_with_checkpoint(input, &|| Ok::<(), std::convert::Infallible>(()))
            .unwrap_or_else(|never| match never {})
    }

    pub fn solve_controlled(
        input: TypeSolveInput<'_>,
        control: &dyn etas_utils::PipelineExecutionControl,
    ) -> Result<SolverReport, etas_utils::PipelineInterruption> {
        Self::solve_with_checkpoint(input, &|| control.checkpoint())
    }

    pub(crate) fn solve_with_checkpoint<E>(
        input: TypeSolveInput<'_>,
        checkpoint: &impl Fn() -> Result<(), E>,
    ) -> Result<SolverReport, E> {
        checkpoint()?;
        let mut report = SolverReport::default();
        let mut pending_access_constraints = Vec::new();
        let mut pending_method_constraints = Vec::new();
        let mut checked_method_constraints = Vec::new();
        let mut pending_unary_constraints = Vec::new();
        let mut numeric_literals = Vec::new();
        let mut inferences = Vec::new();
        let mut returns = Vec::new();
        let mut captures = Vec::new();
        let mut callable_references = Vec::new();
        let mut handler_parameters = Vec::new();
        let mut discarded_values = Vec::new();
        let mut deferred_bottom = Vec::new();
        for constraint in input.constraints {
            checkpoint()?;
            match constraint {
                TypeConstraint::DiscardValue {
                    value,
                    output,
                    unit,
                    origin,
                } => {
                    discarded_values.push(statement::Discard {
                        value: *value,
                        output: *output,
                        unit: *unit,
                        origin: *origin,
                    });
                }
                TypeConstraint::HandlerInstantiation {
                    parameters,
                    requests,
                    deferred,
                    origin,
                } => {
                    if let Some((arm, body)) = deferred {
                        report.deferred_handler_instantiations.insert(
                            *arm,
                            crate::DeferredHandlerInstantiation {
                                body: *body,
                                parameters: parameters
                                    .iter()
                                    .map(|(parameter, candidate, rigid)| {
                                        crate::HandlerTypeBinding {
                                            parameter: parameter.clone(),
                                            candidate: *candidate,
                                            rigid: *rigid,
                                            parametric: false,
                                        }
                                    })
                                    .collect(),
                                span: origin.span,
                            },
                        );
                    } else {
                        handler::instantiate(
                            input.store,
                            &mut report,
                            parameters,
                            requests,
                            *origin,
                            &mut handler_parameters,
                        );
                    }
                }
                TypeConstraint::Constructible { target, origin } => {
                    let target = resolve_known_substitutions(input.store, &report, *target);
                    if let Some(failure) =
                        construction::validate(input.store, input.known_std_types, target, *origin)
                    {
                        report.push(failure);
                    }
                }
                TypeConstraint::ReturnType { .. } => returns.push(constraint),
                TypeConstraint::AgentInference { .. } => inferences.push(constraint),
                TypeConstraint::CallableReference { .. } => callable_references.push(constraint),
                TypeConstraint::Reflection {
                    expr,
                    access_module,
                    binding,
                    members,
                    output,
                    origin,
                } => {
                    match reflection::solve(reflection::ReflectionSolveInput {
                        store: input.store,
                        facts: input.declaration_facts,
                        binding,
                        members,
                        origin: *origin,
                        solved: &report,
                        access_module: *access_module,
                    }) {
                        Ok(fact) => {
                            let mut unifier = TypeUnifier::with_known_substitution(
                                input.store,
                                &report.substitutions,
                            );
                            if unifier.unify(*output, fact.descriptor_type).is_err() {
                                report.push(SolverFailure { code: etas_core::TypeDiagnosticCode::TypeMismatch, span: origin.span, message: "reflection descriptor type does not match the required type".into() });
                            } else {
                                report.substitutions.extend(&unifier.into_substitution());
                                report.reflections.insert(*expr, fact);
                            }
                        }
                        Err(failure) => report.push(failure),
                    }
                }
                TypeConstraint::NumericLiteral { .. } => numeric_literals.push(constraint),
                TypeConstraint::Equal { lhs, rhs, origin } => {
                    let lhs = resolve_known_substitutions(input.store, &report, *lhs);
                    let rhs = resolve_known_substitutions(input.store, &report, *rhs);
                    let mut unifier =
                        TypeUnifier::with_known_substitution(input.store, &report.substitutions);
                    if unifier.unify(lhs, rhs).is_err() {
                        report.push(SolverFailure {
                            code: etas_core::TypeDiagnosticCode::TypeMismatch,
                            span: origin.span,
                            message: "type equality constraint could not be solved".to_owned(),
                        });
                    } else {
                        report.substitutions.extend(&unifier.into_substitution());
                    }
                }
                TypeConstraint::Assignable {
                    from,
                    to,
                    origin,
                    reason,
                } => {
                    let from = resolve_known_substitutions(input.store, &report, *from);
                    let to = resolve_known_substitutions(input.store, &report, *to);
                    if !is_assignable_for_reason(input.store, from, to, reason) {
                        report.push(SolverFailure {
                            code: assignability_failure_code(reason),
                            span: origin.span,
                            message: format!(
                                "{}: `{}` is not assignable to `{}`",
                                assignability_failure_message(reason),
                                display_type(input.store, from),
                                display_type(input.store, to)
                            ),
                        });
                    } else if matches!(
                        input.store.get(from),
                        Some(Type::Primitive(PrimitiveType::Never))
                    ) && is_unresolved_type_var(input.store, to)
                    {
                        // Other return/value constraints get to infer the target
                        // before a bottom-only path supplies its final default.
                        deferred_bottom.push((from, to, *origin));
                    } else {
                        let mut unifier = TypeUnifier::with_known_substitution(
                            input.store,
                            &report.substitutions,
                        );
                        if unifier.unify(from, to).is_ok() {
                            report.substitutions.extend(&unifier.into_substitution());
                        }
                    }
                }
                TypeConstraint::Callable {
                    call,
                    callee,
                    generic_params,
                    generic_args,
                    arg_exprs,
                    args,
                    output,
                    origin,
                } => {
                    let callee = resolve_known_substitutions(input.store, &report, *callee);
                    if let Some(failure) =
                        construction::validate(input.store, input.known_std_types, callee, *origin)
                    {
                        report.push(failure);
                        continue;
                    }
                    let agent =
                        match agent::resolve(input.store, input.declaration_facts, callee, *origin)
                        {
                            Ok(agent) => agent,
                            Err(failure) => {
                                report.push(failure);
                                continue;
                            }
                        };
                    let (callee, generic_params) =
                        agent.map_or((callee, generic_params.as_slice()), |entry| {
                            (
                                entry.callable_type,
                                entry.callable.generic_params.as_slice(),
                            )
                        });
                    let output = resolve_known_substitutions(input.store, &report, *output);
                    let args = args
                        .iter()
                        .map(|arg| resolve_known_substitutions(input.store, &report, *arg))
                        .collect::<Vec<_>>();
                    let known_substitutions = report.substitutions.clone();
                    let solved = solve_callable_constraint(
                        input.store,
                        input.spec_facts,
                        CallableConstraintSolveInput {
                            rigid_type_parameters: input.rigid_type_parameters,
                            call: *call,
                            callee,
                            generic_params,
                            explicit_generic_args: generic_args,
                            arg_exprs,
                            args: &args,
                            output,
                            origin: *origin,
                            known_substitutions,
                        },
                    );
                    if solved.failures.is_empty()
                        && let Some((call, method)) = call.zip(agent.and_then(|entry| entry.method))
                    {
                        report.agent_method_calls.insert(call, method);
                    }
                    report.append(solved);
                }
                TypeConstraint::MethodCall {
                    expr,
                    method,
                    access_module,
                    candidates,
                    generic_args,
                    arg_exprs,
                    args,
                    output,
                    origin,
                } => {
                    let invocation = method::Input {
                        expr,
                        method,
                        access_module,
                        candidates,
                        generic_args,
                        arg_exprs,
                        args,
                        output,
                        origin,
                    };
                    if invocation.ready(input.store, &report) {
                        method::solve(
                            &input,
                            invocation,
                            &mut report,
                            &mut checked_method_constraints,
                            checkpoint,
                        )?;
                    } else {
                        pending_method_constraints.push(invocation);
                    }
                }
                TypeConstraint::FieldAccess { .. } => {
                    solve_or_defer_access_constraint(
                        input.store,
                        input.known_std_types,
                        input.spec_facts,
                        &mut report,
                        constraint,
                        &mut pending_access_constraints,
                    );
                }
                TypeConstraint::IndexAccess { .. } | TypeConstraint::SliceAccess { .. } => {
                    solve_or_defer_access_constraint(
                        input.store,
                        input.known_std_types,
                        input.spec_facts,
                        &mut report,
                        constraint,
                        &mut pending_access_constraints,
                    );
                }
                TypeConstraint::Iterable {
                    iter,
                    item,
                    entry_pair,
                    origin,
                } => iterable::solve(
                    input.store,
                    input.known_std_types,
                    &mut report,
                    *iter,
                    *item,
                    *entry_pair,
                    origin,
                ),
                TypeConstraint::Unary { .. } => pending_unary_constraints.push(constraint),
                TypeConstraint::TryOperand { .. } => captures.push(constraint),
            }
        }
        statement::solve_ready(
            input.store,
            &mut report,
            &mut discarded_values,
            false,
            checkpoint,
        )?;
        return_type::solve(input.store, &mut report, &returns, checkpoint)?;
        try_capture::solve(input.store, &mut report, &captures, checkpoint)?;
        statement::solve_ready(
            input.store,
            &mut report,
            &mut discarded_values,
            false,
            checkpoint,
        )?;
        pending_access_constraints.extend(checked_method_constraints);
        method::solve_ready(
            &input,
            &mut report,
            &mut pending_method_constraints,
            &mut pending_access_constraints,
            checkpoint,
        )?;
        solve_numeric_literals(input.store, &mut report, numeric_literals, checkpoint)?;
        solve_unary_constraints(
            input.store,
            &mut report,
            pending_unary_constraints,
            checkpoint,
        )?;
        statement::solve_ready(
            input.store,
            &mut report,
            &mut discarded_values,
            false,
            checkpoint,
        )?;
        method::solve_pending(
            &input,
            &mut report,
            pending_method_constraints,
            &mut pending_access_constraints,
            checkpoint,
        )?;
        solve_pending_access_constraints(
            input.store,
            input.known_std_types,
            input.spec_facts,
            &mut report,
            pending_access_constraints,
            checkpoint,
        )?;
        callable_reference::solve(input.store, &mut report, &callable_references, checkpoint)?;
        for (from, to, origin) in deferred_bottom {
            checkpoint()?;
            let to = resolve_known_substitutions(input.store, &report, to);
            if is_unresolved_type_var(input.store, to) {
                let mut unifier =
                    TypeUnifier::with_known_substitution(input.store, &report.substitutions);
                if unifier.unify(from, to).is_ok() {
                    report.substitutions.extend(&unifier.into_substitution());
                } else {
                    report.push(SolverFailure {
                        code: etas_core::TypeDiagnosticCode::TypeMismatch,
                        span: origin.span,
                        message: "nonreturning expression result could not be inferred".into(),
                    });
                }
            }
        }
        statement::solve_ready(
            input.store,
            &mut report,
            &mut discarded_values,
            true,
            checkpoint,
        )?;
        handler::finalize_parameters(input.store, &mut report, handler_parameters);
        let deferred_obligations = std::mem::take(&mut report.pending_spec_obligations);
        let obligations = input
            .spec_obligations
            .iter()
            .chain(&deferred_obligations)
            .map(|obligation| crate::SpecObligation {
                ty: resolve_known_substitutions(input.store, &report, obligation.ty),
                spec: obligation.spec.clone(),
                args: obligation
                    .args
                    .iter()
                    .map(|arg| resolve_known_substitutions(input.store, &report, *arg))
                    .collect(),
                span: obligation.span,
            })
            .collect::<Vec<_>>();
        for obligation in &obligations {
            checkpoint()?;
            report.append(spec_solver::solve_spec_obligations(
                input.store,
                input.spec_facts,
                std::slice::from_ref(obligation),
                &report.named_substitutions,
            ));
        }
        for constraint in inferences {
            checkpoint()?;
            let TypeConstraint::AgentInference {
                expr,
                owner,
                prompt,
                prompt_type,
                output,
                schema,
                action_symbol,
                origin,
            } = constraint
            else {
                continue;
            };
            let method = input
                .declaration_facts
                .agent_methods
                .values()
                .find(|method| method.body == *owner);
            let prompt_type = resolve_known_substitutions(input.store, &report, *prompt_type);
            let output = resolve_known_substitutions(input.store, &report, *output);
            let failure = if method.is_none() {
                Some((
                    etas_core::TypeDiagnosticCode::TypeMismatch,
                    "perform infer is available only inside an agent method",
                ))
            } else if !matches!(
                input.store.get(prompt_type),
                Some(Type::Prompt | Type::Primitive(crate::PrimitiveType::Never))
            ) {
                Some((
                    etas_core::TypeDiagnosticCode::TypeMismatch,
                    "perform infer requires a Prompt, not an agent's input or output value",
                ))
            } else if matches!(input.store.get(output), None | Some(Type::Var(_)))
                || (matches!(input.store.get(output), Some(Type::Named(_)))
                    && !input.known_std_types.opaque_values.contains(&output))
            {
                Some((
                    etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    "perform infer requires an output type argument or an expected result type",
                ))
            } else {
                None
            };
            if let Some((code, message)) = failure {
                report.push(SolverFailure {
                    code,
                    span: origin.span,
                    message: message.into(),
                });
            } else if let Some(method) = method {
                report.agent_inferences.insert(
                    *expr,
                    crate::CheckedAgentInferenceFact {
                        owner: *owner,
                        agent: method.agent,
                        method: method.symbol,
                        prompt: *prompt,
                        output,
                        schema: *schema,
                        action_symbol: *action_symbol,
                    },
                );
            }
        }
        checkpoint()?;
        Ok(report)
    }
}

struct CallableConstraintSolveInput<'a> {
    rigid_type_parameters: &'a [crate::CallableGenericParam],
    call: Option<etas_hir::HirExprId>,
    callee: TypeId,
    generic_params: &'a [crate::CallableGenericParam],
    explicit_generic_args: &'a [crate::CallableGenericArg],
    arg_exprs: &'a [Option<etas_hir::HirExprId>],
    args: &'a [TypeId],
    output: TypeId,
    origin: ConstraintOrigin,
    known_substitutions: Substitution,
}

fn solve_callable_constraint(
    store: &TypeStore,
    spec_facts: &SpecFacts,
    input: CallableConstraintSolveInput<'_>,
) -> SolverReport {
    // A callback can mention its enclosing declaration's rigid parameters.
    // Only the callee's own checked generic parameters are instantiable here.
    let declared_names = input
        .generic_params
        .iter()
        .filter(|param| param.kind == crate::CallableGenericParamKind::Type)
        .map(|param| param.name.clone())
        .collect::<Vec<_>>();
    let generic_param_names = declared_names.as_slice();
    let effect_param_names = input
        .generic_params
        .iter()
        .filter(|param| param.kind == crate::CallableGenericParamKind::Effect)
        .map(|param| param.name.clone())
        .collect::<Vec<_>>();
    let mut type_generic_args = Vec::new();
    let mut effect_row_bindings = HashMap::new();
    let mut deferred_effect_row_obligations = Vec::new();
    let mut generic_argument_failure = None;
    if input.explicit_generic_args.len() > input.generic_params.len() {
        generic_argument_failure = Some(format!(
            "call accepts at most {} generic argument(s), got {}",
            input.generic_params.len(),
            input.explicit_generic_args.len()
        ));
    } else {
        for (param, arg) in input.generic_params.iter().zip(input.explicit_generic_args) {
            match (param.kind, arg) {
                (crate::CallableGenericParamKind::Type, crate::CallableGenericArg::Type(ty)) => {
                    type_generic_args.push(*ty);
                }
                (
                    crate::CallableGenericParamKind::Effect,
                    crate::CallableGenericArg::EffectRow(row),
                ) => {
                    effect_row_bindings.insert(param.name.clone(), row.clone());
                }
                (crate::CallableGenericParamKind::Type, _) => {
                    generic_argument_failure = Some(format!(
                        "generic parameter `{}` requires a type argument",
                        param.name
                    ));
                    break;
                }
                (crate::CallableGenericParamKind::Effect, _) => {
                    generic_argument_failure = Some(format!(
                        "generic parameter `effect {}` requires an effect-row argument",
                        param.name
                    ));
                    break;
                }
            }
        }
    }
    if let Some(message) = generic_argument_failure {
        let mut report = SolverReport::default();
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: input.origin.span,
            message,
        });
        return report;
    }
    let mut initial_substitutions = input.known_substitutions;
    let mut explicit_binders = TypeUnifier::with_substitution(store, initial_substitutions.clone());
    for (param, arg) in input.generic_params.iter().zip(input.explicit_generic_args) {
        if let crate::CallableGenericArg::Type(ty) = arg
            && matches!(store.get(param.subject), Some(Type::Var(_)))
            && let Err(error) = explicit_binders.unify(param.subject, *ty)
        {
            let mut report = SolverReport::default();
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                span: input.origin.span,
                message: format!(
                    "explicit generic argument conflicts with its checked binder: {error:?}"
                ),
            });
            return report;
        }
    }
    initial_substitutions.extend(explicit_binders.substitution());
    let mut report = callable::solve_callable_with_named_substitutions(
        store,
        callable::CallableSolveInput {
            callee: input.callee,
            generic_param_names,
            explicit_generic_args: &type_generic_args,
            args: input.args,
            output: input.output,
            origin: input.origin,
            initial_named_substitutions: input
                .rigid_type_parameters
                .iter()
                .filter(|param| {
                    param.kind == crate::CallableGenericParamKind::Type
                        && !generic_param_names.contains(&param.name)
                })
                .map(|param| (param.name.clone(), param.subject))
                .collect(),
            initial_substitutions,
        },
    );
    if report.failures.is_empty() {
        let Some(Type::Function(flow)) = store.get(input.callee) else {
            return report;
        };
        for (index, (actual, expected)) in input.args.iter().zip(&flow.input).enumerate() {
            let inference = effect_row::infer_bindings_from_types(
                store,
                *expected,
                *actual,
                &report.named_substitutions,
                &report.substitutions,
                &effect_param_names,
                &mut effect_row_bindings,
            );
            if !inference.matches {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span: input.origin.span,
                    message: "callable argument effect row does not match the generic effect-row contract"
                        .to_owned(),
                });
                break;
            }
            for name in inference.deferred {
                if effect_row_bindings.contains_key(&name) {
                    continue;
                }
                let Some(expr) = input.arg_exprs.get(index).copied().flatten() else {
                    report.push(SolverFailure {
                        code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                        span: input.origin.span,
                        message: format!(
                            "generic effect-row parameter `effect {name}` requires a checked latent flow source"
                        ),
                    });
                    break;
                };
                if !deferred_effect_row_obligations.iter().any(
                    |obligation: &crate::DeferredEffectRowObligation| {
                        obligation.param == name && obligation.source == expr
                    },
                ) {
                    deferred_effect_row_obligations.push(crate::DeferredEffectRowObligation {
                        param: name,
                        source: expr,
                    });
                }
            }
        }
        for name in &effect_param_names {
            if effect_row_bindings.contains_key(name)
                || deferred_effect_row_obligations
                    .iter()
                    .any(|obligation| obligation.param == *name)
            {
                continue;
            }
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: input.origin.span,
                message: format!(
                    "generic effect-row parameter `effect {name}` could not be inferred for this call"
                ),
            });
        }
    }
    if report.failures.is_empty() {
        let mut obligations = Vec::new();
        for param in input
            .generic_params
            .iter()
            .filter(|param| param.kind == crate::CallableGenericParamKind::Type)
        {
            if param.bounds.is_empty() {
                continue;
            }
            let solved_subject = resolve_substitution(store, &report.substitutions, param.subject);
            let ty = report
                .named_substitutions
                .get(&param.name)
                .copied()
                .or_else(|| {
                    (solved_subject != param.subject
                        || matches!(store.get(solved_subject), Some(Type::Var(_))))
                    .then_some(solved_subject)
                });
            let Some(ty) = ty else {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    span: input.origin.span,
                    message: format!(
                        "bounded generic parameter `{}` could not be inferred for this call",
                        param.name
                    ),
                });
                continue;
            };
            let param_obligations = param
                .bounds
                .iter()
                .map(|bound| SpecObligation {
                    ty,
                    spec: bound.spec.clone(),
                    args: bound
                        .args
                        .iter()
                        .map(|arg| resolve_known_substitutions(store, &report, *arg))
                        .collect(),
                    span: input.origin.span,
                })
                .collect::<Vec<_>>();
            if matches!(store.get(ty), Some(Type::Var(_))) {
                // Literal/default and surrounding constraints may still bind this variable.
                // The final obligation pass must prove the bound after those constraints solve.
                report.pending_spec_obligations.extend(param_obligations);
            } else {
                obligations.extend(param_obligations);
            }
        }
        if report.failures.is_empty() {
            report.append(spec_solver::solve_spec_obligations(
                store,
                spec_facts,
                &obligations,
                &report.named_substitutions,
            ));
        }
    }
    if report.failures.is_empty()
        && let Some(call) = input.call
    {
        if let Some(Type::Function(flow)) = store.get(input.callee)
            && let Some(row) = &flow.effects
        {
            report.invocation_effects.insert(call, row.clone());
        }
        if let Some(Type::Function(flow)) = store.get(input.callee)
            && matches!(
                store.get(flow.output),
                Some(Type::Primitive(crate::PrimitiveType::Never))
            )
        {
            // Assignability to an expected value type does not turn a
            // non-returning call into an expression that completes normally.
            report.inferred_expr_types.insert(call, flow.output);
        }
        let type_bindings = generic_param_names
            .iter()
            .filter_map(|name| {
                report
                    .named_substitutions
                    .get(name)
                    .copied()
                    .or_else(|| {
                        input
                            .generic_params
                            .iter()
                            .find(|param| param.name == *name)
                            .and_then(|param| {
                                let solved = resolve_substitution(
                                    store,
                                    &report.substitutions,
                                    param.subject,
                                );
                                (solved != param.subject
                                    || matches!(store.get(solved), Some(Type::Var(_))))
                                .then_some(solved)
                            })
                    })
                    .map(|ty| (name.clone(), ty))
            })
            .collect::<Vec<_>>();
        let effect_row_bindings = effect_param_names
            .iter()
            .filter_map(|name| {
                effect_row_bindings
                    .get(name)
                    .cloned()
                    .map(|row| (name.clone(), row))
            })
            .collect::<Vec<_>>();
        deferred_effect_row_obligations.sort_by_key(|obligation| {
            effect_param_names
                .iter()
                .position(|name| name == &obligation.param)
                .unwrap_or(effect_param_names.len())
        });
        if !type_bindings.is_empty()
            || !effect_row_bindings.is_empty()
            || !deferred_effect_row_obligations.is_empty()
        {
            report.generic_instantiations.insert(
                call,
                crate::GenericInstantiationFact {
                    type_bindings,
                    effect_row_bindings,
                    deferred_effect_row_obligations,
                },
            );
        }
    }
    // Generic names belong to this call only. Type-variable substitutions carry
    // the solved result into the surrounding body without cross-call collisions.
    report.named_substitutions.clear();
    report
}

fn solve_unary_constraints<E>(
    store: &TypeStore,
    report: &mut SolverReport,
    constraints: Vec<&TypeConstraint>,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    for constraint in constraints {
        checkpoint()?;
        let TypeConstraint::Unary {
            op,
            operand,
            output,
            origin,
        } = constraint
        else {
            continue;
        };
        let operand = resolve_known_substitutions(store, report, *operand);
        let output = resolve_known_substitutions(store, report, *output);
        let valid = match op {
            etas_hir::HirUnaryOp::Not => {
                matches!(
                    store.get(operand),
                    Some(Type::Primitive(crate::PrimitiveType::Bool))
                ) && matches!(
                    store.get(output),
                    Some(Type::Primitive(crate::PrimitiveType::Bool)) | Some(Type::Var(_))
                )
            }
            etas_hir::HirUnaryOp::Neg => {
                is_numeric(store, operand) && assignability::is_assignable(store, operand, output)
            }
        };
        if !valid {
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                span: origin.span,
                message: match op {
                    etas_hir::HirUnaryOp::Not => "logical negation operand must be bool".to_owned(),
                    etas_hir::HirUnaryOp::Neg => {
                        "unary negation operand must be numeric".to_owned()
                    }
                },
            });
        }
    }
    Ok(())
}

fn solve_numeric_literals<E>(
    store: &TypeStore,
    report: &mut SolverReport,
    literals: Vec<&TypeConstraint>,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    let default_integer = primitive_type_id(store, PrimitiveType::I32);
    let default_float = primitive_type_id(store, PrimitiveType::F64);
    for literal in literals {
        checkpoint()?;
        let TypeConstraint::NumericLiteral {
            expr,
            text,
            kind,
            ty,
            origin,
        } = literal
        else {
            continue;
        };
        let mut resolved = resolve_known_substitutions(store, report, *ty);
        if matches!(store.get(resolved), Some(Type::Var(_))) {
            let default = match kind {
                NumericLiteralKind::Integer => default_integer,
                NumericLiteralKind::Float => default_float,
            };
            let Some(default) = default else {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    span: origin.span,
                    message: "numeric literal default type is missing from the type environment"
                        .to_owned(),
                });
                continue;
            };
            let mut unifier = TypeUnifier::with_known_substitution(store, &report.substitutions);
            if unifier.unify(resolved, default).is_ok() {
                report.substitutions.extend(&unifier.into_substitution());
                resolved = default;
            }
        }
        if let Err(reason) = numeric_literal::validate(store, *kind, text, resolved) {
            let message = if expr.is_some() {
                format!(
                    "numeric literal `{text}` cannot have checked type `{}`: {reason}",
                    display_type(store, resolved),
                )
            } else {
                format!(
                    "literal pattern does not match checked type `{}`: {reason} (literal `{text}`)",
                    display_type(store, resolved),
                )
            };
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                span: origin.span,
                message,
            });
            continue;
        }
        if let Some(expr) = expr {
            report.inferred_expr_types.insert(*expr, resolved);
        }
    }
    Ok(())
}

fn primitive_type_id(store: &TypeStore, primitive: PrimitiveType) -> Option<TypeId> {
    store.iter().find_map(|(id, ty)| {
        matches!(ty, Type::Primitive(found) if *found == primitive).then_some(id)
    })
}

fn solve_or_defer_access_constraint(
    store: &TypeStore,
    known: &crate::KnownStdTypes,
    spec_facts: &SpecFacts,
    report: &mut SolverReport,
    constraint: &TypeConstraint,
    pending: &mut Vec<TypeConstraint>,
) {
    match solve_access_constraint(store, known, spec_facts, report, constraint) {
        AccessConstraintOutcome::Solved(solved) => report.append(*solved),
        AccessConstraintOutcome::Progress(inferred) => {
            report.append(*inferred);
            pending.push(constraint.clone());
        }
        AccessConstraintOutcome::Pending => pending.push(constraint.clone()),
    }
}

fn solve_pending_access_constraints<E>(
    store: &TypeStore,
    known: &crate::KnownStdTypes,
    spec_facts: &SpecFacts,
    report: &mut SolverReport,
    mut pending: Vec<TypeConstraint>,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    solve_ready_access_constraints(store, known, spec_facts, report, &mut pending, checkpoint)?;
    for constraint in pending {
        checkpoint()?;
        report.push(unresolved_access_failure(store, report, &constraint));
    }
    Ok(())
}

fn solve_ready_access_constraints<E>(
    store: &TypeStore,
    known: &crate::KnownStdTypes,
    spec_facts: &SpecFacts,
    report: &mut SolverReport,
    pending: &mut Vec<TypeConstraint>,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<bool, E> {
    let mut progress = false;
    while !pending.is_empty() {
        checkpoint()?;
        let mut next = Vec::new();
        let mut solved_any = false;
        for constraint in std::mem::take(pending) {
            checkpoint()?;
            match solve_access_constraint(store, known, spec_facts, report, &constraint) {
                AccessConstraintOutcome::Solved(solved) => {
                    solved_any = true;
                    report.append(*solved);
                }
                AccessConstraintOutcome::Progress(inferred) => {
                    solved_any = true;
                    report.append(*inferred);
                    next.push(constraint);
                }
                AccessConstraintOutcome::Pending => next.push(constraint),
            }
        }
        *pending = next;
        if !solved_any {
            break;
        }
        progress = true;
    }
    Ok(progress)
}

enum AccessConstraintOutcome {
    Solved(Box<SolverReport>),
    Progress(Box<SolverReport>),
    Pending,
}

fn solve_access_constraint(
    store: &TypeStore,
    known: &crate::KnownStdTypes,
    spec_facts: &SpecFacts,
    report: &SolverReport,
    constraint: &TypeConstraint,
) -> AccessConstraintOutcome {
    match constraint {
        TypeConstraint::FieldAccess {
            base,
            field,
            output,
            origin,
        } => {
            let base = resolve_known_substitutions(store, report, *base);
            if is_unresolved_type_var(store, base) {
                return AccessConstraintOutcome::Pending;
            }
            let output = resolve_known_substitutions(store, report, *output);
            AccessConstraintOutcome::Solved(Box::new(field::solve_field_access(
                store, known, base, field, output, *origin,
            )))
        }
        TypeConstraint::IndexAccess {
            expr,
            base,
            index,
            output,
            index_error,
            origin,
        } => {
            let base = resolve_known_substitutions(store, report, *base);
            let index = resolve_known_substitutions(store, report, *index);
            if is_unresolved_type_var(store, base) {
                return AccessConstraintOutcome::Pending;
            }
            let output = resolve_known_substitutions(store, report, *output);
            let input = index::IndexAccessSolveInput {
                spec_facts,
                expr: *expr,
                base,
                index,
                output,
                index_error: *index_error,
                origin: *origin,
                substitutions: &report.substitutions,
            };
            if is_unresolved_type_var(store, index) {
                let inferred = index::infer_index_access(store, &input);
                if !inferred.failures.is_empty() {
                    return AccessConstraintOutcome::Solved(Box::new(inferred));
                }
                if inferred.substitutions.iter().next().is_some() {
                    return AccessConstraintOutcome::Progress(Box::new(inferred));
                }
                return AccessConstraintOutcome::Pending;
            }
            AccessConstraintOutcome::Solved(Box::new(index::solve_index_access(store, input)))
        }
        TypeConstraint::SliceAccess {
            expr,
            base,
            start,
            end,
            output,
            origin,
        } => {
            let base = resolve_known_substitutions(store, report, *base);
            let start = resolve_known_substitutions(store, report, *start);
            let end = resolve_known_substitutions(store, report, *end);
            if is_unresolved_type_var(store, base)
                || is_unresolved_type_var(store, start)
                || is_unresolved_type_var(store, end)
            {
                return AccessConstraintOutcome::Pending;
            }
            let output = resolve_known_substitutions(store, report, *output);
            AccessConstraintOutcome::Solved(Box::new(index::solve_slice_access(
                store,
                index::SliceAccessSolveInput {
                    spec_facts,
                    expr: *expr,
                    base,
                    start,
                    end,
                    output,
                    origin: *origin,
                    substitutions: &report.substitutions,
                },
            )))
        }
        _ => unreachable!("only access constraints can be solved by the access worklist"),
    }
}

fn is_unresolved_type_var(store: &TypeStore, ty: TypeId) -> bool {
    matches!(store.get(ty), Some(Type::Var(_)))
}

fn unresolved_access_failure(
    store: &TypeStore,
    report: &SolverReport,
    constraint: &TypeConstraint,
) -> SolverFailure {
    match constraint {
        TypeConstraint::FieldAccess {
            base,
            field,
            origin,
            ..
        } => {
            let base = resolve_known_substitutions(store, report, *base);
            SolverFailure {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: origin.span,
                message: format!(
                    "field access `{field}` requires a known record-like base type, but `{}` could not be inferred",
                    display_type(store, base)
                ),
            }
        }
        TypeConstraint::IndexAccess { base, origin, .. } => {
            let base = resolve_known_substitutions(store, report, *base);
            SolverFailure {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: origin.span,
                message: format!(
                    "index access requires a known indexable base type, but `{}` could not be inferred",
                    display_type(store, base)
                ),
            }
        }
        TypeConstraint::SliceAccess { base, origin, .. } => {
            let base = resolve_known_substitutions(store, report, *base);
            SolverFailure {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: origin.span,
                message: format!(
                    "slice access requires a known sliceable base type, but `{}` could not be inferred",
                    display_type(store, base)
                ),
            }
        }
        _ => unreachable!("only access constraints can remain pending"),
    }
}

fn is_numeric(store: &TypeStore, ty: TypeId) -> bool {
    matches!(
        store.get(ty),
        Some(Type::IntegerLiteral { .. })
            | Some(Type::Primitive(
                crate::PrimitiveType::I8
                    | crate::PrimitiveType::I16
                    | crate::PrimitiveType::I32
                    | crate::PrimitiveType::I64
                    | crate::PrimitiveType::I128
                    | crate::PrimitiveType::ISize
                    | crate::PrimitiveType::U8
                    | crate::PrimitiveType::U16
                    | crate::PrimitiveType::U32
                    | crate::PrimitiveType::U64
                    | crate::PrimitiveType::U128
                    | crate::PrimitiveType::USize
                    | crate::PrimitiveType::F32
                    | crate::PrimitiveType::F64
            ))
    )
}

fn is_assignable_for_reason(
    store: &TypeStore,
    from: crate::TypeId,
    to: crate::TypeId,
    reason: &crate::AssignabilityReason,
) -> bool {
    if assignability::is_assignable(store, from, to) {
        return true;
    }
    if record_exact_assignable(store, from, to) {
        return true;
    }
    if matches!(reason, crate::AssignabilityReason::Pattern)
        && let Some(crate::Type::Nominal(nominal)) = store.get(from)
        && let Some(representation) = nominal.representation
    {
        return record_subset_assignable(store, representation, to)
            || assignability::is_assignable(store, representation, to);
    }
    if matches!(reason, crate::AssignabilityReason::Pattern)
        && matches!(store.get(from), Some(crate::Type::Applied { .. }))
        && let Ok(Some(from_record)) = crate::record_fields_with_applied_params(store, from)
        && let Some(to_record) = record_fields(store, to)
    {
        return record_fields_subset_assignable(store, &from_record, &to_record);
    }
    if matches!(reason, crate::AssignabilityReason::Pattern)
        && record_subset_assignable(store, from, to)
    {
        return true;
    }
    if matches!(reason, crate::AssignabilityReason::Constructor)
        && let Some(representation) = nominal_representation(store, to)
    {
        return assignability::is_assignable(store, from, representation);
    }
    false
}

fn nominal_representation(store: &TypeStore, ty: crate::TypeId) -> Option<crate::TypeId> {
    let mut current = ty;
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(current) {
            return None;
        }
        let (representation, substitutions) = crate::nominal_representation_parts(store, current)?;
        current =
            crate::substitute_named_params_in_store(store, representation, &substitutions).ok()?;
        if crate::nominal_representation_parts(store, current).is_none() {
            return Some(current);
        }
    }
}

fn assignability_failure_code(
    reason: &crate::AssignabilityReason,
) -> etas_core::TypeDiagnosticCode {
    match reason {
        crate::AssignabilityReason::Finish => etas_core::TypeDiagnosticCode::FinishTypeMismatch,
        crate::AssignabilityReason::Branch => etas_core::TypeDiagnosticCode::BranchTypeMismatch,
        crate::AssignabilityReason::Annotation | crate::AssignabilityReason::Constructor => {
            etas_core::TypeDiagnosticCode::Mismatch
        }
        _ => etas_core::TypeDiagnosticCode::TypeMismatch,
    }
}

fn assignability_failure_message(reason: &crate::AssignabilityReason) -> String {
    match reason {
        crate::AssignabilityReason::Finish => {
            "finish value does not match handler result type".to_owned()
        }
        crate::AssignabilityReason::HandlerResume => {
            "resume payload does not match handled action return type".to_owned()
        }
        crate::AssignabilityReason::HandlerPattern => {
            "handler action pattern does not match action parameter type".to_owned()
        }
        crate::AssignabilityReason::Pattern => {
            "literal pattern does not match the matched value type".to_owned()
        }
        crate::AssignabilityReason::Branch => {
            "match or if branch type does not match the expected branch type".to_owned()
        }
        _ => "type does not match the expected type".to_owned(),
    }
}

fn method_call_failure_message(method: &str, store: &TypeStore, args: &[TypeId]) -> String {
    let method_name = match method {
        "system" | "user" | "assistant" | "data" => format!("Prompt.{method}"),
        "cast" => "Message.cast".to_owned(),
        _ => method.to_owned(),
    };
    if args.iter().any(|arg| contains_secret_wrapper(store, *arg)) {
        return format!(
            "{method_name} does not accept Secret[T] without explicit declassification"
        );
    }
    format!("{method_name} receiver or arguments do not match any standard method signature")
}

fn is_checked_message_cast_candidate(store: &TypeStore, candidate: TypeId) -> bool {
    let Some(Type::Function(flow)) = store.get(candidate) else {
        return false;
    };
    if flow.input.len() != 1 {
        return false;
    }
    matches!(store.get(flow.input[0]), Some(Type::Message(_)))
        && matches!(
            store.get(flow.output),
            Some(Type::Option(inner)) if matches!(store.get(*inner), Some(Type::Message(_)))
        )
}

fn solve_checked_message_cast(
    store: &TypeStore,
    generic_args: &[crate::CallableGenericArg],
    args: &[TypeId],
    output: TypeId,
    origin: ConstraintOrigin,
) -> SolverReport {
    let mut report = SolverReport::default();
    if generic_args.len() != 1 {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::ArityMismatch,
            span: origin.span,
            message: format!(
                "Message.cast expects 1 type argument, got {}",
                generic_args.len()
            ),
        });
        return report;
    }
    if args.len() != 1 {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::ArityMismatch,
            span: origin.span,
            message: format!(
                "Message.cast expects 0 argument(s), got {}",
                args.len().saturating_sub(1)
            ),
        });
        return report;
    }
    if message_inner_type(store, args[0]).is_none() {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message:
                "Message.cast receiver or arguments do not match any standard method signature"
                    .to_owned(),
        });
        return report;
    }
    let crate::CallableGenericArg::Type(payload) = generic_args[0] else {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: "Message.cast requires a type generic argument".to_owned(),
        });
        return report;
    };
    if !is_option_message_of(store, output, payload) {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: "Message.cast result type does not match requested payload type".to_owned(),
        });
    }
    report
}

fn message_inner_type(store: &TypeStore, ty: TypeId) -> Option<TypeId> {
    match store.get(ty) {
        Some(Type::Message(inner)) => Some(*inner),
        Some(Type::Applied { constructor, args }) if args.len() == 1 => matches!(
            store.get(TypeId(constructor.0)),
            Some(Type::Nominal(nominal)) if nominal.name == "std.agent.message.Message"
        )
        .then_some(args[0]),
        _ => None,
    }
}

fn is_option_message_of(store: &TypeStore, ty: TypeId, payload: TypeId) -> bool {
    let Some(Type::Option(inner)) = store.get(ty) else {
        return false;
    };
    message_inner_type(store, *inner).is_some_and(|actual| actual == payload)
}

fn contains_secret_wrapper(store: &TypeStore, ty: TypeId) -> bool {
    match store.get(ty) {
        Some(Type::Trust {
            wrapper: crate::TrustWrapper::Secret,
            ..
        }) => true,
        Some(Type::Array(inner))
        | Some(Type::List(inner))
        | Some(Type::Set(inner))
        | Some(Type::Slice(inner))
        | Some(Type::Option(inner))
        | Some(Type::Message(inner))
        | Some(Type::Schema(inner))
        | Some(Type::MemorySelection(inner))
        | Some(Type::MemoryRegion(inner))
        | Some(Type::Range { index: inner })
        | Some(Type::Trust { inner, .. }) => contains_secret_wrapper(store, *inner),
        Some(Type::Map { key, value }) | Some(Type::Store { key, value }) => {
            contains_secret_wrapper(store, *key) || contains_secret_wrapper(store, *value)
        }
        Some(Type::Result { ok, err }) => {
            contains_secret_wrapper(store, *ok) || contains_secret_wrapper(store, *err)
        }
        Some(Type::Tuple(elements)) => elements
            .iter()
            .any(|element| contains_secret_wrapper(store, *element)),
        Some(Type::Function(flow)) => {
            flow.input
                .iter()
                .any(|input| contains_secret_wrapper(store, *input))
                || contains_secret_wrapper(store, flow.output)
        }
        _ => false,
    }
}

fn resolve_substitution(store: &TypeStore, substitutions: &Substitution, ty: TypeId) -> TypeId {
    let mut current = ty;
    let mut seen = 0usize;
    while let Some(Type::Var(var)) = store.get(current) {
        let Some(next) = substitutions.get(*var) else {
            break;
        };
        if next == current || seen > 64 {
            break;
        }
        current = next;
        seen += 1;
    }
    current
}

fn resolve_known_substitutions(store: &TypeStore, report: &SolverReport, ty: TypeId) -> TypeId {
    let mut current = resolve_substitution(store, &report.substitutions, ty);
    let mut seen = 0usize;
    while let Some(Type::Named(name)) = store.get(current) {
        let Some(next) = report.named_substitutions.get(&name.name).copied() else {
            break;
        };
        if next == current || seen > 64 {
            break;
        }
        current = resolve_substitution(store, &report.substitutions, next);
        seen += 1;
    }
    current
}

fn record_exact_assignable(store: &TypeStore, from: crate::TypeId, to: crate::TypeId) -> bool {
    let Some(from_record) = record_fields(store, from) else {
        return false;
    };
    let Some(to_record) = record_fields(store, to) else {
        return false;
    };
    from_record.fields.len() == to_record.fields.len() && record_subset_assignable(store, from, to)
}

fn record_subset_assignable(store: &TypeStore, from: crate::TypeId, to: crate::TypeId) -> bool {
    let Some(from_record) = record_fields(store, from) else {
        return false;
    };
    let Some(to_record) = record_fields(store, to) else {
        return false;
    };
    record_fields_subset_assignable(store, &from_record, &to_record)
}

fn record_fields_subset_assignable(
    store: &TypeStore,
    from_record: &crate::RecordType,
    to_record: &crate::RecordType,
) -> bool {
    to_record.fields.iter().all(|expected| {
        from_record
            .fields
            .iter()
            .find(|candidate| candidate.name == expected.name)
            .is_some_and(|actual| {
                is_assignable_for_reason(
                    store,
                    actual.ty,
                    expected.ty,
                    &crate::AssignabilityReason::Pattern,
                )
            })
    })
}

fn record_fields(store: &TypeStore, ty: crate::TypeId) -> Option<crate::RecordType> {
    match store.get(ty) {
        Some(crate::Type::Record(record)) => Some(record.clone()),
        _ => None,
    }
}
