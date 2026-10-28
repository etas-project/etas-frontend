use etas_code::CodeFingerprint;
use etas_package_metadata as metadata;

use super::{DefinitionCatalog, ProjectionError};

pub(super) fn request_instances(
    catalog: &DefinitionCatalog,
    template: &metadata::RequestInstantiationTemplate,
) -> Result<std::collections::BTreeSet<CodeFingerprint>, ProjectionError> {
    Fingerprinter { catalog }.requests(template)
}

#[cfg(test)]
mod tests;

/// Portable graph paths are import-local. Contract identity uses the checked
/// declaration index instead; graph edges and lexical binder identities stay intact.
pub(super) fn fingerprint(
    catalog: &DefinitionCatalog,
    graph: &metadata::CallableValueGraph,
) -> Result<CodeFingerprint, ProjectionError> {
    metadata::validate_callable_value_graph(graph)
        .map_err(|error| ProjectionError::Invalid(error.to_string()))?;
    let mut parts = vec![encoded("roots", &graph.sources)?];
    let projector = Fingerprinter { catalog };
    for node in &graph.values {
        parts.push(projector.node(node)?);
    }
    Ok(combine("etas.code.canonical-latent-graph.v1", parts))
}

struct Fingerprinter<'a> {
    catalog: &'a DefinitionCatalog,
}

