use etas_hir::HirItemId;
use etas_types::{CallableGenericParamKind, PrimitiveType as P, Type, TypeOutput};

pub(super) fn comparator_signature_matches(
    types: &TypeOutput,
    item: HirItemId,
    role: etas_std::StdExecutionRole,
) -> bool {
    let Some(etas_types::ItemSignature::Flow(signature)) = types.facts.item_signatures.get(&item)
    else {
        return false;
    };
    let Some(method) = types
        .facts
        .type_impl_methods
        .values()
        .find(|method| method.body == item)
    else {
        return false;
    };
    let identity = method.self_type;
    if !method.inherent
        || !method.owner_params.is_empty()
        || !matches!(types.store.get(identity), Some(Type::Nominal(owner))
            if owner.name == "std.collections.ComparatorIdentity"
                && owner.params.is_empty() && owner.representation.is_none())
        || signature
            .effects
            .as_ref()
            .is_none_or(|row| !row.effects.is_empty() || row.tail.is_some())
        || signature
            .requested_actions
            .as_ref()
            .is_some_and(|row| !row.effects.is_empty() || row.tail.is_some())
    {
        return false;
    }
    if role == etas_std::StdExecutionRole::ComparatorSame {
        return signature.generic_params.is_empty()
            && signature.params == [identity, identity]
            && matches!(
                types.store.get(signature.output),
                Some(Type::Primitive(P::Bool))
            );
    }
    let [parameter] = signature.generic_params.as_slice() else {
        return false;
    };
    role == etas_std::StdExecutionRole::ComparatorCapture
        && parameter.kind == CallableGenericParamKind::Type
        && parameter.bounds.is_empty()
        && signature.output == identity
        && signature.params.len() == 1
        && matches!(types.store.get(signature.params[0]), Some(Type::Function(flow))
            if flow.input == [parameter.subject, parameter.subject]
                && matches!(types.store.get(flow.output), Some(Type::Primitive(P::I32)))
                && flow.effects.as_ref().is_some_and(|row| row.effects.is_empty() && row.tail.is_none()))
}

pub(super) fn observation_signature_matches(
    types: &TypeOutput,
    item: HirItemId,
    role: etas_std::StdExecutionRole,
) -> bool {
    let Some(etas_types::ItemSignature::Flow(signature)) = types.facts.item_signatures.get(&item)
    else {
        return false;
    };
    if signature
        .effects
        .as_ref()
        .is_none_or(|row| !row.effects.is_empty() || row.tail.is_some())
        || signature
            .requested_actions
            .as_ref()
            .is_some_and(|row| !row.effects.is_empty() || row.tail.is_some())
    {
        return false;
    }
    if role == etas_std::StdExecutionRole::CurrentSession {
        return signature.params.is_empty()
            && signature.generic_params.is_empty()
            && matches!(types.store.get(signature.output), Some(Type::Nominal(value))
                if value.name == "std.agent.session.SessionId" && value.params.is_empty()
                    && value.representation.is_some_and(|ty| matches!(types.store.get(ty), Some(Type::Primitive(P::String)))));
    }
    let [parameter] = signature.generic_params.as_slice() else {
        return false;
    };
    role == etas_std::StdExecutionRole::ToolReference
        && parameter.kind == CallableGenericParamKind::Type
        && parameter.bounds.is_empty()
        && signature.params.as_slice() == [parameter.subject]
        && types.facts.known_std_types.code.tool_ref == Some(signature.output)
        && etas_types::record_fields_with_applied_params(&types.store, signature.output)
            .ok()
            .flatten()
            .is_some_and(|record| {
                record.fields.len() == 2
                    && ["id", "revision"].iter().all(|name| {
                        record.fields.iter().any(|field| {
                            field.name == *name
                                && matches!(
                                    types.store.get(field.ty),
                                    Some(Type::Primitive(P::String))
                                )
                        })
                    })
            })
}

