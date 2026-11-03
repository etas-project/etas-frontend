use super::*;

#[test]
fn unavailable_evidence_is_not_an_empty_checked_request_set() {
    let mut requests = RequestInstantiationSummary::default();
    assert!(requests.less_equal(&RequestInstantiationSummary::Unavailable));
    assert!(!RequestInstantiationSummary::Unavailable.less_equal(&requests));
    assert!(requests.union_assign(&RequestInstantiationSummary::Unavailable));
    assert_eq!(requests, RequestInstantiationSummary::Unavailable);
}

#[test]
fn lexical_handler_masks_survive_widening() {
    let action = Effect::Tag(crate::EffectTagId(200));
    let mut requests = RequestInstantiationSummary::action(
        action.clone(),
        vec![("T".into(), TypeId(7))],
        etas_types::ActionRequestSignature {
            params: vec![TypeId(7)],
            output: TypeId(7),
        },
    );
    requests.union_assign(&RequestInstantiationSummary::parameter(
        ParameterCallTarget::Native(etas_hir::SymbolId(8)),
        "callback".into(),
        etas_core::Span::empty(etas_core::SourceId(9), etas_core::TextSize(0)),
    ));
    let registry = crate::EffectRegistry::default();
    let types = etas_types::TypeStore::default();
    requests.subtract_handled(
        &EffectCoverage {
            registry: &registry,
            types: &types,
        },
        &EffectRow::closed(EffectSet::one(action.clone())),
    );
    let RequestInstantiationSummary::Available {
        actions,
        parameters,
    } = &requests
    else {
        panic!("checked requests expected");
    };
    assert!(actions.is_empty());
    assert_eq!(parameters.len(), 1);
    assert!(parameters.first().unwrap().excluded.contains(&action));
    let mut summary = crate::EffectSummary {
        handler_requests: requests.clone(),
        action_trace: crate::ActionTraceDomain::Repeat(Box::new(crate::ActionTraceDomain::Empty)),
        ..Default::default()
    };
    summary.stabilize_for_fixpoint(None);
    assert_eq!(summary.handler_requests, requests);
}
