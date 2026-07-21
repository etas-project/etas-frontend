use etas_types::{
    AgentSignature, EffectArgRef, EffectRef, EffectRowRef, FlowSignature, ItemSignature,
    ToolSignature, TopLevelLetSignature, TypeId,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) enum PersistedItemSignature {
    Flow {
        input: Vec<TypeId>,
        output: TypeId,
        effects: Option<PersistedEffectRowRef>,
        requested_actions: Option<PersistedEffectRowRef>,
    },
    Agent {
        input: Vec<TypeId>,
        output: TypeId,
        effects: Option<PersistedEffectRowRef>,
        requested_actions: Option<PersistedEffectRowRef>,
    },
    Tool {
        input: Vec<TypeId>,
        output: TypeId,
        effects: Option<PersistedEffectRowRef>,
        requested_actions: Option<PersistedEffectRowRef>,
    },
    TopLevelLet {
        ty: TypeId,
    },
}

impl PersistedItemSignature {
    pub(crate) fn from_frontend(signature: &ItemSignature) -> Self {
        match signature {
            ItemSignature::Flow(signature) => Self::Flow {
                input: signature.params.clone(),
                output: signature.output,
                effects: signature
                    .effects
                    .as_ref()
                    .map(PersistedEffectRowRef::from_frontend),
                requested_actions: signature
                    .requested_actions
                    .as_ref()
                    .map(PersistedEffectRowRef::from_frontend),
            },
            ItemSignature::Agent(signature) => Self::Agent {
                input: signature.params.clone(),
                output: signature.output,
                effects: signature
                    .effects
                    .as_ref()
                    .map(PersistedEffectRowRef::from_frontend),
                requested_actions: signature
                    .requested_actions
                    .as_ref()
                    .map(PersistedEffectRowRef::from_frontend),
            },
            ItemSignature::Tool(signature) => Self::Tool {
                input: signature.params.clone(),
                output: signature.output,
                effects: signature
                    .effects
                    .as_ref()
                    .map(PersistedEffectRowRef::from_frontend),
                requested_actions: signature
                    .requested_actions
                    .as_ref()
                    .map(PersistedEffectRowRef::from_frontend),
            },
            ItemSignature::TopLevelLet(signature) => Self::TopLevelLet { ty: signature.ty },
        }
    }

    pub(crate) fn into_frontend(self) -> ItemSignature {
        match self {
            Self::Flow {
                input,
                output,
                effects,
                requested_actions,
            } => ItemSignature::Flow(FlowSignature {
                params: input,
                output,
                effects: effects.map(PersistedEffectRowRef::into_frontend),
                requested_actions: requested_actions.map(PersistedEffectRowRef::into_frontend),
            }),
            Self::Agent {
                input,
                output,
                effects,
                requested_actions,
            } => ItemSignature::Agent(AgentSignature {
                params: input,
                output,
                effects: effects.map(PersistedEffectRowRef::into_frontend),
                requested_actions: requested_actions.map(PersistedEffectRowRef::into_frontend),
            }),
            Self::Tool {
                input,
                output,
                effects,
                requested_actions,
            } => ItemSignature::Tool(ToolSignature {
                params: input,
                output,
                effects: effects.map(PersistedEffectRowRef::into_frontend),
                requested_actions: requested_actions.map(PersistedEffectRowRef::into_frontend),
            }),
            Self::TopLevelLet { ty } => ItemSignature::TopLevelLet(TopLevelLetSignature { ty }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PersistedEffectRowRef {
    effects: Vec<PersistedEffectRef>,
    tail: Option<String>,
}

impl PersistedEffectRowRef {
    fn from_frontend(row: &EffectRowRef) -> Self {
        Self {
            effects: row
                .effects
                .iter()
                .map(PersistedEffectRef::from_frontend)
                .collect(),
            tail: row.tail.clone(),
        }
    }

    fn into_frontend(self) -> EffectRowRef {
        EffectRowRef {
            effects: self
                .effects
                .into_iter()
                .map(PersistedEffectRef::into_frontend)
                .collect(),
            tail: self.tail,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PersistedEffectRef {
    name: String,
    args: Vec<EffectArgRef>,
}

impl PersistedEffectRef {
    fn from_frontend(effect: &EffectRef) -> Self {
        Self {
            name: effect.name.clone(),
            args: effect.args.clone(),
        }
    }

    fn into_frontend(self) -> EffectRef {
        EffectRef {
            name: self.name,
            args: self.args,
        }
    }
}
