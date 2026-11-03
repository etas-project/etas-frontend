use crate::{AgentEntrySignature, ConstraintOrigin, Type, TypeFacts, TypeId, TypeStore};

use super::SolverFailure;

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
