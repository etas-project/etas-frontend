//! Reify contextual constructors as ordinary calls before any effect, callable
//! or execution analysis. Type checking has already selected their real source
//! method and generic arguments; no collection values are manufactured here.
use std::{collections::HashMap, sync::Arc};

use etas_hir::{HirExpr, ResolveResult, ResolvedPath, SymbolDef};
use etas_types::{SymbolTypeFact, Type, TypeId, TypeOutput, method::MethodReceiver};

use crate::HirOutput;

pub(super) fn reify(output: &mut HirOutput, types: &mut TypeOutput) -> Result<bool, String> {
    if !output.hir.source_standard_types {
        return Ok(false);
    }
    let mut calls = types
        .facts
        .checked_method_calls
        .iter()
        .filter_map(|(expr, method)| {
            let (name, span) = match output.hir.exprs.get(*expr)? {
                HirExpr::EmptyRecordOrMap { span } => ("Map", *span),
                _ => return None,
            };
            Some((*expr, name, span, method.receiver_type))
        })
        .collect::<Vec<_>>();
    calls.sort_by_key(|(expr, ..)| expr.0);
    if calls.is_empty() {
        return Ok(false);
    }
    let owners = output.hir.index().expr_item.clone();
    let mut editor = output.hir.clone().edit();
    let hir = editor.program_mut();
    let facts = Arc::make_mut(&mut types.facts);
    let mut aliases = HashMap::new();
    for (expr, name, span, owner) in calls {
        let method = &facts.checked_method_calls[&expr];
        if method.receiver != MethodReceiver::Associated
            || !method.signature.params.is_empty()
            || !method.argument_slots.is_empty()
        {
            return Err(
                "contextual empty constructor is not a checked nullary associated call".into(),
            );
        }
        let Type::Nominal(declaration) = types
            .store
            .get(owner)
            .ok_or("contextual constructor has no checked type subject")?
        else {
            return Err("contextual collection constructor has no nominal source owner".into());
        };
        let item = *owners
            .get(&expr)
            .ok_or("contextual constructor has no enclosing item")?;
        let module = hir
            .item_module(item)
            .ok_or("contextual constructor has no source module")?;
        let existing = aliases.get(&(module, owner)).copied().or_else(|| {
            facts
                .symbol_types
                .iter()
                .filter_map(|(symbol, fact)| match fact {
                    SymbolTypeFact::NominalType { constructor, .. }
                        if TypeId(constructor.0) == owner
                            && hir.symbols.get(*symbol).is_some_and(|symbol| {
                                symbol.defining_module == module
                                    || (symbol.kind == etas_hir::SymbolKind::Type
                                        && symbol.visibility == etas_hir::Visibility::Public)
                            }) =>
                    {
                        Some(*symbol)
                    }
                    _ => None,
                })
                .min_by_key(|symbol| symbol.0)
        });
        let symbol = if let Some(symbol) = existing {
            symbol
        } else {
            // Compiled std can expose a type through a callable signature before
            // an application spells that type name. This private reference
            // names the selected provider's existing declaration.
            let symbol = hir.symbols.alloc(etas_hir::SymbolData {
                name: format!("__literal_{name}"),
                kind: etas_hir::SymbolKind::StdPreludeAlias,
                visibility: etas_hir::Visibility::Private,
                defining_module: module,
                defining_item: Some(item),
                def: SymbolDef::ImportAlias {
                    path: ["std", "collections", name].map(str::to_owned).to_vec(),
                    origin: etas_hir::ImportAliasOrigin::StdPrelude,
                },
                declared_type: None,
                definition_span: span,
            });
            facts.symbol_types.insert(
                symbol,
                SymbolTypeFact::NominalType {
                    constructor: etas_types::TypeConstructorId(owner.0),
                    params: declaration.params.clone(),
                    representation: declaration.representation,
                },
            );
            hir.source_map.symbol_sources.insert(
                symbol,
                etas_hir::HirOrigin::Synthetic {
                    span,
                    reason: etas_hir::SyntheticOriginReason::LoweringGenerated,
                },
            );
            symbol
        };
        aliases.insert((module, owner), symbol);
        let syntax = etas_syntax::ast::Path {
            segments: vec![etas_syntax::ast::Name {
                text: name.into(),
                span,
            }],
            span,
        };
        let receiver = hir.exprs.alloc(HirExpr::Path(ResolvedPath {
            syntax_path: syntax,
            segments: vec![etas_hir::PathSegment {
                name: name.into(),
                span,
            }],
            resolution: ResolveResult::Resolved(symbol),
            span,
        }));
        hir.source_map.expr_sources.insert(
            receiver,
            etas_hir::HirOrigin::Synthetic {
                span,
                reason: etas_hir::SyntheticOriginReason::LoweringGenerated,
            },
        );
        facts.expr_types.insert(receiver, owner);
        hir.source_map.mark_collection_literal(expr, span);
        *hir.exprs
            .get_mut(expr)
            .expect("constructor expression was selected from HIR") = HirExpr::MethodCall {
            receiver,
            method: "new".into(),
            generic_args: Vec::new(),
            args: Vec::new(),
            span,
        };
    }
    output.hir = editor
        .finish()
        .map_err(|error| format!("invalid contextual constructor HIR: {error}"))?;
    Ok(true)
}
