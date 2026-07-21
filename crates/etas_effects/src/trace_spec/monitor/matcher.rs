use etas_utils::automaton::Matcher;

use crate::{ActionEvent, EffectRegistry};

use super::compiler::TraceSpecLabel;

pub(super) struct TraceSpecMatcher<'a> {
    pub registry: &'a EffectRegistry,
    pub types: &'a etas_types::TypeOutput,
}

impl Matcher<TraceSpecLabel, ActionEvent> for TraceSpecMatcher<'_> {
    fn matches(&self, label: &TraceSpecLabel, event: &ActionEvent) -> bool {
        match label {
            TraceSpecLabel::BeforeGuard(guard) => {
                guard.matches_event(event, self.registry, self.types)
            }
            TraceSpecLabel::BeforeTargetWithoutGuard { guard, target } => {
                target.matches_event(event, self.registry, self.types)
                    && !guard.matches_event(event, self.registry, self.types)
            }
            TraceSpecLabel::AfterTarget(target) => {
                target.matches_event(event, self.registry, self.types)
            }
            TraceSpecLabel::AfterObligationWithoutTarget { target, obligation } => {
                obligation.matches_event(event, self.registry, self.types)
                    && !target.matches_event(event, self.registry, self.types)
            }
        }
    }
}
