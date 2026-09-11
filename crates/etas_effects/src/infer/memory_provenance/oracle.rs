use etas_hir::{HirExpr, HirExprId, HirProgram, ResolveResult, SymbolDef, SymbolId};
use etas_hir_analysis::{alias::*, unit::HirSemanticUnit};
use etas_types::{TypeOutput, pipeline::symbols::TypeSymbolIndex};

pub(super) struct MemoryOracle<'a> {
    hir: &'a HirProgram,
    types: &'a TypeOutput,
    registry: &'a etas_std::StdRegistry,
    symbols: TypeSymbolIndex,
}

impl<'a> MemoryOracle<'a> {
    pub(super) fn new(
        hir: &'a HirProgram,
        types: &'a TypeOutput,
        registry: &'a etas_std::StdRegistry,
    ) -> Self {
        Self {
            hir,
            types,
            registry,
            symbols: TypeSymbolIndex::build(hir),
        }
    }

    fn callee_symbol(&self, expr: HirExprId) -> Option<SymbolId> {
        let HirExpr::Path(path) = self.hir.exprs.get(expr)? else {
            return None;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        self.symbols.canonical_symbol(self.hir, symbol)
    }
}

impl AliasOracle for MemoryOracle<'_> {
    fn expression_target(&self, expr: HirExprId) -> Option<AliasTarget> {
        let ty = self.types.facts.expr_memory_places.get(&expr)?;
        let etas_types::Type::MemoryPlace(place) = self.types.store.get(*ty)? else {
            return None;
        };
        Some(AliasTarget::MemoryPlace(place.segments.clone()))
    }

    fn symbol_target(&self, symbol: SymbolId) -> Option<AliasTarget> {
        let symbol = self.symbols.canonical_symbol(self.hir, symbol)?;
        self.types.facts.resource_handles.get(&symbol)?;
        let symbol = self.hir.symbols.get(symbol)?;
        let module = self.hir.modules_arena.get(symbol.defining_module)?;
        let mut path = module
            .name
            .as_ref()
            .map(|name| {
                name.segments
                    .iter()
                    .map(|s| s.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        path.push(symbol.name.clone());
        Some(AliasTarget::MemoryPlace(path))
    }

    fn allocation_site(&self, _expr: HirExprId, _kind: AllocationKind) -> Option<AliasTarget> {
        None
    }

    fn intrinsic_call_summary(
        &self,
        _call: HirExprId,
        callee: HirExprId,
    ) -> Option<AliasIntrinsicSummary> {
        let symbol = self.hir.symbols.get(self.callee_symbol(callee)?)?;
        let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
            return None;
        };
        let intrinsic = self.registry.lookup_qualified(path)?;
        let argument = self.registry.memory_place_result_argument(intrinsic.id)?;
        Some(AliasIntrinsicSummary::returns(AliasValueExpr::FormalParam(
            argument,
        )))
    }

    fn resolved_call_target(&self, _call: HirExprId, callee: HirExprId) -> AliasCallTarget {
        let Some(symbol) = self
            .callee_symbol(callee)
            .and_then(|s| self.hir.symbols.get(s))
        else {
            return AliasCallTarget::Incomplete;
        };
        match symbol.def {
            SymbolDef::Item { item } => AliasCallTarget::Direct(HirSemanticUnit::Item(item)),
            SymbolDef::ImportAlias { .. } => AliasCallTarget::External,
            _ => AliasCallTarget::Dynamic,
        }
    }

    fn resource_constructor(&self, call: HirExprId) -> Option<AliasTarget> {
        self.types.facts.resource_handles.keys().find_map(|symbol| {
            let SymbolDef::TopLevelLet { initializer, .. } = self.hir.symbols.get(*symbol)?.def
            else {
                return None;
            };
            (initializer == call)
                .then(|| self.symbol_target(*symbol))
                .flatten()
        })
    }
}
