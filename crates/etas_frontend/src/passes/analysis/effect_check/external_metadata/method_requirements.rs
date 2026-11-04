use etas_effects::{ExternalMethodRequirements, ExternalMethodTraceRequirement as R};
use etas_package_metadata::{
    MethodLimitKind as K, MethodLimitValue as V, MethodTraceRequirement as T,
};
use etas_types::TypeOutput;

pub(super) fn lower(
    value: &etas_package_metadata::MethodRequirements,
    types: &TypeOutput,
) -> Result<ExternalMethodRequirements, String> {
    use etas_effects::{LimitKind as L, LimitValue as W};
    Ok(ExternalMethodRequirements {
        limits: value
            .limits
            .iter()
            .map(|limit| {
                let kind = match limit.kind {
                    K::Iterations => L::Iterations,
                    K::Tokens => L::Tokens,
                    K::ContextTokens => L::ContextTokens,
                    K::Cost => L::Cost,
                    K::WallTime => L::WallTime,
                    K::Attempts => L::Attempts,
                };
                etas_effects::LimitRequirement {
                    kind,
                    budget: kind.budget_kind(),
                    value: limit.value.as_ref().map(|value| match value {
                        V::Count(count) => W::Count(*count),
                        V::DurationMillis(ms) => W::DurationMillis(*ms),
                        V::MoneyMicros { amount, currency } => W::MoneyMicros {
                            amount: *amount,
                            currency: currency.clone(),
                        },
                    }),
                }
            })
            .collect(),
        trace_requirements: value
            .trace_requirements
            .iter()
            .map(|value| trace(value, types))
            .collect::<Result<_, _>>()?,
        trace_obligations: value
            .trace_obligations
            .iter()
            .map(|value| trace(value, types))
            .collect::<Result<_, _>>()?,
    })
}

fn trace(value: &T, types: &TypeOutput) -> Result<R, String> {
    use etas_effects::ExternalTraceSpecClauseKind as K;
    Ok(match value {
        T::Reference(name) => R::Reference(name.clone()),
        T::Limit(count) => R::Limit(
            (*count)
                .try_into()
                .map_err(|_| "method trace limit exceeds target ABI")?,
        ),
        T::Clause(clause) => {
            if !clause.selector_bounds.is_empty() {
                return Err("method trace requirement still contains selector binders".into());
            }
            R::Clause(etas_effects::ExternalTraceSpecClauseMetadata {
                kind: match clause.kind {
                    etas_package_metadata::TraceSpecClauseKind::Allow => K::Allow,
                    etas_package_metadata::TraceSpecClauseKind::Deny => K::Deny,
                    etas_package_metadata::TraceSpecClauseKind::RequireBefore => K::RequireBefore,
                    etas_package_metadata::TraceSpecClauseKind::RequireAfter => K::RequireAfter,
                },
                pattern: clause
                    .pattern
                    .as_ref()
                    .map(|row| lower_row(row, types))
                    .transpose()?,
                guard: clause
                    .guard
                    .as_ref()
                    .map(|row| lower_row(row, types))
                    .transpose()?,
                target: clause
                    .target
                    .as_ref()
                    .map(|row| lower_row(row, types))
                    .transpose()?,
                obligation: clause
                    .obligation
                    .as_ref()
                    .map(|row| lower_row(row, types))
                    .transpose()?,
            })
        }
    })
}

fn lower_row(
    value: &etas_package_metadata::EffectRow,
    types: &TypeOutput,
) -> Result<etas_effects::ExternalTraceSpecEffectRowMetadata, String> {
    let row = crate::project::lower_package_effect_row(value, types)?;
    if row.tail.is_some() {
        return Err("published method trace pattern must be closed".into());
    }
    Ok(etas_effects::ExternalTraceSpecEffectRowMetadata {
        effects: row
            .effects
            .into_iter()
            .map(|effect| etas_effects::ExternalTraceSpecEffectMetadata {
                path: effect.path,
                args: effect.args,
            })
            .collect(),
    })
}
