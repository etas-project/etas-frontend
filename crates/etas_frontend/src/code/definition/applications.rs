use std::sync::Arc;

use etas_code::{
    CodeFingerprint,
    descriptor::{DescriptorLimits, GenericOwner},
};
use etas_types::{CallableGenericArg, EffectRowRef};

use super::DefinitionCatalog;
use crate::{
    CheckedCallableApplication,
    code::descriptor::{DefinitionDescriptorProjector, ProjectionError},
};

impl DefinitionCatalog {
    /// Portable binder-position identity for a sealed native application. No
    /// TypeId, HIR ID, or descriptive type name participates in the digest.
    pub fn callable_application_fingerprint(
        &self,
        application: &CheckedCallableApplication,
        limits: DescriptorLimits,
    ) -> Result<CodeFingerprint, ProjectionError> {
        if !Arc::ptr_eq(application.program(), &self.checked) {
            return Err(ProjectionError::Invalid(
                "callable application belongs to a different checked program".into(),
            ));
        }
        let symbol = self
            .checked
            .items
            .get(application.owner())
            .and_then(etas_hir::HirItem::symbol)
            .ok_or_else(|| {
                ProjectionError::MissingIdentity("callable application declaration".into())
            })?;
        let callable = self
            .callable_bindings
            .get(&symbol)
            .ok_or(ProjectionError::MissingCallable(symbol))?;
        let owner = match (callable.as_declaration(), callable.as_method()) {
            (Some(declaration), None) => GenericOwner::Declaration(declaration.clone()),
            (None, Some(method)) => GenericOwner::Method(method.clone()),
            _ => {
                return Err(ProjectionError::Invalid(
                    "callable application has no canonical owner".into(),
                ));
            }
        };
        let mut projection =
            DefinitionDescriptorProjector::new(&self.checked, &self.identities, limits);
        let mut types = Vec::new();
        let mut rows = Vec::new();
        let mut positions = Vec::new();
        for argument in application.arguments() {
            match argument {
                CallableGenericArg::Type(ty) => {
                    let projected = projection.project_closed_types(owner.clone(), &[*ty])?;
                    types.push(*projected.first().ok_or(ProjectionError::MissingType(*ty))?);
                    positions.push(Argument::Type(types.len() - 1));
                }
                CallableGenericArg::EffectRow(row) => {
                    // Row members form a set. Fingerprint singleton rows before
                    // sorting, so local interner order cannot choose its identity.
                    let members = row
                        .effects
                        .iter()
                        .cloned()
                        .map(|effect| EffectRowRef {
                            effects: vec![effect],
                            tail: None,
                        })
                        .collect::<Vec<_>>();
                    rows.push(projection.project_closed_rows(owner.clone(), &members)?);
                    positions.push(Argument::Row(rows.len() - 1));
                }
            }
        }
        let graph = projection.finish()?;
        let mut fingerprints = vec![callable.fingerprint()];
        for (position, argument) in positions.into_iter().enumerate() {
            let value = match argument {
                Argument::Type(index) => graph.type_fingerprint(types[index])?,
                Argument::Row(index) => {
                    let mut members = rows[index]
                        .iter()
                        .map(|row| graph.row_fingerprint(*row))
                        .collect::<Result<Vec<_>, _>>()?;
                    members.sort_unstable();
                    members.dedup();
                    CodeFingerprint::of_parts(
                        "etas.code.application-row.v1",
                        members.iter().map(|member| member.as_bytes().as_slice()),
                    )
                }
            };
            fingerprints.push(CodeFingerprint::of_parts(
                match argument {
                    Argument::Type(_) => "etas.code.application-type-argument.v1",
                    Argument::Row(_) => "etas.code.application-effect-argument.v1",
                },
                [
                    (position as u64).to_le_bytes().as_slice(),
                    value.as_bytes().as_slice(),
                ],
            ));
        }
        Ok(CodeFingerprint::of_parts(
            "etas.code.callable-application.v1",
            fingerprints
                .iter()
                .map(|fingerprint| fingerprint.as_bytes().as_slice()),
        ))
    }
}

#[derive(Clone, Copy)]
enum Argument {
    Type(usize),
    Row(usize),
}
