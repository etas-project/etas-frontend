use crate::{AgentEntrySignature, ConstraintOrigin, Type, TypeFacts, TypeId, TypeStore};

use super::SolverFailure;

pub(super) fn resolve_method<'a>(
    store: &TypeStore,
    facts: &'a TypeFacts,
    receiver: TypeId,
    name: &str,
    access_module: Option<etas_hir::HirModuleId>,
    origin: ConstraintOrigin,
) -> Result<Option<&'a crate::facts::signatures::AgentMethodSignature>, SolverFailure> {
    let Some(agent) = facts.agent_signature(receiver) else {
        return Ok(None);
    };
    let mut methods: Vec<_> = facts
        .agent_methods
        .values()
        .filter(|method| method.component == agent.component && method.name == name)
        .collect();
    if methods.iter().any(|method| method.inherent) {
        methods.retain(|method| method.inherent);
    }
    let failure = |message: &str| SolverFailure {
        code: etas_core::TypeDiagnosticCode::TypeMismatch,
        span: origin.span,
        message: message.into(),
    };
    let [method] = methods.as_slice() else {
        return Err(failure(if methods.is_empty() {
            "agent has no checked method with this name"
        } else {
            "agent method is ambiguous; select the defining spec explicitly"
        }));
    };
    if method.private_to.is_some() && method.private_to != access_module {
        return Err(failure("agent method is private to its defining module"));
    }
    if !matches!(store.get(method.callable_type), Some(Type::Function(flow)) if flow.input == method.callable.params && flow.output == method.callable.output && flow.effects == method.callable.effects)
    {
        return Err(SolverFailure {
            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
            span: origin.span,
            message: "agent method disagrees with its checked callable type".into(),
        });
    }
    Ok(Some(*method))
}

/// Callability is a checked declaration property, not nominal representation
/// projection or assignability to an ordinary function value.
pub(super) fn resolve<'a>(
    store: &TypeStore,
    facts: &'a TypeFacts,
    callee: TypeId,
    origin: ConstraintOrigin,
) -> Result<Option<&'a AgentEntrySignature>, SolverFailure> {
    let Some(agent) = facts.agent_signature(callee) else {
        return Ok(None);
    };
    let Some(entry) = agent.default_entry.as_deref() else {
        return Err(SolverFailure {
            code: etas_core::TypeDiagnosticCode::NonCallableCallee,
            span: origin.span,
            message: "agent component has no default callable entry".into(),
        });
    };
    if !matches!(store.get(entry.callable_type), Some(Type::Function(flow)) if flow.input == entry.callable.params && flow.output == entry.callable.output && flow.effects == entry.callable.effects)
    {
        return Err(SolverFailure {
            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
            span: origin.span,
            message: "agent default entry disagrees with its checked callable type".into(),
        });
    }
    Ok(Some(entry))
}
