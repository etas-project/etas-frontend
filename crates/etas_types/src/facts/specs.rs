use crate::{CallableSignature, TypeId};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct SpecFacts {
    pub external_signatures:
        std::collections::HashMap<ExternalSpecIdentity, ExternalSpecSignatureFact>,
    pub external_impls: Vec<ExternalSpecImplFact>,
    pub external_methods: Vec<ExternalSpecImplMethodFact>,
    pub external_aliases: std::collections::HashMap<etas_hir::SymbolId, ExternalSpecIdentity>,
    pub signatures: std::collections::HashMap<etas_hir::SymbolId, SpecSignature>,
    pub impls: Vec<SpecImplFact>,
    pub type_satisfactions: Vec<TypeSpecSatisfactionFact>,
    pub callable_satisfactions: Vec<CallableSpecSatisfactionFact>,
    pub trace_conformances: Vec<TraceSpecConformanceFact>,
    pub external_callable_satisfactions: Vec<ExternalCallableSpecSatisfactionFact>,
    pub external_trace_conformances: Vec<ExternalTraceSpecConformanceFact>,
    pub type_param_bounds: std::collections::HashMap<etas_hir::SymbolId, Vec<TypeParamBoundFact>>,
    pub std_impls: Vec<CheckedStdSpecImplFact>,
    pub std_spec_aliases: std::collections::HashMap<etas_hir::SymbolId, Vec<String>>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum CheckedSpecRef {
    Source(etas_hir::SymbolId),
    Std(Vec<String>),
    External(ExternalSpecIdentity),
}

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ExternalSpecIdentity {
    pub package: u32,
    pub path: Vec<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct ExternalSpecSignatureFact {
    pub kind: SpecKind,
    pub param_names: Vec<String>,
    pub super_specs: Vec<CheckedSpecBound>,
    pub methods: Vec<SpecMethodFact>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct ExternalSpecImplFact {
    pub self_type: TypeId,
    pub spec: ExternalSpecIdentity,
    pub args: Vec<TypeId>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct ExternalSpecImplMethodFact {
    pub span: etas_core::Span,
    pub identity: etas_code::MethodId,
    pub self_type: TypeId,
    pub spec: CheckedSpecRef,
    pub args: Vec<TypeId>,
    pub signature: CallableSignature,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct ExternalSpecFacts {
    pub signatures: std::collections::HashMap<ExternalSpecIdentity, ExternalSpecSignatureFact>,
    pub impls: Vec<ExternalSpecImplFact>,
    pub methods: Vec<ExternalSpecImplMethodFact>,
    pub aliases: std::collections::HashMap<etas_hir::SymbolId, ExternalSpecIdentity>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct CheckedSpecBound {
    pub spec: CheckedSpecRef,
    pub args: Vec<TypeId>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct CheckedStdSpecImplFact {
    pub self_type: TypeId,
    pub spec: Vec<String>,
    pub args: Vec<TypeId>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct SpecSignature {
    pub symbol: etas_hir::SymbolId,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: SpecKind,
    pub params: Vec<etas_hir::SymbolId>,
    #[serde(default)]
    pub param_names: Vec<String>,
    pub callable: Option<CallableSignature>,
    pub methods: Vec<SpecMethodFact>,
    pub super_specs: Vec<SpecSuperBoundFact>,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum SpecKind {
    #[default]
    TypeSpec,
    CallableSpec,
    TraceSpec,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct SpecImplFact {
    pub self_type: TypeId,
    pub spec_symbol: etas_hir::SymbolId,
    pub args: Vec<TypeId>,
    pub methods: Vec<SpecImplMethodFact>,
    pub impl_group: u32,
    pub span: etas_core::Span,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct TypeSpecSatisfactionFact {
    pub self_type: TypeId,
    pub spec_symbol: etas_hir::SymbolId,
    pub args: Vec<TypeId>,
    pub impl_group: u32,
    pub span: etas_core::Span,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct CallableSpecSatisfactionFact {
    pub item: etas_hir::HirItemId,
    pub symbol: etas_hir::SymbolId,
    pub spec_symbol: etas_hir::SymbolId,
    pub args: Vec<TypeId>,
    pub effect_bound: Option<crate::EffectRowRef>,
    pub span: etas_core::Span,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct TraceSpecConformanceFact {
    pub item: etas_hir::HirItemId,
    pub symbol: etas_hir::SymbolId,
    pub target: TraceSpecConformanceTarget,
    pub span: etas_core::Span,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct ExternalCallableSpecSatisfactionFact {
    pub item: Vec<String>,
    pub spec: Vec<String>,
    pub args: Vec<TypeId>,
    pub span: etas_core::Span,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct ExternalTraceSpecConformanceFact {
    pub item: Vec<String>,
    pub target: ExternalTraceSpecConformanceTarget,
    pub span: etas_core::Span,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub enum ExternalTraceSpecConformanceTarget {
    Named {
        spec: Vec<String>,
        args: Vec<TypeId>,
    },
    Inline,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub enum TraceSpecConformanceTarget {
    Named {
        spec_symbol: etas_hir::SymbolId,
        args: Vec<TypeId>,
    },
    Inline,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct SpecMethodFact {
    pub identity: SpecMethodIdentity,
    pub name: String,
    pub receiver_param: Option<u32>,
    #[serde(default)]
    pub signature: Option<CallableSignature>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub enum SpecMethodIdentity {
    Source(etas_hir::SymbolId),
    External { package: u32, path: Vec<String> },
}

impl SpecMethodFact {
    pub fn source_symbol(&self) -> Option<etas_hir::SymbolId> {
        match self.identity {
            SpecMethodIdentity::Source(symbol) => Some(symbol),
            SpecMethodIdentity::External { .. } => None,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct SpecImplMethodFact {
    pub symbol: etas_hir::SymbolId,
    pub name: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct TypeParamBoundFact {
    pub param: etas_hir::SymbolId,
    #[serde(default)]
    pub param_name: String,
    pub spec_symbol: etas_hir::SymbolId,
    pub args: Vec<TypeId>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct SpecSuperBoundFact {
    pub spec_symbol: etas_hir::SymbolId,
    pub super_spec: CheckedSpecRef,
    pub args: Vec<TypeId>,
}
