use super::*;
use etas_package_metadata::{DefaultBehavior, EffectRow, MethodTraceRequirement};

pub(super) fn validate(
    value: &DefaultBehavior,
    scope: &GenericScope,
    parameters: &[String],
) -> Result<(), String> {
    value
        .validate_resources()
        .map_err(|error| error.to_string())?;
    for entry in &value.guarded {
        if entry.guards.is_empty() {
            return Err("default behavior contribution has no action guard".into());
        }
        requests::validate_metadata_row(
            &EffectRow {
                effects: entry.guards.clone(),
                tail: None,
            },
            scope,
        )?;
    }
    for footprint in value.footprints() {
        if !footprint.residual_checks.is_empty() {
            return Err("default behavior residual checks require validated plans".into());
        }
        if footprint
            .behavior
            .independent_determinism
            .is_some_and(|independent| independent > footprint.behavior.determinism)
        {
            return Err("default behavior independent determinism exceeds its summary".into());
        }
        for row in [
            &footprint.escaping_effects,
            &footprint.requested_actions,
            &footprint.default_actions,
            &footprint.handled_actions,
        ] {
            requests::validate_metadata_row(row, scope)?;
        }
        if let Some(template) = &footprint.handler_requests {
            for action in &template.actions {
                requests::validate_metadata_row(
                    &EffectRow {
                        effects: vec![action.action.clone()],
                        tail: None,
                    },
                    scope,
                )?;
                if let Some(signature) = &action.signature {
                    for ty in signature
                        .params
                        .iter()
                        .chain(std::iter::once(&signature.output))
                    {
                        requests::validate_metadata_type(ty, scope)?;
                    }
                }
                let mut names = HashSet::new();
                for binding in action.type_bindings.iter().flatten() {
                    if binding.parameter.is_empty() || !names.insert(&binding.parameter) {
                        return Err("default request has invalid generic bindings".into());
                    }
                    requests::validate_metadata_type(&binding.ty, scope)?;
                }
            }
            for parameter in &template.parameters {
                requests::validate_target(
                    &parameter.parameter,
                    scope,
                    parameters,
                    parameters.len(),
                )?;
                requests::validate_metadata_row(
                    &EffectRow {
                        effects: parameter.excluded.clone(),
                        tail: None,
                    },
                    scope,
                )?;
            }
        }
        for requirement in footprint
            .requirements
            .trace_requirements
            .iter()
            .chain(&footprint.requirements.trace_obligations)
        {
            if let MethodTraceRequirement::Clause(clause) = requirement {
                for row in [
                    &clause.pattern,
                    &clause.guard,
                    &clause.target,
                    &clause.obligation,
                ]
                .into_iter()
                .flatten()
                {
                    requests::validate_metadata_row(row, scope)?;
                }
                for bound in &clause.selector_bounds {
                    validate_path(&bound.effect_path, "default trace effect")?;
                    validate_path(&bound.spec_path, "default trace spec")?;
                    for ty in &bound.spec_args {
                        requests::validate_metadata_type(ty, scope)?;
                    }
                }
            }
        }
    }
    Ok(())
}
