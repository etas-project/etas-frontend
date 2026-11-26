use std::collections::{HashMap, HashSet};

use etas_core::Span;
use etas_hir::*;
use etas_package_metadata::{ConstructorContract, ConstructorGenericArgument, EffectArgKind};

use crate::{ImportTarget, ProjectContext, ProjectEnvironmentInput, ProjectExternalTypeInput};

/// Elaborate definition-owned imported contracts before building the immutable
/// revision. The generated expressions use the same checked perform path as
/// locally declared field annotations.
pub(super) fn elaborate(context: &ProjectContext, hir: &mut HirProgram) -> Result<(), String> {
    let Some(environment) = context.validated_external_environment.as_ref() else {
        return Ok(());
    };
    let environment = environment.environment();
    for contract in environment
        .external_public_metadata
        .iter()
        .flat_map(|metadata| &metadata.constructor_contracts)
    {
        let private = contract
            .fields
            .iter()
            .filter(|field| !field.public)
            .map(|field| field.name.clone())
            .collect::<Vec<_>>();
        if !private.is_empty() {
            hir.private_record_fields
                .insert(contract.path.clone(), private);
        }
    }
    if !environment
        .external_public_metadata
        .iter()
        .any(|metadata| !metadata.constructor_contracts.is_empty())
    {
        return Ok(());
    }
    let canonical = canonical_imports(context, hir);
    let records = hir
        .exprs
        .iter()
        .filter_map(|(expr, value)| match value {
            HirExpr::Record(record)
                if record.injections.is_none()
                    && record.path.is_some()
                    && record.base.is_none() =>
            {
                Some((expr, record.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    for (expr, mut record) in records {
        let Some(path) =
            external_path(hir, record.path.as_ref().expect("named record"), &canonical)
        else {
            continue;
        };
        let Some(contract) = find_contract(environment, &path) else {
            continue;
        };
        let supplied = record
            .fields
            .iter()
            .map(|field| match field {
                HirFieldInit::Named { name, .. } | HirFieldInit::Shorthand { name, .. } => {
                    name.as_str()
                }
            })
            .collect::<HashSet<_>>();
        let missing = contract
            .fields
            .iter()
            .filter(|field| field.injection.is_some() && !supplied.contains(field.name.as_str()))
            .collect::<Vec<_>>();
        if missing.is_empty() {
            continue;
        }
        let module = hir
            .modules
            .iter()
            .copied()
            .find(|module| hir.modules_arena[*module].span.source == record.span.source)
            .or_else(|| {
                hir.symbols
                    .iter()
                    .find(|symbol| symbol.definition_span.source == record.span.source)
                    .map(|symbol| symbol.defining_module)
            })
            .ok_or("constructor has no HIR module")?;
        let mut builder = Builder {
            hir,
            environment,
            module,
            span: record.span,
            parameters: Vec::new(),
        };
        for parameter in &contract.generic_params {
            let symbol = builder.symbol(
                &parameter.name,
                if parameter.kind == etas_package_metadata::GenericParamKind::Effect {
                    SymbolKind::EffectParam
                } else {
                    SymbolKind::TypeParam
                },
                SymbolDef::Synthetic {
                    reason: SyntheticSymbolReason::ConstructorTypeParameter,
                },
            );
            builder.parameters.push(symbol);
        }
        let mut values = Vec::new();
        for field in missing {
            let injection = field.injection.as_ref().expect("missing injected field");
            let mut effect_path = injection.action.path.clone();
            let action_name = effect_path
                .pop()
                .ok_or("constructor provider action identity is empty")?;
            let package = environment
                .external_public_metadata
                .iter()
                .find(|metadata| {
                    metadata
                        .actions
                        .iter()
                        .any(|action| action.path == injection.action.path)
                })
                .map(|metadata| metadata.package)
                .ok_or_else(|| {
                    format!(
                        "constructor provider has no imported checked action: {}",
                        injection.action.path.join(".")
                    )
                })?;
            let effect = builder.import(&effect_path, package.0, SymbolKind::Effect);
            let action_symbol = builder.symbol(
                &action_name,
                SymbolKind::EffectAction,
                SymbolDef::Synthetic {
                    reason: SyntheticSymbolReason::ExternalEffectAction,
                },
            );
            let args = injection
                .action
                .args
                .iter()
                .map(|arg| builder.effect_arg(arg))
                .collect::<Result<_, _>>()?;
            let generic_args = injection
                .generic_args
                .iter()
                .map(|arg| {
                    Ok(match arg {
                        ConstructorGenericArgument::Type(ty) => HirGenericArg::Type(builder.ty(ty)),
                        ConstructorGenericArgument::EffectRow(row) => {
                            HirGenericArg::EffectRow(builder.row(row)?)
                        }
                        ConstructorGenericArgument::Infer => {
                            HirGenericArg::Wildcard { span: builder.span }
                        }
                    })
                })
                .collect::<Result<_, String>>()?;
            let payload = if injection
                .action
                .path
                .iter()
                .map(String::as_str)
                .eq(["std", "effects", "Inject", "get"])
            {
                let value = builder
                    .hir
                    .exprs
                    .alloc(HirExpr::Literal(HirLiteral::String {
                        value: field.name.clone(),
                        span: builder.span,
                    }));
                builder.hir.source_map.expr_sources.insert(
                    value,
                    HirOrigin::Synthetic {
                        span: builder.span,
                        reason: SyntheticOriginReason::LoweringGenerated,
                    },
                );
                vec![HirArg::Positional(value)]
            } else {
                Vec::new()
            };
            let value = builder.hir.exprs.alloc(HirExpr::Perform {
                action: ResolvedActionRef {
                    effect: HirEffectRef {
                        path: builder.path(&effect_path, ResolveResult::Resolved(effect)),
                        args,
                        span: builder.span,
                    },
                    action: action_name,
                    action_symbol: ResolveResult::Resolved(action_symbol),
                    span: builder.span,
                },
                generic_args,
                args: payload,
                span: builder.span,
            });
            let origin = builder
                .hir
                .source_map
                .expr_sources
                .get(&expr)
                .and_then(HirOrigin::primary_syntax)
                .map(|primary| HirOrigin::Desugared {
                    primary,
                    related: Vec::new(),
                    desugaring: DesugaringKind::ConstructorInjection,
                })
                .unwrap_or(HirOrigin::Synthetic {
                    span: builder.span,
                    reason: SyntheticOriginReason::LoweringGenerated,
                });
            builder.hir.source_map.expr_sources.insert(value, origin);
            record.fields.push(HirFieldInit::Named {
                name: field.name.clone(),
                value,
                span: builder.span,
            });
            values.push(value);
        }
        record.injections = Some(HirRecordInjections {
            parameters: builder.parameters,
            values,
        });
        builder.hir.exprs[expr] = HirExpr::Record(record);
    }
    Ok(())
}

pub(super) fn canonical_imports(
    context: &ProjectContext,
    hir: &HirProgram,
) -> HashMap<SymbolId, Vec<String>> {
    let mut canonical = HashMap::new();
    if let Some(imports) = &context.resolved_imports {
        for (_, module) in hir.modules_arena.iter() {
            for import in &module.imports {
                let Some(binding) = &import.binding else {
                    continue;
                };
                let Some(resolved) = imports.imports.iter().find(|resolved| {
                    resolved.span == import.span && resolved.local_name == binding.local_name
                }) else {
                    continue;
                };
                if let ImportTarget::ExternalItem {
                    module_path, name, ..
                } = &resolved.target
                {
                    let mut path = module_path.segments.clone();
                    path.push(name.clone());
                    canonical.insert(binding.symbol, path);
                }
            }
        }
    }
    canonical
}

pub(super) fn external_path(
    hir: &HirProgram,
    path: &ResolvedPath,
    canonical: &HashMap<SymbolId, Vec<String>>,
) -> Option<Vec<String>> {
    let mut symbol = match &path.resolution {
        ResolveResult::Resolved(symbol) => *symbol,
        ResolveResult::PartiallyResolved(partial) => {
            let prefix = partial.resolved_prefix?;
            let mut path = canonical.get(&prefix).cloned().or_else(|| {
                match &hir.symbols.get(prefix)?.def {
                    SymbolDef::ImportAlias { path, .. } => Some(path.clone()),
                    _ => None,
                }
            })?;
            path.extend(partial.remaining.clone());
            return Some(path);
        }
        _ => return None,
    };
    let mut seen = HashSet::new();
    loop {
        symbol = hir.canonical_source_symbol(symbol)?;
        if !seen.insert(symbol) {
            return None;
        }
        if let Some(path) = canonical.get(&symbol) {
            return Some(path.clone());
        }
        match &hir.symbols.get(symbol)?.def {
            SymbolDef::ImportAlias { path, .. } => return Some(path.clone()),
            SymbolDef::Item { item } => match &hir.items[*item] {
                HirItem::TypeAlias(alias) => match &hir.types[alias.target] {
                    HirType::Path { path, .. } => return external_path(hir, path, canonical),
                    _ => return None,
                },
                _ => return None,
            },
            _ => return None,
        }
    }
}

fn find_contract<'a>(
    environment: &'a ProjectEnvironmentInput,
    path: &[String],
) -> Option<&'a ConstructorContract> {
    fn type_path(ty: &ProjectExternalTypeInput) -> Option<&[String]> {
        match ty {
            ProjectExternalTypeInput::Named(path)
            | ProjectExternalTypeInput::Applied { path, .. }
            | ProjectExternalTypeInput::Nominal { path, .. } => Some(path),
            ProjectExternalTypeInput::Alias { target, .. } => type_path(target),
            _ => None,
        }
    }
    let mut path = path.to_vec();
    let mut seen = HashSet::new();
    while seen.insert(path.clone()) {
        if let Some(contract) = environment
            .external_public_metadata
            .iter()
            .flat_map(|metadata| &metadata.constructor_contracts)
            .find(|contract| contract.path == path)
        {
            return Some(contract);
        }
        if let Some(export) = environment
            .external_public_metadata
            .iter()
            .flat_map(|metadata| &metadata.re_exports)
            .find(|export| export.exported == path)
        {
            path = export.from.clone();
            continue;
        }
        let alias = environment
            .external_public_metadata
            .iter()
            .flat_map(|metadata| &metadata.types)
            .find(|signature| signature.path == path)?;
        let ProjectExternalTypeInput::Alias { target, .. } = alias.ty.as_ref()? else {
            return None;
        };
        path = type_path(target)?.to_vec();
    }
    None
}

struct Builder<'a> {
    hir: &'a mut HirProgram,
    environment: &'a ProjectEnvironmentInput,
    module: HirModuleId,
    span: Span,
    parameters: Vec<SymbolId>,
}

impl Builder<'_> {
    fn symbol(&mut self, name: &str, kind: SymbolKind, def: SymbolDef) -> SymbolId {
        self.hir.symbols.alloc(SymbolData {
            name: name.to_owned(),
            kind,
            visibility: Visibility::Private,
            defining_module: self.module,
            defining_item: None,
            def,
            declared_type: None,
            definition_span: self.span,
        })
    }
    fn import(&mut self, path: &[String], package: u32, kind: SymbolKind) -> SymbolId {
        let symbol = self.symbol(
            path.last().expect("checked declaration path"),
            kind,
            SymbolDef::ImportAlias {
                path: path.to_vec(),
                origin: ImportAliasOrigin::SourceImport,
            },
        );
        self.hir
            .constructor_external_bindings
            .push(HirConstructorExternalBinding {
                symbol,
                package,
                path: path.to_vec(),
                span: self.span,
            });
        symbol
    }
    fn path(&self, names: &[String], resolution: ResolveResult) -> ResolvedPath {
        let mut path = unresolved_path_from_segments(
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
            self.span,
        );
        path.resolution = resolution;
        path
    }
    fn ty(&mut self, ty: &etas_package_metadata::Type) -> HirTypeId {
        self.hir.types.alloc(HirType::Metadata {
            ty: Box::new(ty.clone()),
            parameters: self.parameters.clone(),
            span: self.span,
        })
    }
    fn row(&mut self, row: &etas_package_metadata::EffectRow) -> Result<HirEffectRow, String> {
        let effects = row
            .effects
            .iter()
            .map(|effect| {
                let package = self
                    .environment
                    .external_public_metadata
                    .iter()
                    .find(|metadata| {
                        metadata
                            .effects
                            .iter()
                            .any(|signature| signature.path == effect.path)
                            || metadata.actions.iter().any(|action| {
                                action.path.starts_with(&effect.path)
                                    && action.path.len() == effect.path.len() + 1
                            })
                    })
                    .map(|metadata| metadata.package.0)
                    .ok_or_else(|| {
                        format!(
                            "constructor effect has no imported contract: {}",
                            effect.path.join(".")
                        )
                    })?;
                let symbol = self.import(&effect.path, package, SymbolKind::Effect);
                Ok(HirEffectRef {
                    path: self.path(&effect.path, ResolveResult::Resolved(symbol)),
                    args: effect
                        .args
                        .iter()
                        .map(|arg| self.effect_arg(arg))
                        .collect::<Result<_, _>>()?,
                    span: self.span,
                })
            })
            .collect::<Result<_, String>>()?;
        let tail = row
            .tail
            .as_ref()
            .map(|name| {
                let symbol = self
                    .parameters
                    .iter()
                    .copied()
                    .find(|symbol| {
                        self.hir
                            .symbols
                            .get(*symbol)
                            .expect("constructor parameter")
                            .name
                            == *name
                    })
                    .ok_or_else(|| {
                        format!("constructor effect parameter has no declaration: {name}")
                    })?;
                Ok::<_, String>(self.path(&[name.clone()], ResolveResult::Resolved(symbol)))
            })
            .transpose()?;
        Ok(HirEffectRow {
            effects,
            tail,
            span: self.span,
        })
    }
    fn effect_arg(
        &mut self,
        arg: &etas_package_metadata::EffectArg,
    ) -> Result<HirEffectArg, String> {
        Ok(match arg.kind {
            EffectArgKind::Type => HirEffectArg::Type(
                self.ty(arg
                    .ty
                    .as_ref()
                    .ok_or("constructor type selector is missing")?),
            ),
            EffectArgKind::Path => {
                let symbol = self.symbol(
                    arg.path
                        .last()
                        .ok_or("constructor static selector is empty")?,
                    SymbolKind::Import,
                    SymbolDef::ImportAlias {
                        path: arg.path.clone(),
                        origin: ImportAliasOrigin::SourceImport,
                    },
                );
                HirEffectArg::Path(self.path(&arg.path, ResolveResult::Resolved(symbol)))
            }
            EffectArgKind::String => HirEffectArg::String {
                value: arg.value.clone(),
                span: self.span,
            },
            EffectArgKind::Int => HirEffectArg::Int {
                text: arg.value.clone(),
                span: self.span,
            },
            EffectArgKind::Wildcard => HirEffectArg::Wildcard { span: self.span },
        })
    }
}
