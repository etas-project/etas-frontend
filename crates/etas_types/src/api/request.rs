use std::sync::Arc;

use etas_hir::{HirItemId, HirProgram, SymbolId};

#[derive(Clone, Copy)]
pub struct CheckProjectRequest<'a> {
    pub hir: &'a HirProgram,
}

#[derive(Clone, Copy)]
pub struct CheckSignaturesRequest<'a> {
    pub hir: &'a HirProgram,
}

#[derive(Clone, Copy)]
pub struct CheckBodyRequest<'a> {
    pub hir: &'a HirProgram,
    pub item: HirItemId,
}

pub struct SignaturePipelineInput<'a> {
    pub program: &'a HirProgram,
    pub std_registry: Arc<etas_std::StdRegistry>,
    pub source: SourceSignatureInput,
    pub std: StdSignatureInput,
    pub external: ExternalSignatureInput,
}

impl<'a> SignaturePipelineInput<'a> {
    pub fn new(program: &'a HirProgram) -> Self {
        Self {
            program,
            std_registry: Arc::new(etas_std::standard_registry()),
            source: SourceSignatureInput::default(),
            std: StdSignatureInput::default(),
            external: ExternalSignatureInput::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SourceSignatureInput {
    pub symbol_bindings: Vec<SourceSymbolBindingInput>,
}

#[derive(Clone, Debug)]
pub struct SourceSymbolBindingInput {
    pub symbol: SymbolId,
    pub target: SymbolId,
}

#[derive(Clone, Debug, Default)]
pub struct StdSignatureInput {
    pub symbol_bindings: Vec<StdSymbolBindingInput>,
}

#[derive(Clone, Debug)]
pub struct StdSymbolBindingInput {
    pub symbol: SymbolId,
    pub qualified_path: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExternalPackageKey(pub u32);

#[derive(Clone, Debug, Default)]
pub struct ExternalSignatureInput {
    pub metadata: Vec<ExternalPublicMetadataInput>,
    pub symbol_bindings: Vec<ExternalSymbolBindingInput>,
    pub action_bindings: Vec<ExternalActionBindingInput>,
}

#[derive(Clone, Debug)]
pub struct ExternalSymbolBindingInput {
    pub symbol: SymbolId,
    pub package: ExternalPackageKey,
    pub path: Vec<String>,
    pub span: etas_core::Span,
}

#[derive(Clone, Debug)]
pub struct ExternalActionBindingInput {
    pub symbol: SymbolId,
    pub package: ExternalPackageKey,
    pub path: Vec<String>,
    pub span: etas_core::Span,
}

#[derive(Clone, Debug)]
pub struct ExternalPublicMetadataInput {
    pub package: ExternalPackageKey,
    pub types: Vec<ExternalNamedSignatureInput>,
    pub values: Vec<ExternalNamedSignatureInput>,
    pub enums: Vec<ExternalNamedSignatureInput>,
    pub flows: Vec<ExternalFlowSignatureInput>,
    pub agents: Vec<ExternalAgentSignatureInput>,
    pub tools: Vec<ExternalToolSignatureInput>,
    pub effects: Vec<ExternalNamedSignatureInput>,
    pub trace_specs: Vec<ExternalNamedSignatureInput>,
    pub spec_signatures: Vec<ExternalSpecSignatureInput>,
    pub spec_impls: Vec<ExternalSpecImplInput>,
    pub type_spec_satisfactions: Vec<ExternalTypeSpecSatisfactionInput>,
    pub callable_spec_satisfactions: Vec<ExternalCallableSpecSatisfactionInput>,
    pub trace_spec_conformances: Vec<ExternalTraceSpecConformanceInput>,
    pub actions: Vec<ExternalActionSignatureInput>,
}

#[derive(Clone, Debug)]
pub struct ExternalNamedSignatureInput {
    pub path: Vec<String>,
    pub ty: Option<ExternalTypeInput>,
}

#[derive(Clone, Debug)]
pub struct ExternalFlowSignatureInput {
    pub path: Vec<String>,
    pub generic_params: Vec<ExternalCallableGenericParamInput>,
    pub params: Vec<ExternalTypeInput>,
    pub output: ExternalTypeInput,
    pub effects: Option<ExternalEffectRowInput>,
}

#[derive(Clone, Debug)]
pub struct ExternalAgentSignatureInput {
    pub path: Vec<String>,
    pub generic_params: Vec<ExternalCallableGenericParamInput>,
    pub input: Vec<ExternalTypeInput>,
    pub output: ExternalTypeInput,
    pub effects: Option<ExternalEffectRowInput>,
}

#[derive(Clone, Debug)]
pub struct ExternalToolSignatureInput {
    pub path: Vec<String>,
    pub generic_params: Vec<ExternalCallableGenericParamInput>,
    pub input: Vec<ExternalTypeInput>,
    pub output: ExternalTypeInput,
    pub effects: Option<ExternalEffectRowInput>,
}

#[derive(Clone, Debug)]
pub struct ExternalActionSignatureInput {
    pub path: Vec<String>,
    pub generic_params: Vec<ExternalActionGenericParamInput>,
    pub params: Vec<ExternalTypeInput>,
    pub effect_args: Vec<ExternalActionArgKindInput>,
    pub selector_param_names: Vec<String>,
    pub selector_defaults: Vec<Option<ExternalEffectArgInput>>,
    pub output: ExternalTypeInput,
    pub returns_never: bool,
}

#[derive(Clone, Debug)]
pub struct ExternalCallableGenericParamInput {
    pub name: String,
    pub bounds: Vec<ExternalSpecBoundInput>,
}

pub type ExternalActionGenericParamInput = ExternalCallableGenericParamInput;

#[derive(Clone, Debug)]
pub struct ExternalSpecSignatureInput {
    pub path: Vec<String>,
    pub kind: ExternalSpecKindInput,
    pub param_names: Vec<String>,
    pub callable: Option<ExternalFlowSignatureInput>,
    pub methods: Vec<ExternalSpecMethodInput>,
    pub super_specs: Vec<ExternalSpecBoundInput>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalSpecKindInput {
    Type,
    Callable,
    Trace,
}

#[derive(Clone, Debug)]
pub struct ExternalSpecMethodInput {
    pub name: String,
    pub path: Vec<String>,
    pub signature: Option<ExternalFlowSignatureInput>,
}

#[derive(Clone, Debug)]
pub struct ExternalSpecBoundInput {
    pub spec: Vec<String>,
    pub args: Vec<ExternalTypeInput>,
}

#[derive(Clone, Debug)]
pub struct ExternalSpecImplInput {
    pub self_type: ExternalTypeInput,
    pub spec: Vec<String>,
    pub args: Vec<ExternalTypeInput>,
    pub methods: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ExternalTypeSpecSatisfactionInput {
    pub self_type: ExternalTypeInput,
    pub spec: Vec<String>,
    pub args: Vec<ExternalTypeInput>,
}

#[derive(Clone, Debug)]
pub struct ExternalCallableSpecSatisfactionInput {
    pub item: Vec<String>,
    pub spec: Vec<String>,
    pub args: Vec<ExternalTypeInput>,
}

#[derive(Clone, Debug)]
pub struct ExternalTraceSpecConformanceInput {
    pub item: Vec<String>,
    pub target: ExternalTraceSpecConformanceTargetInput,
}

#[derive(Clone, Debug)]
pub enum ExternalTraceSpecConformanceTargetInput {
    Inline,
    Named {
        spec: Vec<String>,
        args: Vec<ExternalTypeInput>,
    },
}

#[derive(Clone, Debug)]
pub enum ExternalActionArgKindInput {
    Type,
    MemoryPlace,
    StaticResourcePath { ty: String },
    StringPattern,
}

#[derive(Clone, Debug)]
pub struct ExternalEffectRowInput {
    pub effects: Vec<ExternalEffectRefInput>,
}

#[derive(Clone, Debug)]
pub struct ExternalEffectRefInput {
    pub path: Vec<String>,
    pub args: Vec<ExternalEffectArgInput>,
}

#[derive(Clone, Debug)]
pub enum ExternalEffectArgInput {
    Type(ExternalTypeInput),
    Path(Vec<String>),
    String(String),
    Int(String),
    Wildcard,
}

#[derive(Clone, Debug)]
pub struct ExternalRecordFieldInput {
    pub name: String,
    pub ty: ExternalTypeInput,
}

#[derive(Clone, Debug)]
pub enum ExternalTypeInput {
    Primitive(String),
    Var(String),
    Named(Vec<String>),
    Applied {
        path: Vec<String>,
        args: Vec<ExternalTypeInput>,
    },
    Alias {
        path: Vec<String>,
        target: Box<ExternalTypeInput>,
    },
    Nominal {
        path: Vec<String>,
        representation: Option<Box<ExternalTypeInput>>,
    },
    Array(Box<ExternalTypeInput>),
    List(Box<ExternalTypeInput>),
    Map {
        key: Box<ExternalTypeInput>,
        value: Box<ExternalTypeInput>,
    },
    Set(Box<ExternalTypeInput>),
    Range(Box<ExternalTypeInput>),
    Slice(Box<ExternalTypeInput>),
    Option(Box<ExternalTypeInput>),
    Result {
        ok: Box<ExternalTypeInput>,
        err: Box<ExternalTypeInput>,
    },
    Record {
        fields: Vec<ExternalRecordFieldInput>,
    },
    Tuple(Vec<ExternalTypeInput>),
    Function {
        input: Vec<ExternalTypeInput>,
        output: Box<ExternalTypeInput>,
        effects: Option<ExternalEffectRowInput>,
    },
    Handler {
        handled: ExternalEffectRowInput,
        produced: Option<ExternalEffectRowInput>,
        result: Option<Box<ExternalTypeInput>>,
    },
    Trust {
        wrapper: String,
        inner: Box<ExternalTypeInput>,
    },
    Prompt,
    PromptPart,
    Message(Box<ExternalTypeInput>),
    MemorySelection(Box<ExternalTypeInput>),
    Store {
        key: Box<ExternalTypeInput>,
        value: Box<ExternalTypeInput>,
    },
    MemoryRegion(Box<ExternalTypeInput>),
    ResourceHandle {
        name: String,
        args: Vec<ExternalTypeInput>,
    },
}
