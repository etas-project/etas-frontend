use std::collections::HashMap;

use crate::{
    ConstraintOrigin, Type, TypeId, TypeStore,
    solver::{
        TypeUnifier,
        assignability::is_assignable,
        report::{SolverFailure, SolverReport},
    },
    ty::display_type,
};

pub fn solve_callable(
    store: &TypeStore,
    callee: TypeId,
    args: &[TypeId],
    output: TypeId,
    origin: ConstraintOrigin,
) -> SolverReport {
    solve_callable_with_named_substitutions(
        store,
        callee,
        &[],
        &[],
        args,
        output,
        origin,
        HashMap::new(),
    )
}

pub fn callable_schematic_param_names(store: &TypeStore, callee: TypeId) -> Vec<String> {
    let Some(Type::Function(flow)) = store.get(callee) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for ty in flow
        .input
        .iter()
        .copied()
        .chain(std::iter::once(flow.output))
    {
        collect_schematic_type_vars(store, ty, &mut names);
    }
    names
}

pub fn solve_callable_with_named_substitutions(
    store: &TypeStore,
    callee: TypeId,
    generic_param_names: &[String],
    explicit_generic_args: &[TypeId],
    args: &[TypeId],
    output: TypeId,
    origin: ConstraintOrigin,
    initial_named_substitutions: HashMap<String, TypeId>,
) -> SolverReport {
    let mut report = SolverReport::default();
    if let Some(constructor) = nominal_constructor_signature(store, callee, explicit_generic_args) {
        return solve_nominal_constructor_call(store, constructor, args, output, origin);
    }
    let Some(Type::Function(flow)) = store.get(callee) else {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::NonCallableCallee,
            span: origin.span,
            message: "callee is not callable".to_owned(),
        });
        return report;
    };
    if flow.input.len() != args.len() {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::ArityMismatch,
            span: origin.span,
            message: format!(
                "call expects {} argument(s), got {}",
                flow.input.len(),
                args.len()
            ),
        });
        return report;
    }
    let mut type_substitutions = initial_named_substitutions;
    apply_explicit_generic_args(
        store,
        flow,
        generic_param_names,
        explicit_generic_args,
        &mut type_substitutions,
    );
    for (actual, expected) in args.iter().copied().zip(flow.input.iter().copied()) {
        infer_type_substitution(store, expected, actual, &mut type_substitutions);
    }
    for (actual, expected) in args.iter().copied().zip(flow.input.iter().copied()) {
        if !is_assignable_to_schematic(store, actual, expected, &mut type_substitutions) {
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                span: origin.span,
                message: format!(
                    "call argument type `{}` does not match parameter type `{}`",
                    display_type(store, actual),
                    display_type(store, expected)
                ),
            });
        } else if let Some(unification_target) =
            unification_target_for_schematic_expected(store, expected, &type_substitutions)
        {
            let mut arg_unifier = TypeUnifier::new(store);
            let unified = if contains_fresh_type_var(store, unification_target) {
                arg_unifier.unify(unification_target, actual)
            } else {
                arg_unifier.unify(actual, unification_target)
            };
            if unified.is_ok() {
                merge_substitution_or_report(
                    store,
                    &mut report,
                    arg_unifier.substitution(),
                    origin,
                );
            }
        }
    }
    infer_type_substitution(store, flow.output, output, &mut type_substitutions);
    if contains_unresolved_schematic_type_var(store, flow.output, &type_substitutions)
        && !unresolved_output_schematics_are_input_bound(
            store,
            flow.output,
            &flow.input,
            &type_substitutions,
        )
        && !inputs_contain_fresh_type_var(store, &flow.input)
        && (matches!(store.get(output), Some(Type::Var(_))) || output == flow.output)
    {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: "constructor output requires an expected result type".to_owned(),
        });
        return report;
    }
    let actual_output =
        unification_target_for_schematic_expected(store, flow.output, &type_substitutions)
            .unwrap_or(flow.output);
    let actual_output = resolve_local_substitution(store, &report.substitutions, actual_output);
    let mut output_unifier = TypeUnifier::new(store);
    let output_unified = if matches!(store.get(output), Some(Type::Var(_))) {
        output_unifier.unify(output, actual_output)
    } else {
        output_unifier.unify(actual_output, output)
    };
    if output_unified.is_ok() {
        merge_substitution_or_report(store, &mut report, output_unifier.substitution(), origin);
    } else if !is_schematic_assignable_to(
        store,
        actual_output,
        output,
        &mut type_substitutions,
        &report.substitutions,
    ) {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: format!(
                "call result type `{}` does not match expected output `{}`",
                display_type(store, actual_output),
                display_type(store, output)
            ),
        });
    }
    report
}

