use std::collections::{BTreeMap, BTreeSet};

use etas_code::CodeFingerprint;
use etas_hir::{HirExprId, HirItemId, SymbolId};

use crate::{EffectRow, EffectUnit, MetadataCallableSource, ParameterCallTarget};

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum CallableGenericParameter {
    Native {
        owner: HirItemId,
        position: u32,
    },
    Graph {
        graph: CodeFingerprint,
        scope: etas_package_metadata::CallableValueScope,
        position: u32,
    },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum CallableGenericValue {
    Type(etas_types::TypeId),
    EffectRow(EffectRow),
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum CallableValueOrigin {
    MetadataSpecMethod {
        invocation: CodeFingerprint,
        environment: CallableEnvironmentId,
    },
    SpecMethod {
        call: HirExprId,
        environment: CallableEnvironmentId,
    },
    Metadata(MetadataCallableSource),
    Unit(EffectUnit),
    Captured {
        unit: EffectUnit,
        environment: CallableEnvironmentId,
    },
    Reference {
        unit: EffectUnit,
        value: HirExprId,
        environment: Option<CallableEnvironmentId>,
    },
    StdReference {
        value: HirExprId,
        environment: Option<CallableEnvironmentId>,
    },
    EnumConstructor(SymbolId),
    Parameter {
        owner: HirItemId,
        symbol: SymbolId,
    },
    Unavailable {
        expr: HirExprId,
    },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct MetadataSpecMethodDefinition {
    pub invocation: std::sync::Arc<crate::ExternalSpecMethodInvocation>,
    pub generics: Vec<(String, etas_types::CallableGenericParamKind)>,
    pub parameters: Vec<(String, ParameterCallTarget)>,
    pub span: etas_core::Span,
}

impl MetadataSpecMethodDefinition {
    pub fn identity(&self) -> Result<CodeFingerprint, String> {
        let context = serde_json::to_vec(&(&self.parameters, &self.generics))
            .map_err(|error| format!("method binding identity could not be encoded: {error}"))?;
        Ok(CodeFingerprint::of_parts(
            "etas.effects.metadata-method.v1",
            [
                self.invocation.identity.as_bytes().as_slice(),
                context.as_slice(),
            ],
        ))
    }
}

/// Static creation/application sites, not a runtime address or execution permit.
/// Keeping the origin fixed makes recursive instantiation a finite graph.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct CallableEnvironmentId {
    pub origin: CodeFingerprint,
    pub application: CodeFingerprint,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct CapturedCallableId {
    pub unit: EffectUnit,
    pub environment: CallableEnvironmentId,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum CapturedCallableAnalysis {
    /// A descriptor may retain lexically scoped parameter-call obligations.
    /// Invocation analysis must still bind them through the checked call site.
    Solved(std::sync::Arc<crate::EffectSummary>),
    Unavailable {
        reason: String,
    },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct MetadataCallableTypeFact {
    pub params: Vec<etas_types::TypeId>,
    pub output: etas_types::TypeId,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct MetadataCallableGraphFact {
    pub graph: std::sync::Arc<etas_package_metadata::CallableValueGraph>,
    pub signatures: Vec<MetadataCallableTypeFact>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum MetadataCallableAnalysis {
    Solved {
        summary: std::sync::Arc<crate::EffectSummary>,
        type_bindings: Vec<(String, etas_types::TypeId)>,
        effect_row_bindings: Vec<(String, crate::EffectRow)>,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum CallableReferenceAnalysis {
    Solved {
        summary: std::sync::Arc<crate::EffectSummary>,
        type_bindings: Vec<(String, etas_types::TypeId)>,
        effect_row_bindings: Vec<(String, crate::EffectRow)>,
        invocation_environment: Option<CallableEnvironmentId>,
        param_names: Vec<String>,
        param_symbols: Vec<SymbolId>,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct CallableEnvironment {
    pub owner: HirItemId,
    pub parameters: BTreeMap<ParameterCallTarget, BTreeSet<CallableValueOrigin>>,
    pub generics: BTreeMap<CallableGenericParameter, CallableGenericValue>,
}

impl CallableEnvironment {
    pub(crate) fn join_assign(&mut self, other: &Self) -> Result<bool, &'static str> {
        if self.owner != other.owner {
            return Err("callable environment has conflicting checked owners");
        }
        let mut changed = false;
        for (parameter, value) in &other.generics {
            match self.generics.get(parameter) {
                Some(previous) if previous != value => {
                    return Err("callable environment has conflicting generic bindings");
                }
                Some(_) => {}
                None => {
                    self.generics.insert(*parameter, value.clone());
                    changed = true;
                }
            }
        }
        for (parameter, sources) in &other.parameters {
            let entry = self.parameters.entry(*parameter).or_default();
            let before = entry.len();
            entry.extend(sources);
            changed |= before != entry.len();
        }
        Ok(changed)
    }
}
