//! Trusted std native declarations become ordinary checked calls to the
//! primitive ABI. The source annotation and the typed call survive in the image.
use crate::StandardLibraryMode;
use etas_core::{Diagnostic, Span, TypeDiagnosticCode};
use etas_hir::*;
use etas_std::{StdRegistry, StdSymbolId};

pub(crate) fn declarations(
    hir: &HirProgram,
    registry: &StdRegistry,
) -> Vec<(HirItemId, StdSymbolId, Span)> {
    let mut result = Vec::new();
    for (item, annotations) in &hir.item_annotations {
        for annotation in annotations {
            if annotation
                .path
                .segments
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                != ["std", "binding"]
            {
                continue;
            }
            if let [HirAnnotationArg::Positional { value, .. }] = annotation.args.as_slice()
                && let Some(HirExpr::Literal(HirLiteral::String { value, .. })) =
                    hir.exprs.get(*value)
                && let Some(symbol) =
                    registry.lookup_qualified(&value.split('.').collect::<Vec<_>>())
                && matches!(symbol.decl, etas_std::StdDecl::Flow(_))
                && symbol.intrinsic.is_some()
            {
                result.push((*item, symbol.id, annotation.span));
            }
        }
    }
    result.sort_by_key(|(item, _, _)| *item);
    result
}

pub(crate) fn bind(hir: &mut HirProgram, mode: StandardLibraryMode, registry: &StdRegistry) {
    let mut annotations = hir
        .item_annotations
        .iter()
        .flat_map(|(item, values)| {
            values
                .iter()
                .filter(|a| {
                    a.path
                        .segments
                        .iter()
                        .map(|s| s.name.as_str())
                        .collect::<Vec<_>>()
                        == ["std", "binding"]
                })
                .map(|a| (*item, a.clone()))
        })
        .collect::<Vec<_>>();
    annotations.sort_by_key(|(item, _)| *item);
    let targets = declarations(hir, registry)
        .into_iter()
        .map(|(item, target, _)| (item, target))
        .collect::<std::collections::BTreeMap<_, _>>();
    for (item, annotation) in annotations {
        let error = |reason: &str| {
            Diagnostic::type_check(TypeDiagnosticCode::TypeMismatch, annotation.span, reason)
        };
        if mode != StandardLibraryMode::SourceBootstrap {
            hir.diagnostics.push(error(
                "std.binding is reserved for the toolchain's std source build",
            ));
            continue;
        }
        let Some(target) = targets.get(&item).copied() else {
            hir.diagnostics.push(error(
                "std.binding requires one string naming a native flow ABI",
            ));
            continue;
        };
        let Some(HirItem::Flow(flow)) = hir.items.get(item).cloned() else {
            hir.diagnostics
                .push(error("std.binding requires a flow declaration"));
            continue;
        };
        let block = flow.body.block();
        if !hir.blocks[block].stmts.is_empty()
            || hir.blocks[block].final_expr.is_some()
            || flow.effects.is_none()
            || flow.return_type.is_none()
        {
            hir.diagnostics.push(error(
                "native flow declarations require an empty body, a result type and an explicit public effect row",
            ));
            continue;
        }
        let native = registry.symbol(target).expect("selected native binding");
        let name = format!("$std_binding_{}", flow.symbol.0);
        let symbol = hir.symbols.alloc(SymbolData {
            name: name.clone(),
            kind: SymbolKind::Import,
            visibility: Visibility::Private,
            defining_module: hir.symbols.get(flow.symbol).unwrap().defining_module,
            defining_item: Some(item),
            def: SymbolDef::ImportAlias {
                path: native.qualified_path.clone(),
                origin: ImportAliasOrigin::NativeBinding,
            },
            declared_type: None,
            definition_span: annotation.span,
        });
        hir.scopes.insert(flow.scope, name.clone(), symbol);
        let callee = hir
            .exprs
            .alloc(HirExpr::Path(path(name, symbol, annotation.span)));
        let args = flow
            .params
            .iter()
            .map(|id| {
                let parameter = hir.symbols.get(*id).unwrap();
                let expr = hir.exprs.alloc(HirExpr::Path(path(
                    parameter.name.clone(),
                    *id,
                    parameter.definition_span,
                )));
                HirArg::Positional(expr)
            })
            .collect();
        let generic_args = flow
            .type_params
            .iter()
            .map(|id| {
                let parameter = hir.symbols.get(*id).unwrap();
                let parameter_path = path(parameter.name.clone(), *id, parameter.definition_span);
                if parameter.kind == SymbolKind::EffectParam {
                    HirGenericArg::EffectRow(HirEffectRow {
                        effects: vec![],
                        tail: Some(parameter_path),
                        span: parameter.definition_span,
                    })
                } else {
                    HirGenericArg::Type(hir.types.alloc(HirType::Path {
                        path: parameter_path,
                        args: vec![],
                        span: parameter.definition_span,
                    }))
                }
            })
            .collect();
        let call = hir.exprs.alloc(HirExpr::Call {
            callee,
            generic_args,
            args,
            span: annotation.span,
        });
        hir.blocks.get_mut(block).unwrap().final_expr = Some(call);
    }
}

fn path(name: String, symbol: SymbolId, span: Span) -> ResolvedPath {
    ResolvedPath {
        syntax_path: etas_syntax::ast::Path {
            segments: vec![etas_syntax::ast::Name {
                text: name.clone(),
                span,
            }],
            span,
        },
        segments: vec![PathSegment { name, span }],
        resolution: ResolveResult::Resolved(symbol),
        span,
    }
}
