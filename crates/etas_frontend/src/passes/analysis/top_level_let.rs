use etas_core::{Diagnostic, SyntaxDiagnosticCode};
use etas_hir::{
    HirArg, HirExpr, HirFieldInit, HirGenericArg, HirItem, HirTypeId, PathSegment, ResourceKind,
    SymbolDef, SymbolId, TopLevelLetClassification,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::ProjectContext;
use crate::output::{TopLevelLetFact, TopLevelLetFacts};
use crate::passes::artifacts::{
    HIR_OUTPUT, RESOLVED_PATHS, SIGNATURE_FACTS, TOP_LEVEL_LET_FACTS, global_with_diagnostics,
};

pub struct ValidateTopLevelLetPass;

impl Pass<ProjectContext> for ValidateTopLevelLetPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ValidateTopLevelLetPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                SIGNATURE_FACTS,
                RESOLVED_PATHS,
            ]))
            .produces(global_with_diagnostics([TOP_LEVEL_LET_FACTS]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let hir = context.hir.as_mut().expect("HIR output should exist");
        let mut types = context
            .signature_types
            .take()
            .expect("signature facts should exist before top-level let validation");
        types.diagnostics.clear();
        let mut checked = etas_types::check_top_level_items(&hir.hir, types);
        context
            .diagnostics
            .extend(checked.diagnostics.iter().cloned());

        let mut facts = TopLevelLetFacts::default();
        let mut changed = true;
        while changed {
            changed = false;
            for (item_id, item) in hir.hir.items.iter() {
                let HirItem::TopLevelLet(top_level_let) = item else {
                    continue;
                };
                let Some(ty_fact) = checked.facts.symbol_types.get(&top_level_let.symbol) else {
                    continue;
                };
                let etas_types::SymbolTypeFact::TopLevelLet { ty, classification } = ty_fact else {
                    continue;
                };
                let next =
                    classify_top_level_let(&hir.hir, &checked.facts, &facts, top_level_let.value)
                        .unwrap_or_else(|| classification.clone());
                let existing = facts
                    .symbols
                    .get(&top_level_let.symbol)
                    .map(|fact| fact.classification.clone());
                if existing.as_ref() != Some(&next) {
                    changed = true;
                }
                let fact = TopLevelLetFact {
                    ty: *ty,
                    classification: next.clone(),
                };
                facts.items.insert(item_id, fact.clone());
                facts.symbols.insert(top_level_let.symbol, fact);
            }
        }

        for (item_id, fact) in &facts.items {
            if let Some(HirItem::TopLevelLet(item)) = hir.hir.items.get_mut(*item_id) {
                item.classification = fact.classification.clone();
                if let Some(symbol) = hir.hir.symbols.get_mut(item.symbol)
                    && let SymbolDef::TopLevelLet { classification, .. } = &mut symbol.def
                {
                    *classification = fact.classification.clone();
                }
            }
        }

        for (symbol_id, fact) in &facts.symbols {
            if let Some(etas_types::SymbolTypeFact::TopLevelLet { classification, .. }) =
                checked.facts.symbol_types.get_mut(symbol_id)
            {
                *classification = fact.classification.clone();
            }
        }

        for (item_id, fact) in &facts.items {
            if matches!(fact.classification, TopLevelLetClassification::Invalid)
                && let Some(HirItem::TopLevelLet(item)) = hir.hir.items.get(*item_id)
            {
                context.diagnostics.push(Diagnostic::syntax(
                    SyntaxDiagnosticCode::InvalidItem,
                    item.span,
                    "top-level `let` initializer must be a compile-time constant, handler value, or `std.memory.region<...>` resource constructor",
                ));
            }
        }

        context.signature_types = Some(checked);
        context.top_level_lets = Some(facts);
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(TOP_LEVEL_LET_FACTS),
        )
    }
}

