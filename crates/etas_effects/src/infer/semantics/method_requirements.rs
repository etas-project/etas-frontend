use super::{engine::EffectSemantics, specialize::EffectSpecializationError};
use crate::{
    ExternalMethodRequirements, ExternalMethodTraceRequirement as R, RequirementFact,
    RequirementSet, TraceSpecClauseFact as F,
};

impl EffectSemantics<'_> {
    pub(super) fn method_requirements(
        &self,
        input: &ExternalMethodRequirements,
    ) -> Result<(RequirementSet, RequirementSet), EffectSpecializationError> {
        let mut requirements = input
            .limits
            .iter()
            .cloned()
            .map(RequirementFact::Limit)
            .collect::<RequirementSet>();
        for requirement in &input.trace_requirements {
            requirements.insert(RequirementFact::TraceSpec(
                self.method_trace_requirement(requirement)?,
            ));
        }
        let mut obligations = RequirementSet::new();
        for requirement in &input.trace_obligations {
            obligations.insert(RequirementFact::TraceSpec(
                self.method_trace_requirement(requirement)?,
            ));
        }
        Ok((requirements, obligations))
    }

    fn method_trace_requirement(&self, input: &R) -> Result<F, EffectSpecializationError> {
        use crate::ExternalTraceSpecClauseKind as K;
        let invalid = || EffectSpecializationError::InvalidCallableEnvironment {
            reason: "published method trace requirement has incomplete effect facts".into(),
        };
        let clause = match input {
            R::Reference(name) => return Ok(F::TraceSpecReference { name: name.clone() }),
            R::Limit(count) => return Ok(F::Limit { count: *count }),
            R::Clause(clause) => clause,
        };
        let row = |value: Option<&crate::ExternalTraceSpecEffectRowMetadata>| {
            let value = value.ok_or_else(invalid)?;
            let row = crate::EffectRow {
                effects: value
                    .effects
                    .iter()
                    .map(|effect| {
                        self.external_effect_from_metadata(&crate::ExternalEffectMetadata {
                            path: effect.path.clone(),
                            args: effect.args.clone(),
                        })
                    })
                    .collect::<Option<_>>()
                    .ok_or_else(invalid)?,
                open: None,
            };
            let label = value
                .effects
                .iter()
                .map(|effect| effect.path.join("."))
                .collect::<Vec<_>>()
                .join(" | ");
            Ok::<_, EffectSpecializationError>((row, label))
        };
        Ok(match clause.kind {
            K::Allow => {
                let (pattern, label) = row(clause.pattern.as_ref())?;
                F::Allow { pattern, label }
            }
            K::Deny => {
                let (pattern, label) = row(clause.pattern.as_ref())?;
                F::Deny { pattern, label }
            }
            K::RequireBefore => {
                let (guard, guard_label) = row(clause.guard.as_ref())?;
                let (target, target_label) = row(clause.target.as_ref())?;
                F::RequireBefore {
                    guard,
                    guard_label,
                    target,
                    target_label,
                }
            }
            K::RequireAfter => {
                let (target, target_label) = row(clause.target.as_ref())?;
                let (obligation, obligation_label) = row(clause.obligation.as_ref())?;
                F::RequireAfter {
                    target,
                    target_label,
                    obligation,
                    obligation_label,
                }
            }
        })
    }
}
