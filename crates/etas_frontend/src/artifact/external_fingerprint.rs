//! Canonical encoding for external type and effect-row inputs.
//!
//! This does not fingerprint the complete project environment. Each node is
//! tagged and each variable-length value is length-delimited; the worklist
//! avoids recursion through untrusted type or effect metadata.

use crate::{
    ProjectExternalActionArgKindInput as ActionKind,
    ProjectExternalActionRequestInput as ActionRequest,
    ProjectExternalActionRequestSignatureInput as RequestSignature,
    ProjectExternalActionSignatureInput as Action,
    ProjectExternalActionTraceEventSourceInput as TraceSource,
    ProjectExternalActionTraceInput as Trace, ProjectExternalAgentSignatureInput as Agent,
    ProjectExternalCallableGenericParamInput as Generic,
    ProjectExternalCallableGenericParamKindInput as GenericKind,
    ProjectExternalEffectArgInput as Arg, ProjectExternalEffectRefInput as Effect,
    ProjectExternalEffectRowInput as Row, ProjectExternalFlowSignatureInput as Flow,
    ProjectExternalNamedSignatureInput as Named,
    ProjectExternalParameterRequestInput as ParameterRequest,
    ProjectExternalRecordFieldInput as Field, ProjectExternalRequestInstantiationInput as Requests,
    ProjectExternalSpecBoundInput as Bound, ProjectExternalSpecKindInput as SpecKind,
    ProjectExternalSpecMethodInput as SpecMethod, ProjectExternalSpecSignatureInput as Spec,
    ProjectExternalToolSignatureInput as Tool, ProjectExternalTypeInput as Type,
};

