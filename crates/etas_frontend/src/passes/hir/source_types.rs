use etas_hir::{HirItem, HirProgram, HirType, HirTypeId, ResolveResult, SymbolDef, SymbolId};
use etas_std::{
    CodeTypeRole, StdDecl, StdGenericParam, StdGenericParamKind, StdRegistry, StdRegistryBuilder,
    StdSpecRef, StdSymbolKind, StdType, TypeDecl, TypeDeclKind,
};

/// Supply constructor identities to native signatures from the actual source
/// declarations. Representations, enum layouts and source bounds are checked by
/// the ordinary signature pipeline; no legacy API declaration is substituted.
pub(super) fn bind(hir: &HirProgram, registry: &StdRegistry) -> Result<StdRegistry, String> {
    let mut builder = StdRegistryBuilder::from_registry(registry.clone());
    for (_, item) in hir.items.iter() {
        let (symbol, parameters, kind) = match item {
            HirItem::Type(source) => (
                source.symbol,
                &source.type_params,
                if matches!(source.body, etas_hir::HirTypeDeclBody::Bodyless) {
                    TypeDeclKind::Support
                } else {
                    TypeDeclKind::Struct
                },
            ),
            HirItem::Enum(source) => (source.symbol, &source.type_params, TypeDeclKind::Enum),
            HirItem::Spec(source) => (source.symbol, &source.type_params, TypeDeclKind::Spec),
            // Aliases are resolved by the ordinary source alias pass, rather
            // than inserted into the compiler's structural constructor table.
            _ => continue,
        };
        let path = super::native_role::canonical_path(hir, symbol);
        if path.first().map(String::as_str) != Some("std") {
            continue;
        }
        if registry.lookup_qualified(&path).is_some() {
            return Err(format!(
                "source type `{}` duplicates a native compiler contract",
                path.join(".")
            ));
        }
        let (name, module) = path.split_last().expect("source type path");
        let module = builder.module(&module.iter().map(String::as_str).collect::<Vec<_>>(), "");
        let mut declaration = TypeDecl::generic_params(name, generics(hir, parameters)?, kind);
        declaration.code_role = CodeTypeRole::from_source_path(&path.join("."));
        let declaration = builder.symbol(
            module,
            name,
            StdSymbolKind::Type,
            StdDecl::Type(declaration),
            "",
        );
        builder.source_visibility(
            declaration,
            match hir
                .symbols
                .get(symbol)
                .expect("source type symbol")
                .visibility
            {
                etas_hir::Visibility::Public => etas_std::StdSymbolVisibility::Public,
                etas_hir::Visibility::Private => etas_std::StdSymbolVisibility::Private,
            },
        );
    }
    // Native execution headers still carry transitional value signatures.
    // Checked source action signatures complete them after normal alias/type
    // inference; image publication only accepts the validated final query.
    Ok(builder.finish_bootstrap())
}

