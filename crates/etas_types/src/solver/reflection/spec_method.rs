use etas_hir::SymbolId;

use crate::{
    CheckedReflectionFact, CheckedReflectionTarget, CheckedSpecBound, CheckedSpecRef,
    SymbolTypeFact, TypeId, constraint::reflection::ReflectionBinding,
};

use super::{ReflectionSolveInput, SolverFailure, resolve_known_substitutions};

pub(super) fn solve(
    input: &ReflectionSolveInput<'_>,
    receiver: &ReflectionBinding,
    spec: SymbolId,
    args: &[TypeId],
    name: &str,
) -> Result<CheckedReflectionFact, SolverFailure> {
    let invalid =
        |message: &str| failure(input, etas_core::TypeDiagnosticCode::TypeMismatch, message);
    let missing = |message: &str| {
        failure(
            input,
            etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
            message,
        )
    };
    let (component, binding) = match receiver {
        ReflectionBinding::AgentDeclaration(symbol) => {
            let Some(SymbolTypeFact::Agent { signature }) = input.facts.symbol_types.get(symbol)
            else {
                return Err(missing(
                    "spec reflection receiver has no checked agent signature",
                ));
            };
            (signature.component, None)
        }
        ReflectionBinding::Value { binding, ty } => {
            let component = resolve_known_substitutions(input.store, input.solved, *ty);
            if input.facts.agent_signature(component).is_none() {
                return Err(invalid(
                    "spec reflection receiver must be a nominal agent value",
                ));
            }
            (component, Some(*binding))
        }
        ReflectionBinding::Invalid { reason } => return Err(invalid(reason)),
        _ => {
            return Err(invalid(
                "spec reflection receiver must name an agent or agent binding",
            ));
        }
    };
    let Some(signature) = input.facts.spec_signatures.get(&spec) else {
        return Err(invalid(
            "spec reflection must name a checked spec declaration",
        ));
    };
    match signature.kind {
        crate::SpecKind::TypeSpec
            if !signature.methods.iter().any(|method| method.name == name) =>
        {
            return Err(invalid("selected type spec does not declare this method"));
        }
        crate::SpecKind::CallableSpec | crate::SpecKind::TraceSpec if name != "run" => {
            return Err(invalid(
                "selected callable or trace spec has no such invocation method",
            ));
        }
        _ => {}
    }
    if let ReflectionBinding::AgentDeclaration(symbol) = receiver
        && let Some(reflection) = input.facts.external_agent_reflections.get(symbol)
    {
        return super::external::spec_method(input, reflection, spec, args, name);
    }
    let method_specs = crate::facts::method_specs::AgentMethodSpecIndex::new(input.facts);
    let candidates = input
        .facts
        .agent_methods
        .values()
        .filter(|method| method.component == component && method.name == name)
        .flat_map(|method| {
            method_specs
                .bindings(method.symbol)
                .iter()
                .filter(move |binding| binding.spec == spec && binding.kind == signature.kind)
                .map(|binding| binding.args)
                .filter(|bound| {
                    args.is_empty()
                        || (bound.len() == args.len()
                            && bound.iter().zip(args).all(|(actual, expected)| {
                                resolve_known_substitutions(input.store, input.solved, *actual)
                                    == resolve_known_substitutions(
                                        input.store,
                                        input.solved,
                                        *expected,
                                    )
                            }))
                })
                .map(move |bound| (method, bound))
        })
        .collect::<Vec<_>>();
    let [(method, bound)] = candidates.as_slice() else {
        return Err(invalid(if candidates.is_empty() {
            "agent has no checked method satisfying the selected spec and type arguments"
        } else {
            "spec reflection is ambiguous between multiple checked implementations"
        }));
    };
    if method.private_to.is_some() && method.private_to != input.access_module {
        return Err(invalid("agent method is private to its defining module"));
    }
    let descriptor_type = input
        .facts
        .known_std_types
        .code
        .callable_definition
        .ok_or_else(|| {
            missing("spec reflection is missing the checked CallableDef standard type")
        })?;
    Ok(CheckedReflectionFact {
        target: match binding {
            Some(binding) => CheckedReflectionTarget::AgentValue {
                binding,
                component,
                method: Some(method.symbol),
            },
            None => CheckedReflectionTarget::Callable {
                declaration: method.symbol,
            },
        },
        descriptor_type,
        access_module: input.access_module,
        selected_spec: Some(CheckedSpecBound {
            spec: CheckedSpecRef::Source(spec),
            args: bound
                .iter()
                .map(|ty| resolve_known_substitutions(input.store, input.solved, *ty))
                .collect(),
        }),
    })
}

fn failure(
    input: &ReflectionSolveInput<'_>,
    code: etas_core::TypeDiagnosticCode,
    message: &str,
) -> SolverFailure {
    SolverFailure {
        code,
        span: input.origin.span,
        message: message.into(),
    }
}
