use super::{PrimitiveType, Type, TypeId, TypeStore};

pub fn display_type(store: &TypeStore, ty: TypeId) -> String {
    match store.get(ty) {
        Some(Type::Existential { bounds }) => {
            let bounds = bounds
                .iter()
                .map(|bound| {
                    let name = match &bound.spec {
                        crate::CheckedSpecRef::Source(symbol) => format!("spec#{}", symbol.0),
                        crate::CheckedSpecRef::Std(path) => path.join("."),
                        crate::CheckedSpecRef::External(identity) => identity.path.join("."),
                        crate::CheckedSpecRef::Canonical(identity) => {
                            identity.declaration().join(".")
                        }
                    };
                    if bound.args.is_empty() {
                        name
                    } else {
                        let args = bound
                            .args
                            .iter()
                            .map(|arg| display_type(store, *arg))
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("{name}<{args}>")
                    }
                })
                .collect::<Vec<_>>()
                .join(" + ");
            format!("? ~ {bounds}")
        }
        Some(Type::Primitive(primitive)) => primitive.source_name().to_owned(),
        Some(Type::IntegerLiteral { .. }) => "i32".to_owned(),
        Some(Type::Var(var)) => format!("T{}", var.0),
        Some(Type::Array(inner)) => format!("Array<{}>", display_type(store, *inner)),
        Some(Type::List(inner)) => format!("List<{}>", display_type(store, *inner)),
        Some(Type::Map { key, value }) => {
            format!(
                "Map<{}, {}>",
                display_type(store, *key),
                display_type(store, *value)
            )
        }
        Some(Type::Set(inner)) => format!("Set<{}>", display_type(store, *inner)),
        Some(Type::Range { index }) => format!("Range<{}>", display_type(store, *index)),
        Some(Type::Slice(inner)) => format!("Slice<{}>", display_type(store, *inner)),
        Some(Type::Option(inner)) => format!("Option<{}>", display_type(store, *inner)),
        Some(Type::Result { ok, err }) => {
            format!(
                "Result<{}, {}>",
                display_type(store, *ok),
                display_type(store, *err)
            )
        }
        Some(Type::Named(named)) => named.name.clone(),
        Some(Type::Nominal(nominal)) => nominal.name.clone(),
        Some(Type::Enum(enumeration)) => enumeration.name.clone(),
        Some(Type::Applied { constructor, args }) => {
            let args = args
                .iter()
                .map(|arg| display_type(store, *arg))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}<{args}>", display_type(store, TypeId(constructor.0)))
        }
        Some(Type::Prompt) => "Prompt".to_owned(),
        Some(Type::PromptPart) => "PromptPart".to_owned(),
        Some(Type::Message(inner)) => format!("Message<{}>", display_type(store, *inner)),
        Some(Type::MemorySelection(inner)) => {
            format!("MemorySelection<{}>", display_type(store, *inner))
        }
        Some(Type::Store { key, value }) => {
            format!(
                "Store<{}, {}>",
                display_type(store, *key),
                display_type(store, *value)
            )
        }
        Some(Type::MemoryRegion(inner)) => format!("MemoryRegion<{}>", display_type(store, *inner)),
        Some(Type::ResourceHandle(super::ResourceHandleType::MemoryRegion { schema })) => {
            format!(
                "ResourceHandle<MemoryRegion<{}>>",
                display_type(store, *schema)
            )
        }
        Some(Type::ResourceHandle(super::ResourceHandleType::ExternalTool { signature })) => {
            format!(
                "ResourceHandle<ExternalTool<{}>>",
                display_type(store, *signature)
            )
        }
        Some(Type::ResourceHandle(super::ResourceHandleType::Other { name, args })) => {
            let args = args
                .iter()
                .map(|arg| display_type(store, *arg))
                .collect::<Vec<_>>()
                .join(", ");
            format!("ResourceHandle<{name}<{args}>>")
        }
        Some(Type::Function(_)) => "function".to_owned(),
        Some(Type::Handler(_)) => "handler".to_owned(),
        Some(Type::Task(task)) => {
            let effects = task
                .effects
                .effects
                .iter()
                .map(|effect| effect.name.as_str())
                .chain(task.effects.tail.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(", ");
            format!("Task<{}, ![{effects}]>", display_type(store, task.output))
        }
        Some(Type::SchedulerHandle) => "Scheduler".to_owned(),
        Some(Type::Continuation(continuation)) => format!(
            "Continuation<{}>",
            display_type(store, continuation.action_output)
        ),
        Some(Type::Record(record)) => {
            let fields = record
                .fields
                .iter()
                .map(|field| format!("{}: {}", field.name, display_type(store, field.ty)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{fields}}}")
        }
        Some(Type::Tuple(elements)) => {
            let elements = elements
                .iter()
                .map(|element| display_type(store, *element))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({elements})")
        }
        Some(_) => "<type>".to_owned(),
        None => "<unknown>".to_owned(),
    }
}

#[allow(dead_code)]
fn _primitive_name(primitive: PrimitiveType) -> &'static str {
    primitive.source_name()
}
