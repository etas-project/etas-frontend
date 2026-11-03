use crate::{CheckedSpecRef, TypeId};

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum MethodEvidence {
    Inherent,
    Implementation {
        spec: CheckedSpecRef,
        subject: TypeId,
        args: Vec<TypeId>,
    },
    Bound {
        spec: CheckedSpecRef,
        subject: TypeId,
        args: Vec<TypeId>,
    },
}
