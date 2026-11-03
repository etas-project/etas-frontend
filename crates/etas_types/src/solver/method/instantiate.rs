use super::*;
use crate::method::{MethodEvidence, MethodSelection, MethodSignatureFact};

pub(super) struct Probe {
    pub report: SolverReport,
    pub slots: Vec<u32>,
}

pub(super) fn probe(
    input: &TypeSolveInput<'_>,
    call: Input<'_>,
    candidate: &crate::CallableCandidate,
    entry: &MethodSignatureFact,
    args: &[TypeId],
    output: TypeId,
    known: &Substitution,
) -> Probe {
    let mut result = Probe {
        report: SolverReport::default(),
        slots: Vec::new(),
    };
    super::operation::validate(input.store, &entry.target, call, args, &mut result.report);
    if !result.report.is_solved() {
        return result;
    }
    let crate::CallableCandidateOperation::Method {
        owner_params,
        evidence,
        ..
    } = &candidate.operation
    else {
        unreachable!()
    };
    let mut names = entry.parameter_names.clone();
    if entry.receiver == crate::method::MethodReceiver::Associated {
        names.insert(0, None);
    }
    let slots = match argument_slots(&names, call.argument_names) {
        Ok(slots) => slots,
        Err(message) => {
            result.report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::ArityMismatch,
                span: call.origin.span,
                message,
            });
            return result;
        }
    };
    let mut ordered_types = vec![args[0]; slots.len()];
    let mut ordered_exprs = vec![None; slots.len()];
    for (source, slot) in slots.iter().enumerate() {
        ordered_types[*slot as usize] = args[source];
        ordered_exprs[*slot as usize] = call.arg_exprs[source];
    }
    let mut substitutions = known.clone();
    if let MethodSelection::ExplicitSpec {
        args: selected_args,
        ..
    } = call.selection
    {
        let args = match evidence {
            MethodEvidence::Implementation { args, .. } | MethodEvidence::Bound { args, .. } => {
                args
            }
            MethodEvidence::Inherent => unreachable!(),
        };
        if selected_args.len() != args.len() {
            result.report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::ArityMismatch,
                span: call.origin.span,
                message: "selected spec type argument arity differs".into(),
            });
            return result;
        }
        let mut unifier = TypeUnifier::with_substitution(input.store, substitutions);
        for (expected, actual) in args.iter().zip(selected_args) {
            if unifier.unify(*expected, *actual).is_err() {
                result.report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span: call.origin.span,
                    message: "selected spec type arguments do not match the implementation".into(),
                });
                return result;
            }
        }
        substitutions = unifier.into_substitution();
    }
    result.report = solve_callable_constraint(
        input.store,
        input.spec_facts,
        CallableConstraintSolveInput {
            rigid_type_parameters: input.rigid_type_parameters,
            call: Some(*call.expr),
            callee: candidate.ty,
            generic_params: &candidate.signature.generic_params,
            explicit_generic_args: call.generic_args,
            arg_exprs: &ordered_exprs,
            args: &ordered_types,
            output,
            origin: *call.origin,
            known_substitutions: substitutions,
        },
    );
    if result.report.failures.is_empty() {
        let mut obligations = Vec::new();
        for parameter in owner_params {
            let subject =
                resolve_known_substitutions(input.store, &result.report, parameter.subject);
            for bound in &parameter.bounds {
                obligations.push(SpecObligation {
                    ty: subject,
                    spec: bound.spec.clone(),
                    args: bound
                        .args
                        .iter()
                        .map(|arg| resolve_known_substitutions(input.store, &result.report, *arg))
                        .collect(),
                    span: call.origin.span,
                });
            }
        }
        if let MethodEvidence::Bound {
            spec,
            subject,
            args,
        } = evidence
        {
            obligations.push(SpecObligation {
                ty: resolve_known_substitutions(input.store, &result.report, *subject),
                spec: spec.clone(),
                args: args
                    .iter()
                    .map(|arg| resolve_known_substitutions(input.store, &result.report, *arg))
                    .collect(),
                span: call.origin.span,
            });
        }
        result.report.append(spec_solver::solve_spec_obligations(
            input.store,
            input.spec_facts,
            &obligations,
            &HashMap::new(),
        ));
    }
    result.slots = slots;
    result
}

fn argument_slots(
    parameters: &[Option<String>],
    arguments: &[Option<String>],
) -> Result<Vec<u32>, String> {
    if parameters.len() != arguments.len() {
        return Err(format!(
            "method expects {} explicit argument(s), got {}",
            parameters.len().saturating_sub(1),
            arguments.len().saturating_sub(1)
        ));
    }
    let mut used = vec![false; parameters.len()];
    let mut next = 0;
    let mut slots = Vec::with_capacity(arguments.len());
    for name in arguments {
        let slot = if let Some(name) = name {
            parameters
                .iter()
                .position(|parameter| parameter.as_ref() == Some(name))
                .ok_or_else(|| format!("unknown method argument `{name}`"))?
        } else {
            while next < used.len() && used[next] {
                next += 1;
            }
            next
        };
        if slot >= used.len() || used[slot] {
            return Err("method argument is supplied more than once".into());
        }
        used[slot] = true;
        slots.push(slot as u32);
    }
    Ok(slots)
}
