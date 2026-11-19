use etas_hir::{HirArg, HirExpr, HirGenericArg, HirLiteral, ResolveResult, SymbolDef};
use etas_std::{FlowDecl, StdDecl, StdType};

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
    let registry = ctx.std_registry.clone();
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return None;
    };
    let SymbolDef::ImportAlias { path, .. } = &ctx.hir.symbols.get(symbol)?.def else {
        return None;
    };
    let std_symbol = registry.lookup_qualified(path)?;
    let StdDecl::Flow(flow) = &std_symbol.decl else {
        return None;
    };
    let schema = match &flow.output {
        StdType::ResourceHandleMemoryRegion(schema) => schema.as_ref(),
        StdType::NamedApplied { name, args }
            if name == "std.memory.MemoryRegion"
                && std_symbol.qualified_path == ["std", "memory", "region"] =>
        {
            let [schema] = args.as_slice() else {
                return None;
            };
            schema
        }
        _ => return None,
    };
    let schema = lower_std_output_type_arg(ctx, flow, schema, generic_args)?;
    let ty = ctx
        .interner
        .intern(Type::ResourceHandle(ResourceHandleType::MemoryRegion {
            schema,
        }));
    let stable_id = stable_id_arg(ctx, expr)?;
    Some((ty, ResourceHandleFact::MemoryRegion { schema, stable_id }))
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

fn stable_id_arg(ctx: &TypePipelineContext<'_>, expr: etas_hir::HirExprId) -> Option<String> {
    let HirExpr::Call { args, .. } = &ctx.hir.exprs[expr] else {
        return None;
    };
    let value = args
        .iter()
        .find_map(|arg| match arg {
            HirArg::Named { name, value, .. } if name == "stable_id" => Some(*value),
            _ => None,
        })
        .or_else(|| match args.first() {
            Some(HirArg::Positional(value)) => Some(*value),
            _ => None,
        })?;
    match &ctx.hir.exprs[value] {
        HirExpr::Literal(HirLiteral::String { value, .. }) => Some(value.clone()),
        _ => None,
    }
}
