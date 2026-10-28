use super::*;
use etas_effects::{Determinism, HostRequirementKind, InterpreterFeatureKind, InterpreterSupport};
use etas_package_metadata::{
    CallableValueBehavior, CallableValueDeterminism as D, CallableValueHostRequirement as H,
    CallableValueInterpreterFeature as I,
};

pub(super) fn behavior(summary: &EffectSummary) -> Option<CallableValueBehavior> {
    let (host, features) = match &summary.support {
        InterpreterSupport::LocalOnly => (vec![], vec![]),
        InterpreterSupport::RequiresHost(host) => {
            (host.kinds.iter().map(host_kind).collect(), vec![])
        }
        InterpreterSupport::RequiresInterpreterOrchestration(requirements) => (
            requirements.host.kinds.iter().map(host_kind).collect(),
            requirements
                .features
                .kinds
                .iter()
                .map(|kind| match kind {
                    InterpreterFeatureKind::EffectHandler => I::EffectHandler,
                    InterpreterFeatureKind::Resume => I::Resume,
                    InterpreterFeatureKind::Retry => I::Retry,
                    InterpreterFeatureKind::Checkpoint => I::Checkpoint,
                    InterpreterFeatureKind::WorkflowOrchestration => I::WorkflowOrchestration,
                })
                .collect(),
        ),
        InterpreterSupport::Rejected(_) => return None,
    };
    Some(CallableValueBehavior {
        determinism: match summary.determinism {
            Determinism::Deterministic => D::Deterministic,
            Determinism::RuntimeMediated => D::RuntimeMediated,
            Determinism::NonDeterministic => D::NonDeterministic,
        },
        host_requirements: host,
        interpreter_features: features,
    })
}

fn host_kind(kind: &HostRequirementKind) -> H {
    match kind {
        HostRequirementKind::Agentic => H::Agentic,
        HostRequirementKind::ToolCall => H::ToolCall,
        HostRequirementKind::HostAuthority => H::HostAuthority,
        HostRequirementKind::DurableMemory => H::DurableMemory,
        HostRequirementKind::Approval => H::Approval,
        HostRequirementKind::Checkpoint => H::Checkpoint,
        HostRequirementKind::Time => H::Time,
        HostRequirementKind::Network => H::Network,
        HostRequirementKind::Tcp => H::Tcp,
        HostRequirementKind::Stream => H::Stream,
        HostRequirementKind::Tls => H::Tls,
        HostRequirementKind::Browser => H::Browser,
        HostRequirementKind::Console => H::Console,
        HostRequirementKind::FileIO => H::FileIO,
        HostRequirementKind::Command => H::Command,
        HostRequirementKind::SecretAccess => H::SecretAccess,
        HostRequirementKind::Compiler => H::Compiler,
    }
}