pub(super) fn task_control_signature_matches(
    types: &TypeOutput,
    item: HirItemId,
    role: etas_std::StdExecutionRole,
) -> bool {
    let Some(etas_types::ItemSignature::Flow(signature)) = types.facts.item_signatures.get(&item)
    else {
        return false;
    };
    let Some(method) = types
        .facts
        .type_impl_methods
        .values()
        .find(|method| method.body == item)
    else {
        return false;
    };
    if !method.inherent
        || !method.owner_params.is_empty()
        || !etas_types::source_execution::is_scheduler(
            &types.store,
            &types.facts.known_std_types,
            method.self_type,
        )
        || signature.params.first() != Some(&method.self_type)
        || signature
            .effects
            .as_ref()
            .is_none_or(|row| !row.effects.is_empty() || row.tail.is_some())
        || signature
            .requested_actions
            .as_ref()
            .is_some_and(|row| !row.effects.is_empty() || row.tail.is_some())
    {
        return false;
    }
    if role == etas_std::StdExecutionRole::TaskNextCompletion {
        return signature.params.len() == 1
            && signature.generic_params.is_empty()
            && matches!(types.store.get(signature.output), Some(Type::Nominal(value)) if value.params.is_empty())
            && etas_types::record_fields_with_applied_params(&types.store, signature.output)
                .ok()
                .flatten()
                .is_some_and(|record| {
                    record.fields.len() == 3
                        && ["task", "order"].into_iter().all(|name| {
                            record.fields.iter().any(|field| {
                                field.name == name
                                    && matches!(
                                        types.store.get(field.ty),
                                        Some(Type::Primitive(P::U64))
                                    )
                            })
                        })
                        && record.fields.iter().any(|field| {
                            field.name == "completed"
                                && matches!(
                                    types.store.get(field.ty),
                                    Some(Type::Primitive(P::Bool))
                                )
                        })
                });
    }
    let [result, effects] = signature.generic_params.as_slice() else {
        return false;
    };
    signature.params.len() == 2
        && result.kind == CallableGenericParamKind::Type
        && effects.kind == CallableGenericParamKind::Effect
        && result.bounds.is_empty()
        && effects.bounds.is_empty()
        && matches!(
            types.store.get(signature.output),
            Some(Type::Primitive(P::Unit))
        )
        && etas_types::source_execution::task(
            &types.store,
            &types.facts.known_std_types,
            signature.params[1],
        )
        .is_some_and(|task| {
            task.output == result.subject
                && task.effects.effects.is_empty()
                && task.effects.tail.as_deref() == Some(effects.name.as_str())
        })
}

/// The VM action contract describes Task's latent result and row. Its source
/// record retains its own identity; this is not a language assignability rule.
pub(super) fn type_abi_matches(
    store: &etas_types::TypeStore,
    facts: &etas_types::TypeFacts,
    source: etas_types::TypeId,
    native: etas_types::TypeId,
    substitution: &mut etas_types::TypeSubstitutionEngine<'_>,
) -> Result<bool, etas_types::TypeSubstitutionError> {
    match (
        etas_types::source_execution::task(store, &facts.known_std_types, source),
        etas_types::source_execution::task(store, &facts.known_std_types, native),
    ) {
        (Some(source), Some(native)) => Ok(substitution.substitute(native.output)?
            == source.output
            && substitution.substitute_effect_row(native.effects.clone())? == *source.effects),
        (Some(_), None) | (None, Some(_)) => Ok(false),
        (None, None) => Ok(substitution.substitute(native)? == source),
    }
}

pub(super) fn start_signature_matches(types: &TypeOutput, item: HirItemId) -> bool {
    let Some(etas_types::ItemSignature::Flow(signature)) = types.facts.item_signatures.get(&item)
    else {
        return false;
    };
    let [result, effects] = signature.generic_params.as_slice() else {
        return false;
    };
    let [callback] = signature.params.as_slice() else {
        return false;
    };
    let Some(Type::Function(callback)) = types.store.get(*callback) else {
        return false;
    };
    let Some(task) = etas_types::source_execution::task(
        &types.store,
        &types.facts.known_std_types,
        signature.output,
    ) else {
        return false;
    };
    result.kind == CallableGenericParamKind::Type
        && effects.kind == CallableGenericParamKind::Effect
        && result.bounds.is_empty()
        && effects.bounds.is_empty()
        && callback.input.is_empty()
        && callback.output == result.subject
        && task.output == result.subject
        && task.effects.effects.is_empty()
        && task.effects.tail.as_deref() == Some(effects.name.as_str())
        && callback.effects.as_ref() == Some(task.effects)
        && signature.effects.as_ref() == Some(task.effects)
        && signature
            .requested_actions
            .as_ref()
            .is_none_or(|row| row.effects.is_empty() && row.tail.is_none())
}

