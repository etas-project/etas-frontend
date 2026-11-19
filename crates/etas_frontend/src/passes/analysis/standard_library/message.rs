//! Checked data consumed or emitted by message execution primitives.
use etas_hir::HirItemId;
use etas_std::StdExecutionRole as R;
use etas_types::{CallableGenericParamKind, PrimitiveType as P, Type, TypeId, TypeOutput};

pub(super) fn signature_matches(types: &TypeOutput, item: HirItemId, role: R) -> bool {
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
    if role == R::PayloadConvert {
        let [source, target] = signature.generic_params.as_slice() else {
            return false;
        };
        return method.inherent
            && method.owner_params.is_empty()
            && named(
                types,
                method.self_type,
                "std.agent.message.PayloadRepresentation",
            )
            && fields(types, method.self_type).is_some_and(|record| record.fields.is_empty())
            && [source, target].iter().all(|param| {
                param.kind == CallableGenericParamKind::Type && param.bounds.is_empty()
            })
            && signature.params.as_slice() == [source.subject]
            && option(types, signature.output) == Some(target.subject)
            && signature
                .effects
                .as_ref()
                .is_some_and(|row| row.effects.is_empty() && row.tail.is_none())
            && signature
                .requested_actions
                .as_ref()
                .is_none_or(|row| row.effects.is_empty() && row.tail.is_none());
    }
    if !method.inherent
        || !method.owner_params.is_empty()
        || !named(types, method.self_type, "std.agent.message.MessageContext")
        || signature
            .effects
            .as_ref()
            .is_none_or(|row| !row.effects.is_empty() || row.tail.is_some())
        || signature
            .requested_actions
            .as_ref()
            .is_some_and(|row| !row.effects.is_empty() || row.tail.is_some())
        || !context_layout(types, method.self_type)
    {
        return false;
    }
    if role == R::MessageContextCapture {
        return signature.params.is_empty()
            && signature.generic_params.is_empty()
            && signature.output == method.self_type;
    }
    let [payload] = signature.generic_params.as_slice() else {
        return false;
    };
    let [context, message] = signature.params.as_slice() else {
        return false;
    };
    role == R::MessageContextRecord
        && *context == method.self_type
        && payload.kind == CallableGenericParamKind::Type
        && payload.bounds.is_empty()
        && primitive(types, signature.output, P::Unit)
        && matches!(types.store.get(*message), Some(Type::Applied { constructor, args })
            if matches!(types.store.get(TypeId(constructor.0)), Some(Type::Nominal(owner))
                if owner.name == "std.agent.message.Message" && owner.params.len() == 1)
                && args.as_slice() == [payload.subject])
        && message_layout(types, *message, payload.subject)
}

fn context_layout(types: &TypeOutput, ty: TypeId) -> bool {
    let Some(record) = fields(types, ty) else {
        return false;
    };
    record.fields.len() == 4
        && record.fields.iter().all(|field| match field.name.as_str() {
            "sequence" => primitive(types, field.ty, P::U64),
            "session" => optional_string(types, field.ty, "std.agent.session.SessionId"),
            "created_at" => time(types, field.ty),
            "trace" => optional_string(types, field.ty, "std.agent.message.TraceId"),
            _ => false,
        })
}

fn message_layout(types: &TypeOutput, ty: TypeId, payload: TypeId) -> bool {
    let Some(record) = fields(types, ty) else {
        return false;
    };
    record.fields.len() == 8 && record.fields.iter().all(|field| match field.name.as_str() {
        "body" | "content" => field.ty == payload,
        "id" => nominal_string(types, field.ty, "std.agent.message.MessageId"),
        "from" => optional_string(types, field.ty, "std.agent.message.Participant"),
        "session" => optional_string(types, field.ty, "std.agent.session.SessionId"),
        "created_at" => time(types, field.ty),
        "role" => matches!(types.store.get(field.ty), Some(Type::Enum(value)) if value.name == "std.agent.message.Role")
            && types.facts.enum_layouts.get(&field.ty).is_some_and(|layout| {
                layout.type_params.is_empty() && layout.variants.len() == 4
                    && ["System", "User", "Assistant", "Tool"].into_iter().all(|name|
                        layout.variants.iter().any(|variant| variant.name == name
                            && variant.fields.is_empty() && variant.field_names.is_none()))
            }),
        "provenance" => option(types, field.ty).is_some_and(|ty| {
            named(types, ty, "std.agent.message.Provenance")
                && fields(types, ty).is_some_and(|fields| fields.fields.len() == 2
                    && fields.fields.iter().all(|field| match field.name.as_str() {
                        "trace_id" => optional_string(types, field.ty, "std.agent.message.TraceId"),
                        "source" => option(types, field.ty).is_some_and(|ty| primitive(types, ty, P::String)),
                        _ => false,
                    }))
        }),
        _ => false,
    })
}

fn time(types: &TypeOutput, ty: TypeId) -> bool {
    named(types, ty, "std.runtime.time.Time") && fields(types, ty).is_some_and(|record|
        matches!(record.fields.as_slice(), [field] if field.name == "unix_millis" && primitive(types, field.ty, P::U128)))
}

fn fields(types: &TypeOutput, ty: TypeId) -> Option<etas_types::RecordType> {
    etas_types::record_fields_with_applied_params(&types.store, ty)
        .ok()
        .flatten()
}

fn option(types: &TypeOutput, ty: TypeId) -> Option<TypeId> {
    etas_types::value_representation::option_parameter(
        &types.store,
        &types.facts.known_std_types,
        ty,
    )
}

fn optional_string(types: &TypeOutput, ty: TypeId, name: &str) -> bool {
    option(types, ty).is_some_and(|ty| nominal_string(types, ty, name))
}

fn nominal_string(types: &TypeOutput, ty: TypeId, name: &str) -> bool {
    named(types, ty, name)
        && matches!(types.store.get(ty), Some(Type::Nominal(value))
        if value.representation.is_some_and(|ty| primitive(types, ty, P::String)))
}

fn named(types: &TypeOutput, ty: TypeId, name: &str) -> bool {
    matches!(types.store.get(ty), Some(Type::Nominal(value)) if value.name == name && value.params.is_empty())
}

fn primitive(types: &TypeOutput, ty: TypeId, expected: P) -> bool {
    matches!(types.store.get(ty), Some(Type::Primitive(actual)) if *actual == expected)
}