fn classify_top_level_let(
    hir: &etas_hir::HirProgram,
    types: &etas_types::TypeFacts,
    facts: &TopLevelLetFacts,
    expr: etas_hir::HirExprId,
) -> Option<TopLevelLetClassification> {
    match &hir.exprs[expr] {
        HirExpr::Literal(_) => Some(TopLevelLetClassification::Const),
        HirExpr::Tuple { elems, .. }
        | HirExpr::Array { elems, .. }
        | HirExpr::List { elems, .. }
        | HirExpr::Set { elems, .. } => elems
            .iter()
            .map(|expr| classify_top_level_let(hir, types, facts, *expr))
            .collect::<Option<Vec<_>>>()
            .map(|_| TopLevelLetClassification::Const),
        HirExpr::ListCons { head, tail, .. } => [
            classify_top_level_let(hir, types, facts, *head),
            classify_top_level_let(hir, types, facts, *tail),
        ]
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .map(|_| TopLevelLetClassification::Const),
        HirExpr::Range { start, end, .. } => [
            classify_top_level_let(hir, types, facts, *start),
            classify_top_level_let(hir, types, facts, *end),
        ]
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .map(|_| TopLevelLetClassification::Const),
        HirExpr::Slice {
            base, start, end, ..
        } => [
            classify_top_level_let(hir, types, facts, *base),
            classify_top_level_let(hir, types, facts, *start),
            classify_top_level_let(hir, types, facts, *end),
        ]
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .map(|_| TopLevelLetClassification::Const),
        HirExpr::EmptySequence { .. } | HirExpr::EmptyRecordOrMap { .. } => {
            Some(TopLevelLetClassification::Const)
        }
        HirExpr::Map { entries, .. } => entries
            .iter()
            .flat_map(|entry| [entry.key, entry.value])
            .map(|expr| classify_top_level_let(hir, types, facts, expr))
            .collect::<Option<Vec<_>>>()
            .map(|_| TopLevelLetClassification::Const),
        HirExpr::Record(record) => record
            .fields
            .iter()
            .map(|field| match field {
                HirFieldInit::Named { value, .. } => {
                    classify_top_level_let(hir, types, facts, *value)
                }
                HirFieldInit::Shorthand { resolution, .. } => match resolution {
                    etas_hir::ResolveResult::Resolved(symbol) => {
                        classify_symbol_reference(facts, *symbol)
                    }
                    _ => Some(TopLevelLetClassification::Invalid),
                },
            })
            .collect::<Option<Vec<_>>>()
            .map(|_| TopLevelLetClassification::Const),
        HirExpr::Path(path) => match path.resolution {
            etas_hir::ResolveResult::Resolved(symbol) => classify_symbol_reference(facts, symbol),
            _ => Some(TopLevelLetClassification::Invalid),
        },
        HirExpr::Handler { .. } => Some(TopLevelLetClassification::Handler),
        HirExpr::MethodCall {
            receiver,
            method,
            generic_args,
            args,
            ..
        } if method == "region" && is_std_memory_module_receiver(hir, *receiver) => {
            for arg in args {
                let value = match arg {
                    HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
                };
                if !matches!(
                    classify_top_level_let(hir, types, facts, value),
                    Some(TopLevelLetClassification::Const)
                ) {
                    return Some(TopLevelLetClassification::Invalid);
                }
            }
            let Some(schema_ref) = single_type_generic_arg(generic_args) else {
                return Some(TopLevelLetClassification::Invalid);
            };
            if !types.type_refs.contains_key(&schema_ref) {
                return Some(TopLevelLetClassification::Invalid);
            }
            Some(TopLevelLetClassification::ResourceHandle(
                ResourceKind::MemoryRegion,
            ))
        }
        HirExpr::Call {
            callee,
            generic_args,
            args,
            ..
        } if is_std_memory_region_callee(hir, *callee) => {
            for arg in args {
                let value = match arg {
                    HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
                };
                if !matches!(
                    classify_top_level_let(hir, types, facts, value),
                    Some(TopLevelLetClassification::Const)
                ) {
                    return Some(TopLevelLetClassification::Invalid);
                }
            }
            let Some(schema_ref) = single_type_generic_arg(generic_args) else {
                return Some(TopLevelLetClassification::Invalid);
            };
            if !types.type_refs.contains_key(&schema_ref) {
                return Some(TopLevelLetClassification::Invalid);
            }
            Some(TopLevelLetClassification::ResourceHandle(
                ResourceKind::MemoryRegion,
            ))
        }
        _ => Some(TopLevelLetClassification::Invalid),
    }
}

fn classify_symbol_reference(
    facts: &TopLevelLetFacts,
    symbol: SymbolId,
) -> Option<TopLevelLetClassification> {
    match facts.symbols.get(&symbol)?.classification {
        TopLevelLetClassification::Const => Some(TopLevelLetClassification::Const),
        TopLevelLetClassification::Handler => Some(TopLevelLetClassification::Handler),
        TopLevelLetClassification::Unknown => None,
        TopLevelLetClassification::ResourceHandle(_) | TopLevelLetClassification::Invalid => {
            Some(TopLevelLetClassification::Invalid)
        }
    }
}

fn is_std_memory_region_callee(hir: &etas_hir::HirProgram, callee: etas_hir::HirExprId) -> bool {
    let HirExpr::Path(path) = &hir.exprs[callee] else {
        return false;
    };
    match &path.resolution {
        etas_hir::ResolveResult::Resolved(symbol) => {
            hir.symbols.get(*symbol).is_some_and(|symbol| {
                matches!(
                    &symbol.def,
                    SymbolDef::ImportAlias { path, .. }
                        if path == &["std".to_owned(), "memory".to_owned(), "region".to_owned()]
                ) || path_name(&path.segments) == "std.memory.region"
            })
        }
        etas_hir::ResolveResult::PartiallyResolved(partial)
            if partial.reason == etas_hir::PartialResolutionReason::PackageResolverRequired =>
        {
            path_name(&path.segments) == "std.memory.region"
        }
        _ => false,
    }
}

fn path_name(segments: &[PathSegment]) -> String {
    segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

fn single_type_generic_arg(args: &[HirGenericArg]) -> Option<HirTypeId> {
    match args {
        [HirGenericArg::Type(ty)] => Some(*ty),
        _ => None,
    }
}

fn is_std_memory_module_receiver(
    hir: &etas_hir::HirProgram,
    receiver: etas_hir::HirExprId,
) -> bool {
    let HirExpr::Path(path) = &hir.exprs[receiver] else {
        return false;
    };
    path_name(&path.segments) == "std.memory"
}