fn unresolved_output_schematics_are_input_bound(
    store: &TypeStore,
    output: TypeId,
    inputs: &[TypeId],
    substitutions: &HashMap<String, TypeId>,
) -> bool {
    let mut output_vars = Vec::new();
    collect_unresolved_schematic_names(store, output, substitutions, &mut output_vars);
    if output_vars.is_empty() {
        return true;
    }
    output_vars.into_iter().all(|name| {
        inputs
            .iter()
            .any(|input| contains_schematic_name(store, *input, &name))
    })
}

fn collect_unresolved_schematic_names(
    store: &TypeStore,
    ty: TypeId,
    substitutions: &HashMap<String, TypeId>,
    out: &mut Vec<String>,
) {
    match store.get(ty) {
        Some(Type::Named(name)) if is_type_variable_name(&name.name) => {
            if !substitutions.contains_key(&name.name) && !out.contains(&name.name) {
                out.push(name.name.clone());
            }
        }
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
        | Some(Type::Trust { inner, .. }) => {
            collect_unresolved_schematic_names(store, *inner, substitutions, out);
        }
        Some(Type::Map { key, value }) | Some(Type::Store { key, value }) => {
            collect_unresolved_schematic_names(store, *key, substitutions, out);
            collect_unresolved_schematic_names(store, *value, substitutions, out);
        }
        Some(Type::Result { ok, err }) => {
            collect_unresolved_schematic_names(store, *ok, substitutions, out);
            collect_unresolved_schematic_names(store, *err, substitutions, out);
        }
        Some(Type::Tuple(elements)) => {
            for element in elements {
                collect_unresolved_schematic_names(store, *element, substitutions, out);
            }
        }
        Some(Type::Function(flow)) => {
            for input in &flow.input {
                collect_unresolved_schematic_names(store, *input, substitutions, out);
            }
            collect_unresolved_schematic_names(store, flow.output, substitutions, out);
        }
        Some(Type::Applied { args, .. }) => {
            for arg in args {
                collect_unresolved_schematic_names(store, *arg, substitutions, out);
            }
        }
        _ => {}
    }
}

fn contains_schematic_name(store: &TypeStore, ty: TypeId, needle: &str) -> bool {
    match store.get(ty) {
        Some(Type::Named(name)) if name.name == needle => true,
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
        | Some(Type::Trust { inner, .. }) => contains_schematic_name(store, *inner, needle),
        Some(Type::Map { key, value }) | Some(Type::Store { key, value }) => {
            contains_schematic_name(store, *key, needle)
                || contains_schematic_name(store, *value, needle)
        }
        Some(Type::Result { ok, err }) => {
            contains_schematic_name(store, *ok, needle)
                || contains_schematic_name(store, *err, needle)
        }
        Some(Type::Tuple(elements)) => elements
            .iter()
            .any(|element| contains_schematic_name(store, *element, needle)),
        Some(Type::Function(flow)) => {
            flow.input
                .iter()
                .any(|input| contains_schematic_name(store, *input, needle))
                || contains_schematic_name(store, flow.output, needle)
        }
        Some(Type::Applied { args, .. }) => args
            .iter()
            .any(|arg| contains_schematic_name(store, *arg, needle)),
        _ => false,
    }
}

fn inputs_contain_fresh_type_var(store: &TypeStore, inputs: &[TypeId]) -> bool {
    inputs
        .iter()
        .any(|input| contains_fresh_type_var(store, *input))
}

