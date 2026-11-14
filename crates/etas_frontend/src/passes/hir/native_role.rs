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
            let declaration = hir.items.get(*item).expect("source role item");
            let (source_symbol, parameters, shape) = match declaration {
                HirItem::Type(source) => (
                    source.symbol,
                    source.type_params.as_slice(),
                    matches!(
                        native.runtime_role,
                        Some(
                            RuntimeTypeRole::Task
                                | RuntimeTypeRole::Scheduler
                                | RuntimeTypeRole::Continuation
                        )
                    ) && matches!(source.body, HirTypeDeclBody::Bodyless),
                ),
                HirItem::Enum(source) => (
                    source.symbol,
                    source.type_params.as_slice(),
                    target.qualified_path == ["std", "result", "Result"]
                        && result_shape(hir, source),
                ),
                HirItem::Flow(source) => (
                    source.symbol,
                    source.type_params.as_slice(),
                    target.kind == etas_std::StdSymbolKind::Constructor
                        && matches!(target.qualified_path.as_slice(), [std, result, name] if std == "std" && result == "result" && (name == "Ok" || name == "Err"))
                        && result_constructor(hir, source, &target.name),
                ),
                HirItem::Spec(source) => (
                    source.symbol,
                    source.type_params.as_slice(),
                    target.qualified_path == ["std", "core", "Index"]
                        && matches!(source.kind, etas_hir::HirSpecKind::TypeSpec)
                        && source.items.is_empty()
                        && source.bounds.is_empty(),
                ),
                _ => {
                    diagnostics.push(invalid(
                        "std.role requires its canonical type, enum or spec declaration",
                    ));
                    continue;
                }
            };
            let symbol = hir.symbols.get(source_symbol).expect("source role symbol");
            let mut path = hir.modules_arena[symbol.defining_module]
                .name
                .as_ref()
                .expect("source role module")
                .segments
                .iter()
                .map(|part| part.name.clone())
                .collect::<Vec<_>>();
            path.push(symbol.name.clone());
            let parameters_match = parameters.len() == native.params.len()
                && parameters
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
                || !shape
                || !parameters_match
            {
                diagnostics.push(invalid("std.role requires the public canonical declaration with its checked body and generic type/effect parameter shape"));
                continue;
            }
            if !seen.insert(target.id) {
                diagnostics.push(invalid(
                    "a native role can be bound to only one std source declaration",
                ));
                continue;
            }
            result.push(etas_types::StdSymbolBindingInput {
                symbol: source_symbol,
                qualified_path: target.qualified_path.clone(),
            });
        }
    }
    (result, diagnostics)
}

fn result_shape(hir: &HirProgram, source: &etas_hir::HirEnumDecl) -> bool {
    source.type_params.len() == 2 && source.variants.len() == 2
        && source.variants.iter().zip(["Ok", "Err"]).enumerate().all(|(position, (variant, name))| {
            hir.symbols.get(variant.symbol).is_some_and(|symbol| symbol.name == name)
                && variant.field_names.is_none()
                && matches!(variant.fields.as_slice(), [field]
                    if matches!(hir.types.get(*field), Some(etas_hir::HirType::Path { path, args, .. })
                        if args.is_empty() && path.resolution == etas_hir::ResolveResult::Resolved(source.type_params[position])))
        })
}

fn result_constructor(hir: &HirProgram, source: &etas_hir::HirFlowDecl, name: &str) -> bool {
    let block = &hir.blocks[source.body.block()];
    let expression = match (block.stmts.as_slice(), block.final_expr) {
        ([statement], None) => match hir.stmts.get(*statement) {
            Some(etas_hir::HirStmt::Return {
                value: Some(value), ..
            }) => *value,
            _ => return false,
        },
        ([], Some(expression)) => expression,
        _ => return false,
    };
    let Some(HirExpr::Call { callee, args, .. }) = hir.exprs.get(expression) else {
        return false;
    };
    let Some(HirExpr::Path(path)) = hir.exprs.get(*callee) else {
        return false;
    };
    let etas_hir::ResolveResult::Resolved(variant) = path.resolution else {
        return false;
    };
    let Some(symbol) = hir.symbols.get(variant) else {
        return false;
    };
    let etas_hir::SymbolDef::EnumVariant { enum_item, .. } = symbol.def else {
        return false;
    };
    let Some(HirItem::Enum(owner)) = hir.items.get(enum_item) else {
        return false;
    };
    let declaration = hir.symbols.get(owner.symbol).unwrap();
    let module = hir.modules_arena[declaration.defining_module]
        .name
        .as_ref()
        .unwrap();
    let owner_path = module
        .segments
        .iter()
        .map(|part| part.name.as_str())
        .collect::<Vec<_>>();
    let parameter_index = usize::from(name == "Err");
    let type_parameter = |ty: etas_hir::HirTypeId, parameter: etas_hir::SymbolId| {
        matches!(hir.types.get(ty), Some(etas_hir::HirType::Path { path, args, .. })
            if args.is_empty() && path.resolution == etas_hir::ResolveResult::Resolved(parameter))
    };
    let signature = source.type_params.len() == 2 && source.params.len() == 1
        && source.effects.as_ref().is_some_and(|row| row.effects.is_empty() && row.tail.is_none())
        && hir.symbols.get(source.params[0]).and_then(|parameter| parameter.declared_type)
            .is_some_and(|ty| type_parameter(ty, source.type_params[parameter_index]))
        && source.return_type.is_some_and(|ty| matches!(hir.types.get(ty), Some(etas_hir::HirType::Path { path, args, .. })
            if path.resolution == etas_hir::ResolveResult::Resolved(owner.symbol)
                && matches!(args.as_slice(), [ok, err] if type_parameter(*ok, source.type_params[0]) && type_parameter(*err, source.type_params[1]))));
    signature
        && symbol.name == name
        && declaration.name == "Result"
        && owner_path == ["std", "result"]
        && result_shape(hir, owner)
        && matches!(args.as_slice(), [etas_hir::HirArg::Positional(value)]
            if matches!(hir.exprs.get(*value), Some(HirExpr::Path(path))
                if path.resolution == etas_hir::ResolveResult::Resolved(source.params[0])))
}
