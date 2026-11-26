use super::super::state::BodyPipelineState;
use crate::{Type, TypeId, pipeline::context::TypePipelineContext};
use etas_core::{Diagnostic, Span, TypeDiagnosticCode};

pub(super) fn validate(
    ctx: &mut TypePipelineContext<'_>,
    state: &BodyPipelineState,
    ty: TypeId,
    span: Span,
) {
    let checked = crate::substitute_type_params(
        &mut ctx.interner,
        ty,
        &Default::default(),
        &state.solver_report.substitutions,
    )
    .and_then(|ty| {
        crate::applied_representation(&mut ctx.interner, ty)
            .map(|representation| (ty, representation.unwrap_or(ty)))
    });
    let (ty, representation) = match checked {
        Ok(types) => types,
        Err(error) => {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                format!("record update type could not be resolved: {error}"),
            ));
            return;
        }
    };
    if !matches!(
        ctx.interner.store().get(representation),
        Some(Type::Record(_))
    ) || ctx.agent_signature(ty).is_some()
    {
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::TypeMismatch,
            span,
            "record update requires an ordinary record value",
        ));
        return;
    }
    let owner = match ctx.interner.store().get(ty) {
        Some(Type::Applied { constructor, .. }) => TypeId(constructor.0),
        _ => ty,
    };
    if ctx
        .signature_facts
        .known_std_types
        .intrinsic_only_constructors
        .contains(&owner)
    {
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::TypeMismatch,
            span,
            "this standard type can only be created by its registered backend operations",
        ));
        return;
    }
    let Some(Type::Nominal(nominal)) = ctx.interner.store().get(owner) else {
        return;
    };
    let path = nominal
        .name
        .split('.')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let source = ctx
        .symbols
        .canonical_path(ctx.hir, &path)
        .and_then(|symbol| ctx.symbols.canonical_symbol(ctx.hir, symbol))
        .and_then(|symbol| ctx.hir.symbols.get(symbol));
    let source_private = source.and_then(|symbol| {
        let etas_hir::SymbolDef::Item { item } = symbol.def else {
            return None;
        };
        let etas_hir::HirItem::Type(declaration) = &ctx.hir.items[item] else {
            return None;
        };
        let etas_hir::HirTypeDeclBody::Representation(representation) = declaration.body else {
            return None;
        };
        let etas_hir::HirType::Record { fields, .. } = &ctx.hir.types[representation] else {
            return None;
        };
        Some(
            fields.iter().any(|field| !field.is_public())
                && ctx.hir.item_module(state.item) != Some(symbol.defining_module),
        )
    });
    if source_private.unwrap_or_else(|| ctx.hir.private_record_fields.contains_key(&path)) {
        ctx.diagnostics.push(Diagnostic::type_check(TypeDiagnosticCode::TypeMismatch, span,
            "record update requires reconstruction access to every field, including retained private fields"));
    }
}