fn resolve_local_substitution(
    store: &TypeStore,
    substitutions: &crate::Substitution,
    ty: TypeId,
) -> TypeId {
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

fn contains_fresh_type_var(store: &TypeStore, ty: TypeId) -> bool {
    match store.get(ty) {
        Some(Type::Var(_)) => true,
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
        | Some(Type::Trust { inner, .. }) => contains_fresh_type_var(store, *inner),
        Some(Type::Map { key, value }) | Some(Type::Store { key, value }) => {
            contains_fresh_type_var(store, *key) || contains_fresh_type_var(store, *value)
        }
        Some(Type::Result { ok, err }) => {
            contains_fresh_type_var(store, *ok) || contains_fresh_type_var(store, *err)
        }
        Some(Type::Tuple(elements)) => elements
            .iter()
            .any(|element| contains_fresh_type_var(store, *element)),
        Some(Type::Function(flow)) => {
            flow.input
                .iter()
                .any(|input| contains_fresh_type_var(store, *input))
                || contains_fresh_type_var(store, flow.output)
        }
        _ => false,
    }
}

struct NominalConstructorCall {
    target: TypeId,
    representation: TypeId,
    substitutions: HashMap<String, TypeId>,
}

fn nominal_constructor_signature(
    store: &TypeStore,
    callee: TypeId,
    explicit_generic_args: &[TypeId],
) -> Option<NominalConstructorCall> {
    match store.get(callee)? {
        Type::Nominal(nominal) => Some(NominalConstructorCall {
            target: callee,
            representation: nominal.representation?,
            substitutions: explicit_nominal_constructor_substitutions(
                &nominal.params,
                explicit_generic_args,
            ),
        }),
        Type::Applied { constructor, args } => {
            let constructor_type = TypeId(constructor.0);
            let Type::Nominal(nominal) = store.get(constructor_type)? else {
                return None;
            };
            let representation = nominal.representation?;
            Some(NominalConstructorCall {
                target: callee,
                representation,
                substitutions: explicit_nominal_constructor_substitutions(&nominal.params, args),
            })
        }
        _ => None,
    }
}

fn explicit_nominal_constructor_substitutions(
    params: &[String],
    explicit_generic_args: &[TypeId],
) -> HashMap<String, TypeId> {
    params
        .iter()
        .cloned()
        .zip(explicit_generic_args.iter().copied())
        .collect()
}

fn solve_nominal_constructor_call(
    store: &TypeStore,
    constructor: NominalConstructorCall,
    args: &[TypeId],
    output: TypeId,
    origin: ConstraintOrigin,
) -> SolverReport {
    let mut report = SolverReport::default();
    if args.len() != 1 {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::ArityMismatch,
            span: origin.span,
            message: format!("nominal constructor expects 1 argument, got {}", args.len()),
        });
        return report;
    }
    let mut substitutions = constructor.substitutions;
    if !is_assignable_to_schematic(
        store,
        args[0],
        constructor.representation,
        &mut substitutions,
    ) {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: "nominal constructor argument does not match representation type".to_owned(),
        });
        return report;
    }
    let mut output_unifier = TypeUnifier::new(store);
    if output_unifier.unify(constructor.target, output).is_ok() {
        merge_substitution_or_report(store, &mut report, output_unifier.substitution(), origin);
    } else {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: format!(
                "nominal constructor result type `{}` does not match expected output `{}`",
                display_type(store, constructor.target),
                display_type(store, output)
            ),
        });
    }
    report
}

fn merge_substitution_or_report(
    store: &TypeStore,
    report: &mut SolverReport,
    incoming: &crate::Substitution,
    origin: ConstraintOrigin,
) {
    for (var, incoming_ty) in incoming.iter() {
        if let Some(existing_ty) = report.substitutions.get(var) {
            let mut unifier = TypeUnifier::new(store);
            if unifier.unify(existing_ty, incoming_ty).is_err() {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span: origin.span,
                    message: format!(
                        "inferred generic type constraints are inconsistent: `{}` conflicts with `{}`",
                        display_type(store, existing_ty),
                        display_type(store, incoming_ty)
                    ),
                });
                continue;
            }
            report.substitutions.extend(unifier.substitution());
            report.substitutions.insert(var, existing_ty);
            continue;
        }
        report.substitutions.insert(var, incoming_ty);
    }
}

fn apply_explicit_generic_args(
    store: &TypeStore,
    flow: &crate::FlowType,
    generic_param_names: &[String],
    explicit_generic_args: &[TypeId],
    substitutions: &mut HashMap<String, TypeId>,
) {
    if explicit_generic_args.is_empty() {
        return;
    }
    if !generic_param_names.is_empty() {
        for (name, ty) in generic_param_names
            .iter()
            .cloned()
            .zip(explicit_generic_args.iter().copied())
        {
            substitutions.entry(name).or_insert(ty);
        }
        return;
    }
    let mut params = Vec::<String>::new();
    for ty in flow
        .input
        .iter()
        .copied()
        .chain(std::iter::once(flow.output))
    {
        collect_schematic_type_vars(store, ty, &mut params);
    }
    for (name, ty) in params
        .into_iter()
        .zip(explicit_generic_args.iter().copied())
    {
        substitutions.entry(name).or_insert(ty);
    }
}

fn collect_schematic_type_vars(store: &TypeStore, ty: TypeId, out: &mut Vec<String>) {
    match store.get(ty) {
        Some(Type::Named(name)) if is_type_variable_name(&name.name) => {
            if !out.contains(&name.name) {
                out.push(name.name.clone());
            }
        }
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
        | Some(Type::Trust { inner, .. }) => collect_schematic_type_vars(store, *inner, out),
        Some(Type::Map { key, value }) | Some(Type::Store { key, value }) => {
            collect_schematic_type_vars(store, *key, out);
            collect_schematic_type_vars(store, *value, out);
        }
        Some(Type::Result { ok, err }) => {
            collect_schematic_type_vars(store, *ok, out);
            collect_schematic_type_vars(store, *err, out);
        }
        Some(Type::Tuple(elements)) => {
            for element in elements {
                collect_schematic_type_vars(store, *element, out);
            }
        }
        Some(Type::Record(record)) => {
            for field in &record.fields {
                collect_schematic_type_vars(store, field.ty, out);
            }
        }
        Some(Type::Function(flow)) => {
            for input in &flow.input {
                collect_schematic_type_vars(store, *input, out);
            }
            collect_schematic_type_vars(store, flow.output, out);
        }
        Some(Type::Nominal(nominal)) => {
            if let Some(representation) = nominal.representation {
                collect_schematic_type_vars(store, representation, out);
            }
        }
        Some(Type::Applied { args, .. }) => {
            for arg in args {
                collect_schematic_type_vars(store, *arg, out);
            }
        }
        Some(Type::Refined { base, .. }) => collect_schematic_type_vars(store, *base, out),
        Some(Type::ResourceHandle(crate::ResourceHandleType::MemoryRegion { schema })) => {
            collect_schematic_type_vars(store, *schema, out);
        }
        Some(Type::ResourceHandle(crate::ResourceHandleType::ExternalTool { signature })) => {
            collect_schematic_type_vars(store, *signature, out);
        }
        Some(Type::ResourceHandle(crate::ResourceHandleType::Other { args, .. })) => {
            for arg in args {
                collect_schematic_type_vars(store, *arg, out);
            }
        }
        _ => {}
    }
}

