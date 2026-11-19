use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_hir::{
    HirAnnotationArg, HirExpr, HirItem, HirLiteral, HirProgram, HirTypeDeclBody, SymbolKind,
    Visibility,
};
use etas_std::{StdGenericParamKind, StdSourceRole};

use crate::StandardLibraryMode;

pub(crate) fn bind_parameters(
    hir: &HirProgram,
    registry: &etas_std::StdRegistry,
    bindings: &[etas_types::StdSymbolBindingInput],
) -> etas_std::StdRegistry {
    let mut builder = etas_std::StdRegistryBuilder::from_registry(registry.clone());
    for binding in bindings {
        let role = StdSourceRole::from_path(&binding.qualified_path.join("."))
            .expect("checked compiler role ABI");
        let item = hir
            .symbols
            .get(binding.symbol)
            .unwrap()
            .defining_item
            .unwrap();
        let parameters = match &hir.items[item] {
            HirItem::Type(source) => &source.type_params,
            HirItem::Enum(source) => &source.type_params,
            HirItem::Spec(source) => &source.type_params,
            HirItem::Flow(source) => &source.type_params,
            HirItem::Effect(source) => &source.type_params,
            _ => unreachable!("checked source role declaration"),
        };
        let native = registry
            .lookup_qualified(&binding.qualified_path)
            .expect("compiler role query identity");
        let parameters: Vec<_> = parameters
            .iter()
            .zip(role.parameter_kinds())
            .enumerate()
            .map(|(index, (source, kind))| etas_std::StdGenericParam {
                name: hir.symbols.get(*source).unwrap().name.clone(),
                kind: *kind,
                bounds: role.parameter_bounds(index),
            })
            .collect();
        if let HirItem::Effect(source) = &hir.items[item] {
            let extends = source
                .extends
                .iter()
                .map(|parent| {
                    let etas_hir::ResolveResult::Resolved(symbol) = parent.path.resolution else {
                        unreachable!("resolved source effect parent");
                    };
                    canonical_path(hir, symbol).join(".")
                })
                .collect();
            builder.source_effect_declaration(native.id, parameters, extends);
        } else {
            let constructors: &[&str] = match role {
                StdSourceRole::Option => &["Some", "None"],
                StdSourceRole::Result => &["Ok", "Err"],
                _ => &[],
            };
            for constructor in constructors {
                let mut path = native.qualified_path[..native.qualified_path.len() - 1].to_vec();
                path.push((*constructor).to_owned());
                if !bindings
                    .iter()
                    .any(|binding| binding.qualified_path == path)
                {
                    let constructor = registry
                        .lookup_qualified(&path)
                        .expect("compiler enum constructor role");
                    builder.source_type_parameters(constructor.id, parameters.clone());
                }
            }
            builder.source_type_parameters(native.id, parameters);
        }
    }
    builder.finish_bootstrap()
}