enum Task<'a> {
    Type(&'a Type),
    Row(&'a Row),
    Effect(&'a Effect),
    Arg(&'a Arg),
    Field(&'a Field),
    Named(&'a Named),
    Generic(&'a Generic),
    Bound(&'a Bound),
    Trace(&'a Trace),
    Flow(&'a Flow),
    Agent(&'a Agent),
    Tool(&'a Tool),
    Action(&'a Action),
    ActionKind(&'a ActionKind),
    OptionalArg(&'a Option<Arg>),
    Spec(&'a Spec),
    SpecMethod(&'a SpecMethod),
    Requests(&'a Requests),
    ActionRequest(&'a ActionRequest),
    RequestSignature(&'a RequestSignature),
    ParameterRequest(&'a ParameterRequest),
    TypeBinding(&'a (String, Type)),
    Behavior(&'a etas_package_metadata::CallableValueBehavior),
}

struct Encoder(blake3::Hasher);

impl Encoder {
    fn new() -> Self {
        Self(blake3::Hasher::new_derive_key(
            "etas.frontend.external-type-row.v2",
        ))
    }
    fn byte(&mut self, value: u8) {
        self.0.update(&[value]);
    }
    fn number(&mut self, value: usize) {
        self.0.update(&(value as u64).to_le_bytes());
    }
    fn text(&mut self, value: &str) {
        self.number(value.len());
        self.0.update(value.as_bytes());
    }
    fn request_target(&mut self, target: &etas_package_metadata::CallableRequestTarget) {
        self.0.update(target.input_fingerprint().as_bytes());
    }
    fn contract_target(&mut self, target: &etas_package_metadata::CallableContractTarget) {
        match target {
            etas_package_metadata::CallableContractTarget::Item(path) => {
                self.byte(0);
                self.path(path);
            }
            etas_package_metadata::CallableContractTarget::Method(method) => {
                self.byte(1);
                self.0.update(method.fingerprint().as_bytes());
            }
        }
    }
    fn path(&mut self, path: &[String]) {
        self.number(path.len());
        for segment in path {
            self.text(segment);
        }
    }
    fn optional(&mut self, present: bool) {
        self.byte(u8::from(present));
    }
    fn texts(&mut self, values: &[String]) {
        self.number(values.len());
        for value in values {
            self.text(value);
        }
    }
    fn finish(self) -> String {
        self.0.finalize().to_hex().to_string()
    }
    fn encode(&mut self, root: Task<'_>) {
        let mut tasks = vec![root];
        while let Some(task) = tasks.pop() {
            match task {
                Task::Row(row) => {
                    self.byte(0);
                    self.optional(row.tail.is_some());
                    if let Some(tail) = &row.tail {
                        self.text(tail);
                    }
                    self.number(row.effects.len());
                    tasks.extend(row.effects.iter().rev().map(Task::Effect));
                }
                Task::Effect(effect) => {
                    self.byte(1);
                    self.path(&effect.path);
                    self.number(effect.args.len());
                    tasks.extend(effect.args.iter().rev().map(Task::Arg));
                }
                Task::Arg(arg) => {
                    self.byte(2);
                    match arg {
                        Arg::Type(ty) => {
                            self.byte(0);
                            tasks.push(Task::Type(ty));
                        }
                        Arg::Path(path) => {
                            self.byte(1);
                            self.path(path);
                        }
                        Arg::String(value) => {
                            self.byte(2);
                            self.text(value);
                        }
                        Arg::Int(value) => {
                            self.byte(3);
                            self.text(value);
                        }
                        Arg::Wildcard => self.byte(4),
                    }
                }
                Task::Field(field) => {
                    self.byte(3);
                    self.text(&field.name);
                    tasks.push(Task::Type(&field.ty));
                }
                Task::Named(named) => {
                    self.byte(5);
                    self.path(&named.path);
                    self.text(&named.visibility);
                    self.number(named.generic_params.len());
                    self.optional(named.ty.is_some());
                    if let Some(ty) = &named.ty {
                        tasks.push(Task::Type(ty));
                    }
                    tasks.extend(named.generic_params.iter().rev().map(Task::Generic));
                }
                Task::Generic(generic) => {
                    self.byte(6);
                    self.text(&generic.name);
                    self.byte(match generic.kind {
                        GenericKind::Type => 0,
                        GenericKind::Effect => 1,
                    });
                    self.number(generic.bounds.len());
                    tasks.extend(generic.bounds.iter().rev().map(Task::Bound));
                }
                Task::Bound(bound) => {
                    self.byte(7);
                    self.path(&bound.spec);
                    self.number(bound.args.len());
                    tasks.extend(bound.args.iter().rev().map(Task::Type));
                }
                Task::Trace(trace) => {
                    self.byte(8);
                    match trace {
                        Trace::Empty => self.byte(0),
                        Trace::DefaultHandler { guards, body } => {
                            self.byte(8);
                            self.number(guards.len());
                            tasks.push(Task::Trace(body));
                            tasks.extend(guards.iter().rev().map(Task::Effect));
                        }
                        Trace::Event { action, source } => {
                            self.byte(1);
                            self.byte(match source {
                                TraceSource::Perform => 0,
                                TraceSource::StdIntrinsic => 1,
                                TraceSource::AgentCall => 2,
                                TraceSource::ExternalMetadata => 3,
                                TraceSource::Transfer => 4,
                                TraceSource::Unknown => 5,
                            });
                            tasks.push(Task::Effect(action));
                        }
                        Trace::ParameterCall { parameter } => {
                            self.byte(2);
                            self.request_target(parameter);
                        }
                        Trace::Seq(children) => {
                            self.byte(3);
                            self.number(children.len());
                            tasks.extend(children.iter().rev().map(Task::Trace));
                        }
                        Trace::Choice(children) => {
                            self.byte(4);
                            self.number(children.len());
                            tasks.extend(children.iter().rev().map(Task::Trace));
                        }
                        Trace::Repeat(child) => {
                            self.byte(5);
                            tasks.push(Task::Trace(child));
                        }
                        Trace::UnknownOrder(actions) => {
                            self.byte(6);
                            self.number(actions.len());
                            tasks.extend(actions.iter().rev().map(Task::Effect));
                        }
                        Trace::Widened {
                            actions,
                            parameter_calls,
                        } => {
                            self.byte(7);
                            self.number(actions.len());
                            self.number(parameter_calls.len());
                            for parameter in parameter_calls {
                                self.request_target(parameter);
                            }
                            tasks.extend(actions.iter().rev().map(Task::Effect));
                        }
                    }
                }
                Task::Flow(flow) => {
                    self.byte(9);
                    self.path(&flow.path);
                    self.text(&flow.visibility);
                    self.texts(&flow.param_names);
                    self.number(flow.generic_params.len());
                    self.number(flow.params.len());
                    self.optional(flow.effects.is_some());
                    if let Some(row) = &flow.effects {
                        tasks.push(Task::Row(row));
                    }
                    tasks.push(Task::Type(&flow.output));
                    tasks.extend(flow.params.iter().rev().map(Task::Type));
                    tasks.extend(flow.generic_params.iter().rev().map(Task::Generic));
                }
                Task::Agent(agent) => {
                    self.byte(10);
                    self.path(&agent.path);
                    self.text(&agent.visibility);
                    self.texts(&agent.param_names);
                    self.number(agent.generic_params.len());
                    self.number(agent.input.len());
                    self.optional(agent.output.is_some());
                    self.optional(agent.effects.is_some());
                    if let Some(row) = &agent.effects {
                        tasks.push(Task::Row(row));
                    }
                    if let Some(output) = &agent.output {
                        tasks.push(Task::Type(output));
                    }
                    tasks.extend(agent.input.iter().rev().map(Task::Type));
                    tasks.extend(agent.generic_params.iter().rev().map(Task::Generic));
                }
                Task::Tool(tool) => {
                    self.byte(11);
                    self.path(&tool.path);
                    self.text(&tool.visibility);
                    self.texts(&tool.param_names);
                    self.number(tool.generic_params.len());
                    self.number(tool.input.len());
                    self.optional(tool.effects.is_some());
                    if let Some(row) = &tool.effects {
                        tasks.push(Task::Row(row));
                    }
                    tasks.push(Task::Type(&tool.output));
                    tasks.extend(tool.input.iter().rev().map(Task::Type));
                    tasks.extend(tool.generic_params.iter().rev().map(Task::Generic));
                }
                Task::Action(action) => {
                    self.byte(12);
                    self.path(&action.path);
                    self.text(&action.visibility);
                    self.byte(u8::from(action.returns_never));
                    self.texts(&action.selector_param_names);
                    self.number(action.generic_params.len());
                    self.number(action.params.len());
                    self.number(action.effect_args.len());
                    self.number(action.selector_defaults.len());
                    tasks.push(Task::Type(&action.output));
                    tasks.extend(action.selector_defaults.iter().rev().map(Task::OptionalArg));
                    tasks.extend(action.effect_args.iter().rev().map(Task::ActionKind));
                    tasks.extend(action.params.iter().rev().map(Task::Type));
                    tasks.extend(action.generic_params.iter().rev().map(Task::Generic));
                }
                Task::ActionKind(kind) => {
                    self.byte(13);
                    match kind {
                        ActionKind::Type => self.byte(0),
                        ActionKind::MemoryPlace => self.byte(1),
                        ActionKind::StaticResourcePath { ty } => {
                            self.byte(2);
                            self.text(ty);
                        }
                        ActionKind::StringPattern => self.byte(3),
                    }
                }
                Task::OptionalArg(value) => {
                    self.byte(14);
                    self.optional(value.is_some());
                    if let Some(value) = value {
                        tasks.push(Task::Arg(value));
                    }
                }
                Task::Spec(spec) => {
                    self.byte(15);
                    self.path(&spec.path);
                    self.text(&spec.visibility);
                    self.byte(match spec.kind {
                        SpecKind::Type => 0,
                        SpecKind::Callable => 1,
                        SpecKind::Trace => 2,
                    });
                    self.texts(&spec.param_names);
                    self.number(spec.generic_params.len());
                    self.optional(spec.callable.is_some());
                    self.number(spec.methods.len());
                    self.number(spec.super_specs.len());
                    tasks.extend(spec.super_specs.iter().rev().map(Task::Bound));
                    tasks.extend(spec.methods.iter().rev().map(Task::SpecMethod));
                    tasks.extend(spec.generic_params.iter().rev().map(Task::Generic));
                    if let Some(callable) = &spec.callable {
                        tasks.push(Task::Flow(callable));
                    }
                }
                Task::SpecMethod(method) => {
                    self.byte(16);
                    self.text(&method.name);
                    self.path(&method.path);
                    self.optional(method.signature.is_some());
                    if let Some(signature) = &method.signature {
                        tasks.push(Task::Flow(signature));
                    }
                }
                Task::Requests(requests) => {
                    self.byte(17);
                    self.number(requests.actions.len());
                    self.number(requests.parameters.len());
                    tasks.extend(requests.parameters.iter().rev().map(Task::ParameterRequest));
                    tasks.extend(requests.actions.iter().rev().map(Task::ActionRequest));
                }
                Task::ActionRequest(request) => {
                    self.byte(18);
                    self.optional(request.type_bindings.is_some());
                    self.optional(request.signature.is_some());
                    if let Some(bindings) = &request.type_bindings {
                        self.number(bindings.len());
                    }
                    if let Some(signature) = &request.signature {
                        tasks.push(Task::RequestSignature(signature));
                    }
                    if let Some(bindings) = &request.type_bindings {
                        tasks.extend(bindings.iter().rev().map(Task::TypeBinding));
                    }
                    tasks.push(Task::Effect(&request.action));
                }
                Task::RequestSignature(signature) => {
                    self.byte(19);
                    self.number(signature.params.len());
                    tasks.push(Task::Type(&signature.output));
                    tasks.extend(signature.params.iter().rev().map(Task::Type));
                }
                Task::ParameterRequest(parameter) => {
                    self.byte(20);
                    self.request_target(&parameter.parameter);
                    self.number(parameter.excluded.len());
                    tasks.extend(parameter.excluded.iter().rev().map(Task::Effect));
                }
                Task::TypeBinding((name, ty)) => {
                    self.byte(21);
                    self.text(name);
                    tasks.push(Task::Type(ty));
                }
                Task::Behavior(behavior) => {
                    self.byte(22);
                    self.number(behavior.determinism as usize);
                    self.optional(behavior.independent_determinism.is_some());
                    if let Some(value) = behavior.independent_determinism {
                        self.number(value as usize);
                    }
                    self.number(behavior.host_requirements.len());
                    for requirement in &behavior.host_requirements {
                        self.number(*requirement as usize);
                    }
                    self.number(behavior.interpreter_features.len());
                    for feature in &behavior.interpreter_features {
                        self.number(*feature as usize);
                    }
                }
                Task::Type(ty) => {
                    self.byte(4);
                    match ty {
                        Type::AgentContext(method) => {
                            self.byte(30);
                            self.0.update(method.fingerprint().as_bytes());
                        }
                        Type::Existential { bounds } => {
                            self.byte(27);
                            self.number(bounds.len());
                            tasks.extend(bounds.iter().rev().map(Task::Bound));
                        }
                        Type::Primitive(name) => {
                            self.byte(0);
                            self.text(name);
                        }
                        Type::Var(name) => {
                            self.byte(1);
                            self.text(name);
                        }
                        Type::Parameter { owner, position } => {
                            self.byte(28);
                            self.0.update(owner.fingerprint().as_bytes());
                            self.number(*position as usize);
                        }
                        Type::Named(path) => {
                            self.byte(2);
                            self.path(path);
                        }
                        Type::Enum { path, variants } => {
                            self.byte(3);
                            self.path(path);
                            self.number(variants.len());
                            tasks.extend(variants.iter().rev().map(Task::Field));
                        }
                        Type::Applied { path, args } => {
                            self.byte(4);
                            self.path(path);
                            self.number(args.len());
                            tasks.extend(args.iter().rev().map(Task::Type));
                        }
                        Type::Alias { path, target } => {
                            self.byte(5);
                            self.path(path);
                            tasks.push(Task::Type(target));
                        }
                        Type::Nominal {
                            path,
                            representation,
                        } => {
                            self.byte(6);
                            self.path(path);
                            self.optional(representation.is_some());
                            if let Some(value) = representation {
                                tasks.push(Task::Type(value));
                            }
                        }
                        Type::Array(value) => {
                            self.byte(7);
                            tasks.push(Task::Type(value));
                        }
                        Type::List(value) => {
                            self.byte(8);
                            tasks.push(Task::Type(value));
                        }
                        Type::Map { key, value } => {
                            self.byte(9);
                            tasks.push(Task::Type(value));
                            tasks.push(Task::Type(key));
                        }
                        Type::Set(value) => {
                            self.byte(10);
                            tasks.push(Task::Type(value));
                        }
                        Type::Range(value) => {
                            self.byte(11);
                            tasks.push(Task::Type(value));
                        }
                        Type::Slice(value) => {
                            self.byte(12);
                            tasks.push(Task::Type(value));
                        }
                        Type::Option(value) => {
                            self.byte(13);
                            tasks.push(Task::Type(value));
                        }
                        Type::Result { ok, err } => {
                            self.byte(14);
                            tasks.push(Task::Type(err));
                            tasks.push(Task::Type(ok));
                        }
                        Type::Record { fields } => {
                            self.byte(15);
                            self.number(fields.len());
                            tasks.extend(fields.iter().rev().map(Task::Field));
                        }
                        Type::Tuple(elements) => {
                            self.byte(16);
                            self.number(elements.len());
                            tasks.extend(elements.iter().rev().map(Task::Type));
                        }
                        Type::Function {
                            input,
                            output,
                            effects,
                        } => {
                            self.byte(17);
                            self.number(input.len());
                            self.optional(effects.is_some());
                            if let Some(row) = effects {
                                tasks.push(Task::Row(row));
                            }
                            tasks.push(Task::Type(output));
                            tasks.extend(input.iter().rev().map(Task::Type));
                        }
                        Type::Task { output, effects } => {
                            self.byte(29);
                            tasks.push(Task::Row(effects));
                            tasks.push(Task::Type(output));
                        }
                        Type::Handler {
                            handled,
                            produced,
                            result,
                        } => {
                            self.byte(18);
                            self.optional(produced.is_some());
                            self.optional(result.is_some());
                            if let Some(value) = result {
                                tasks.push(Task::Type(value));
                            }
                            if let Some(row) = produced {
                                tasks.push(Task::Row(row));
                            }
                            tasks.push(Task::Row(handled));
                        }
                        Type::Trust { wrapper, inner } => {
                            self.byte(19);
                            self.text(wrapper);
                            tasks.push(Task::Type(inner));
                        }
                        Type::Prompt => self.byte(20),
                        Type::PromptPart => self.byte(21),
                        Type::Message(value) => {
                            self.byte(22);
                            tasks.push(Task::Type(value));
                        }
                        Type::MemorySelection(value) => {
                            self.byte(23);
                            tasks.push(Task::Type(value));
                        }
                        Type::Store { key, value } => {
                            self.byte(24);
                            tasks.push(Task::Type(value));
                            tasks.push(Task::Type(key));
                        }
                        Type::MemoryRegion(value) => {
                            self.byte(25);
                            tasks.push(Task::Type(value));
                        }
                        Type::ResourceHandle { name, args } => {
                            self.byte(26);
                            self.text(name);
                            self.number(args.len());
                            tasks.extend(args.iter().rev().map(Task::Type));
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
fn external_type(ty: &Type) -> String {
    let mut encoder = Encoder::new();
    encoder.encode(Task::Type(ty));
    encoder.finish()
}

#[cfg(test)]
fn external_row(row: &Row) -> String {
    let mut encoder = Encoder::new();
    encoder.encode(Task::Row(row));
    encoder.finish()
}

pub(crate) fn external_named_signature(named: &Named) -> String {
    let mut encoder = Encoder::new();
    encoder.encode(Task::Named(named));
    encoder.finish()
}

#[cfg(test)]
fn external_trace(trace: &Trace) -> String {
    let mut encoder = Encoder::new();
    encoder.encode(Task::Trace(trace));
    encoder.finish()
}

macro_rules! fingerprint_root {
    ($name:ident, $ty:ty, $variant:ident) => {
        pub(crate) fn $name(value: &$ty) -> String {
            let mut encoder = Encoder::new();
            encoder.encode(Task::$variant(value));
            encoder.finish()
        }
    };
}

fingerprint_root!(external_flow, Flow, Flow);
fingerprint_root!(external_agent, Agent, Agent);
fingerprint_root!(external_tool, Tool, Tool);
fingerprint_root!(external_action, Action, Action);
fingerprint_root!(external_spec, Spec, Spec);

pub(crate) fn external_behavior(
    value: &Option<etas_package_metadata::CallableValueBehavior>,
) -> String {
    let mut encoder = Encoder::new();
    encoder.byte(23);
    encoder.optional(value.is_some());
    if let Some(value) = value {
        encoder.encode(Task::Behavior(value));
    }
    encoder.finish()
}

pub(crate) fn external_requests(value: &Option<Requests>) -> String {
    let mut encoder = Encoder::new();
    encoder.byte(24);
    encoder.optional(value.is_some());
    if let Some(value) = value {
        encoder.encode(Task::Requests(value));
    }
    encoder.finish()
}

pub(crate) fn external_dependency_effects(
    value: &etas_effects::DependencyEffectMetadata,
) -> String {
    use etas_effects::{EffectActionArgKind as Kind, RuntimeRequirementReason as Requirement};
    use etas_types::EffectArgRef;
    let mut encoder = Encoder::new();
    let requirement = |encoder: &mut Encoder, value: &Option<Requirement>| {
        encoder.optional(value.is_some());
        if let Some(value) = value {
            encoder.byte(match value {
                Requirement::Agentic => 0,
                Requirement::ToolCall => 1,
                Requirement::HostAuthority => 2,
                Requirement::DurableMemory => 3,
                Requirement::Approval => 4,
                Requirement::Checkpoint => 5,
                Requirement::Time => 6,
                Requirement::Network => 7,
                Requirement::Tcp => 8,
                Requirement::Stream => 9,
                Requirement::Tls => 10,
                Requirement::Browser => 11,
                Requirement::Console => 12,
                Requirement::FileIO => 13,
                Requirement::Command => 14,
                Requirement::SecretAccess => 15,
                Requirement::Compiler => 16,
                Requirement::RuntimeHandler => 17,
                Requirement::TaskScheduler => 18,
            });
        }
    };
    encoder.byte(25);
    encoder.number(value.tags.len());
    for tag in &value.tags {
        encoder.path(&tag.path);
        requirement(&mut encoder, &tag.runtime_requirement);
    }
    encoder.number(value.actions.len());
    for action in &value.actions {
        encoder.path(&action.path);
        encoder.byte(u8::from(action.returns_never));
        requirement(&mut encoder, &action.runtime_requirement);
        encoder.number(action.effect_args.len());
        for arg in &action.effect_args {
            match arg {
                Kind::Type => encoder.byte(0),
                Kind::MemoryPlace => encoder.byte(1),
                Kind::StaticResourcePath { ty } => {
                    encoder.byte(2);
                    encoder.text(ty);
                }
                Kind::StringPattern => encoder.byte(3),
            }
        }
        encoder.texts(&action.selector_param_names);
        encoder.number(action.selector_defaults.len());
        for value in &action.selector_defaults {
            encoder.optional(value.is_some());
            if let Some(value) = value {
                match value {
                    EffectArgRef::Type(id) => {
                        encoder.byte(0);
                        encoder.number(id.0 as usize);
                    }
                    EffectArgRef::Path(path) => {
                        encoder.byte(1);
                        encoder.path(path);
                    }
                    EffectArgRef::String(value) => {
                        encoder.byte(2);
                        encoder.text(value);
                    }
                    EffectArgRef::Int(value) => {
                        encoder.byte(3);
                        encoder.text(value);
                    }
                    EffectArgRef::Wildcard => encoder.byte(4),
                }
            }
        }
    }
    encoder.number(value.extensions.len());
    for extension in &value.extensions {
        encoder.optional(extension.package.is_some());
        if let Some(package) = extension.package {
            encoder.number(package as usize);
        }
        encoder.path(&extension.child);
        encoder.path(&extension.parent);
    }
    encoder.number(value.source_bindings.len());
    for binding in &value.source_bindings {
        encoder.number(binding.symbol.0 as usize);
        encoder.path(&binding.path);
    }
    encoder.finish()
}

pub(crate) fn external_spec_impl(value: &crate::ProjectExternalSpecImplInput) -> String {
    let mut encoder = Encoder::new();
    encoder.byte(26);
    encoder.path(&value.spec);
    encoder.number(value.methods.len());
    for method in &value.methods {
        encoder.0.update(method.fingerprint().as_bytes());
    }
    encoder.number(value.args.len());
    encoder.encode(Task::Type(&value.self_type));
    for arg in &value.args {
        encoder.encode(Task::Type(arg));
    }
    encoder.finish()
}

pub(crate) fn external_type_satisfaction(
    value: &crate::ProjectExternalTypeSpecSatisfactionInput,
) -> String {
    let mut encoder = Encoder::new();
    encoder.byte(27);
    encoder.path(&value.spec);
    encoder.number(value.args.len());
    encoder.encode(Task::Type(&value.self_type));
    for arg in &value.args {
        encoder.encode(Task::Type(arg));
    }
    encoder.finish()
}

pub(crate) fn external_callable_satisfaction(
    value: &crate::ProjectExternalCallableSpecSatisfactionInput,
) -> String {
    let mut encoder = Encoder::new();
    encoder.byte(28);
    encoder.contract_target(&value.item);
    encoder.path(&value.spec);
    encoder.number(value.args.len());
    for arg in &value.args {
        encoder.encode(Task::Type(arg));
    }
    encoder.finish()
}

pub(crate) fn external_trace_conformance(
    value: &crate::ProjectExternalTraceSpecConformanceInput,
) -> String {
    use crate::ProjectExternalTraceSpecConformanceTargetInput as Target;
    let mut encoder = Encoder::new();
    encoder.byte(29);
    encoder.contract_target(&value.item);
    match &value.target {
        Target::Inline => encoder.byte(0),
        Target::Named {
            spec,
            definition,
            args,
        } => {
            encoder.byte(1);
            encoder.path(spec);
            encoder.0.update(definition.fingerprint().as_bytes());
            encoder.number(args.len());
            for arg in args {
                encoder.encode(Task::Type(arg));
            }
        }
    }
    encoder.finish()
}

pub(crate) fn external_trace_summary(
    value: &crate::ProjectExternalTraceSpecSummaryInput,
) -> String {
    use crate::{
        ProjectExternalTraceSelectorOperandInput as Operand,
        ProjectExternalTraceSpecClauseKindInput as Kind,
    };
    let mut encoder = Encoder::new();
    encoder.byte(30);
    encoder.path(&value.trace_spec);
    encoder.0.update(value.definition.fingerprint().as_bytes());
    encoder.number(value.alternatives.len());
    for alternative in &value.alternatives {
        encoder.number(alternative.len());
        for clause in alternative {
            encoder.byte(match clause.kind {
                Kind::Allow => 0,
                Kind::Deny => 1,
                Kind::RequireBefore => 2,
                Kind::RequireAfter => 3,
            });
            for row in [
                &clause.pattern,
                &clause.guard,
                &clause.target,
                &clause.obligation,
            ] {
                encoder.optional(row.is_some());
                if let Some(row) = row {
                    encoder.encode(Task::Row(row));
                }
            }
            encoder.number(clause.selector_bounds.len());
            for bound in &clause.selector_bounds {
                encoder.byte(match bound.operand {
                    Operand::Pattern => 0,
                    Operand::Guard => 1,
                    Operand::Target => 2,
                    Operand::Obligation => 3,
                });
                encoder.path(&bound.effect_path);
                encoder.number(bound.argument as usize);
                encoder.text(&bound.binding);
                encoder.path(&bound.spec_path);
                encoder.number(bound.spec_args.len());
                for arg in &bound.spec_args {
                    encoder.encode(Task::Type(arg));
                }
            }
        }
    }
    encoder.finish()
}

/// The returned-callable graph is fingerprinted separately.
pub(crate) fn external_effect_summary(value: &crate::ProjectExternalEffectSummaryInput) -> String {
    let mut encoder = Encoder::new();
    encoder.byte(31);
    encoder.path(&value.item);
    encoder.number(value.residual_checks.len());
    for reference in &value.residual_checks {
        encoder.0.update(reference.fingerprint().as_bytes());
    }
    encoder.text(&external_behavior(&value.behavior));
    encoder.text(&external_requests(&value.handler_requests));
    encoder.optional(value.default_handler_effects.is_some());
    if let Some(defaults) = &value.default_handler_effects {
        encoder.optional(defaults.behavior.is_some());
        if let Some(behavior) = &defaults.behavior {
            encoder.0.update(behavior.fingerprint().as_bytes());
        }
        encoder.encode(Task::Row(&defaults.direct_effects));
        encoder.encode(Task::Row(&defaults.default_actions));
        encoder.number(defaults.handlers.len());
        for handler in &defaults.handlers {
            encoder.encode(Task::Effect(&handler.action));
            encoder.encode(Task::Row(&handler.produced));
        }
    }
    encoder.encode(Task::Row(&value.public_effects));
    encoder.encode(Task::Row(&value.requested_actions));
    encoder.encode(Task::Row(&value.handled_requested_actions));
    encoder.number(value.latent_flows.len());
    for latent in &value.latent_flows {
        encoder.number(latent.residual_checks.len());
        for reference in &latent.residual_checks {
            encoder.0.update(reference.fingerprint().as_bytes());
        }
        encoder.encode(Task::Row(&latent.declared_bound));
        encoder.encode(Task::Row(&latent.inferred_effects));
    }
    encoder.encode(Task::Trace(&value.action_trace));
    encoder.finish()
}

pub(crate) fn external_action_summary(value: &crate::ProjectExternalActionSummaryInput) -> String {
    let mut encoder = Encoder::new();
    encoder.byte(32);
    encoder.path(&value.action);
    encoder.texts(&value.args);
    encoder.finish()
}

pub(crate) fn external_re_export(value: &crate::ProjectExternalReExportInput) -> String {
    let mut encoder = Encoder::new();
    encoder.byte(33);
    encoder.path(&value.from);
    encoder.path(&value.exported);
    encoder.finish()
}

pub(crate) fn external_tool_schema(value: &crate::ProjectExternalToolSchemaInput) -> String {
    let mut encoder = Encoder::new();
    encoder.byte(34);
    encoder.path(&value.path);
    encoder.text(&value.schema_json);
    encoder.finish()
}

pub(crate) fn external_tool_binding(value: &crate::ProjectToolBindingInput) -> String {
    let mut encoder = Encoder::new();
    encoder.byte(35);
    encoder.text(&value.tool);
    encoder.text(&value.provider);
    encoder.texts(&value.effect_row);
    encoder.texts(&value.action_row);
    encoder.finish()
}

pub(crate) fn external_package(value: &crate::ProjectExternalPackageInput) -> String {
    let mut encoder = Encoder::new();
    encoder.byte(36);
    encoder.number(value.id.0 as usize);
    encoder.text(&value.name);
    encoder.text(&value.version);
    encoder.text(&value.edition);
    encoder.text(&value.import_root);
    encoder.text(&value.std_compatibility);
    encoder.finish()
}

pub(crate) fn dependency_source_package(value: &crate::ProjectDependencySourcePackage) -> String {
    let mut encoder = Encoder::new();
    encoder.byte(38);
    encoder.number(value.package.0 as usize);
    encoder.text(&value.import_root);
    encoder
        .0
        .update(value.definition_package.fingerprint().as_bytes());
    encoder.finish()
}

pub(crate) fn external_module(value: &crate::ProjectExternalModuleInput) -> String {
    let mut encoder = Encoder::new();
    encoder.byte(37);
    encoder.optional(value.package.is_some());
    if let Some(package) = value.package {
        encoder.number(package.0 as usize);
    }
    encoder.number(value.id.0 as usize);
    encoder.path(&value.path.segments);
    let mut exports: Vec<_> = value.exports.iter().collect();
    let visibility = |value: etas_hir::Visibility| match value {
        etas_hir::Visibility::Private => 0,
        etas_hir::Visibility::Public => 1,
    };
    exports.sort_by_key(|entry| (&entry.name, entry.symbol, visibility(entry.visibility)));
    encoder.number(exports.len());
    for export in exports {
        encoder.number(export.symbol.0 as usize);
        encoder.text(&export.name);
        encoder.byte(visibility(export.visibility));
    }
    encoder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_tails_and_segment_boundaries_are_distinct() {
        let closed = Row::default();
        let open = Row {
            effects: vec![],
            tail: Some("E".into()),
        };
        assert_ne!(external_row(&closed), external_row(&open));
        let row = |path| Row {
            effects: vec![Effect { path, args: vec![] }],
            tail: None,
        };
        assert_ne!(
            external_row(&row(vec!["a.b".into()])),
            external_row(&row(vec!["a".into(), "b".into()]))
        );
        let ty = |row| Type::Function {
            input: vec![],
            output: Box::new(Type::Prompt),
            effects: row,
        };
        assert_ne!(external_type(&ty(None)), external_type(&ty(Some(closed))));
    }

    #[test]
    fn named_signature_tracks_representation_and_generic_bounds() {
        let mut named = Named {
            path: vec!["lib".into(), "Value".into()],
            visibility: "public".into(),
            generic_params: vec![Generic {
                name: "T".into(),
                kind: GenericKind::Type,
                bounds: vec![Bound {
                    spec: vec!["lib".into(), "Bound".into()],
                    args: vec![Type::Primitive("i32".into())],
                }],
            }],
            ty: Some(Type::Primitive("i32".into())),
        };
        let original = external_named_signature(&named);
        named.ty = Some(Type::Primitive("string".into()));
        assert_ne!(original, external_named_signature(&named));
        named.ty = Some(Type::Primitive("i32".into()));
        named.generic_params[0].bounds[0].args[0] = Type::Primitive("string".into());
        assert_ne!(original, external_named_signature(&named));
        named.generic_params[0].bounds[0].args[0] = Type::Primitive("i32".into());
        named.generic_params[0].kind = GenericKind::Effect;
        assert_ne!(original, external_named_signature(&named));
    }

    #[test]
    fn trace_encoding_keeps_order_branches_sources_and_placeholders() {
        let event = |name: &str, source| Trace::Event {
            action: Effect {
                path: vec!["app".into(), "Gate".into(), name.into()],
                args: vec![],
            },
            source,
        };
        let a = event("a", TraceSource::Perform);
        let b = event("b", TraceSource::Perform);
        assert_ne!(
            external_trace(&Trace::Seq(vec![a.clone(), b.clone()])),
            external_trace(&Trace::Seq(vec![b.clone(), a.clone()]))
        );
        assert_ne!(
            external_trace(&Trace::Seq(vec![a.clone(), b.clone()])),
            external_trace(&Trace::Choice(vec![a.clone(), b.clone()]))
        );
        assert_ne!(
            external_trace(&a),
            external_trace(&event("a", TraceSource::StdIntrinsic))
        );
        assert_ne!(
            external_trace(&Trace::ParameterCall {
                parameter: "f".into()
            }),
            external_trace(&Trace::ParameterCall {
                parameter: "g".into()
            })
        );
        assert_ne!(
            external_trace(&Trace::Repeat(Box::new(a.clone()))),
            external_trace(&a)
        );
        assert_ne!(
            external_trace(&Trace::Widened {
                actions: vec![],
                parameter_calls: vec!["f".into()]
            }),
            external_trace(&Trace::Widened {
                actions: vec![],
                parameter_calls: vec![]
            })
        );
    }

    #[test]
    fn callable_spec_and_action_fields_are_part_of_structural_identity() {
        let flow = Flow {
            path: vec!["app".into(), "f".into()],
            generic_params: vec![],
            param_names: vec![],
            params: vec![],
            output: Type::Primitive("i32".into()),
            effects: None,
            visibility: "public".into(),
        };
        let mut changed = flow.clone();
        changed.effects = Some(Row::default());
        assert_ne!(external_flow(&flow), external_flow(&changed));
        let mut spec = Spec {
            path: vec!["app".into(), "S".into()],
            visibility: "public".into(),
            kind: SpecKind::Callable,
            param_names: vec![],
            generic_params: vec![],
            callable: Some(flow),
            methods: vec![],
            super_specs: vec![],
        };
        let before = external_spec(&spec);
        spec.callable.as_mut().unwrap().output = Type::Primitive("string".into());
        assert_ne!(before, external_spec(&spec));
        let mut action = Action {
            path: vec!["app".into(), "E".into(), "a".into()],
            generic_params: vec![],
            params: vec![],
            effect_args: vec![ActionKind::Type],
            selector_param_names: vec!["T".into()],
            selector_defaults: vec![None],
            output: Type::Primitive("unit".into()),
            returns_never: false,
            visibility: "public".into(),
        };
        let before = external_action(&action);
        action.selector_defaults[0] = Some(Arg::Wildcard);
        assert_ne!(before, external_action(&action));
        action.selector_defaults[0] = None;
        action.selector_param_names[0] = "U".into();
        assert_ne!(before, external_action(&action));
    }

    #[test]
    fn behavior_and_handler_requests_distinguish_absent_and_changed_facts() {
        use etas_package_metadata::{CallableValueBehavior, CallableValueDeterminism};
        let mut behavior = Some(CallableValueBehavior {
            determinism: CallableValueDeterminism::Deterministic,
            independent_determinism: None,
            host_requirements: vec![],
            interpreter_features: vec![],
        });
        assert_ne!(external_behavior(&None), external_behavior(&behavior));
        let before = external_behavior(&behavior);
        behavior.as_mut().unwrap().independent_determinism =
            Some(CallableValueDeterminism::Deterministic);
        assert_ne!(before, external_behavior(&behavior));
        let mut requests = Some(Requests {
            actions: vec![],
            parameters: vec![],
        });
        assert_ne!(external_requests(&None), external_requests(&requests));
        let before = external_requests(&requests);
        requests
            .as_mut()
            .unwrap()
            .parameters
            .push(ParameterRequest {
                parameter: "f".into(),
                excluded: vec![],
            });
        assert_ne!(before, external_requests(&requests));
    }

    #[test]
    fn dependency_effect_selector_metadata_and_source_bindings_are_fingerprinted() {
        let mut metadata = etas_effects::DependencyEffectMetadata {
            actions: vec![etas_effects::DependencyEffectAction {
                path: vec!["dep".into(), "E".into(), "act".into()],
                effect_args: vec![etas_effects::EffectActionArgKind::Type],
                selector_param_names: vec!["T".into()],
                selector_defaults: vec![None],
                returns_never: false,
                runtime_requirement: None,
            }],
            ..Default::default()
        };
        let before = external_dependency_effects(&metadata);
        metadata.actions[0].selector_defaults[0] = Some(etas_types::EffectArgRef::Wildcard);
        assert_ne!(before, external_dependency_effects(&metadata));
        metadata.actions[0].selector_defaults[0] = None;
        metadata.actions[0].selector_param_names[0] = "U".into();
        assert_ne!(before, external_dependency_effects(&metadata));
        metadata.actions[0].selector_param_names[0] = "T".into();
        metadata
            .source_bindings
            .push(etas_effects::DependencySourceEffectBinding {
                symbol: etas_hir::SymbolId(17),
                path: vec!["dep".into(), "E".into()],
            });
        assert_ne!(before, external_dependency_effects(&metadata));
    }

    #[test]
    fn spec_implementations_and_trace_alternatives_keep_full_structure() {
        use crate::{
            ProjectExternalTraceSpecClauseInput as Clause,
            ProjectExternalTraceSpecClauseKindInput as Kind,
            ProjectExternalTraceSpecSummaryInput as Summary,
        };
        let clause = |kind| Clause {
            kind,
            pattern: Some(Row::default()),
            guard: None,
            target: None,
            obligation: None,
            selector_bounds: vec![],
        };
        let allow = clause(Kind::Allow);
        let deny = clause(Kind::Deny);
        let mut summary = Summary {
            trace_spec: vec!["app".into(), "Safe".into()],
            definition: etas_code::DefinitionKey::new(
                etas_code::PackageKey::from_canonical_fingerprint(
                    etas_code::CodeFingerprint::from_bytes([7; 32]),
                ),
                vec!["app".into()],
                vec!["Safe".into()],
                etas_code::DefinitionKind::Spec,
            )
            .unwrap(),
            alternatives: vec![vec![allow.clone()], vec![deny.clone()]],
        };
        let original = external_trace_summary(&summary);
        let mut different_identity = summary.clone();
        different_identity.definition = etas_code::DefinitionKey::new(
            etas_code::PackageKey::from_canonical_fingerprint(
                etas_code::CodeFingerprint::from_bytes([8; 32]),
            ),
            vec!["app".into()],
            vec!["Safe".into()],
            etas_code::DefinitionKind::Spec,
        )
        .unwrap();
        assert_ne!(original, external_trace_summary(&different_identity));
        summary.alternatives = vec![vec![allow, deny]];
        assert_ne!(original, external_trace_summary(&summary));
        summary.alternatives[0][0].pattern = None;
        assert_ne!(original, external_trace_summary(&summary));
    }

    #[test]
    fn effect_summary_distinguishes_absent_default_handlers_and_open_latent_rows() {
        use crate::{
            ProjectExternalDefaultHandlerEffectsInput as Defaults,
            ProjectExternalEffectSummaryInput as Summary,
            ProjectExternalLatentFlowSummaryInput as Latent,
        };
        let mut summary = Summary {
            residual_checks: vec![],
            behavior: None,
            handler_requests: None,
            returned_callables: None,
            default_handler_effects: None,
            item: vec!["app".into(), "f".into()],
            public_effects: Row::default(),
            requested_actions: Row::default(),
            handled_requested_actions: Row::default(),
            latent_flows: vec![],
            action_trace: Trace::Empty,
        };
        let before = external_effect_summary(&summary);
        summary.default_handler_effects = Some(Defaults {
            behavior: None,
            direct_effects: Row::default(),
            default_actions: Row::default(),
            handlers: vec![],
        });
        assert_ne!(before, external_effect_summary(&summary));
        summary.default_handler_effects = None;
        summary.latent_flows.push(Latent {
            residual_checks: vec![],
            declared_bound: Row::default(),
            inferred_effects: Row::default(),
        });
        let before = external_effect_summary(&summary);
        summary.latent_flows[0].inferred_effects.tail = Some("E".into());
        assert_ne!(before, external_effect_summary(&summary));
        let reference = etas_code::residual::identity::ResidualObligationRef::new(
            etas_code::residual::identity::ObligationId::from_canonical_fingerprint(
                etas_code::CodeFingerprint::from_bytes([7; 32]),
            ),
            Default::default(),
        )
        .unwrap();
        let before = external_effect_summary(&summary);
        summary.residual_checks.push(reference.clone());
        assert_ne!(before, external_effect_summary(&summary));
        summary.residual_checks.clear();
        summary.latent_flows[0].residual_checks.push(reference);
        assert_ne!(before, external_effect_summary(&summary));
    }

    #[test]
    fn type_and_row_encoding_does_not_recurse_on_deep_metadata() {
        let mut ty = Type::Primitive("i32".into());
        for _ in 0..10_000 {
            ty = Type::Array(Box::new(ty));
        }
        let first = external_type(&ty);
        assert_eq!(first, external_type(&ty));
        // Dropping a recursive input is outside the encoder's traversal.
        std::mem::forget(ty);
    }
}