/// Check only the fields crossing the VM task boundary. The declarations,
/// generic names and all ordinary methods are owned by the source library.
pub(super) fn type_layout_diagnostics(
    hir: &etas_hir::HirProgram,
    types: &TypeOutput,
) -> Vec<etas_core::Diagnostic> {
    use CallableGenericParamKind::{Effect, Type as Value};
    let known = &types.facts.known_std_types.execution;
    let capabilities = [
        known.task_handle,
        known.scheduler_state,
        known.continuation_state,
    ];
    let mut diagnostics = Vec::new();
    for (_, item) in hir.items.iter() {
        let etas_hir::HirItem::Type(declaration) = item else {
            continue;
        };
        let Some(etas_types::SymbolTypeFact::NominalType { constructor, .. }) =
            types.facts.symbol_types.get(&declaration.symbol)
        else {
            continue;
        };
        let owner = etas_types::TypeId(constructor.0);
        let (capability, slot, kinds, fields) = if known.task == Some(owner) {
            (known.task_handle, "handle", &[Value, Effect][..], 2)
        } else if known.scheduler == Some(owner) {
            (known.scheduler_state, "state", &[][..], 1)
        } else if known.continuation == Some(owner) {
            (known.continuation_state, "state", &[Value][..], 1)
        } else {
            continue;
        };
        let private_opaque = capability.is_some_and(|ty| {
            matches!(types.store.get(ty), Some(Type::Nominal(value))
                if value.params.is_empty() && value.representation.is_none())
                && capabilities
                    .iter()
                    .filter(|other| **other == Some(ty))
                    .count()
                    == 1
                && hir.symbols.iter().any(|symbol| {
                    symbol.visibility == etas_hir::Visibility::Private
                        && matches!(types.facts.symbol_types.get(&symbol.id),
                            Some(etas_types::SymbolTypeFact::NominalType { constructor, .. })
                            if constructor.0 == ty.0)
                })
        });
        let private_record = match declaration.body {
            etas_hir::HirTypeDeclBody::Representation(body) => {
                matches!(&hir.types[body], etas_hir::HirType::Record { fields, .. }
                    if fields.iter().all(|field| field.visibility == etas_syntax::ast::Visibility::Private))
            }
            _ => false,
        };
        let layout = etas_types::record_fields_with_applied_params(&types.store, owner)
            .ok()
            .flatten();
        let valid = private_opaque
            && private_record
            && hir
                .symbols
                .get(declaration.symbol)
                .is_some_and(|symbol| symbol.visibility == etas_hir::Visibility::Public)
            && types
                .facts
                .type_generics
                .get(&owner)
                .is_some_and(|parameters| {
                    parameters
                        .iter()
                        .map(|value| value.kind)
                        .eq(kinds.iter().copied())
                })
            && layout.is_some_and(|record| {
                record.fields.len() == fields
                    && record
                        .fields
                        .iter()
                        .any(|field| field.name == slot && Some(field.ty) == capability)
                    && (fields == 1
                        || record.fields.iter().any(|field| {
                            field.name == "id"
                                && matches!(
                                    types.store.get(field.ty),
                                    Some(Type::Primitive(P::U64))
                                )
                        }))
            });
        if !valid {
            diagnostics.push(etas_core::Diagnostic::type_check(
                etas_core::TypeDiagnosticCode::TypeMismatch, declaration.span,
                "source task record requires its private opaque VM capability, declared generic kinds and task identity field",
            ));
        }
    }
    diagnostics
}