/// Bind source actions to their execution ABI. Empty effect declarations still
/// expose transitional actions while the remaining modules are migrated.
pub(crate) fn bind_effect_actions(
    hir: &mut HirProgram,
    registry: &etas_std::StdRegistry,
    bindings: &[etas_types::StdSymbolBindingInput],
) {
    use etas_hir::{ResolveResult, SymbolData, SymbolDef, SyntheticSymbolReason};
    let effects = bindings
        .iter()
        .filter(|binding| {
            matches!(
                registry
                    .lookup_qualified(&binding.qualified_path)
                    .map(|s| &s.decl),
                Some(etas_std::StdDecl::Effect(_))
            )
        })
        .map(|binding| (binding.qualified_path.clone(), binding.symbol))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut owners = effects
        .values()
        .map(|symbol| (*symbol, *symbol))
        .collect::<std::collections::BTreeMap<_, _>>();
    for symbol in hir.symbols.iter() {
        if let SymbolDef::ImportAlias { path, .. } = &symbol.def
            && let Some(owner) = effects.get(path)
        {
            owners.insert(symbol.id, *owner);
        }
    }
    loop {
        let additional = hir
            .source_import_targets
            .iter()
            .filter_map(|(alias, target)| {
                (!owners.contains_key(alias))
                    .then(|| owners.get(target).map(|owner| (*alias, *owner)))
                    .flatten()
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        if additional.is_empty() {
            break;
        }
        owners.extend(additional);
    }
    let mut actions = std::collections::BTreeMap::new();
    for (path, owner) in effects {
        let owner_data = hir.symbols.get(owner).unwrap().clone();
        for native in registry.symbols() {
            let etas_std::StdDecl::EffectAction(action) = &native.decl else {
                continue;
            };
            if Some(&action.owner) != path.last() {
                continue;
            }
            let source_action = owner_data
                .defining_item
                .and_then(|item| match &hir.items[item] {
                    HirItem::Effect(effect) => effect
                        .body
                        .actions()
                        .iter()
                        .find(|source| {
                            hir.symbols
                                .get(source.symbol)
                                .unwrap()
                                .name
                                .rsplit('.')
                                .next()
                                == Some(action.name.as_str())
                        })
                        .map(|source| source.symbol),
                    _ => None,
                });
            let existing = source_action.or_else(|| {
                hir.standard_action_targets
                    .iter()
                    .filter_map(|(symbol, target)| (*target == native.id).then_some(*symbol))
                    .min()
            });
            let symbol = existing.unwrap_or_else(|| {
                let symbol = hir.symbols.alloc(SymbolData {
                    name: format!("{}.{}", action.owner, action.name),
                    kind: SymbolKind::EffectAction,
                    visibility: Visibility::Public,
                    defining_module: owner_data.defining_module,
                    defining_item: None,
                    def: SymbolDef::Synthetic {
                        reason: SyntheticSymbolReason::QualifiedEffectAction,
                    },
                    declared_type: None,
                    definition_span: owner_data.definition_span,
                });
                hir.standard_action_targets.insert(symbol, native.id);
                symbol
            });
            hir.standard_action_targets.insert(symbol, native.id);
            if let Some(HirItem::Effect(effect)) = owner_data
                .defining_item
                .and_then(|item| hir.items.get(item))
            {
                let overloads = effect
                    .body
                    .actions()
                    .iter()
                    .filter(|source| {
                        hir.symbols
                            .get(source.symbol)
                            .unwrap()
                            .name
                            .rsplit('.')
                            .next()
                            == Some(action.name.as_str())
                    })
                    .map(|source| source.symbol)
                    .collect::<Vec<_>>();
                for overload in overloads {
                    hir.standard_action_targets.insert(overload, native.id);
                }
            }
            // Aliases resolve through the source action so every use shares its
            // checked generic binders, rather than an independently lowered ABI.
            if source_action.is_some() {
                for (alias, target) in hir.standard_action_targets.clone() {
                    if target == native.id
                        && alias != symbol
                        && !matches!(
                            hir.symbols.get(alias).map(|value| &value.def),
                            Some(SymbolDef::EffectAction { .. })
                        )
                    {
                        hir.source_import_targets.insert(alias, symbol);
                    }
                }
            }
            hir.symbols
                .insert_member(owner, action.name.clone(), symbol);
            actions.insert((owner, action.name.clone()), symbol);
        }
    }
    let resolve = |action: &etas_hir::ResolvedActionRef| {
        let ResolveResult::Resolved(owner) = action.effect.path.resolution else {
            return None;
        };
        let owner = owners.get(&owner)?;
        actions.get(&(*owner, action.action.clone())).copied()
    };
    let updates = hir
        .handler_arms
        .iter()
        .filter_map(|(id, arm)| resolve(&arm.action).map(|symbol| (id, symbol)))
        .collect::<Vec<_>>();
    for (id, symbol) in updates {
        hir.handler_arms.get_mut(id).unwrap().action.action_symbol =
            ResolveResult::Resolved(symbol);
    }
    let updates = hir
        .exprs
        .iter()
        .filter_map(|(id, expression)| match expression {
            HirExpr::Perform { action, .. } | HirExpr::Infer { action, .. } => {
                resolve(action).map(|symbol| (id, symbol))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    for (id, symbol) in updates {
        match hir.exprs.get_mut(id).unwrap() {
            HirExpr::Perform { action, .. } | HirExpr::Infer { action, .. } => {
                action.action_symbol = ResolveResult::Resolved(symbol)
            }
            _ => unreachable!(),
        }
    }
}

pub(crate) fn bindings(
    hir: &HirProgram,
    mode: StandardLibraryMode,
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
                        if etas_std::StdExecutionRole::from_role(value).is_some() {
                            continue;
                        }
                        StdSourceRole::from_path(value)
                    }
                    _ => None,
                },
                _ => None,
            };
            let Some(target) = target else {
                diagnostics.push(invalid(
                    "std.role requires one string naming a compiler role ABI",
                ));
                continue;
            };
            let target_path = target
                .path()
                .split('.')
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let declaration = hir.items.get(*item).expect("source role item");
            let (source_symbol, parameters, shape) = match declaration {
                HirItem::Type(source) => (
                    source.symbol,
                    source.type_params.as_slice(),
                    if target == StdSourceRole::Iter {
                        source_iterator_shape(hir, source)
                    } else if target == StdSourceRole::WorkspacePath {
                        source_workspace_path_shape(hir, source)
                    } else if target == StdSourceRole::Money {
                        source_money_shape(hir, source)
                    } else if target == StdSourceRole::Range {
                        source_range_shape(hir, source)
                    } else {
                        (target.is_bodyless_type()
                            && matches!(source.body, HirTypeDeclBody::Bodyless))
                            || ((target.is_projected_type() || target.is_represented_type())
                                && matches!(source.body, HirTypeDeclBody::Representation(_)))
                    },
                ),
                HirItem::Enum(source) => (
                    source.symbol,
                    source.type_params.as_slice(),
                    match target {
                        StdSourceRole::Result => result_shape(hir, source),
                        StdSourceRole::Option => option_shape(hir, source),
                        StdSourceRole::Limit => source.type_params.is_empty(),
                        StdSourceRole::NativeContextPolicy
                        | StdSourceRole::NativeRetentionPolicy
                        | StdSourceRole::NativeJsonValue => source.type_params.is_empty(),
                        _ => false,
                    },
                ),
                HirItem::Flow(source) => (
                    source.symbol,
                    source.type_params.as_slice(),
                    match target {
                        StdSourceRole::Ok | StdSourceRole::Err => result_constructor(
                            hir,
                            source,
                            target.path().rsplit('.').next().unwrap(),
                        ),
                        StdSourceRole::Some | StdSourceRole::None => option_constructor(
                            hir,
                            source,
                            target.path().rsplit('.').next().unwrap(),
                        ),
                        _ => false,
                    },
                ),
                HirItem::Effect(source) => (
                    source.symbol,
                    source.type_params.as_slice(),
                    target.effect_execution().is_some(),
                ),
                HirItem::Spec(source) => (
                    source.symbol,
                    source.type_params.as_slice(),
                    // The role identifies the spec; its behavioral members and
                    // super bounds belong to source and use ordinary spec checks.
                    target.is_spec()
                        && matches!(source.kind, etas_hir::HirSpecKind::TypeSpec)
                        && (target != StdSourceRole::Number
                            || (source.items.is_empty() && source.bounds.is_empty())),
                ),
                _ => {
                    diagnostics.push(invalid(
                        "std.role requires its canonical type, enum, spec or effect declaration",
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
            let parameters_match = parameters.len() == target.parameter_kinds().len()
                && parameters
                    .iter()
                    .zip(target.parameter_kinds())
                    .enumerate()
                    .all(|(index, (source, native))| {
                        let kind = hir
                            .symbols
                            .get(*source)
                            .expect("source role parameter")
                            .kind;
                        let expected = target.parameter_bounds(index);
                        let bounds_match = match &hir.symbols.get(*source).unwrap().def {
                            etas_hir::SymbolDef::TypeParam { bounds, .. } => {
                                bounds.len() == expected.len() && bounds.iter().zip(&expected).all(|(bound, native)| {
                                    bound.args.is_empty() && matches!(bound.path.resolution,
                                        etas_hir::ResolveResult::Resolved(symbol) if canonical_path(hir, symbol) == native.path)
                                })
                            }
                            _ => expected.is_empty(),
                        };
                        bounds_match
                            && matches!(
                                (kind, *native),
                                (SymbolKind::TypeParam, StdGenericParamKind::Type)
                                    | (SymbolKind::EffectParam, StdGenericParamKind::EffectRow)
                            )
                    });
            if symbol.visibility != Visibility::Public
                || path != target_path
                || !shape
                || !parameters_match
            {
                diagnostics.push(invalid("std.role requires the public canonical declaration with its checked body and generic type/effect parameter shape"));
                continue;
            }
            if !seen.insert(target) {
                diagnostics.push(invalid(
                    "a native role can be bound to only one std source declaration",
                ));
                continue;
            }
            result.push(etas_types::StdSymbolBindingInput {
                symbol: source_symbol,
                qualified_path: target_path,
            });
        }
    }
    (result, diagnostics)
}

fn source_range_shape(hir: &HirProgram, source: &etas_hir::HirTypeDecl) -> bool {
    use etas_hir::{HirPrimitiveType, HirType, ResolveResult};
    let [index] = source.type_params.as_slice() else {
        return false;
    };
    let HirTypeDeclBody::Representation(representation) = source.body else {
        return false;
    };
    let HirType::Record { fields, .. } = &hir.types[representation] else {
        return false;
    };
    fields.len() == 4
        && ["start", "end", "include_start", "include_end"]
            .iter()
            .all(|name| {
                fields.iter().any(|field| {
                    hir.symbols.get(field.symbol).unwrap().name == *name
                        && match *name {
                            "start" | "end" => {
                                matches!(&hir.types[field.ty], HirType::Path { path, args, .. }
                    if args.is_empty() && path.resolution == ResolveResult::Resolved(*index))
                            }
                            _ => matches!(
                                hir.types[field.ty],
                                HirType::Primitive {
                                    kind: HirPrimitiveType::Bool,
                                    ..
                                }
                            ),
                        }
                })
            })
}

fn source_workspace_path_shape(hir: &HirProgram, source: &etas_hir::HirTypeDecl) -> bool {
    let HirTypeDeclBody::Representation(representation) = source.body else {
        return false;
    };
    let etas_hir::HirType::Record { fields, .. } = &hir.types[representation] else {
        return false;
    };
    let [relative] = fields.as_slice() else {
        return false;
    };
    relative.visibility == etas_syntax::ast::ty::Visibility::Private
        && hir.symbols.get(relative.symbol).unwrap().name == "relative"
        && matches!(
            hir.types[relative.ty],
            etas_hir::HirType::Primitive {
                kind: etas_hir::HirPrimitiveType::String,
                ..
            }
        )
}

fn source_money_shape(hir: &HirProgram, source: &etas_hir::HirTypeDecl) -> bool {
    let HirTypeDeclBody::Representation(representation) = source.body else {
        return false;
    };
    let etas_hir::HirType::Record { fields, .. } = &hir.types[representation] else {
        return false;
    };
    fields.len() == 2
        && fields.iter().any(|field| {
            hir.symbols
                .get(field.symbol)
                .is_some_and(|symbol| symbol.name == "amount")
                && matches!(
                    hir.types[field.ty],
                    etas_hir::HirType::Primitive {
                        kind: etas_hir::HirPrimitiveType::F64,
                        ..
                    }
                )
        })
        && fields.iter().any(|field| {
            hir.symbols
                .get(field.symbol)
                .is_some_and(|symbol| symbol.name == "currency")
                && matches!(
                    hir.types[field.ty],
                    etas_hir::HirType::Primitive {
                        kind: etas_hir::HirPrimitiveType::String,
                        ..
                    }
                )
        })
}

fn source_iterator_shape(hir: &HirProgram, source: &etas_hir::HirTypeDecl) -> bool {
    use etas_hir::{HirType, ResolveResult};
    let [element] = source.type_params.as_slice() else {
        return false;
    };
    let HirTypeDeclBody::Representation(representation) = source.body else {
        return false;
    };
    let HirType::Record { fields, .. } = &hir.types[representation] else {
        return false;
    };
    let [next] = fields.as_slice() else {
        return false;
    };
    if hir.symbols.get(next.symbol).unwrap().name != "next" {
        return false;
    }
    let HirType::Arrow {
        input,
        output,
        effect,
        ..
    } = &hir.types[next.ty]
    else {
        return false;
    };
    if !matches!(&hir.types[*input], HirType::Tuple { elems, .. } if elems.is_empty())
        && !matches!(
            &hir.types[*input],
            HirType::Primitive {
                kind: etas_hir::HirPrimitiveType::Unit,
                ..
            }
        )
    {
        return false;
    }
    if !effect
        .as_ref()
        .is_some_and(|row| row.effects.is_empty() && row.tail.is_none())
    {
        return false;
    }
    let HirType::Path { path, args, .. } = &hir.types[*output] else {
        return false;
    };
    let ResolveResult::Resolved(option) = path.resolution else {
        return false;
    };
    if canonical_path(hir, option) != ["std", "option", "Option"] {
        return false;
    }
    let [pair] = args.as_slice() else {
        return false;
    };
    let HirType::Tuple { elems, .. } = &hir.types[*pair] else {
        return false;
    };
    let [value, remaining] = elems.as_slice() else {
        return false;
    };
    if !matches!(&hir.types[*value], HirType::Path { path, args, .. }
        if args.is_empty() && path.resolution == ResolveResult::Resolved(*element))
    {
        return false;
    }
    let HirType::Path { path, args, .. } = &hir.types[*remaining] else {
        return false;
    };
    let ResolveResult::Resolved(iterator) = path.resolution else {
        return false;
    };
    canonical_path(hir, iterator) == ["std", "collections", "Iter"]
        && matches!(args.as_slice(), [arg] if matches!(&hir.types[*arg], HirType::Path { path, args, .. }
            if args.is_empty() && path.resolution == ResolveResult::Resolved(*element)))
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

fn option_shape(hir: &HirProgram, source: &etas_hir::HirEnumDecl) -> bool {
    source.type_params.len() == 1 && source.variants.len() == 2
        && source.variants.iter().zip(["Some", "None"]).all(|(variant, name)| {
            hir.symbols.get(variant.symbol).is_some_and(|symbol| symbol.name == name)
                && variant.field_names.is_none()
                && if name == "None" { variant.fields.is_empty() } else {
                    matches!(variant.fields.as_slice(), [field]
                        if matches!(hir.types.get(*field), Some(etas_hir::HirType::Path { path, args, .. })
                            if args.is_empty() && path.resolution == etas_hir::ResolveResult::Resolved(source.type_params[0])))
                }
        })
}

fn option_constructor(hir: &HirProgram, source: &etas_hir::HirFlowDecl, name: &str) -> bool {
    let Some(block) = source.body.as_block() else {
        return false;
    };
    let block = &hir.blocks[block];
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
    let (callee, args) = match hir.exprs.get(expression) {
        Some(HirExpr::Call { callee, args, .. }) => (*callee, args.as_slice()),
        Some(HirExpr::Path(_)) if name == "None" => (expression, &[][..]),
        _ => return false,
    };
    let Some(HirExpr::Path(path)) = hir.exprs.get(callee) else {
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
    let signature = source.type_params.len() == 1
        && source.params.len() == usize::from(name == "Some")
        && source.effects.as_ref().is_some_and(|row| row.effects.is_empty() && row.tail.is_none())
        && source.return_type.is_some_and(|ty| matches!(hir.types.get(ty), Some(etas_hir::HirType::Path { path, args, .. })
            if path.resolution == etas_hir::ResolveResult::Resolved(owner.symbol)
                && matches!(args.as_slice(), [inner]
                    if matches!(hir.types.get(*inner), Some(etas_hir::HirType::Path { path, args, .. })
                        if args.is_empty() && path.resolution == etas_hir::ResolveResult::Resolved(source.type_params[0])))));
    signature
        && symbol.name == name
        && declaration.name == "Option"
        && module
            .segments
            .iter()
            .map(|part| part.name.as_str())
            .eq(["std", "option"])
        && option_shape(hir, owner)
        && if name == "None" {
            args.is_empty()
        } else {
            hir.symbols.get(source.params[0]).and_then(|parameter| parameter.declared_type)
                .is_some_and(|ty| matches!(hir.types.get(ty), Some(etas_hir::HirType::Path { path, args, .. })
                    if args.is_empty() && path.resolution == etas_hir::ResolveResult::Resolved(source.type_params[0])))
                && matches!(args, [etas_hir::HirArg::Positional(value)]
                    if matches!(hir.exprs.get(*value), Some(HirExpr::Path(path))
                        if path.resolution == etas_hir::ResolveResult::Resolved(source.params[0])))
        }
}

fn result_constructor(hir: &HirProgram, source: &etas_hir::HirFlowDecl, name: &str) -> bool {
    let Some(block) = source.body.as_block() else {
        return false;
    };
    let block = &hir.blocks[block];
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

pub(super) fn canonical_path(hir: &HirProgram, symbol: etas_hir::SymbolId) -> Vec<String> {
    let symbol = hir.symbols.get(symbol).unwrap();
    if let etas_hir::SymbolDef::ImportAlias { path, .. } = &symbol.def {
        return path.clone();
    }
    let mut path = hir.modules_arena[symbol.defining_module]
        .name
        .as_ref()
        .unwrap()
        .segments
        .iter()
        .map(|part| part.name.clone())
        .collect::<Vec<_>>();
    path.push(symbol.name.clone());
    path
}