fn infer_type_substitution(
    store: &TypeStore,
    expected: TypeId,
    actual: TypeId,
    substitutions: &mut HashMap<String, TypeId>,
) {
    match store.get(expected) {
        Some(Type::Named(name)) if is_type_variable_name(&name.name) => {
            if expected == actual {
                return;
            }
            substitutions.entry(name.name.clone()).or_insert(actual);
        }
        Some(Type::Array(expected)) => {
            if let Some(Type::Array(actual)) = store.get(actual) {
                infer_type_substitution(store, *expected, *actual, substitutions);
            }
        }
        Some(Type::List(expected)) => {
            if let Some(Type::List(actual)) = store.get(actual) {
                infer_type_substitution(store, *expected, *actual, substitutions);
            }
        }
        Some(Type::Set(expected)) => {
            if let Some(Type::Set(actual)) = store.get(actual) {
                infer_type_substitution(store, *expected, *actual, substitutions);
            }
        }
        Some(Type::Slice(expected)) => {
            if let Some(Type::Slice(actual)) = store.get(actual) {
                infer_type_substitution(store, *expected, *actual, substitutions);
            }
        }
        Some(Type::Option(expected)) => {
            if let Some(Type::Option(actual)) = store.get(actual) {
                infer_type_substitution(store, *expected, *actual, substitutions);
            }
        }
        Some(Type::Schema(expected)) => {
            if let Some(Type::Schema(actual)) = store.get(actual) {
                infer_type_substitution(store, *expected, *actual, substitutions);
            }
        }
        Some(Type::Trust {
            wrapper: expected_wrapper,
            inner: expected,
        }) => {
            if let Some(Type::Trust {
                wrapper: actual_wrapper,
                inner: actual,
            }) = store.get(actual)
                && expected_wrapper == actual_wrapper
            {
                infer_type_substitution(store, *expected, *actual, substitutions);
            }
        }
        Some(Type::Message(expected)) => {
            if let Some(Type::Message(actual)) = store.get(actual) {
                infer_type_substitution(store, *expected, *actual, substitutions);
            }
        }
        Some(Type::MemorySelection(expected)) => {
            if let Some(Type::MemorySelection(actual)) = store.get(actual) {
                infer_type_substitution(store, *expected, *actual, substitutions);
            }
        }
        Some(Type::MemoryRegion(expected)) => {
            if let Some(Type::MemoryRegion(actual)) = store.get(actual) {
                infer_type_substitution(store, *expected, *actual, substitutions);
            }
        }
        Some(Type::Range { index: expected }) => match store.get(actual) {
            Some(Type::Range { index: actual }) => {
                infer_type_substitution(store, *expected, *actual, substitutions);
            }
            _ => {}
        },
        Some(Type::Result {
            ok: expected_ok,
            err: expected_err,
        }) => {
            if let Some(Type::Result {
                ok: actual_ok,
                err: actual_err,
            }) = store.get(actual)
            {
                infer_type_substitution(store, *expected_ok, *actual_ok, substitutions);
                infer_type_substitution(store, *expected_err, *actual_err, substitutions);
            }
        }
        Some(Type::Map {
            key: expected_key,
            value: expected_value,
        })
        | Some(Type::Store {
            key: expected_key,
            value: expected_value,
        }) => {
            if let Some(Type::Map {
                key: actual_key,
                value: actual_value,
            })
            | Some(Type::Store {
                key: actual_key,
                value: actual_value,
            }) = store.get(actual)
            {
                infer_type_substitution(store, *expected_key, *actual_key, substitutions);
                infer_type_substitution(store, *expected_value, *actual_value, substitutions);
            }
        }
        Some(Type::Tuple(expected_elements)) => {
            if let Some(Type::Tuple(actual_elements)) = store.get(actual) {
                for (expected, actual) in expected_elements
                    .iter()
                    .copied()
                    .zip(actual_elements.iter().copied())
                {
                    infer_type_substitution(store, expected, actual, substitutions);
                }
            }
        }
        Some(Type::Applied {
            constructor: expected_constructor,
            args: expected_args,
        }) => match store.get(actual) {
            Some(Type::Applied {
                constructor: actual_constructor,
                args: actual_args,
            }) if constructors_match(store, *expected_constructor, *actual_constructor)
                && expected_args.len() == actual_args.len() =>
            {
                for (expected, actual) in expected_args
                    .iter()
                    .copied()
                    .zip(actual_args.iter().copied())
                {
                    infer_type_substitution(store, expected, actual, substitutions);
                }
            }
            _ => {}
        },
        _ => {}
    }
}