pub(super) fn transfer_signature_matches(
    types: &TypeOutput,
    item: HirItemId,
    role: etas_std::StdExecutionRole,
) -> bool {
    use etas_std::StdExecutionRole as R;
    let Some(etas_types::ItemSignature::Flow(signature)) = types.facts.item_signatures.get(&item)
    else {
        return false;
    };
    let Some(method) = types
        .facts
        .type_impl_methods
        .values()
        .find(|method| method.body == item)
    else {
        return false;
    };
    let store = &types.store;
    if !method.inherent
        || !method.owner_params.is_empty()
        || !etas_types::source_execution::is_scheduler(
            store,
            &types.facts.known_std_types,
            method.self_type,
        )
        || signature.params.len() != 3
        || signature.params[0] != method.self_type
        || !matches!(store.get(signature.output), Some(Type::Primitive(P::Never)))
        || !signature
            .effects
            .as_ref()
            .is_some_and(|row| row.effects.is_empty() && row.tail.is_none())
        || signature
            .requested_actions
            .as_ref()
            .is_some_and(|row| !row.effects.is_empty() || row.tail.is_some())
        || signature
            .generic_params
            .iter()
            .any(|param| !param.bounds.is_empty())
    {
        return false;
    }
    let expected_kinds = if role == R::Requeue {
        &[CallableGenericParamKind::Type][..]
    } else {
        &[
            CallableGenericParamKind::Type,
            CallableGenericParamKind::Effect,
        ][..]
    };
    if !signature
        .generic_params
        .iter()
        .map(|param| param.kind)
        .eq(expected_kinds.iter().copied())
    {
        return false;
    }
    let output = signature.generic_params[0].subject;
    let continuation_slot = if role == R::Requeue { 1 } else { 2 };
    if etas_types::source_execution::continuation_output(
        store,
        &types.facts.known_std_types,
        signature.params[continuation_slot],
    ) != Some(output)
    {
        return false;
    }
    if role == R::Requeue {
        return signature.params[2] == output;
    }
    role == R::Park
        && etas_types::source_execution::task(
            store,
            &types.facts.known_std_types,
            signature.params[1],
        )
        .is_some_and(|task| {
            task.output == output
                && task.effects.effects.is_empty()
                && task.effects.tail.as_deref() == Some(signature.generic_params[1].name.as_str())
        })
}

/// This ABI describes a VM representation boundary. Data and DataError remain
/// ordinary source definitions, without a duplicate public native declaration.
pub(super) fn read_scope_signature_matches(types: &TypeOutput, item: HirItemId) -> bool {
    let Some(etas_types::ItemSignature::Flow(signature)) = types.facts.item_signatures.get(&item)
    else {
        return false;
    };
    let [stream] = signature.params.as_slice() else {
        return false;
    };
    let Some(etas_types::Type::Tuple(fields)) = types.store.get(signature.output) else {
        return false;
    };
    let [returned, owner] = fields.as_slice() else {
        return false;
    };
    if returned != stream
        || signature.generic_params.len() != 1
        || signature
            .effects
            .as_ref()
            .is_none_or(|row| !row.effects.is_empty() || row.tail.is_some())
    {
        return false;
    }
    let Some(owner) = etas_types::value_representation::option_parameter(
        &types.store,
        &types.facts.known_std_types,
        *owner,
    ) else {
        return false;
    };
    matches!(types.store.get(owner), Some(etas_types::Type::Nominal(value))
        if value.name == "std.stream.ReadOwner" && value.representation.is_none())
        && types.facts.known_std_types.opaque_values.contains(&owner)
}

