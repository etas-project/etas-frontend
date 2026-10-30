use crate::{
    CheckedReflectionFact, CheckedReflectionTarget, ConstraintOrigin, SymbolTypeFact, TypeFacts,
    TypeStore, constraint::reflection::ReflectionBinding,
};

use super::{SolverFailure, SolverReport, resolve_known_substitutions};

mod descriptor_types;
mod external_descriptor;
pub use descriptor_types::{
    checked_descriptor_type_matches, checked_descriptor_type_matches_with_bindings,
};
pub use external_descriptor::{ExternalDescriptorIdentities, ExternalDescriptorMatcher};
mod external;
mod spec_method;

pub(super) struct ReflectionSolveInput<'a> {
    pub store: &'a TypeStore,
    pub facts: &'a TypeFacts,
    pub binding: &'a ReflectionBinding,
    pub callable_descriptor: Option<crate::TypeId>,
    pub members: &'a [String],
    pub origin: ConstraintOrigin,
    pub solved: &'a SolverReport,
    pub access_module: Option<etas_hir::HirModuleId>,
}

pub(super) fn solve(
    input: ReflectionSolveInput<'_>,
) -> Result<CheckedReflectionFact, SolverFailure> {
    let invalid = |message: &str| SolverFailure {
        code: etas_core::TypeDiagnosticCode::TypeMismatch,
        span: input.origin.span,
        message: message.into(),
    };
    let missing = |message: &str| SolverFailure {
        code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
        span: input.origin.span,
        message: message.into(),
    };
    let member = match input.members {
        [] => None,
        [method] => Some(method.as_str()),
        _ => {
            return Err(invalid(
                "reflection member is missing a checked agent method identity",
            ));
        }
    };
    let callable_type = || {
        input
            .facts
            .known_std_types
            .code
            .callable_definition
            .ok_or_else(|| missing("reflection is missing the checked CallableDef standard type"))
    };
    let (target, descriptor_type) = match input.binding {
        ReflectionBinding::SpecMethod {
            receiver,
            spec,
            args,
            method,
        } => return spec_method::solve(&input, receiver, *spec, args, method),
        ReflectionBinding::Invalid { reason } => return Err(invalid(reason)),
        ReflectionBinding::AgentDeclaration(symbol) => {
            let Some(SymbolTypeFact::Agent { signature }) = input.facts.symbol_types.get(symbol)
            else {
                return Err(missing(
                    "reflection target is missing its checked agent component facts",
                ));
            };
            if let Some(member) = member {
                if let Some(reflection) = input.facts.external_agent_reflections.get(symbol) {
                    return external::member(&input, reflection, member, callable_type()?);
                }
                let method = selected_method(input.facts, signature, member).ok_or_else(|| {
                    invalid("agent component has no uniquely selected method to reflect")
                })?;
                if method.private_to.is_some() && method.private_to != input.access_module {
                    return Err(invalid("agent method is private to its defining module"));
                }
                (
                    method_target(input.facts, method, input.origin.span)?,
                    callable_type()?,
                )
            } else {
                let ty = signature.definition_type.ok_or_else(|| {
                    missing("reflection is missing the checked AgentDef standard type")
                })?;
                (
                    CheckedReflectionTarget::Agent {
                        declaration: *symbol,
                        component: signature.component,
                    },
                    ty,
                )
            }
        }
        ReflectionBinding::AgentMethod(symbol) => {
            if member.is_some() {
                return Err(invalid(
                    "agent method reflection cannot select another member",
                ));
            }
            let method = input.facts.agent_methods.get(symbol).ok_or_else(|| {
                missing("reflected agent method is missing checked identity facts")
            })?;
            if method.private_to.is_some() && method.private_to != input.access_module {
                return Err(invalid("agent method is private to its defining module"));
            }
            (
                method_target(input.facts, method, input.origin.span)?,
                callable_type()?,
            )
        }
        ReflectionBinding::CallableDeclaration(symbol) => {
            if member.is_some() {
                return Err(invalid(
                    "a flow declaration has no agent default-entry member",
                ));
            }
            if !matches!(
                input.facts.symbol_types.get(symbol),
                Some(SymbolTypeFact::Flow { .. })
            ) {
                return Err(missing(
                    "reflected flow is missing its checked declaration signature",
                ));
            }
            (
                CheckedReflectionTarget::Callable {
                    declaration: *symbol,
                },
                input.callable_descriptor.ok_or_else(|| {
                    missing("flow reflection is missing its typed CallableDef contract")
                })?,
            )
        }
        ReflectionBinding::Value { binding, ty } => {
            let component = resolve_known_substitutions(input.store, input.solved, *ty);
            if matches!(input.store.get(component), Some(crate::Type::Function(_))) {
                if member.is_some() {
                    return Err(invalid(
                        "an ordinary flow value has no reflected method members",
                    ));
                }
                return Ok(CheckedReflectionFact {
                    target: CheckedReflectionTarget::CallableValue {
                        binding: *binding,
                        signature: component,
                    },
                    descriptor_type: input.callable_descriptor.ok_or_else(|| {
                        missing("flow value reflection is missing its typed CallableDef contract")
                    })?,
                    selected_spec: None,
                    access_module: input.access_module,
                });
            }
            let agent = input.facts.agent_signature(component).ok_or_else(|| {
                invalid("only checked ordinary flow or nominal agent values can be reflected")
            })?;
            let method = member
                .map(|member| {
                    selected_method(input.facts, agent, member).ok_or_else(|| {
                        invalid("agent value has no uniquely selected method to reflect")
                    })
                })
                .transpose()?;
            if method.is_some_and(|method| {
                method.private_to.is_some() && method.private_to != input.access_module
            }) {
                return Err(invalid("agent method is private to its defining module"));
            }
            let descriptor_type = if method.is_some() {
                callable_type()?
            } else {
                agent.definition_type.ok_or_else(|| {
                    missing("agent value is missing its checked AgentDef standard type")
                })?
            };
            (
                CheckedReflectionTarget::AgentValue {
                    binding: *binding,
                    component,
                    method: method.map(|method| method.symbol),
                },
                descriptor_type,
            )
        }
    };
    Ok(CheckedReflectionFact {
        target,
        descriptor_type,
        selected_spec: None,
        access_module: input.access_module,
    })
}

