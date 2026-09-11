use std::collections::HashSet;

use etas_types::{
    EffectArgRef, EffectRowRef, HandlerProducedEffects, ResourceHandleType, Type, TypeId, TypeStore,
};

use super::PackageMetadataError;

/// The current package representation codec is a tree, unlike the checked type graph.
/// Reject cycles before entering that codec; never expand a recursive identity indefinitely.
pub(super) fn require_tree_representation(
    root: TypeId,
    store: &TypeStore,
) -> Result<(), PackageMetadataError> {
    let mut pending = vec![(root, false)];
    let mut active = HashSet::new();
    let mut complete = HashSet::new();
    while let Some((ty, leaving)) = pending.pop() {
        if leaving {
            active.remove(&ty);
            complete.insert(ty);
            continue;
        }
        if complete.contains(&ty) {
            continue;
        }
        if !active.insert(ty) {
            return Err(PackageMetadataError::UnsupportedType { ty, reason: "recursive nominal representation requires reference-aware package type metadata".into() });
        }
        let data = store
            .get(ty)
            .ok_or_else(|| PackageMetadataError::UnsupportedType {
                ty,
                reason: "missing checked type in package type graph".into(),
            })?;
        pending.push((ty, true));
        pending.extend(children(data).into_iter().map(|child| (child, false)));
    }
    Ok(())
}

fn row_children(row: &EffectRowRef, out: &mut Vec<TypeId>) {
    out.extend(
        row.effects
            .iter()
            .flat_map(|effect| effect.args.iter())
            .filter_map(|arg| {
                if let EffectArgRef::Type(ty) = arg {
                    Some(*ty)
                } else {
                    None
                }
            }),
    );
}

fn children(ty: &Type) -> Vec<TypeId> {
    match ty {
        Type::Array(inner)
        | Type::List(inner)
        | Type::Set(inner)
        | Type::Slice(inner)
        | Type::Option(inner)
        | Type::Range { index: inner }
        | Type::Trust { inner, .. }
        | Type::Message(inner)
        | Type::MemorySelection(inner)
        | Type::MemoryRegion(inner) => vec![*inner],
        Type::Map { key, value } | Type::Store { key, value } => vec![*key, *value],
        Type::Result { ok, err } => vec![*ok, *err],
        Type::Record(record) => record.fields.iter().map(|field| field.ty).collect(),
        Type::Tuple(elems) | Type::Applied { args: elems, .. } => elems.clone(),
        Type::Nominal(nominal) => nominal.representation.into_iter().collect(),
        Type::Function(flow) => {
            let mut out = flow.input.clone();
            out.push(flow.output);
            if let Some(row) = &flow.effects {
                row_children(row, &mut out);
            }
            out
        }
        Type::Handler(handler) => {
            let mut out = handler.result.into_iter().collect();
            row_children(&handler.handled, &mut out);
            if let HandlerProducedEffects::Explicit(row) = &handler.produced {
                row_children(row, &mut out);
            }
            out
        }
        Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => vec![*schema],
        Type::ResourceHandle(ResourceHandleType::ExternalTool { signature }) => vec![*signature],
        Type::ResourceHandle(ResourceHandleType::Other { args, .. }) => args.clone(),
        Type::Enum(_)
        | Type::Named(_)
        | Type::Primitive(_)
        | Type::IntegerLiteral { .. }
        | Type::Var(_)
        | Type::Prompt
        | Type::PromptPart
        | Type::MemoryPlace(_)
        | Type::Refined { .. }
        | Type::Schema(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursive_record_metadata_stops_at_the_explicit_codec_boundary() {
        let output = crate::Frontend.check_project(crate::ProjectInput::single_source(
            crate::SourceInput::anonymous(
                "module app; type Node = { next: Option<Node> } flow main() -> unit { return; }",
            ),
        ));
        let checked = output.checked.expect("recursive source is valid");
        let ty = checked
            .type_store
            .iter()
            .find_map(|(id, ty)| {
                matches!(ty, Type::Nominal(n) if n.name == "app.Node").then_some(id)
            })
            .unwrap();
        let error = require_tree_representation(ty, &checked.type_store).unwrap_err();
        assert!(error.to_string().contains("reference-aware"));
    }
}
