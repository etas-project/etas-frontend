use etas_hir::{HirArg, HirExpr, HirGenericArg, HirLiteral, ResolveResult, SymbolDef};
use etas_std::{FlowDecl, StdDecl, StdSymbol, StdType};

use crate::{
    ResourceHandleFact, ResourceHandleType, Type, TypeId, lower::type_ref::lower_type_ref,
    pipeline::context::TypePipelineContext,
};

pub fn top_level_resource_handle_signature(
    ctx: &mut TypePipelineContext<'_>,
    expr: etas_hir::HirExprId,
) -> Option<(TypeId, ResourceHandleFact)> {
    let HirExpr::Call {
        callee,
        generic_args,
        ..
    } = &ctx.hir.exprs[expr]
    else {
        return None;
    };
    let HirExpr::Path(path) = &ctx.hir.exprs[*callee] else {
        return None;
    };
    let path_segments = path
        .segments
        .iter()
        .map(|segment| segment.name.clone())
        .collect::<Vec<_>>();
    let registry = ctx.std_registry.clone();
    let std_symbol = registry.lookup_qualified(&path_segments)?;
    if !is_checked_std_path(ctx, &path_segments, &path.resolution, std_symbol) {
        return None;
    }
    let StdDecl::Flow(flow) = &std_symbol.decl else {
        return None;
    };
    match &flow.output {
        StdType::ResourceHandleMemoryRegion(schema) => {
            let schema = lower_std_output_type_arg(ctx, flow, schema, generic_args)?;
            let ty = ctx
                .interner
                .intern(Type::ResourceHandle(ResourceHandleType::MemoryRegion {
                    schema,
                }));
            let stable_id = named_string_arg(ctx, expr, "stable_id")?;
            Some((ty, ResourceHandleFact::MemoryRegion { schema, stable_id }))
        }
        _ => None,
    }
}

fn is_checked_std_path(
    ctx: &TypePipelineContext<'_>,
    path: &[String],
    resolution: &ResolveResult,
    std_symbol: &StdSymbol,
) -> bool {
    if path != std_symbol.qualified_path {
        return false;
    }
    match resolution {
        ResolveResult::Resolved(symbol) => ctx.hir.symbols.get(*symbol).is_some_and(|symbol| {
            matches!(
                &symbol.def,
                SymbolDef::ImportAlias { path: alias_path, .. }
                    if alias_path == &std_symbol.qualified_path
            )
        }),
        _ => false,
    }
}

fn lower_std_output_type_arg(
    ctx: &mut TypePipelineContext<'_>,
    flow: &FlowDecl,
    ty: &StdType,
    generic_args: &[HirGenericArg],
) -> Option<TypeId> {
    match ty {
        StdType::Var(name) => {
            let index = flow
                .type_params
                .iter()
                .position(|param| param.name == *name)?;
            let HirGenericArg::Type(ty) = generic_args.get(index)? else {
                return None;
            };
            lower_type_ref(ctx, *ty)
        }
        _ => None,
    }
}

fn named_string_arg(
    ctx: &TypePipelineContext<'_>,
    expr: etas_hir::HirExprId,
    expected_name: &str,
) -> Option<String> {
    let HirExpr::Call { args, .. } = &ctx.hir.exprs[expr] else {
        return None;
    };
    args.iter().find_map(|arg| match arg {
        HirArg::Named { name, value, .. } if name == expected_name => {
            match &ctx.hir.exprs[*value] {
                HirExpr::Literal(HirLiteral::String { value, .. }) => Some(value.clone()),
                _ => None,
            }
        }
        _ => None,
    })
}