fn is_assignable_to_schematic(
    store: &TypeStore,
    actual: TypeId,
    expected: TypeId,
    substitutions: &mut HashMap<String, TypeId>,
) -> bool {
    if let Some(Type::Named(name)) = store.get(actual)
        && is_type_variable_name(&name.name)
        && let Some(actual) = substitutions.get(&name.name).copied()
    {
        return is_assignable_to_schematic(store, actual, expected, substitutions);
    }
    match store.get(expected) {
        Some(Type::Var(_)) => true,
        Some(Type::Named(name)) if is_type_variable_name(&name.name) => {
            if expected == actual {
                return true;
            }
            if let Some(bound) = substitutions.get(&name.name).copied() {
                is_assignable(store, actual, bound)
            } else {
                substitutions.insert(name.name.clone(), actual);
                true
            }
        }
        Some(Type::Array(expected)) => match store.get(actual) {
            Some(Type::Array(actual)) => {
                is_assignable_to_schematic(store, *actual, *expected, substitutions)
            }
            _ => false,
        },
        Some(Type::List(expected)) => match store.get(actual) {
            Some(Type::List(actual)) => {
                is_assignable_to_schematic(store, *actual, *expected, substitutions)
            }
            _ => false,
        },
        Some(Type::Set(expected)) => match store.get(actual) {
            Some(Type::Set(actual)) => {
                is_assignable_to_schematic(store, *actual, *expected, substitutions)
            }
            _ => false,
        },
        Some(Type::Slice(expected)) => match store.get(actual) {
            Some(Type::Slice(actual)) => {
                is_assignable_to_schematic(store, *actual, *expected, substitutions)
            }
            _ => false,
        },
        Some(Type::Option(expected)) => match store.get(actual) {
            Some(Type::Option(actual)) => {
                is_assignable_to_schematic(store, *actual, *expected, substitutions)
            }
            _ => false,
        },
        Some(Type::Schema(expected)) => match store.get(actual) {
            Some(Type::Schema(actual)) => {
                is_assignable_to_schematic(store, *actual, *expected, substitutions)
            }
            _ => false,
        },
        Some(Type::Trust {
            wrapper: expected_wrapper,
            inner: expected,
        }) => match store.get(actual) {
            Some(Type::Trust {
                wrapper: actual_wrapper,
                inner: actual,
            }) if expected_wrapper == actual_wrapper => {
                is_assignable_to_schematic(store, *actual, *expected, substitutions)
            }
            _ => false,
        },
        Some(Type::Message(expected)) => match store.get(actual) {
            Some(Type::Message(actual)) => {
                is_assignable_to_schematic(store, *actual, *expected, substitutions)
            }
            _ => false,
        },
        Some(Type::MemorySelection(expected)) => match store.get(actual) {
            Some(Type::MemorySelection(actual)) => {
                is_assignable_to_schematic(store, *actual, *expected, substitutions)
            }
            _ => false,
        },
        Some(Type::MemoryRegion(expected)) => match store.get(actual) {
            Some(Type::MemoryRegion(actual)) => {
                is_assignable_to_schematic(store, *actual, *expected, substitutions)
            }
            _ => false,
        },
        Some(Type::Range { index: expected }) => match store.get(actual) {
            Some(Type::Range { index: actual }) => {
                is_assignable_to_schematic(store, *actual, *expected, substitutions)
            }
            _ => false,
        },
        Some(Type::Map {
            key: expected_key,
            value: expected_value,
        })
        | Some(Type::Store {
            key: expected_key,
            value: expected_value,
        }) => match store.get(actual) {
            Some(Type::Map {
                key: actual_key,
                value: actual_value,
            })
            | Some(Type::Store {
                key: actual_key,
                value: actual_value,
            }) => {
                is_assignable_to_schematic(store, *actual_key, *expected_key, substitutions)
                    && is_assignable_to_schematic(
                        store,
                        *actual_value,
                        *expected_value,
                        substitutions,
                    )
            }
            _ => false,
        },
        Some(Type::Result {
            ok: expected_ok,
            err: expected_err,
        }) => match store.get(actual) {
            Some(Type::Result {
                ok: actual_ok,
                err: actual_err,
            }) => {
                is_assignable_to_schematic(store, *actual_ok, *expected_ok, substitutions)
                    && is_assignable_to_schematic(store, *actual_err, *expected_err, substitutions)
            }
            _ => false,
        },
        Some(Type::Tuple(expected_elements)) => match store.get(actual) {
            Some(Type::Tuple(actual_elements))
                if actual_elements.len() == expected_elements.len() =>
            {
                actual_elements
                    .iter()
                    .copied()
                    .zip(expected_elements.iter().copied())
                    .all(|(actual, expected)| {
                        is_assignable_to_schematic(store, actual, expected, substitutions)
                    })
            }
            _ => false,
        },
        Some(Type::Applied {
            constructor: expected_constructor,
            args: expected_args,
        }) => match store.get(actual) {
            Some(Type::Applied {
                constructor: actual_constructor,
                args: actual_args,
            }) if constructors_match(store, *expected_constructor, *actual_constructor)
                && expected_args.len() == actual_args.len() =>
            {
                actual_args
                    .iter()
                    .copied()
                    .zip(expected_args.iter().copied())
                    .all(|(actual, expected)| {
                        is_assignable_to_schematic(store, actual, expected, substitutions)
                    })
            }
            _ => false,
        },
        _ => is_assignable(store, actual, expected),
    }
}

