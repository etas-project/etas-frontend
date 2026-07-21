use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_hir::HirProgram;
use std::collections::HashMap;

use crate::{TypeFacts, TypeInterner, TypeOutput, pipeline::symbols::TypeSymbolIndex};

pub struct TypePipelineContext<'a> {
    pub hir: &'a HirProgram,
    pub interner: TypeInterner,
    pub signature_facts: TypeFacts,
    pub symbols: TypeSymbolIndex,
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
        let id = crate::TypeVarId(self.ctx.interner.store().iter().count() as u32);
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
        Self {
            hir,
            interner: TypeInterner::new(),
            signature_facts: TypeFacts::default(),
            symbols: TypeSymbolIndex::build(hir),
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
            facts: self.signature_facts,
            store: self.interner.into_store(),
            diagnostics: self.diagnostics,
        }
    }
}