fn selected_method<'a>(
    facts: &'a TypeFacts,
    agent: &crate::AgentSignature,
    name: &str,
) -> Option<&'a crate::facts::signatures::AgentMethodSignature> {
    if name == "run" {
        return facts
            .agent_methods
            .get(&agent.default_entry.as_ref()?.method?);
    }
    let candidates = facts
        .agent_methods
        .values()
        .filter(|method| method.component == agent.component && method.name == name)
        .collect::<Vec<_>>();
    let inherent = candidates
        .iter()
        .copied()
        .filter(|method| method.inherent)
        .collect::<Vec<_>>();
    let candidates = if inherent.is_empty() {
        candidates
    } else {
        inherent
    };
    match candidates.as_slice() {
        [method] => Some(*method),
        _ => None,
    }
}

fn method_target(
    facts: &TypeFacts,
    method: &crate::facts::signatures::AgentMethodSignature,
    span: etas_core::Span,
) -> Result<CheckedReflectionTarget, SolverFailure> {
    let Some(agent) = facts.agent_signature(method.component) else {
        return Err(SolverFailure {
            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
            span,
            message: "reflected method has no checked agent component".into(),
        });
    };
    if agent.default_entry.as_ref().and_then(|entry| entry.method) == Some(method.symbol) {
        let Some(declaration) = facts.agent_components.get(&method.component) else {
            return Err(SolverFailure {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                message: "reflected default entry has no component declaration".into(),
            });
        };
        Ok(CheckedReflectionTarget::AgentDefaultEntry {
            declaration: *declaration,
            component: method.component,
        })
    } else {
        Ok(CheckedReflectionTarget::Callable {
            declaration: method.symbol,
        })
    }
}