fn is_schematic_assignable_to(
    store: &TypeStore,
    from: TypeId,
    to: TypeId,
    substitutions: &mut HashMap<String, TypeId>,
    solver_substitutions: &crate::Substitution,
) -> bool {
    match store.get(from) {
        Some(Type::Var(var)) => solver_substitutions
            .get(*var)
            .is_some_and(|resolved| resolved != from && is_assignable(store, resolved, to)),
        Some(Type::Named(name)) if is_type_variable_name(&name.name) => substitutions
            .get(&name.name)
            .copied()
            .is_some_and(|from| is_assignable(store, from, to)),
        Some(Type::Array(from)) => match store.get(to) {
            Some(Type::Array(to)) => {
                is_schematic_assignable_to(store, *from, *to, substitutions, solver_substitutions)
            }
            _ => false,
        },
        Some(Type::List(from)) => match store.get(to) {
            Some(Type::List(to)) => {
                is_schematic_assignable_to(store, *from, *to, substitutions, solver_substitutions)
            }
            _ => false,
        },
        Some(Type::Set(from)) => match store.get(to) {
            Some(Type::Set(to)) => {
                is_schematic_assignable_to(store, *from, *to, substitutions, solver_substitutions)
            }
            _ => false,
        },
        Some(Type::Slice(from)) => match store.get(to) {
            Some(Type::Slice(to)) => {
                is_schematic_assignable_to(store, *from, *to, substitutions, solver_substitutions)
            }
            _ => false,
        },
        Some(Type::Option(from)) => match store.get(to) {
            Some(Type::Option(to)) => {
                is_schematic_assignable_to(store, *from, *to, substitutions, solver_substitutions)
            }
            _ => false,
        },
        Some(Type::Schema(from)) => match store.get(to) {
            Some(Type::Schema(to)) => {
                is_schematic_assignable_to(store, *from, *to, substitutions, solver_substitutions)
            }
            _ => false,
        },
        Some(Type::Trust {
            wrapper: from_wrapper,
            inner: from,
        }) => match store.get(to) {
            Some(Type::Trust { wrapper, inner }) if from_wrapper == wrapper => {
                is_schematic_assignable_to(
                    store,
                    *from,
                    *inner,
                    substitutions,
                    solver_substitutions,
                )
            }
            _ => false,
        },
        Some(Type::Message(from)) => match store.get(to) {
            Some(Type::Message(to)) => {
                is_schematic_assignable_to(store, *from, *to, substitutions, solver_substitutions)
            }
            _ => false,
        },
        Some(Type::MemorySelection(from)) => match store.get(to) {
            Some(Type::MemorySelection(to)) => {
                is_schematic_assignable_to(store, *from, *to, substitutions, solver_substitutions)
            }
            _ => false,
        },
        Some(Type::MemoryRegion(from)) => match store.get(to) {
            Some(Type::MemoryRegion(to)) => {
                is_schematic_assignable_to(store, *from, *to, substitutions, solver_substitutions)
            }
            _ => false,
        },
        Some(Type::Range { index: from }) => match store.get(to) {
            Some(Type::Range { index: to }) => {
                is_schematic_assignable_to(store, *from, *to, substitutions, solver_substitutions)
            }
            _ => false,
        },
        Some(Type::Map {
            key: from_key,
            value: from_value,
        })
        | Some(Type::Store {
            key: from_key,
            value: from_value,
        }) => match store.get(to) {
            Some(Type::Map { key, value }) => {
                is_schematic_assignable_to(
                    store,
                    *from_key,
                    *key,
                    substitutions,
                    solver_substitutions,
                ) && is_schematic_assignable_to(
                    store,
                    *from_value,
                    *value,
                    substitutions,
                    solver_substitutions,
                )
            }
            Some(Type::Store { key, value }) => {
                is_schematic_assignable_to(
                    store,
                    *from_key,
                    *key,
                    substitutions,
                    solver_substitutions,
                ) && is_schematic_assignable_to(
                    store,
                    *from_value,
                    *value,
                    substitutions,
                    solver_substitutions,
                )
            }
            _ => false,
        },
        Some(Type::Result {
            ok: from_ok,
            err: from_err,
        }) => match store.get(to) {
            Some(Type::Result { ok, err }) => {
                is_schematic_assignable_to(
                    store,
                    *from_ok,
                    *ok,
                    substitutions,
                    solver_substitutions,
                ) && is_schematic_assignable_to(
                    store,
                    *from_err,
                    *err,
                    substitutions,
                    solver_substitutions,
                )
            }
            _ => false,
        },
        Some(Type::Tuple(from_elements)) => match store.get(to) {
            Some(Type::Tuple(to_elements)) if from_elements.len() == to_elements.len() => {
                from_elements
                    .iter()
                    .copied()
                    .zip(to_elements.iter().copied())
                    .all(|(from, to)| {
                        is_schematic_assignable_to(
                            store,
                            from,
                            to,
                            substitutions,
                            solver_substitutions,
                        )
                    })
            }
            _ => false,
        },
        Some(Type::Function(from_flow)) => match store.get(to) {
            Some(Type::Function(to_flow)) if from_flow.input.len() == to_flow.input.len() => {
                from_flow
                    .input
                    .iter()
                    .copied()
                    .zip(to_flow.input.iter().copied())
                    .all(|(from, to)| {
                        is_schematic_assignable_to(
                            store,
                            from,
                            to,
                            substitutions,
                            solver_substitutions,
                        )
                    })
                    && is_schematic_assignable_to(
                        store,
                        from_flow.output,
                        to_flow.output,
                        substitutions,
                        solver_substitutions,
                    )
            }
            _ => false,
        },
        Some(Type::Applied {
            constructor: from_constructor,
            args: from_args,
        }) => match store.get(to) {
            Some(Type::Applied {
                constructor: to_constructor,
                args: to_args,
            }) if constructors_match(store, *from_constructor, *to_constructor)
                && from_args.len() == to_args.len() =>
            {
                from_args
                    .iter()
                    .copied()
                    .zip(to_args.iter().copied())
                    .all(|(from, to)| {
                        is_schematic_assignable_to(
                            store,
                            from,
                            to,
                            substitutions,
                            solver_substitutions,
                        )
                    })
            }
            _ => false,
        },
        Some(Type::ResourceHandle(crate::ResourceHandleType::MemoryRegion { schema: from })) => {
            match store.get(to) {
                Some(Type::ResourceHandle(crate::ResourceHandleType::MemoryRegion {
                    schema: to,
                })) => is_schematic_assignable_to(
                    store,
                    *from,
                    *to,
                    substitutions,
                    solver_substitutions,
                ),
                _ => false,
            }
        }
        Some(Type::ResourceHandle(crate::ResourceHandleType::ExternalTool { signature: from })) => {
            match store.get(to) {
                Some(Type::ResourceHandle(crate::ResourceHandleType::ExternalTool {
                    signature: to,
                })) => is_schematic_assignable_to(
                    store,
                    *from,
                    *to,
                    substitutions,
                    solver_substitutions,
                ),
                _ => false,
            }
        }
        Some(Type::ResourceHandle(crate::ResourceHandleType::Other {
            name: from_name,
            args: from_args,
        })) => match store.get(to) {
            Some(Type::ResourceHandle(crate::ResourceHandleType::Other {
                name: to_name,
                args: to_args,
            })) if from_name == to_name && from_args.len() == to_args.len() => from_args
                .iter()
                .copied()
                .zip(to_args.iter().copied())
                .all(|(from, to)| {
                    is_schematic_assignable_to(store, from, to, substitutions, solver_substitutions)
                }),
            _ => false,
        },
        _ => is_assignable(store, from, to),
    }
}