pub(super) fn storage_signature_matches(
    types: &TypeOutput,
    item: HirItemId,
    role: etas_std::StdExecutionRole,
) -> bool {
    use etas_std::StdExecutionRole as R;
    let store = &types.store;
    let Some(etas_types::ItemSignature::Flow(signature)) = types.facts.item_signatures.get(&item)
    else {
        return false;
    };
    let [generic] = signature.generic_params.as_slice() else {
        return false;
    };
    let Some(method) = types
        .facts
        .type_impl_methods
        .values()
        .find(|method| method.body == item)
    else {
        return false;
    };
    let data = method.self_type;
    if generic.kind != CallableGenericParamKind::Type
        || !generic.bounds.is_empty()
        || !method.inherent
        || !method.owner_params.is_empty()
        || !matches!(store.get(data), Some(Type::Enum(owner)) if owner.name == "std.memory.StorageValue")
        || signature
            .effects
            .as_ref()
            .is_none_or(|row| !row.effects.is_empty() || row.tail.is_some())
        || signature
            .requested_actions
            .as_ref()
            .is_some_and(|row| !row.effects.is_empty() || row.tail.is_some())
    {
        return false;
    }
    let primitive =
        |ty, kind| matches!(store.get(ty), Some(Type::Primitive(actual)) if *actual == kind);
    let Some(layout) = types.facts.enum_layouts.get(&data) else {
        return false;
    };
    let array = |ty, inner| matches!(store.get(ty), Some(Type::Array(value)) if *value == inner);
    let pairs = |ty, first, second| match store.get(ty) {
        Some(Type::Array(pair)) => {
            matches!(store.get(*pair), Some(Type::Tuple(values)) if values == &[first, second])
        }
        _ => false,
    };
    let Some(string) = layout
        .variants
        .iter()
        .find(|variant| variant.name == "String")
        .and_then(|variant| variant.fields.first())
        .copied()
    else {
        return false;
    };
    if !layout.type_params.is_empty()
        || layout.variants.len() != 11
        || !layout.variants.iter().all(|variant| {
            if variant.field_names.is_some() {
                return false;
            }
            match (variant.name.as_str(), variant.fields.as_slice()) {
                ("Unit", []) => true,
                ("Bool", [ty]) => primitive(*ty, P::Bool),
                ("Scalar", [a, b, c]) => {
                    primitive(*a, P::U8) && primitive(*b, P::U8) && primitive(*c, P::U128)
                }
                ("String", [ty]) => primitive(*ty, P::String),
                ("Bytes", [ty]) => primitive(*ty, P::Bytes),
                ("Sequence", [tag, values]) => primitive(*tag, P::U8) && array(*values, data),
                ("Map", [values]) => pairs(*values, data, data),
                ("Record", [values]) => pairs(*values, string, data),
                ("Nominal", [id, value]) => primitive(*id, P::String) && *value == data,
                ("Variant", [id, name, values]) => {
                    primitive(*id, P::String) && primitive(*name, P::String) && array(*values, data)
                }
                ("Trust", [label, proof, value]) => {
                    primitive(*label, P::U8) && primitive(*proof, P::Bytes) && *value == data
                }
                _ => false,
            }
        })
    {
        return false;
    }
    if role == R::StorageSchema {
        return signature.params.is_empty() && primitive(signature.output, P::String);
    }
    let Some((ok, err)) = etas_types::value_representation::result_parameters(
        store,
        &types.facts.known_std_types,
        signature.output,
    ) else {
        return false;
    };
    storage_error_layout_matches(types, err)
        && match role {
            R::CaptureStorage => signature.params == [generic.subject] && ok == data,
            R::RestoreStorage => signature.params == [data] && ok == generic.subject,
            _ => false,
        }
}

