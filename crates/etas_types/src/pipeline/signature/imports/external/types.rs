use std::collections::{BTreeMap, BTreeSet};

use crate::{
    ExternalSignatureInput, ExternalTypeInput as Ty, pipeline::context::TypePipelineContext,
};

use super::{
    incomplete_external_type_facts, lower_external_type_declaration, package_binding_span,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Visit {
    Active,
    Done,
}

pub(super) fn prepare(ctx: &mut TypePipelineContext<'_>, input: &ExternalSignatureInput) -> bool {
    let mut aliases = BTreeMap::new();
    for metadata in &input.metadata {
        let Some(span) = package_binding_span(input, metadata.package) else {
            continue;
        };
        for item in &metadata.enums {
            if let Some(ty @ Ty::Enum { .. }) = &item.ty {
                crate::lower::external::lower_external_type(ctx, span, ty);
            } else {
                incomplete_external_type_facts(
                    ctx,
                    span,
                    "external enum has no checked variant layout".into(),
                );
                return false;
            }
        }
    }
    // Register all nominal identities before resolving any representation.
    for metadata in &input.metadata {
        let Some(span) = package_binding_span(input, metadata.package) else {
            continue;
        };
        for item in &metadata.types {
            if matches!(item.ty, Some(Ty::Alias { .. })) {
                aliases.insert(item.path.clone(), (span, item));
            } else {
                lower_external_type_declaration(ctx, span, &item.path, &item.generic_params, None);
            }
        }
    }
    let mut visited = BTreeMap::new();
    for root in aliases.keys() {
        let mut pending = vec![(root.clone(), false)];
        while let Some((path, exit)) = pending.pop() {
            let (span, item) = aliases[&path];
            if exit {
                lower_external_type_declaration(
                    ctx,
                    span,
                    &path,
                    &item.generic_params,
                    item.ty.as_ref(),
                );
                visited.insert(path, Visit::Done);
                continue;
            }
            match visited.get(&path) {
                Some(Visit::Done) => continue,
                Some(Visit::Active) => {
                    incomplete_external_type_facts(
                        ctx,
                        span,
                        format!("recursive external alias `{}`", path.join(".")),
                    );
                    return false;
                }
                None => {}
            }
            visited.insert(path.clone(), Visit::Active);
            pending.push((path, true));
            if let Some(ty) = &item.ty {
                for dependency in references(ty).into_iter().rev() {
                    if aliases.contains_key(&dependency) {
                        pending.push((dependency, false));
                    }
                }
            }
        }
    }
    true
}

fn references(root: &Ty) -> BTreeSet<Vec<String>> {
    let mut result = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(ty) = pending.pop() {
        match ty {
            Ty::Named(path) => {
                result.insert(path.clone());
            }
            Ty::Applied { path, args } => {
                result.insert(path.clone());
                pending.extend(args);
            }
            Ty::Alias { target, .. }
            | Ty::Array(target)
            | Ty::List(target)
            | Ty::Set(target)
            | Ty::Range(target)
            | Ty::Slice(target)
            | Ty::Option(target)
            | Ty::Message(target)
            | Ty::MemorySelection(target)
            | Ty::MemoryRegion(target)
            | Ty::Trust { inner: target, .. } => pending.push(target),
            Ty::Nominal { representation, .. } => pending.extend(representation.as_deref()),
            Ty::Map { key, value }
            | Ty::Store { key, value }
            | Ty::Result {
                ok: key,
                err: value,
            } => {
                pending.push(key);
                pending.push(value);
            }
            Ty::Record { fields }
            | Ty::Enum {
                variants: fields, ..
            } => pending.extend(fields.iter().map(|field| &field.ty)),
            Ty::Tuple(elements) | Ty::ResourceHandle { args: elements, .. } => {
                pending.extend(elements)
            }
            Ty::Function {
                input,
                output,
                effects,
            } => {
                pending.extend(input);
                pending.push(output);
                if let Some(row) = effects {
                    row_types(row, &mut pending);
                }
            }
            Ty::Handler {
                handled,
                produced,
                result,
            } => {
                row_types(handled, &mut pending);
                if let Some(row) = produced {
                    row_types(row, &mut pending);
                }
                pending.extend(result.as_deref());
            }
            Ty::Primitive(_) | Ty::Var(_) | Ty::Prompt | Ty::PromptPart => {}
        }
    }
    result
}

fn row_types<'a>(row: &'a crate::ExternalEffectRowInput, pending: &mut Vec<&'a Ty>) {
    for effect in &row.effects {
        for arg in &effect.args {
            if let crate::ExternalEffectArgInput::Type(ty) = arg {
                pending.push(ty);
            }
        }
    }
}
