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
    pub template_fingerprint: CodeFingerprint,
}

/// Identity of a checked application of an immutable generic contract. This
/// binds the template, checked arguments and projected behavior; it is not
/// executable evidence and does not replace candidate compatibility checks.
pub(in crate::code::definition) fn applied_contract_fingerprint(
    template: CodeFingerprint,
    application: CodeFingerprint,
    graph: &DescriptorGraph,
    header: &CallableHeaderInfo,
) -> Result<CodeFingerprint, ProjectionError> {
    let mut fields = vec![
        template,
        application,
        header.id.fingerprint(),
        graph.type_fingerprint(header.signature)?,
    ];
    let rows = &header.analysis.effects;
    for row in [
        rows.declared_bound,
        rows.escaping,
        rows.requested,
        rows.default_handled,
    ] {
        fields.push(match row {
            Some(row) => CodeFingerprint::of_parts(
                "etas.code.applied-contract-row.v1",
                [graph.row_fingerprint(row)?.as_bytes().as_slice()],
            ),
            None => CodeFingerprint::of_bytes("etas.code.applied-contract-no-row.v1", b""),
        });
    }
    for trace in [header.analysis.trace, header.analysis.outcomes] {
        fields.push(match trace {
            Some(trace) => CodeFingerprint::of_parts(
                "etas.code.applied-contract-trace.v1",
                [graph.trace_fingerprint(trace)?.as_bytes().as_slice()],
            ),
            None => CodeFingerprint::of_bytes("etas.code.applied-contract-no-trace.v1", b""),
        });
    }
    for determinism in [
        header.analysis.determinism,
        header.analysis.independent_determinism,
    ] {
        fields.push(CodeFingerprint::of_bytes(
            "etas.code.applied-contract-determinism.v1",
            &[match determinism {
                None => 0,
                Some(etas_code::descriptor::DeterminismInfo::Deterministic) => 1,
                Some(etas_code::descriptor::DeterminismInfo::RuntimeMediated) => 2,
                Some(etas_code::descriptor::DeterminismInfo::NonDeterministic) => 3,
            }],
        ));
    }
    Ok(CodeFingerprint::of_parts(
        "etas.code.applied-frozen-contract.v1",
        fields.iter().map(|field| field.as_bytes().as_slice()),
    ))
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
        let template_header = &catalog
            .callables
            .get(callable)
            .ok_or_else(|| ProjectionError::MissingIdentity("generic callable template".into()))?
            .header;
        let template_fingerprint = Self::capture_flow(catalog, template_header)?.fingerprint();
        let residual_bindings = if catalog
            .checked
            .effects
            .item_effects
            .get(&application.owner())
            .is_some_and(|summary| !summary.residual_checks.is_empty())
        {
            let owner = catalog.residual_source_owner(application.owner())?;
            Some(catalog.residual_application_bindings(application, limits, owner)?)
        } else {
            None
        };
        let behavior = application
            .applied_behavior_with_residual_bindings(limits, None, residual_bindings.as_ref())
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
            template_fingerprint,
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