fn constructors_match(
    store: &TypeStore,
    lhs: crate::TypeConstructorId,
    rhs: crate::TypeConstructorId,
) -> bool {
    if lhs == rhs {
        return true;
    }
    let mut unifier = TypeUnifier::new(store);
    unifier.unify(TypeId(lhs.0), TypeId(rhs.0)).is_ok()
}

fn contains_unresolved_schematic_type_var(
    store: &TypeStore,
    ty: TypeId,
    substitutions: &HashMap<String, TypeId>,
) -> bool {
    match store.get(ty) {
        Some(Type::Named(name)) if is_type_variable_name(&name.name) => {
            !substitutions.contains_key(&name.name)
        }
        Some(Type::Array(inner))
        | Some(Type::List(inner))
        | Some(Type::Set(inner))
        | Some(Type::Slice(inner))
        | Some(Type::Option(inner))
        | Some(Type::Message(inner))
        | Some(Type::Schema(inner))
        | Some(Type::MemorySelection(inner))
        | Some(Type::MemoryRegion(inner))
        | Some(Type::Range { index: inner }) => {
            contains_unresolved_schematic_type_var(store, *inner, substitutions)
        }
        Some(Type::Trust { inner, .. }) => {
            contains_unresolved_schematic_type_var(store, *inner, substitutions)
        }
        Some(Type::Map { key, value }) | Some(Type::Store { key, value }) => {
            contains_unresolved_schematic_type_var(store, *key, substitutions)
                || contains_unresolved_schematic_type_var(store, *value, substitutions)
        }
        Some(Type::Result { ok, err }) => {
            contains_unresolved_schematic_type_var(store, *ok, substitutions)
                || contains_unresolved_schematic_type_var(store, *err, substitutions)
        }
        Some(Type::Tuple(elements)) => elements
            .iter()
            .any(|element| contains_unresolved_schematic_type_var(store, *element, substitutions)),
        Some(Type::Function(flow)) => {
            flow.input
                .iter()
                .any(|input| contains_unresolved_schematic_type_var(store, *input, substitutions))
                || contains_unresolved_schematic_type_var(store, flow.output, substitutions)
        }
        Some(Type::Applied { args, .. }) => args
            .iter()
            .any(|arg| contains_unresolved_schematic_type_var(store, *arg, substitutions)),
        Some(Type::ResourceHandle(crate::ResourceHandleType::MemoryRegion { schema })) => {
            contains_unresolved_schematic_type_var(store, *schema, substitutions)
        }
        Some(Type::ResourceHandle(crate::ResourceHandleType::ExternalTool { signature })) => {
            contains_unresolved_schematic_type_var(store, *signature, substitutions)
        }
        Some(Type::ResourceHandle(crate::ResourceHandleType::Other { args, .. })) => args
            .iter()
            .any(|arg| contains_unresolved_schematic_type_var(store, *arg, substitutions)),
        _ => false,
    }
}