pub(super) fn generics(
    hir: &HirProgram,
    parameters: &[SymbolId],
) -> Result<Vec<StdGenericParam>, String> {
    parameters
        .iter()
        .map(|id| {
            let symbol = hir.symbols.get(*id).expect("source generic binder");
            let (kind, bounds) = match &symbol.def {
                SymbolDef::TypeParam { bounds, .. } => (
                    StdGenericParamKind::Type,
                    bounds
                        .iter()
                        .map(|bound| {
                            Ok(StdSpecRef {
                                path: resolved_path(hir, &bound.path)?,
                                args: bound
                                    .args
                                    .iter()
                                    .map(|ty| project_type(hir, *ty))
                                    .collect::<Result<_, _>>()?,
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?,
                ),
                SymbolDef::EffectParam { .. } => (StdGenericParamKind::EffectRow, Vec::new()),
                _ => unreachable!("source declaration generic binder"),
            };
            Ok(StdGenericParam {
                name: symbol.name.clone(),
                kind,
                bounds,
            })
        })
        .collect()
}

fn resolved_path(hir: &HirProgram, path: &etas_hir::ResolvedPath) -> Result<Vec<String>, String> {
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return Err("source type query has an unresolved type or bound".to_owned());
    };
    let symbol = hir
        .canonical_source_symbol(symbol)
        .ok_or_else(|| "source type query has an invalid import binding".to_owned())?;
    Ok(super::native_role::canonical_path(hir, symbol))
}

pub(super) fn project_type(hir: &HirProgram, ty: HirTypeId) -> Result<StdType, String> {
    Ok(match &hir.types[ty] {
        HirType::Existential { bounds, .. } => StdType::Existential {
            bounds: bounds
                .iter()
                .map(|bound| {
                    Ok(StdSpecRef {
                        path: resolved_path(hir, &bound.path)?,
                        args: bound
                            .args
                            .iter()
                            .map(|ty| project_type(hir, *ty))
                            .collect::<Result<_, String>>()?,
                    })
                })
                .collect::<Result<_, String>>()?,
        },
        HirType::Handler {
            handled,
            produced,
            result,
            ..
        } => StdType::Handler {
            handled: project_row(hir, handled)?,
            produced: match produced {
                etas_hir::HirHandlerProducedEffects::Infer => None,
                etas_hir::HirHandlerProducedEffects::Explicit(row) => Some(project_row(hir, row)?),
            },
            result: result
                .map(|ty| project_type(hir, ty).map(Box::new))
                .transpose()?,
        },
        HirType::Arrow {
            input,
            output,
            effect,
            ..
        } => StdType::Function {
            input: match &hir.types[*input] {
                HirType::Tuple { elems, .. } => elems
                    .iter()
                    .map(|ty| project_type(hir, *ty))
                    .collect::<Result<_, String>>()?,
                _ => vec![project_type(hir, *input)?],
            },
            output: Box::new(project_type(hir, *output)?),
            effects: effect
                .as_ref()
                .map(|row| project_row(hir, row))
                .transpose()?
                .unwrap_or_default(),
        },
        HirType::Primitive { kind, .. } => StdType::parse(kind.as_str()),
        HirType::Path { path, args, .. } => {
            let ResolveResult::Resolved(symbol) = path.resolution else {
                return Err("source type query has an unresolved type".to_owned());
            };
            let symbol = hir
                .canonical_source_symbol(symbol)
                .ok_or_else(|| "source type query has an invalid import binding".to_owned())?;
            let data = hir.symbols.get(symbol).expect("resolved source type");
            if matches!(
                data.def,
                SymbolDef::TypeParam { .. } | SymbolDef::EffectParam { .. }
            ) {
                StdType::Var(data.name.clone())
            } else {
                let name = resolved_path(hir, path)?.join(".");
                if args.is_empty() {
                    StdType::Named(name)
                } else {
                    StdType::NamedApplied {
                        name,
                        args: args
                            .iter()
                            .map(|ty| project_type(hir, *ty))
                            .collect::<Result<_, _>>()?,
                    }
                }
            }
        }
        HirType::Array { element, .. } => StdType::Array(Box::new(project_type(hir, *element)?)),
        HirType::Tuple { elems, .. } => StdType::Tuple(
            elems
                .iter()
                .map(|ty| project_type(hir, *ty))
                .collect::<Result<_, _>>()?,
        ),
        HirType::Record { fields, .. } => StdType::Record(
            fields
                .iter()
                .map(|field| {
                    Ok(etas_std::StdRecordField {
                        name: hir
                            .symbols
                            .get(field.symbol)
                            .expect("source field")
                            .name
                            .clone(),
                        ty: project_type(hir, field.ty)?,
                    })
                })
                .collect::<Result<_, String>>()?,
        ),
        _ => {
            return Err(
                "source generic bound cannot be represented by the std query type vocabulary"
                    .to_owned(),
            );
        }
    })
}

fn project_row(
    hir: &HirProgram,
    row: &etas_hir::HirEffectRow,
) -> Result<etas_std::StdEffectRowExpr, String> {
    use etas_hir::HirEffectArg as A;
    use etas_std::StdStaticArg as S;
    Ok(etas_std::StdEffectRowExpr {
        tail: row.tail.as_ref().map(|path| {
            let ResolveResult::Resolved(symbol) = path.resolution else { return Err("source row tail is unresolved".to_owned()) };
            Ok(hir.symbols.get(symbol).expect("source row binder").name.clone())
        }).transpose()?,
        effects: row.effects.iter().map(|effect| Ok(etas_std::StdEffectRef {
            path: resolved_path(hir, &effect.path)?,
            args: effect.args.iter().map(|arg| Ok(match arg {
                A::Type(ty) => S::Type(project_type(hir, *ty)?),
                A::Path(path) => S::Path(resolved_path(hir, path)?),
                A::String { value, .. } => S::String(value.clone()),
                A::Int { text, .. } => S::Int(text.clone()),
                A::Wildcard { .. } => S::Wildcard,
                A::PatternBinding { .. } => return Err("query row requires declared type parameters instead of a handler pattern binder".into()),
            })).collect::<Result<_, String>>()?,
        })).collect::<Result<_, String>>()?,
    })
}