pub(super) fn capture_signature_matches(types: &TypeOutput, item: HirItemId) -> bool {
    let store = &types.store;
    let Some(etas_types::ItemSignature::Flow(signature)) = types.facts.item_signatures.get(&item)
    else {
        return false;
    };
    let [generic] = signature.generic_params.as_slice() else {
        return false;
    };
    let Some(method) = types
        .facts
        .type_impl_methods
        .values()
        .find(|method| method.body == item)
    else {
        return false;
    };
    let Some((data, err)) = etas_types::value_representation::result_parameters(
        store,
        &types.facts.known_std_types,
        signature.output,
    ) else {
        return false;
    };
    let Some(Type::Enum(owner)) = store.get(data) else {
        return false;
    };
    let Some(Type::Nominal(error)) = store.get(err) else {
        return false;
    };
    let Some(error_fields) = etas_types::record_fields_with_applied_params(store, err)
        .ok()
        .flatten()
    else {
        return false;
    };
    let Some(layout) = types.facts.enum_layouts.get(&data) else {
        return false;
    };
    if generic.kind != CallableGenericParamKind::Type
        || !generic.bounds.is_empty()
        || signature.params != [generic.subject]
        || !signature
            .effects
            .as_ref()
            .is_some_and(|row| row.effects.is_empty() && row.tail.is_none())
        || signature
            .requested_actions
            .as_ref()
            .is_some_and(|row| !row.effects.is_empty() || row.tail.is_some())
        || !method.inherent
        || method.self_type != data
        || !method.owner_params.is_empty()
        || owner.name != "std.runtime.value.Data"
        || error.name != "std.runtime.value.DataError"
        || !error.params.is_empty()
        || error_fields.fields.len() != 2
        || !["code", "message"].into_iter().all(|name| {
            error_fields.fields.iter().any(|field| {
                field.name == name
                    && matches!(store.get(field.ty), Some(Type::Primitive(P::String)))
            })
        })
        || !layout.type_params.is_empty()
        || layout.variants.len() != 12
    {
        return false;
    }
    let array_of = |ty, inner| matches!(store.get(ty), Some(Type::Array(value)) if *value == inner);
    layout.variants.iter().all(|variant| {
        if variant.field_names.is_some() { return false }
        match (variant.name.as_str(), variant.fields.as_slice()) {
            ("Unit", []) => true,
            (name, [ty]) if matches!(name, "Bool" | "Int" | "UInt" | "Float" | "String" | "Bytes") => {
                let primitive = match name {
                    "Bool" => P::Bool, "Int" => P::I128, "UInt" => P::U128,
                    "Float" => P::F64, "String" => P::String, "Bytes" => P::Bytes, _ => unreachable!(),
                };
                matches!(store.get(*ty), Some(Type::Primitive(actual)) if *actual == primitive)
            }
            ("List", [ty]) => array_of(*ty, data),
            ("Map" | "Record", [ty]) => {
                let Some(Type::Array(pair)) = store.get(*ty) else { return false };
                matches!(store.get(*pair), Some(Type::Tuple(fields)) if matches!(fields.as_slice(), [key, value]
                    if *value == data && if variant.name == "Map" { *key == data }
                    else { matches!(store.get(*key), Some(Type::Primitive(P::String))) }))
            }
            ("Variant", [name, fields]) => matches!(store.get(*name), Some(Type::Primitive(P::String))) && array_of(*fields, data),
            ("Json", [ty]) => types.facts.known_std_types.json_value == Some(*ty),
            _ => false,
        }
    })
}

pub(super) fn context_signature_matches(
    role: etas_std::StdExecutionRole,
    types: &TypeOutput,
    item: HirItemId,
) -> bool {
    use etas_std::StdExecutionRole as R;
    let store = &types.store;
    let Some(etas_types::ItemSignature::Flow(signature)) = types.facts.item_signatures.get(&item)
    else {
        return false;
    };
    let Some(method) = types
        .facts
        .type_impl_methods
        .values()
        .find(|method| method.body == item)
    else {
        return false;
    };
    let Some(Type::Nominal(owner)) = store.get(method.self_type) else {
        return false;
    };
    if !method.inherent
        || !method.owner_params.is_empty()
        || !owner.params.is_empty()
        || role.associated_owner() != Some(owner.name.as_str())
        || !signature.generic_params.is_empty()
        || !signature.params.is_empty()
        || !signature
            .effects
            .as_ref()
            .is_some_and(|row| row.effects.is_empty() && row.tail.is_none())
        || signature
            .requested_actions
            .as_ref()
            .is_some_and(|row| !row.effects.is_empty() || row.tail.is_some())
    {
        return false;
    }
    if role == R::StreamClock {
        return etas_types::record_fields_with_applied_params(store, method.self_type)
            .ok()
            .flatten()
            .is_some_and(|record| record.fields.is_empty())
            && matches!(store.get(signature.output), Some(Type::Primitive(P::U128)));
    }
    let Some(fields) = etas_types::record_fields_with_applied_params(store, method.self_type)
        .ok()
        .flatten()
    else {
        return false;
    };
    let expected = [
        ("max_value_bytes", P::USize),
        ("max_result_bytes", P::USize),
        ("max_nodes", P::USize),
        ("max_depth", P::USize),
        ("max_scan_rows", P::USize),
        ("max_page_entries", P::USize),
        ("receipt_retention_seconds", P::U64),
        ("now_seconds", P::U64),
    ];
    if fields.fields.len() != expected.len()
        || !expected.into_iter().all(|(name, ty)| {
            fields.fields.iter().any(|field| {
                field.name == name
                    && matches!(store.get(field.ty), Some(Type::Primitive(actual)) if *actual == ty)
            })
        })
    {
        return false;
    }
    if role == R::StorageContext {
        return signature.output == method.self_type;
    }
    let Some((ok, err)) = etas_types::value_representation::result_parameters(
        store,
        &types.facts.known_std_types,
        signature.output,
    ) else {
        return false;
    };
    role == R::StorageNonce
        && matches!(store.get(ok), Some(Type::Primitive(P::U128)))
        && storage_error_layout_matches(types, err)
}

