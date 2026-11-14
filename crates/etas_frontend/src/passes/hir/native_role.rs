use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_hir::{
    HirAnnotationArg, HirExpr, HirItem, HirLiteral, HirProgram, HirTypeDeclBody, SymbolKind,
    Visibility,
};
use etas_std::{RuntimeTypeRole, StdDecl, StdGenericParamKind, StdRegistry};

use crate::StandardLibraryMode;

pub(crate) fn bindings(
    hir: &HirProgram,
    mode: StandardLibraryMode,
    registry: &StdRegistry,
) -> (Vec<etas_types::StdSymbolBindingInput>, Vec<Diagnostic>) {
    let mut result = Vec::new();
    let mut diagnostics = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut items = hir.item_annotations.iter().collect::<Vec<_>>();
    items.sort_by_key(|(item, _)| **item);
    for (item, annotations) in items {
        for annotation in annotations {
            if annotation
                .path
                .segments
                .iter()
                .map(|part| part.name.as_str())
                .collect::<Vec<_>>()
                != ["std", "role"]
            {
                continue;
            }
            let invalid = |reason: &str| {
                Diagnostic::type_check(TypeDiagnosticCode::TypeMismatch, annotation.span, reason)
            };
            if mode != StandardLibraryMode::SourceBootstrap {
                diagnostics.push(invalid(
                    "std.role is reserved for the toolchain's std source build",
                ));
                continue;
            }
            let target = match annotation.args.as_slice() {
                [HirAnnotationArg::Positional { value, .. }] => match hir.exprs.get(*value) {
                    Some(HirExpr::Literal(HirLiteral::String { value, .. })) => {
                        registry.lookup_qualified(&value.split('.').collect::<Vec<_>>())
                    }
                    _ => None,
                },
                _ => None,
            };
            let Some(target) = target else {
                diagnostics.push(invalid(
                    "std.role requires one string naming a native role contract",
                ));
                continue;
            };
            let StdDecl::Type(native) = &target.decl else {
                diagnostics.push(invalid("std.role requires a type role contract"));
                continue;
            };
            if !matches!(
                native.runtime_role,
                Some(
                    RuntimeTypeRole::Task
                        | RuntimeTypeRole::Scheduler
                        | RuntimeTypeRole::Continuation
                )
            ) {
                diagnostics.push(invalid(
                    "this native role requires its source declaration contract before migration",
                ));
                continue;
            }
            let Some(HirItem::Type(source)) = hir.items.get(*item) else {
                diagnostics.push(invalid(
                    "task runtime roles require an opaque type declaration",
                ));
                continue;
            };
            let symbol = hir.symbols.get(source.symbol).expect("source role symbol");
            let mut path = hir.modules_arena[symbol.defining_module]
                .name
                .as_ref()
                .expect("source role module")
                .segments
                .iter()
                .map(|part| part.name.clone())
                .collect::<Vec<_>>();
            path.push(symbol.name.clone());
            let parameters_match = source.type_params.len() == native.params.len()
                && source
                    .type_params
                    .iter()
                    .zip(&native.params)
                    .all(|(source, native)| {
                        let kind = hir
                            .symbols
                            .get(*source)
                            .expect("source role parameter")
                            .kind;
                        let unbounded = !matches!(&hir.symbols.get(*source).unwrap().def,
                        etas_hir::SymbolDef::TypeParam { bounds, .. } if !bounds.is_empty());
                        unbounded
                            && matches!(
                                (kind, native.kind),
                                (SymbolKind::TypeParam, StdGenericParamKind::Type)
                                    | (SymbolKind::EffectParam, StdGenericParamKind::EffectRow)
                            )
                    });
            if symbol.visibility != Visibility::Public
                || path != target.qualified_path
                || !matches!(source.body, HirTypeDeclBody::Bodyless)
                || !parameters_match
            {
                diagnostics.push(invalid("std.role requires the public canonical opaque declaration with the native generic type/effect parameter shape"));
                continue;
            }
            if !seen.insert(target.id) {
                diagnostics.push(invalid(
                    "a native role can be bound to only one std source declaration",
                ));
                continue;
            }
            result.push(etas_types::StdSymbolBindingInput {
                symbol: source.symbol,
                qualified_path: target.qualified_path.clone(),
            });
        }
    }
    (result, diagnostics)
}