impl Fingerprinter<'_> {
    fn node(&self, node: &metadata::CallableValueNode) -> Result<CodeFingerprint, ProjectionError> {
        let mut parts = vec![
            encoded("behavior", &node.behavior)?,
            encoded("parent", &node.parent_scope)?,
            encoded("generic-bindings", &node.generic_bindings)?,
            encoded("parameters", &node.param_names)?,
            encoded("parameter-bindings", &node.parameter_bindings)?,
            encoded("returns", &node.returned_sources)?,
        ];
        let mut generics = Vec::new();
        for generic in &node.generic_params {
            let mut bounds = Vec::new();
            for bound in &generic.bounds {
                let mut args = vec![self.definition(&bound.spec)?];
                for arg in &bound.args {
                    args.push(self.ty(arg)?);
                }
                bounds.push(combine("etas.code.latent-spec-bound.v1", args));
            }
            bounds.sort_unstable();
            generics.push(combine(
                "etas.code.latent-generic.v1",
                [
                    encoded("parameter", &(&generic.name, generic.kind))?,
                    combine("etas.code.latent-bounds.v1", bounds),
                ],
            ));
        }
        parts.push(combine("etas.code.latent-generics.v1", generics));
        parts.push(combine(
            "etas.code.latent-inputs.v1",
            node.params
                .iter()
                .map(|ty| self.ty(ty))
                .collect::<Result<Vec<_>, _>>()?,
        ));
        parts.push(self.ty(&node.output)?);
        for row in [
            &node.public_effects,
            &node.requested_actions,
            &node.handled_requested_actions,
            &node.default_handler_effects.direct_effects,
            &node.default_handler_effects.default_actions,
        ] {
            parts.push(self.row(row)?);
        }
        let mut handlers = Vec::new();
        for handler in &node.default_handler_effects.handlers {
            handlers.push(combine(
                "etas.code.latent-default-handler.v1",
                [self.effect(&handler.action)?, self.row(&handler.produced)?],
            ));
        }
        handlers.sort_unstable();
        parts.push(combine("etas.code.latent-default-handlers.v1", handlers));
        parts.push(self.trace(&node.action_trace)?);
        parts.push(match &node.handler_requests {
            Some(template) => combine(
                "etas.code.latent-request-instances.v1",
                self.requests(template)?,
            ),
            None => combine("etas.code.unavailable-request-instances.v1", []),
        });
        Ok(combine("etas.code.latent-node.v1", parts))
    }

    fn requests(
        &self,
        template: &metadata::RequestInstantiationTemplate,
    ) -> Result<std::collections::BTreeSet<CodeFingerprint>, ProjectionError> {
        let mut instances = std::collections::BTreeSet::new();
        for request in &template.actions {
            let mut parts = vec![self.effect(&request.action)?];
            parts.push(match &request.type_bindings {
                Some(bindings) => {
                    let mut bindings = bindings
                        .iter()
                        .map(|binding| {
                            Ok((
                                binding.parameter.clone(),
                                combine(
                                    "etas.code.request-binding.v1",
                                    [
                                        encoded("parameter", &binding.parameter)?,
                                        self.ty(&binding.ty)?,
                                    ],
                                ),
                            ))
                        })
                        .collect::<Result<Vec<_>, ProjectionError>>()?;
                    bindings.sort_unstable_by(|a, b| a.0.cmp(&b.0));
                    combine(
                        "etas.code.request-bindings.v1",
                        bindings.into_iter().map(|(_, value)| value),
                    )
                }
                None => combine("etas.code.unavailable-request-bindings.v1", []),
            });
            instances.insert(combine("etas.code.request-instance.v1", parts));
        }
        for parameter in &template.parameters {
            let mut excluded = parameter
                .excluded
                .iter()
                .map(|action| self.effect(action))
                .collect::<Result<Vec<_>, _>>()?;
            excluded.sort_unstable();
            instances.insert(combine(
                "etas.code.parameter-request.v1",
                [
                    encoded("parameter", &parameter.parameter)?,
                    combine("etas.code.request-mask.v1", excluded),
                ],
            ));
        }
        Ok(instances)
    }

    fn definition(&self, path: &[String]) -> Result<CodeFingerprint, ProjectionError> {
        self.catalog
            .identities
            .path(path)
            .map(|identity| identity.fingerprint())
            .ok_or_else(|| ProjectionError::MissingIdentity(path.join(".")))
    }

    fn ty(&self, ty: &metadata::Type) -> Result<CodeFingerprint, ProjectionError> {
        let mut parts = vec![encoded("kind", &ty.kind)?];
        if matches!(
            ty.kind,
            metadata::TypeKind::Named
                | metadata::TypeKind::Applied
                | metadata::TypeKind::Alias
                | metadata::TypeKind::Nominal
        ) {
            parts.push(self.definition(&ty.path)?);
        } else {
            parts.push(encoded("name", &ty.name)?);
        }
        parts.push(combine(
            "etas.code.latent-type-children.v1",
            ty.children
                .iter()
                .map(|ty| self.ty(ty))
                .collect::<Result<Vec<_>, _>>()?,
        ));
        let mut fields = Vec::new();
        for field in &ty.fields {
            fields.push((
                &field.name,
                combine(
                    "etas.code.latent-type-field.v1",
                    [encoded("field", &field.name)?, self.ty(&field.ty)?],
                ),
            ));
        }
        fields.sort_unstable_by_key(|(name, _)| *name);
        parts.push(combine(
            "etas.code.latent-type-fields.v1",
            fields.into_iter().map(|(_, field)| field),
        ));
        for row in [&ty.effects, &ty.produced_effects] {
            parts.push(match row {
                Some(row) => combine("etas.code.latent-present-row.v1", [self.row(row)?]),
                None => combine("etas.code.latent-absent-row.v1", []),
            });
        }
        Ok(combine("etas.code.latent-type.v1", parts))
    }

    fn row(&self, row: &metadata::EffectRow) -> Result<CodeFingerprint, ProjectionError> {
        let mut parts = row
            .effects
            .iter()
            .map(|effect| self.effect(effect))
            .collect::<Result<Vec<_>, _>>()?;
        parts.sort_unstable();
        parts.push(encoded("tail", &row.tail)?);
        Ok(combine("etas.code.latent-row.v1", parts))
    }

    fn effect(&self, effect: &metadata::EffectRef) -> Result<CodeFingerprint, ProjectionError> {
        let target = self.catalog.identities.effect_target(
            &self.catalog.checked.as_ref().into(),
            &effect.path.join("."),
        )?;
        let mut parts = vec![encoded("target", &target)?];
        for arg in &effect.args {
            let mut fields = vec![encoded("kind", &arg.kind)?];
            match arg.kind {
                metadata::EffectArgKind::Type => {
                    fields.push(self.ty(arg.ty.as_ref().ok_or_else(|| {
                        ProjectionError::Invalid("latent type selector has no type".into())
                    })?)?)
                }
                metadata::EffectArgKind::Path => {
                    fields.push(match self.catalog.identities.callable_path(&arg.path)? {
                        Some(callable) => combine(
                            "etas.code.latent-callable-selector.v1",
                            [callable.fingerprint()],
                        ),
                        None => combine(
                            "etas.code.latent-resource-selector.v1",
                            [self.definition(&arg.path)?],
                        ),
                    });
                }
                metadata::EffectArgKind::String | metadata::EffectArgKind::Int => {
                    fields.push(encoded("literal", &arg.value)?);
                }
                metadata::EffectArgKind::Wildcard => {}
            }
            parts.push(combine("etas.code.latent-selector.v1", fields));
        }
        Ok(combine("etas.code.latent-effect.v1", parts))
    }

    fn trace(&self, trace: &metadata::ActionTrace) -> Result<CodeFingerprint, ProjectionError> {
        use metadata::ActionTrace as T;
        let (tag, parts) = match trace {
            T::Empty => ("empty", Vec::new()),
            T::Event(event) => (
                "event",
                vec![
                    encoded("source", &event.source)?,
                    self.effect(&event.action)?,
                ],
            ),
            T::ParameterCall { parameter } => ("parameter", vec![encoded("parameter", parameter)?]),
            T::Seq(parts) | T::Choice(parts) => {
                let mut parts = parts
                    .iter()
                    .map(|part| self.trace(part))
                    .collect::<Result<Vec<_>, _>>()?;
                let choice = matches!(trace, T::Choice(_));
                if choice {
                    parts.sort_unstable();
                }
                (if choice { "choice" } else { "sequence" }, parts)
            }
            T::Repeat(part) => ("repeat", vec![self.trace(part)?]),
            T::UnknownOrder(actions) | T::Widened { actions, .. } => {
                let mut parts = actions
                    .iter()
                    .map(|action| self.effect(action))
                    .collect::<Result<Vec<_>, _>>()?;
                parts.sort_unstable();
                let tag = if let T::Widened {
                    parameter_calls, ..
                } = trace
                {
                    parts.push(encoded("parameters", parameter_calls)?);
                    "widened"
                } else {
                    "unknown-order"
                };
                (tag, parts)
            }
        };
        Ok(combine(
            "etas.code.latent-trace.v1",
            std::iter::once(encoded("trace", &tag)?).chain(parts),
        ))
    }
}

fn encoded(
    field: &str,
    value: &(impl serde::Serialize + ?Sized),
) -> Result<CodeFingerprint, ProjectionError> {
    let bytes = bincode::serde::encode_to_vec(value, bincode::config::standard())
        .map_err(|error| ProjectionError::Invalid(error.to_string()))?;
    Ok(CodeFingerprint::of_parts(
        "etas.code.latent-field.v1",
        [field.as_bytes(), bytes.as_slice()],
    ))
}

fn combine(
    domain: &'static str,
    parts: impl IntoIterator<Item = CodeFingerprint>,
) -> CodeFingerprint {
    let parts = parts.into_iter().collect::<Vec<_>>();
    CodeFingerprint::of_parts(domain, parts.iter().map(|part| part.as_bytes().as_slice()))
}
