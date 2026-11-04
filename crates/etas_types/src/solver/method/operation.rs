use etas_std::FlowSourceMethodOperation;

use super::*;
use crate::method::MethodTarget;

pub(super) fn validate(
    store: &TypeStore,
    target: &MethodTarget,
    call: Input<'_>,
    args: &[TypeId],
    report: &mut SolverReport,
) {
    let MethodTarget::Standard {
        operation,
        qualified_path,
        ..
    } = target
    else {
        return;
    };
    let message = match operation {
        FlowSourceMethodOperation::CheckedMessageCast
            if !matches!(call.generic_args, [crate::CallableGenericArg::Type(_)]) =>
        {
            Some(format!(
                "{} expects one payload type argument",
                qualified_path.join(".")
            ))
        }
        FlowSourceMethodOperation::RejectSecretData
            if args.iter().skip(1).any(|arg| contains_secret(store, *arg)) =>
        {
            Some(format!(
                "{} does not accept Secret[T] without explicit declassification",
                qualified_path.join(".")
            ))
        }
        _ => None,
    };
    if let Some(message) = message {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: call.origin.span,
            message,
        });
    }
}

fn contains_secret(store: &TypeStore, root: TypeId) -> bool {
    let mut pending = vec![root];
    let mut visited = HashSet::new();
    while let Some(ty) = pending.pop() {
        if !visited.insert(ty) {
            continue;
        }
        match store.get(ty) {
            Some(Type::Trust {
                wrapper: crate::TrustWrapper::Secret,
                ..
            }) => return true,
            Some(
                Type::Array(inner)
                | Type::List(inner)
                | Type::Set(inner)
                | Type::Slice(inner)
                | Type::Option(inner)
                | Type::Message(inner)
                | Type::Schema(inner)
                | Type::MemorySelection(inner)
                | Type::MemoryRegion(inner)
                | Type::Range { index: inner }
                | Type::Trust { inner, .. }
                | Type::Refined { base: inner, .. },
            ) => pending.push(*inner),
            Some(
                Type::Map { key, value }
                | Type::Store { key, value }
                | Type::Result {
                    ok: key,
                    err: value,
                },
            ) => pending.extend([*key, *value]),
            Some(Type::Tuple(elements) | Type::Applied { args: elements, .. }) => {
                pending.extend(elements)
            }
            Some(Type::Record(record)) => {
                pending.extend(record.fields.iter().map(|field| field.ty))
            }
            Some(Type::Function(flow)) => {
                pending.extend(&flow.input);
                pending.push(flow.output);
            }
            _ => {}
        }
    }
    false
}
