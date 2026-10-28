use etas_effects::{EffectCoverage, RequestInstantiationSummary};
use etas_hir::{HirExprId, HirHandlerArmId};
use etas_types::TypeId;

#[derive(Debug)]
pub(super) enum RequestError {
    Missing(String),
    Interrupted(etas_utils::PipelineInterruption),
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(reason) => f.write_str(reason),
            Self::Interrupted(error) => f.write_str(&error.reason),
        }
    }
}

pub(super) struct RequestInput<'a> {
    pub types: &'a etas_types::TypeOutput,
    pub effects: &'a etas_effects::EffectFacts,
    pub registry: &'a etas_effects::EffectRegistry,
    pub control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
}

pub(super) fn collect(
    input: RequestInput<'_>,
    arm: HirHandlerArmId,
    body: HirExprId,
) -> Result<Vec<Vec<(String, TypeId)>>, RequestError> {
    let missing = |reason: &str| RequestError::Missing(reason.into());
    let target = input
        .effects
        .handler_action_effects
        .get(&arm)
        .ok_or_else(|| missing("handler has no checked action identity"))?;
    let summary = input
        .effects
        .handled_body_requests
        .get(&body)
        .ok_or_else(|| missing("handled body has no checked request instantiation summary"))?;
    let RequestInstantiationSummary::Available {
        actions,
        parameters,
    } = summary
    else {
        return Err(missing(
            "handled body request instantiation evidence is unavailable",
        ));
    };
    if !parameters.is_empty() {
        return Err(missing(
            "handled body still has unresolved callable request parameters",
        ));
    }
    let coverage = EffectCoverage {
        registry: input.registry,
        types: &input.types.store,
    };
    let mut requests = Vec::new();
    for instance in actions {
        if let Some(control) = input.control {
            control.checkpoint().map_err(RequestError::Interrupted)?;
        }
        if coverage.covers(target, &instance.action) || coverage.covers(&instance.action, target) {
            requests.push(instance.type_bindings.clone().ok_or_else(|| {
                missing("handled request has no checked generic action instantiation")
            })?);
        }
    }
    Ok(requests)
}
