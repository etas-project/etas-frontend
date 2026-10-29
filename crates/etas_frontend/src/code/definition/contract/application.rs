use etas_code::{
    CodeFingerprint,
    descriptor::{CallableHeaderInfo, DescriptorGraph, DescriptorLimits, GenericOwner},
};

use super::{DefinitionCatalog, FrozenContractOwner, FrozenDefinitionContract, ProjectionError};
use crate::{
    AppliedCallableBehavior, CheckedCallableApplication,
    code::descriptor::DefinitionDescriptorProjector,
};

pub(super) struct AppliedContract {
    pub header: CallableHeaderInfo,
    pub graph: DescriptorGraph,
    pub behavior: AppliedCallableBehavior,
    pub fingerprint: CodeFingerprint,
}

impl FrozenDefinitionContract {
    pub(crate) fn capture_application(
        catalog: &DefinitionCatalog,
        application: &CheckedCallableApplication,
        limits: DescriptorLimits,
    ) -> Result<std::sync::Arc<Self>, ProjectionError> {
        // This validates native ownership as well as phantom arguments. An
        // equal arrow or user-created descriptor cannot establish this proof.
        let fingerprint = catalog.callable_application_fingerprint(application, limits)?;
        let symbol = catalog
            .checked
            .items
            .get(application.owner())
            .and_then(etas_hir::HirItem::symbol)
            .ok_or_else(|| ProjectionError::MissingIdentity("applied flow declaration".into()))?;
        let callable = catalog
            .callable_bindings
            .get(&symbol)
            .ok_or(ProjectionError::MissingCallable(symbol))?;
        let identity = callable
            .as_declaration()
            .filter(|identity| identity.kind() == etas_code::DefinitionKind::Flow)
            .ok_or_else(|| {
                ProjectionError::Invalid("applied reflection requires an ordinary flow".into())
            })?
            .clone();
        let behavior = application
            .applied_behavior(limits, None)
            .map_err(|error| ProjectionError::Invalid(error.to_string()))?;
        let (names, declared) = super::super::collect::callable_source_header(
            catalog.checked.as_ref().into(),
            application.owner(),
        )?;
        let mut projector =
            DefinitionDescriptorProjector::new(&catalog.checked, &catalog.identities, limits);
        let signature = projector.project_signature(
            GenericOwner::Declaration(identity.clone()),
            application.signature(),
            &names,
            declared.map(|_| application.signature().output),
        )?;
        let analysis = projector.project_analysis_with_summary(
            callable,
            application.owner(),
            signature,
            Some(behavior.summary()),
        )?;
        let applied = AppliedContract {
            header: CallableHeaderInfo {
                id: callable.clone(),
                signature: signature.ty,
                analysis,
            },
            graph: projector.finish()?,
            behavior,
            fingerprint,
        };
        Self::capture_with_application(
            catalog,
            FrozenContractOwner::Flow {
                identity,
                entry: callable.clone(),
            },
            Some(applied),
        )
    }
}
