use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_hir::HirProgram;
use std::{collections::HashMap, sync::Arc};

use crate::{TypeFacts, TypeInterner, TypeOutput, pipeline::symbols::TypeSymbolIndex};

pub struct TypePipelineContext<'a> {
    pub hir: &'a HirProgram,
    pub std_registry: Arc<etas_std::StdRegistry>,
    pub interner: TypeInterner,
    pub signature_facts: TypeFacts,
    pub symbols: Arc<TypeSymbolIndex>,
    pub(crate) signature_base: Option<&'a crate::SignatureEnvironment>,
    pub diagnostics: Vec<Diagnostic>,
    pub lowering_type_stack: Vec<etas_hir::SymbolId>,
    pub external_type_paths: HashMap<Vec<String>, crate::TypeId>,
}

pub struct BodyCollectContext<'a, 's> {
    pub ctx: &'s mut TypePipelineContext<'a>,
    pub state: &'s mut crate::pipeline::body::state::BodyPipelineState,
}

impl BodyCollectContext<'_, '_> {
    pub fn fresh_type_var(&mut self) -> crate::TypeId {
        let id = crate::TypeVarId(self.ctx.interner.store().len() as u32);
        self.ctx.interner.intern(crate::Type::Var(id))
    }

    pub fn primitive(&mut self, primitive: crate::PrimitiveType) -> crate::TypeId {
        self.ctx.interner.primitive(primitive)
    }

    pub fn record_expr_type(
        &mut self,
        expr: etas_hir::HirExprId,
        ty: crate::TypeId,
    ) -> crate::TypeId {
        self.state.provisional.expr_types.insert(expr, ty);
        ty
    }

    pub fn record_expr_memory_place(
        &mut self,
        expr: etas_hir::HirExprId,
        ty: crate::TypeId,
    ) -> crate::TypeId {
        self.state.provisional.expr_memory_places.insert(expr, ty);
        ty
    }

    pub fn record_stmt_type(
        &mut self,
        stmt: etas_hir::HirStmtId,
        ty: crate::TypeId,
    ) -> crate::TypeId {
        self.state.provisional.stmt_types.insert(stmt, ty);
        ty
    }

    pub fn record_pat_type(&mut self, pat: etas_hir::HirPatId, ty: crate::TypeId) -> crate::TypeId {
        self.state.provisional.pat_types.insert(pat, ty);
        ty
    }

    pub fn record_symbol_type(&mut self, symbol: etas_hir::SymbolId, fact: crate::SymbolTypeFact) {
        self.state.provisional.symbol_types.insert(symbol, fact);
    }

    pub fn emit(&mut self, constraint: crate::TypeConstraint) {
        self.state.constraints.push(constraint);
    }

    pub fn validate(&mut self, request: crate::ValidationRequest) {
        self.state.validations.push(request);
    }
}

impl<'a> TypePipelineContext<'a> {
    pub fn new(hir: &'a HirProgram) -> Self {
        Self::new_with_std_registry(hir, Arc::new(etas_std::standard_registry()))
    }

    pub fn new_with_std_registry(
        hir: &'a HirProgram,
        std_registry: Arc<etas_std::StdRegistry>,
    ) -> Self {
        Self {
            hir,
            std_registry,
            interner: TypeInterner::new(),
            signature_facts: TypeFacts::default(),
            symbols: Arc::new(TypeSymbolIndex::build(hir)),
            signature_base: None,
            diagnostics: Vec::new(),
            lowering_type_stack: Vec::new(),
            external_type_paths: HashMap::new(),
        }
    }

    pub fn unknown_type(
        &mut self,
        span: etas_core::Span,
        message: impl Into<String>,
    ) -> crate::TypeId {
        self.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::UnknownType,
            span,
            message,
        ));
        self.interner.primitive(crate::PrimitiveType::Never)
    }

    pub fn finish(self) -> TypeOutput {
        TypeOutput {
            facts: Arc::new(self.signature_facts),
            store: Arc::new(self.interner.into_store()),
            diagnostics: self.diagnostics,
        }
    }

    pub(crate) fn for_body(
        hir: &'a HirProgram,
        base: &'a crate::SignatureEnvironment,
        std_registry: Arc<etas_std::StdRegistry>,
    ) -> Self {
        Self {
            hir,
            std_registry,
            interner: TypeInterner::from_store(crate::TypeStore::with_base(base.types.clone())),
            signature_facts: TypeFacts::default(),
            symbols: base.symbols.clone(),
            signature_base: Some(base),
            diagnostics: Vec::new(),
            lowering_type_stack: Vec::new(),
            external_type_paths: HashMap::new(),
        }
    }

    fn lookup<'b, K: std::hash::Hash + Eq + 'b, V>(
        &'b self,
        field: impl Fn(&TypeFacts) -> &HashMap<K, V>,
        key: &K,
    ) -> Option<&'b V> {
        field(&self.signature_facts)
            .get(key)
            .or_else(|| field(&self.signature_base?.facts).get(key))
    }

    pub(crate) fn symbol_type(&self, symbol: etas_hir::SymbolId) -> Option<&crate::SymbolTypeFact> {
        self.lookup(|facts| &facts.symbol_types, &symbol)
    }

    pub(crate) fn symbol_fact(&self, symbol: etas_hir::SymbolId) -> Option<&crate::SymbolTypeFact> {
        let canonical = self
            .symbols
            .canonical_symbol(self.hir, symbol)
            .unwrap_or(symbol);
        self.symbol_type(canonical)
            .or_else(|| self.symbol_type(symbol))
    }

    pub(crate) fn type_fact_as_type_id(&self, symbol: etas_hir::SymbolId) -> Option<crate::TypeId> {
        crate::pipeline::symbols::symbol_type_fact_as_type_id(self.symbol_fact(symbol)?)
    }

    pub(crate) fn item_signature(
        &self,
        item: etas_hir::HirItemId,
    ) -> Option<&crate::ItemSignature> {
        self.lookup(|facts| &facts.item_signatures, &item)
    }

    pub(crate) fn spec_signature(
        &self,
        symbol: etas_hir::SymbolId,
    ) -> Option<&crate::SpecSignature> {
        self.lookup(|facts| &facts.spec_signatures, &symbol)
    }

    pub(crate) fn action_signature(
        &self,
        symbol: etas_hir::SymbolId,
    ) -> Option<&crate::EffectActionSignature> {
        self.lookup(|facts| &facts.action_signatures, &symbol)
    }

    pub(crate) fn qualified_action_signature(
        &self,
        name: &String,
    ) -> Option<&crate::EffectActionSignature> {
        self.lookup(|facts| &facts.qualified_action_signatures, name)
    }

    pub(crate) fn enum_layout(&self, ty: crate::TypeId) -> Option<&crate::EnumLayoutFact> {
        self.lookup(|facts| &facts.enum_layouts, &ty)
    }

    pub(crate) fn type_ref(&self, ty: etas_hir::HirTypeId) -> Option<crate::TypeId> {
        self.lookup(|facts| &facts.type_refs, &ty).copied()
    }

    pub(crate) fn expr_memory_place(&self, expr: etas_hir::HirExprId) -> Option<&crate::TypeId> {
        self.lookup(|facts| &facts.expr_memory_places, &expr)
    }

    pub(crate) fn known_std_types(&self) -> &crate::KnownStdTypes {
        &self
            .signature_base
            .map_or(&self.signature_facts, |base| &base.facts)
            .known_std_types
    }
}
