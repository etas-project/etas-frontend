use etas_hir::{HirBodyRef, HirItemId, HirModuleId, SymbolId};

use crate::{CallableGenericParam, CallableSignature, CheckedSpecRef, TypeId};

use super::MethodEvidence;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MethodSelection {
    Unqualified,
    Associated,
    ExplicitSpec {
        spec: CheckedSpecRef,
        args: Vec<TypeId>,
    },
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum MethodReceiver {
    Value { slot: u32 },
    Component,
    Associated,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum MethodOwner {
    Inherent(TypeId),
    Spec(CheckedSpecRef),
    Standard,
    Agent(TypeId),
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum MethodTarget {
    Source {
        symbol: SymbolId,
        callable: HirItemId,
        body: HirBodyRef,
    },
    Standard {
        qualified_path: Vec<String>,
        intrinsic: Option<u32>,
        overload: usize,
        operation: etas_std::FlowSourceMethodOperation,
    },
    Agent {
        symbol: SymbolId,
        callable: HirItemId,
    },
    External {
        identity: etas_code::MethodId,
    },
    BoundSpec {
        spec: CheckedSpecRef,
        member: crate::SpecMethodIdentity,
        receiver_parameter: Option<u32>,
    },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct MethodSignatureFact {
    pub name: String,
    pub owner: MethodOwner,
    pub receiver: MethodReceiver,
    pub receiver_type: TypeId,
    pub owner_params: Vec<CallableGenericParam>,
    pub signature: CallableSignature,
    pub parameter_names: Vec<Option<String>>,
    pub target: MethodTarget,
    pub spec_member: Option<crate::SpecMethodIdentity>,
    pub evidence: MethodEvidence,
    pub defining_module: Option<HirModuleId>,
    pub public: bool,
}

impl MethodSignatureFact {
    pub(crate) fn try_map_types<E>(
        &mut self,
        mut map: impl FnMut(TypeId) -> Result<TypeId, E>,
    ) -> Result<(), E> {
        self.receiver_type = map(self.receiver_type)?;
        match &mut self.owner {
            MethodOwner::Inherent(owner) | MethodOwner::Agent(owner) => *owner = map(*owner)?,
            _ => {}
        }
        crate::facts::methods::map_signature(&mut self.signature, &mut map)?;
        crate::facts::methods::map_parameters(&mut self.owner_params, &mut map)?;
        match &mut self.evidence {
            MethodEvidence::Implementation { subject, args, .. }
            | MethodEvidence::Bound { subject, args, .. } => {
                *subject = map(*subject)?;
                for arg in args {
                    *arg = map(*arg)?;
                }
            }
            MethodEvidence::Inherent => {}
        }
        Ok(())
    }
}
