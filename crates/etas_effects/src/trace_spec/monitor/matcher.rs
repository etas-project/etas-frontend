use etas_code::residual::monitor::PriorEvidenceLabel;
use etas_utils::automaton::Matcher;

use crate::{ActionEvent, EffectRegistry};

use crate::trace_spec::model::TraceSpecPattern;

pub(super) struct TraceObservation<'a> {
    pub event: &'a ActionEvent,
    pub prior_guard_request: bool,
}

pub(super) struct TraceSpecMatcher<'a> {
    pub registry: &'a EffectRegistry,
    pub types: &'a etas_types::TypeOutput,
}

impl Matcher<PriorEvidenceLabel<TraceSpecPattern>, TraceObservation<'_>> for TraceSpecMatcher<'_> {
    fn matches(
        &self,
        label: &PriorEvidenceLabel<TraceSpecPattern>,
        observation: &TraceObservation<'_>,
    ) -> bool {
        let PriorEvidenceLabel::Missing { target, .. } = label;
        target.matches_event(observation.event, self.registry, self.types)
            && !observation.prior_guard_request
    }
}
