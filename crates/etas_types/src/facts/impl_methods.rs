#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct TypeImplMethodFact {
    pub symbol: etas_hir::SymbolId,
    pub name: String,
    pub implementation: etas_hir::HirItemId,
    pub body: etas_hir::HirItemId,
    pub self_type: crate::TypeId,
    pub inherent: bool,
}