pub(super) fn resource_scope_signature_matches(types: &TypeOutput, item: HirItemId) -> bool {
    let Some(etas_types::ItemSignature::Flow(signature)) = types.facts.item_signatures.get(&item)
    else {
        return false;
    };
    let [result, effects] = signature.generic_params.as_slice() else {
        return false;
    };
    let [body_type] = signature.params.as_slice() else {
        return false;
    };
    let Some(Type::Function(body)) = types.store.get(*body_type) else {
        return false;
    };
    let Some(Type::Applied { constructor, args }) = types.store.get(signature.output) else {
        return false;
    };
    if !matches!(types.store.get(etas_types::TypeId(constructor.0)), Some(Type::Nominal(owner)) if matches!(owner.name.as_str(), "std.resource.ScopeReport" | "std.agent.session.LeaseScopeReport"))
        || args != &[result.subject]
    {
        return false;
    }
    let Ok(Some(fields)) =
        etas_types::record_fields_with_applied_params(&types.store, signature.output)
    else {
        return false;
    };
    result.kind == CallableGenericParamKind::Type
        && effects.kind == CallableGenericParamKind::Effect
        && result.bounds.is_empty()
        && effects.bounds.is_empty()
        && body.input.is_empty()
        && body.output == result.subject
        && body.effects.as_ref() == signature.effects.as_ref()
        && signature.effects.as_ref().is_some_and(|row| {
            row.effects.is_empty() && row.tail.as_deref() == Some(effects.name.as_str())
        })
        && signature
            .requested_actions
            .as_ref()
            .is_none_or(|row| row.effects.is_empty() && row.tail.is_none())
        && fields.fields.len() == 2
        && fields
            .fields
            .iter()
            .any(|field| field.name == "value" && field.ty == result.subject)
        && fields.fields.iter().any(|field| field.name == "failures")
}

