use super::{ReflectionSolveInput, SolverFailure};
use crate::{
    CheckedReflectionFact, CheckedReflectionTarget, TypeId,
    facts::reflection::ExternalAgentReflectionFact,
};

pub(super) fn spec_method(
    input: &ReflectionSolveInput<'_>,
    reflection: &ExternalAgentReflectionFact,
    spec: etas_hir::SymbolId,
    args: &[TypeId],
    name: &str,
) -> Result<CheckedReflectionFact, SolverFailure> {
    let failure = |code, message: &str| SolverFailure {
        code,
        span: input.origin.span,
        message: message.into(),
    };
    let missing =
        |message: &str| failure(etas_core::TypeDiagnosticCode::IncompleteTypeFacts, message);
    let invalid = |message: &str| failure(etas_core::TypeDiagnosticCode::TypeMismatch, message);
    let ExternalAgentReflectionFact::Available { header, graph } = reflection else {
        return Err(missing(
            "external spec method reflection requires published method selection facts",
        ));
    };
    let methods = header
        .method_info
        .as_ref()
        .ok_or_else(|| missing("external agent description has no method selection facts"))?;
    let identity = input
        .facts
        .external_definition_bindings
        .get(&spec)
        .ok_or_else(|| invalid("selected spec has no matching canonical external declaration"))?;
    let mut candidates = Vec::new();
    for method in methods.iter().filter(|method| {
        method
            .callable
            .as_method()
            .is_some_and(|method| method.name() == name)
    }) {
        for bound in method
            .specs
            .iter()
            .filter(|bound| &bound.definition == identity)
        {
            if !args.is_empty() {
                if args.len() != bound.arguments.len() {
                    continue;
                }
                let mut matched = true;
                for (actual, expected) in args.iter().zip(&bound.arguments) {
                    if !super::descriptor_types::matches(input, graph, *actual, *expected)
                        .map_err(missing)?
                    {
                        matched = false;
                        break;
                    }
                }
                if !matched {
                    continue;
                }
            }
            candidates.push(method);
        }
    }
    let [method] = candidates.as_slice() else {
        return Err(invalid(if candidates.is_empty() {
            "agent has no checked method satisfying the selected spec and type arguments"
        } else {
            "spec reflection is ambiguous between multiple checked implementations"
        }));
    };
    if !method.is_public {
        return Err(invalid(
            "external agent method is private to its defining module",
        ));
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
        target: CheckedReflectionTarget::ExternalCallable {
            callable: method.callable.clone(),
            selected_spec: Some(identity.clone()),
        },
        descriptor_type,
        selected_spec: None,
        access_module: input.access_module,
    })
}

pub(super) fn member(
    input: &ReflectionSolveInput<'_>,
    reflection: &ExternalAgentReflectionFact,
    name: &str,
    descriptor_type: TypeId,
) -> Result<CheckedReflectionFact, SolverFailure> {
    let failure = |code, message: &str| SolverFailure {
        code,
        span: input.origin.span,
        message: message.into(),
    };
    let ExternalAgentReflectionFact::Available { header, .. } = reflection else {
        return Err(failure(
            etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
            "external agent method reflection requires published method selection facts",
        ));
    };
    let methods = super::select_external_methods(header)
        .map_err(|message| failure(etas_core::TypeDiagnosticCode::IncompleteTypeFacts, message))?
        .ok_or_else(|| {
            failure(
                etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                "external agent description has no method selection facts",
            )
        })?;
    let callable = match methods.get(name) {
        Some(super::ExternalMethodSelection::Selected(callable)) => callable,
        Some(super::ExternalMethodSelection::Private) => {
            return Err(failure(
                etas_core::TypeDiagnosticCode::TypeMismatch,
                "external agent method is private to its defining module",
            ));
        }
        _ if name == "run" => {
            return Err(failure(
                etas_core::TypeDiagnosticCode::TypeMismatch,
                "agent has no default run method to reflect",
            ));
        }
        _ => {
            return Err(failure(
                etas_core::TypeDiagnosticCode::TypeMismatch,
                "external agent has no uniquely selected public method to reflect",
            ));
        }
    };
    Ok(CheckedReflectionFact {
        target: CheckedReflectionTarget::ExternalCallable {
            callable: callable.clone(),
            selected_spec: None,
        },
        descriptor_type,
        selected_spec: None,
        access_module: input.access_module,
    })
}
