use crate::TypeId;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct CheckedExistentialPackFact {
    pub source_type: TypeId,
    pub object_type: TypeId,
    pub witnesses: Vec<ExistentialMethodWitnessFact>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct ExistentialMethodWitnessFact {
    pub member: crate::SpecMethodIdentity,
    pub bound: crate::CheckedSpecBound,
    pub target: crate::method::MethodTarget,
}

impl CheckedExistentialPackFact {
    pub(crate) fn try_map_types<E>(
        &mut self,
        mut map: impl FnMut(TypeId) -> Result<TypeId, E>,
    ) -> Result<(), E> {
        self.source_type = map(self.source_type)?;
        self.object_type = map(self.object_type)?;
        for witness in &mut self.witnesses {
            for arg in &mut witness.bound.args {
                *arg = map(*arg)?;
            }
        }
        Ok(())
    }
}
