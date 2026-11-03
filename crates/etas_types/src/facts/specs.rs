use crate::{CallableSignature, TypeId};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct SpecFacts {
    #[serde(default)]
    pub canonical: CanonicalSpecFacts,
    pub external_signatures:
        std::collections::HashMap<ExternalSpecIdentity, ExternalSpecSignatureFact>,
    pub external_impls: Vec<ExternalSpecImplFact>,
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
    Canonical(etas_code::DefinitionKey),
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct CanonicalSpecFacts {
    pub signatures: std::collections::HashMap<etas_code::DefinitionKey, CanonicalSpecSignatureFact>,
    pub impls: Vec<CanonicalSpecImplFact>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct CanonicalSpecSignatureFact {
    pub kind: SpecKind,
    pub param_names: Vec<String>,
    pub param_kinds: Vec<crate::CallableGenericParamKind>,
    pub param_bounds: Vec<Vec<CheckedSpecBound>>,
    pub super_specs: Vec<CheckedSpecBound>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct CanonicalSpecImplFact {
    pub self_type: TypeId,
    pub spec: etas_code::DefinitionKey,
    pub args: Vec<TypeId>,
}

#[derive(Clone, Debug, Default)]
pub struct RelocatedSpecFacts {
    pub canonical: CanonicalSpecFacts,
    pub std_impls: Vec<CheckedStdSpecImplFact>,
}

#[derive(Debug)]
pub enum RelocatedSpecValidationError {
    Invalid(&'static str),
    Interrupted(etas_utils::PipelineInterruption),
}

impl From<&'static str> for RelocatedSpecValidationError {
    fn from(reason: &'static str) -> Self {
        Self::Invalid(reason)
    }
}

impl RelocatedSpecFacts {
    pub fn validate(
        &self,
        existing: &crate::TypeFacts,
        store: &crate::TypeStore,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<(), RelocatedSpecValidationError> {
        let checkpoint = || {
            control
                .map_or(Ok(()), |control| control.checkpoint())
                .map_err(RelocatedSpecValidationError::Interrupted)
        };
        let signature = |key: &etas_code::DefinitionKey| {
            self.canonical
                .signatures
                .get(key)
                .or_else(|| existing.canonical_specs.signatures.get(key))
        };
        for (identity, fact) in &self.canonical.signatures {
            checkpoint()?;
            if identity.kind() != etas_code::DefinitionKind::Spec
                || fact.param_names.len() != fact.param_kinds.len()
                || fact.param_names.len() != fact.param_bounds.len()
                || existing
                    .canonical_specs
                    .signatures
                    .get(identity)
                    .is_some_and(|old| old != fact)
            {
                return Err("invalid or conflicting canonical spec declaration".into());
            }
            for bound in fact.param_bounds.iter().flatten().chain(&fact.super_specs) {
                checkpoint()?;
                for ty in &bound.args {
                    checkpoint()?;
                    if store.get(*ty).is_none() {
                        return Err("canonical spec bound references a missing type".into());
                    }
                }
                match &bound.spec {
                    CheckedSpecRef::Canonical(key) => {
                        let Some(target) = signature(key) else {
                            return Err("canonical super-spec declaration is missing".into());
                        };
                        if target.kind != SpecKind::TypeSpec
                            || target.param_names.len() != bound.args.len()
                        {
                            return Err(
                                "canonical super-spec has an incompatible kind or arity".into()
                            );
                        }
                    }
                    CheckedSpecRef::Std(_) => {}
                    CheckedSpecRef::Source(_) | CheckedSpecRef::External(_) => {
                        return Err("relocated spec contains a source-local reference".into());
                    }
                }
            }
        }
        for implementation in &self.canonical.impls {
            checkpoint()?;
            let Some(declaration) = signature(&implementation.spec) else {
                return Err("canonical implementation has no declaration".into());
            };
            if declaration.kind != SpecKind::TypeSpec
                || declaration.param_names.len() != implementation.args.len()
                || store.get(implementation.self_type).is_none()
            {
                return Err("canonical implementation has invalid checked types or arity".into());
            }
            for ty in &implementation.args {
                checkpoint()?;
                if store.get(*ty).is_none() {
                    return Err(
                        "canonical implementation has invalid checked types or arity".into(),
                    );
                }
            }
        }
        for implementation in &self.std_impls {
            checkpoint()?;
            if store.get(implementation.self_type).is_none() {
                return Err("standard spec implementation references a missing type".into());
            }
            for ty in &implementation.args {
                checkpoint()?;
                if store.get(*ty).is_none() {
                    return Err("standard spec implementation references a missing type".into());
                }
            }
        }
        Ok(())
    }

    pub fn merge_into(
        self,
        facts: &mut crate::TypeFacts,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<(), etas_utils::PipelineInterruption> {
        let checkpoint = || control.map_or(Ok(()), |control| control.checkpoint());
        for (identity, signature) in self.canonical.signatures {
            checkpoint()?;
            facts.canonical_specs.signatures.insert(identity, signature);
        }
        let mut canonical = std::collections::HashSet::new();
        for item in &facts.canonical_specs.impls {
            checkpoint()?;
            canonical.insert((item.self_type, item.spec.clone(), item.args.clone()));
        }
        for item in self.canonical.impls {
            checkpoint()?;
            if canonical.insert((item.self_type, item.spec.clone(), item.args.clone())) {
                facts.canonical_specs.impls.push(item);
            }
        }
        let mut standard = std::collections::HashSet::new();
        for item in &facts.std_spec_impls {
            checkpoint()?;
            standard.insert((item.self_type, item.spec.clone(), item.args.clone()));
        }
        for item in self.std_impls {
            checkpoint()?;
            if standard.insert((item.self_type, item.spec.clone(), item.args.clone())) {
                facts.std_spec_impls.push(item);
            }
        }
        Ok(())
    }
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
    pub generic_params: Vec<crate::CallableGenericParam>,
    pub methods: Vec<SpecMethodFact>,
    pub super_specs: Vec<CheckedSpecBound>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct ExternalSpecImplFact {
    pub self_type: TypeId,
    pub spec: ExternalSpecIdentity,
    pub args: Vec<TypeId>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct ExternalSpecFacts {
    pub signatures: std::collections::HashMap<ExternalSpecIdentity, ExternalSpecSignatureFact>,
    pub impls: Vec<ExternalSpecImplFact>,
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
    pub item: etas_package_metadata::CallableContractTarget,
    pub spec: Vec<String>,
    pub args: Vec<TypeId>,
    pub span: etas_core::Span,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct ExternalTraceSpecConformanceFact {
    pub package: u32,
    pub item: etas_package_metadata::CallableContractTarget,
    pub target: ExternalTraceSpecConformanceTarget,
    pub span: etas_core::Span,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub enum ExternalTraceSpecConformanceTarget {
    Named {
        spec: Vec<String>,
        definition: etas_code::DefinitionKey,
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
    pub parameter_names: Vec<String>,
    pub receiver_param: Option<u32>,
    #[serde(default)]
    pub signature: Option<CallableSignature>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn relocated_spec_validation_checks_cancellation_inside_arguments() {
        struct CancelAfterTwo(AtomicUsize);
        impl etas_utils::PipelineExecutionControl for CancelAfterTwo {
            fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
                if self.0.fetch_add(1, Ordering::Relaxed) >= 2 {
                    return Err(etas_utils::PipelineInterruption {
                        kind: etas_utils::PipelineInterruptionKind::Cancelled,
                        reason: "cancelled".into(),
                    });
                }
                Ok(())
            }
        }
        let mut store = crate::TypeStore::new();
        let ty = store.intern(crate::Type::Primitive(crate::PrimitiveType::Bool));
        let facts = RelocatedSpecFacts {
            std_impls: vec![CheckedStdSpecImplFact {
                self_type: ty,
                spec: vec!["std".into(), "S".into()],
                args: vec![ty, ty, ty],
            }],
            ..Default::default()
        };
        let control = CancelAfterTwo(AtomicUsize::new(0));
        assert!(matches!(
            facts.validate(&crate::TypeFacts::default(), &store, Some(&control)),
            Err(RelocatedSpecValidationError::Interrupted(_))
        ));
        assert_eq!(control.0.load(Ordering::Relaxed), 3);
    }
}
