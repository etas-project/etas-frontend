use std::collections::{BTreeMap, BTreeSet};

use etas_code::CodeFingerprint;
use etas_hir::{HirExprId, HirHandlerArmId, HirItemId, SymbolId};

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
    /// Checked binder identity retained when portable method evidence crosses
    /// a callable's local generic interface.
    TypeParameter {
        subject: etas_types::TypeId,
    },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum CallableGenericValue {
    Type(etas_types::TypeId),
    EffectRow(EffectRow),
    /// The open row was supplied at a checked call site. The site is a
    /// session-local lookup, not portable identity or evidence by itself.
    EffectRowFromCall {
        row: EffectRow,
        source: HirExprId,
    },
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
    /// A checked path into a source parameter, with its own callable contract.
    Projection(CodeFingerprint),
    Existential {
        pack: HirExprId,
        environment: CallableEnvironmentId,
    },
    /// The caller-visible transformer of a definition, not an edited body's analysis.
    FrozenContract {
        unit: EffectUnit,
        reference: Option<HirExprId>,
        environment: Option<CallableEnvironmentId>,
    },
    /// The validated metadata contract named by a checked external reflection.
    ExternalFrozen {
        reflection: HirExprId,
    },
    /// A checked external callable captured at its generic instantiation site.
    ExternalReference {
        value: HirExprId,
        frozen: bool,
    },
    MetadataSpecMethod {
        invocation: CodeFingerprint,
        environment: CallableEnvironmentId,
    },
    SpecMethod {
        call: HirExprId,
        environment: CallableEnvironmentId,
    },
    Metadata(MetadataCallableSource),
    MetadataObject(crate::MetadataObjectSource),
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
    DerivedResponseDecoder(HirHandlerArmId),
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct NativeParameterProjection {
    pub owner: HirItemId,
    pub parameter: SymbolId,
    pub path: Vec<etas_code::descriptor::ValueProjectionStep>,
    pub ty: etas_types::TypeId,
    #[serde(default)]
    pub call_inputs: BTreeMap<u32, std::sync::Arc<NativeCallInputs>>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct NativeCallInputs {
    pub call: HirExprId,
    /// Lexical instance of a native application equation; portable imports
    /// retain immutable recipes until their own graph equations are rebound.
    #[serde(default)]
    pub scope: Option<crate::EffectUnit>,
    /// Stable application frame, distinct from the shared callable template.
    #[serde(default)]
    pub context: Option<CodeFingerprint>,
    /// Body equations join alternatives at the checked application site.
    /// Contextual equations store derived evaluations; portable recipes are
    /// immutable, content-addressed values.
    #[serde(default)]
    pub joined: bool,
    pub arguments: Vec<NativeCallArgument>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct NativeCallArgument {
    pub name: Option<String>,
    pub sources: BTreeSet<CallableValueOrigin>,
    pub projections: Vec<NativeArgumentProjection>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct NativeArgumentProjection {
    pub path: Vec<etas_code::descriptor::ValueProjectionStep>,
    pub sources: BTreeSet<CallableValueOrigin>,
}

/// A parameter selector in an imported contract. Its input is supplied by the
/// checked call's binding frame; it is neither a value origin nor authority.
#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ExternalParameterProjection {
    pub root: ParameterCallTarget,
    pub path: Vec<etas_code::descriptor::ValueProjectionStep>,
}

impl ExternalParameterProjection {
    pub fn identity(&self) -> Result<CodeFingerprint, serde_json::Error> {
        Ok(CodeFingerprint::of_bytes(
            "etas.effects.external-parameter-projection.v1",
            &serde_json::to_vec(&(self.root, &self.path))?,
        ))
    }
}

impl NativeParameterProjection {
    pub fn identity(&self) -> Result<CodeFingerprint, serde_json::Error> {
        Self::path_identity(self.owner, self.parameter, &self.path)
    }

    pub fn path_identity(
        owner: HirItemId,
        parameter: SymbolId,
        path: &[etas_code::descriptor::ValueProjectionStep],
    ) -> Result<CodeFingerprint, serde_json::Error> {
        Ok(CodeFingerprint::of_bytes(
            "etas.effects.parameter-projection.v1",
            &serde_json::to_vec(&(owner, parameter, path))?,
        ))
    }
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
    #[serde(default)]
    pub scope: Option<CodeFingerprint>,
    pub origin: CodeFingerprint,
    pub application: CodeFingerprint,
    /// The original lexical application retained when this frame is rebound.
    /// Definition templates and executed invocations keep distinct identities
    /// without accumulating a hash of the intervening call history.
    #[serde(default)]
    pub instance: Option<CodeFingerprint>,
    /// Fully substituted checked generics, independent of invocation history.
    #[serde(default)]
    pub generic_context: Option<CodeFingerprint>,
}

impl CallableEnvironmentId {
    pub(crate) fn lexical_instance(self) -> CodeFingerprint {
        self.instance.unwrap_or_else(|| {
            CodeFingerprint::of_parts(
                "etas.effects.lexical-frame-instance.v1",
                [
                    self.origin.as_bytes().as_slice(),
                    self.application.as_bytes().as_slice(),
                ],
            )
        })
    }
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
    pub implementation: Option<etas_code::MethodId>,
    pub graph: std::sync::Arc<etas_package_metadata::CallableValueGraph>,
    pub signatures: Vec<MetadataCallableTypeFact>,
    pub objects: Vec<crate::ExternalExistentialValueMetadata>,
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
    /// Creation identity carried across invocations. Argument equations belong
    /// to the frame, rather than recursively becoming part of its identity.
    #[serde(default)]
    pub invocation_origin: Option<CodeFingerprint>,
    /// Contributions to a shared invocation frame, indexed by the stable input
    /// equation. Refining one equation must not overwrite another caller.
    #[serde(default)]
    pub invocation_inputs:
        BTreeMap<CodeFingerprint, BTreeMap<ParameterCallTarget, BTreeSet<CallableValueOrigin>>>,
    pub parameters: BTreeMap<ParameterCallTarget, BTreeSet<CallableValueOrigin>>,
    pub generics: BTreeMap<CallableGenericParameter, CallableGenericValue>,
}

impl CallableEnvironment {
    pub(crate) fn generic_identity(
        generics: &BTreeMap<CallableGenericParameter, CallableGenericValue>,
    ) -> Option<CodeFingerprint> {
        if generics.is_empty() {
            return None;
        }
        let bytes = serde_json::to_vec(&generics.iter().collect::<Vec<_>>())
            .expect("checked generic context serializes");
        Some(CodeFingerprint::of_bytes(
            "etas.effects.frame-generics.v1",
            &bytes,
        ))
    }

    pub(crate) fn join_assign(&mut self, other: &Self) -> Result<bool, &'static str> {
        if self.owner != other.owner {
            return Err("callable environment has conflicting checked owners");
        }
        if self.invocation_origin != other.invocation_origin {
            return Err("callable environment has conflicting invocation origins");
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

impl NativeCallInputs {
    /// Persist the checked equation identity, not its session-local table slot.
    pub(crate) fn equation_identity(&self) -> CodeFingerprint {
        if self.joined {
            let bytes = serde_json::to_vec(&(self.scope, self.context, self.call))
                .expect("checked input equation serializes");
            CodeFingerprint::of_bytes("etas.effects.joined-call-inputs.v1", &bytes)
        } else {
            let bytes = serde_json::to_vec(self).expect("checked input recipe serializes");
            CodeFingerprint::of_bytes("etas.effects.call-inputs.v1", &bytes)
        }
    }

    pub(crate) fn nodes(&self) -> usize {
        self.arguments.iter().fold(0usize, |count, argument| {
            count
                .saturating_add(
                    1 + argument.name.as_ref().map_or(0, String::len) + argument.sources.len(),
                )
                .saturating_add(
                    argument
                        .projections
                        .iter()
                        .map(|projection| {
                            1 + etas_code::descriptor::ValueProjectionStep::work(&projection.path)
                                + projection.sources.len()
                        })
                        .sum::<usize>(),
                )
        })
    }
}