pub(super) fn scope_signature_matches(
    hir: &etas_hir::HirProgram,
    types: &TypeOutput,
    item: HirItemId,
) -> bool {
    let Some(etas_types::ItemSignature::Flow(signature)) = types.facts.item_signatures.get(&item)
    else {
        return false;
    };
    let [result, effects] = signature.generic_params.as_slice() else {
        return false;
    };
    if result.kind != CallableGenericParamKind::Type
        || effects.kind != CallableGenericParamKind::Effect
        || !result.bounds.is_empty()
        || !effects.bounds.is_empty()
        || signature.params.len() != 3
        || signature.output != result.subject
        || !matches!(
            types.store.get(signature.params[2]),
            Some(Type::Primitive(P::Bool))
        )
        || signature.effects.as_ref().is_none_or(|row| {
            !row.effects.is_empty() || row.tail.as_deref() != Some(effects.name.as_str())
        })
        || signature
            .requested_actions
            .as_ref()
            .is_some_and(|row| !row.effects.is_empty() || row.tail.is_some())
    {
        return false;
    }
    let Some(Type::Function(body)) = types.store.get(signature.params[1]) else {
        return false;
    };
    if body.input.len() != 1
        || !etas_types::source_execution::is_scheduler(
            &types.store,
            &types.facts.known_std_types,
            body.input[0],
        )
        || body.output != result.subject
        || body.effects.as_ref() != signature.effects.as_ref()
    {
        return false;
    }
    let Some(Type::Existential { bounds }) = types.store.get(signature.params[0]) else {
        return false;
    };
    let [bound] = bounds.as_slice() else {
        return false;
    };
    let etas_types::CheckedSpecRef::Source(symbol) = bound.spec else {
        return false;
    };
    let symbol = hir.canonical_source_symbol(symbol).unwrap_or(symbol);
    let Some(declaration) = hir.symbols.get(symbol) else {
        return false;
    };
    let module = &hir.modules_arena[declaration.defining_module];
    if !bound.args.is_empty()
        || declaration.name != "Scheduler"
        || module.name.as_ref().is_none_or(|path| {
            !path
                .segments
                .iter()
                .map(|part| part.name.as_str())
                .eq(["std", "task", "scheduler"])
        })
    {
        return false;
    }
    let Some(spec) = types.facts.spec_signatures.get(&symbol) else {
        return false;
    };
    let [method] = spec.methods.as_slice() else {
        return false;
    };
    let Some(select) = method.signature_in(&types.facts) else {
        return false;
    };
    if !spec.params.is_empty()
        || !spec.super_specs.is_empty()
        || method.name != "select"
        || !method.updating_receiver
        || method.receiver_param.is_none()
        || select.params.len() != 2
        || !matches!(
            types.store.get(select.output),
            Some(Type::Primitive(P::USize))
        )
        || select
            .effects
            .as_ref()
            .is_none_or(|row| !row.effects.is_empty() || row.tail.is_some())
    {
        return false;
    }
    let Some(Type::Array(ready)) = types.store.get(select.params[1]) else {
        return false;
    };
    let Ok(Some(fields)) = etas_types::record_fields_with_applied_params(&types.store, *ready)
    else {
        return false;
    };
    fields.fields.len() == 2
        && ["id", "enqueue_order"].iter().all(|name| {
            fields.fields.iter().any(|field| {
                field.name == *name
                    && matches!(types.store.get(field.ty), Some(Type::Primitive(P::U64)))
            })
        })
}

// Source owns the record; these are the fields that the host error ABI emits.
fn storage_error_layout_matches(types: &TypeOutput, ty: etas_types::TypeId) -> bool {
    let store = &types.store;
    if !matches!(store.get(ty), Some(Type::Nominal(owner))
        if owner.name == "std.memory.StorageError" && owner.params.is_empty())
    {
        return false;
    }
    let Ok(Some(fields)) = etas_types::record_fields_with_applied_params(store, ty) else {
        return false;
    };
    if fields.fields.len() != 3
        || !["code", "message"].into_iter().all(|name| {
            fields.fields.iter().any(|field| {
                field.name == name
                    && matches!(store.get(field.ty), Some(Type::Primitive(P::String)))
            })
        })
    {
        return false;
    }
    let Some(operation) = fields
        .fields
        .iter()
        .find(|field| field.name == "operation")
        .and_then(|field| {
            etas_types::value_representation::option_parameter(
                store,
                &types.facts.known_std_types,
                field.ty,
            )
        })
    else {
        return false;
    };
    if !matches!(store.get(operation), Some(Type::Nominal(owner))
        if owner.name == "std.memory.StorageOperationRef" && owner.params.is_empty())
    {
        return false;
    }
    let Ok(Some(fields)) = etas_types::record_fields_with_applied_params(store, operation) else {
        return false;
    };
    fields.fields.len() == 2
        && ["key", "fingerprint"].into_iter().all(|name| {
            fields.fields.iter().any(|field| {
                field.name == name
                    && matches!(store.get(field.ty), Some(Type::Primitive(P::String)))
            })
        })
}
