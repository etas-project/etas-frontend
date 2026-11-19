use etas_hir::{HirExpr, HirExprId, HirFlowBody, HirItem, HirProgram};

use super::MethodTarget;
use crate::facts::CheckedMethodCallFact;

/// Only a source declaration admitted by the compiler role binder has an ABI.
pub fn execution_role(
    hir: &HirProgram,
    facts: &crate::TypeFacts,
    target: &MethodTarget,
) -> Option<etas_std::StdExecutionRole> {
    match target {
        MethodTarget::Source { callable, .. } => match hir.items.get(*callable)? {
            HirItem::Flow(etas_hir::HirFlowDecl {
                body: HirFlowBody::Runtime { role, .. },
                ..
            }) => Some(*role),
            _ => None,
        },
        MethodTarget::External { identity } => {
            facts.external_execution_roles.get(identity).copied()
        }
        _ => None,
    }
}

/// The source argument occupying a checked ABI slot, after named-argument binding.
pub fn argument_expression(
    hir: &HirProgram,
    call: HirExprId,
    fact: &CheckedMethodCallFact,
    slot: u32,
) -> Option<HirExprId> {
    let args = match hir.exprs.get(call)? {
        HirExpr::MethodCall { receiver, args, .. } => {
            if fact.receiver.value_slot() == Some(slot) {
                return Some(*receiver);
            }
            args
        }
        HirExpr::Call { args, .. } | HirExpr::SpecMethodCall { args, .. } => args,
        _ => return None,
    };
    let index = fact
        .argument_slots
        .iter()
        .position(|bound| *bound == slot)?;
    match args.get(index)? {
        etas_hir::HirArg::Positional(expr) | etas_hir::HirArg::Named { value: expr, .. } => {
            Some(*expr)
        }
    }
}
