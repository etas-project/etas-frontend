use super::SummaryMapper;
use crate::{ActionRequestInstance, ParameterRequest, RequestInstantiationSummary};

pub(crate) fn map_requests<M: SummaryMapper>(
    mapper: &mut M,
    source: &RequestInstantiationSummary,
) -> Result<RequestInstantiationSummary, M::Error> {
    let RequestInstantiationSummary::Available {
        actions,
        parameters,
    } = source
    else {
        return Ok(RequestInstantiationSummary::Unavailable);
    };
    let mut mapped_actions = std::collections::BTreeSet::new();
    for action in actions {
        mapper.checkpoint()?;
        let mapped = mapper.effect(action.action.clone())?;
        if !mapped.is_action() {
            return Err(mapper.invalid_structure());
        }
        mapped_actions.insert(ActionRequestInstance {
            action: mapped,
            type_bindings: action
                .type_bindings
                .as_ref()
                .map(|bindings| {
                    bindings
                        .iter()
                        .map(|(name, ty)| Ok((name.clone(), mapper.ty(*ty)?)))
                        .collect::<Result<_, M::Error>>()
                })
                .transpose()?,
            signature: action
                .signature
                .as_ref()
                .map(|signature| {
                    Ok::<_, M::Error>(etas_types::ActionRequestSignature {
                        params: signature
                            .params
                            .iter()
                            .map(|ty| mapper.ty(*ty))
                            .collect::<Result<_, _>>()?,
                        output: mapper.ty(signature.output)?,
                    })
                })
                .transpose()?,
        });
    }
    let mut mapped_parameters = std::collections::BTreeSet::new();
    for parameter in parameters {
        mapper.checkpoint()?;
        mapped_parameters.insert(ParameterRequest {
            target: parameter.target,
            parameter: parameter.parameter.clone(),
            span: parameter.span,
            excluded: parameter
                .excluded
                .iter()
                .cloned()
                .map(|effect| mapper.effect(effect))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .collect(),
        });
    }
    Ok(RequestInstantiationSummary::Available {
        actions: mapped_actions,
        parameters: mapped_parameters,
    })
}
