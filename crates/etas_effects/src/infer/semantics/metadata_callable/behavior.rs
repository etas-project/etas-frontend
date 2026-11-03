use crate::infer::domain::InterpreterFeatureSet;
use crate::{
    Determinism, HostRequirementKind, HostRequirementSet, InterpreterFeatureKind,
    InterpreterOrchestrationRequirementSet, InterpreterSupport,
};
use etas_package_metadata::{
    CallableValueBehavior, CallableValueDeterminism as D, CallableValueHostRequirement as H,
    CallableValueInterpreterFeature as I,
};

pub(super) fn lower(behavior: &CallableValueBehavior) -> (Determinism, InterpreterSupport) {
    let host = HostRequirementSet {
        kinds: behavior
            .host_requirements
            .iter()
            .map(|kind| match kind {
                H::Agentic => HostRequirementKind::Agentic,
                H::ToolCall => HostRequirementKind::ToolCall,
                H::HostAuthority => HostRequirementKind::HostAuthority,
                H::DurableMemory => HostRequirementKind::DurableMemory,
                H::Approval => HostRequirementKind::Approval,
                H::Checkpoint => HostRequirementKind::Checkpoint,
                H::Time => HostRequirementKind::Time,
                H::Network => HostRequirementKind::Network,
                H::Tcp => HostRequirementKind::Tcp,
                H::Stream => HostRequirementKind::Stream,
                H::Tls => HostRequirementKind::Tls,
                H::Browser => HostRequirementKind::Browser,
                H::Console => HostRequirementKind::Console,
                H::FileIO => HostRequirementKind::FileIO,
                H::Command => HostRequirementKind::Command,
                H::SecretAccess => HostRequirementKind::SecretAccess,
                H::Compiler => HostRequirementKind::Compiler,
            })
            .collect(),
    };
    let features = InterpreterFeatureSet {
        kinds: behavior
            .interpreter_features
            .iter()
            .map(|kind| match kind {
                I::EffectHandler => InterpreterFeatureKind::EffectHandler,
                I::Resume => InterpreterFeatureKind::Resume,
                I::Retry => InterpreterFeatureKind::Retry,
                I::Checkpoint => InterpreterFeatureKind::Checkpoint,
                I::WorkflowOrchestration => InterpreterFeatureKind::WorkflowOrchestration,
            })
            .collect(),
    };
    let support = if !features.kinds.is_empty() {
        InterpreterSupport::RequiresInterpreterOrchestration(
            InterpreterOrchestrationRequirementSet { host, features },
        )
    } else if !host.kinds.is_empty() {
        InterpreterSupport::RequiresHost(host)
    } else {
        InterpreterSupport::LocalOnly
    };
    (lower_determinism(behavior.determinism), support)
}

pub(super) fn lower_determinism(value: D) -> Determinism {
    match value {
        D::Deterministic => Determinism::Deterministic,
        D::RuntimeMediated => Determinism::RuntimeMediated,
        D::NonDeterministic => Determinism::NonDeterministic,
    }
}