fn unification_target_for_schematic_expected(
    store: &TypeStore,
    expected: TypeId,
    substitutions: &HashMap<String, TypeId>,
) -> Option<TypeId> {
    match store.get(expected) {
        Some(Type::Named(name)) if is_type_variable_name(&name.name) => {
            substitutions.get(&name.name).copied()
        }
        _ if contains_any_schematic_type_var(store, expected) => None,
        _ => Some(expected),
    }
}

fn contains_any_schematic_type_var(store: &TypeStore, ty: TypeId) -> bool {
    match store.get(ty) {
        Some(Type::Named(name)) if is_type_variable_name(&name.name) => true,
        Some(Type::Array(inner))
        | Some(Type::List(inner))
        | Some(Type::Set(inner))
        | Some(Type::Slice(inner))
        | Some(Type::Option(inner))
        | Some(Type::Message(inner))
        | Some(Type::Schema(inner))
        | Some(Type::MemorySelection(inner))
        | Some(Type::MemoryRegion(inner))
        | Some(Type::Range { index: inner }) => contains_any_schematic_type_var(store, *inner),
        Some(Type::Trust { inner, .. }) => contains_any_schematic_type_var(store, *inner),
        Some(Type::Map { key, value }) | Some(Type::Store { key, value }) => {
            contains_any_schematic_type_var(store, *key)
                || contains_any_schematic_type_var(store, *value)
        }
        Some(Type::Result { ok, err }) => {
            contains_any_schematic_type_var(store, *ok)
                || contains_any_schematic_type_var(store, *err)
        }
        Some(Type::Tuple(elements)) => elements
            .iter()
            .any(|element| contains_any_schematic_type_var(store, *element)),
        Some(Type::Function(flow)) => {
            flow.input
                .iter()
                .any(|input| contains_any_schematic_type_var(store, *input))
                || contains_any_schematic_type_var(store, flow.output)
        }
        Some(Type::Applied { args, .. }) => args
            .iter()
            .any(|arg| contains_any_schematic_type_var(store, *arg)),
        Some(Type::ResourceHandle(crate::ResourceHandleType::MemoryRegion { schema })) => {
            contains_any_schematic_type_var(store, *schema)
        }
        Some(Type::ResourceHandle(crate::ResourceHandleType::ExternalTool { signature })) => {
            contains_any_schematic_type_var(store, *signature)
        }
        Some(Type::ResourceHandle(crate::ResourceHandleType::Other { args, .. })) => args
            .iter()
            .any(|arg| contains_any_schematic_type_var(store, *arg)),
        _ => false,
    }
}

fn is_type_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(ch) if ch.is_ascii_uppercase()) && chars.next().is_none()
}
