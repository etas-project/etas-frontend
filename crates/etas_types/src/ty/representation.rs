use std::collections::HashSet;

use super::enum_payload::EnumPayloadExpansion;
use super::{Type, TypeId, TypeInterner, TypeSubstitutionError, applied_representation};

// Expansion is a checked resource limit, not a recursion fallback. An artifact
// that exceeds it must not publish partially materialized runtime type facts.
pub(super) const MAX_REPRESENTATION_TYPES: usize = 100_000;

pub fn materialize_representations(
    interner: &mut TypeInterner,
    roots: impl IntoIterator<Item = TypeId>,
) -> Result<(), TypeSubstitutionError> {
    TypeRepresentationMaterializer::default().materialize(interner, roots)
}

#[derive(Default)]
pub struct TypeRepresentationMaterializer<'a> {
    visited: HashSet<TypeId>,
    enum_payloads: Option<super::enum_payload::AcyclicEnumPayloads<'a>>,
}

impl<'a> TypeRepresentationMaterializer<'a> {
    pub fn with_acyclic_enum_payloads(
        store: &super::TypeStore,
        layouts: &'a std::collections::HashMap<TypeId, crate::EnumLayoutFact>,
    ) -> Result<Self, TypeSubstitutionError> {
        Ok(Self {
            enum_payloads: Some(super::enum_payload::AcyclicEnumPayloads::new(
                store, layouts,
            )?),
            ..Self::default()
        })
    }

    pub fn materialize(
        &mut self,
        interner: &mut TypeInterner,
        roots: impl IntoIterator<Item = TypeId>,
    ) -> Result<(), TypeSubstitutionError> {
        let mut pending: Vec<_> = roots.into_iter().collect();
        while let Some(ty) = pending.pop() {
            if !self.visited.insert(ty) {
                continue;
            }
            if self.visited.len() > MAX_REPRESENTATION_TYPES {
                return Err(TypeSubstitutionError::RepresentationLimit(
                    MAX_REPRESENTATION_TYPES,
                ));
            }
            let data = interner
                .store()
                .get(ty)
                .cloned()
                .ok_or(TypeSubstitutionError::MissingType(ty))?;
            match data {
                Type::Applied { constructor, args } => {
                    let owner = TypeId(constructor.0);
                    if matches!(interner.store().get(owner), Some(Type::Enum(_)))
                        && let Some(payloads) = &self.enum_payloads
                    {
                        enqueue_enum_payloads(
                            &mut pending,
                            payloads.expand(interner, owner, &args, true)?,
                        );
                    }
                    pending.extend(args);
                    pending.extend(applied_representation(interner, ty)?);
                }
                Type::Enum(_) => {
                    if let Some(payloads) = &self.enum_payloads {
                        enqueue_enum_payloads(
                            &mut pending,
                            payloads.expand(interner, ty, &[], false)?,
                        );
                    }
                }
                Type::Nominal(_) => {
                    pending.extend(applied_representation(interner, ty)?);
                }
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
                | Type::Refined { base: inner, .. } => pending.push(inner),
                Type::Map { key, value }
                | Type::Store { key, value }
                | Type::Result {
                    ok: key,
                    err: value,
                } => pending.extend([key, value]),
                Type::Tuple(items) => pending.extend(items),
                Type::Record(record) => {
                    pending.extend(record.fields.into_iter().map(|field| field.ty))
                }
                Type::Function(flow) => {
                    pending.extend(flow.input);
                    pending.push(flow.output);
                    if let Some(row) = flow.effects {
                        enqueue_row(&mut pending, row);
                    }
                }
                Type::Handler(handler) => {
                    pending.extend(handler.result);
                    enqueue_row(&mut pending, handler.handled);
                    if let super::HandlerProducedEffects::Explicit(row) = handler.produced {
                        enqueue_row(&mut pending, row);
                    }
                }
                Type::ResourceHandle(super::ResourceHandleType::MemoryRegion { schema }) => {
                    pending.push(schema)
                }
                Type::ResourceHandle(super::ResourceHandleType::ExternalTool { signature }) => {
                    pending.push(signature)
                }
                Type::ResourceHandle(super::ResourceHandleType::Other { args, .. }) => {
                    pending.extend(args)
                }
                Type::Primitive(_)
                | Type::IntegerLiteral { .. }
                | Type::Var(_)
                | Type::Named(_)
                | Type::Prompt
                | Type::PromptPart
                | Type::MemoryPlace(_) => {}
            }
        }
        Ok(())
    }
}

fn enqueue_enum_payloads(pending: &mut Vec<TypeId>, expansion: EnumPayloadExpansion) {
    match expansion {
        EnumPayloadExpansion::Concrete(fields) => pending.extend(fields),
        // These are declaration-level layouts, not empty runtime payloads.
        EnumPayloadExpansion::GenericDeclaration | EnumPayloadExpansion::RecursiveTemplate => {}
    }
}

fn enqueue_row(pending: &mut Vec<TypeId>, row: super::EffectRowRef) {
    for effect in row.effects {
        for arg in effect.args {
            if let super::EffectArgRef::Type(ty) = arg {
                pending.push(ty);
            }
        }
    }
}
