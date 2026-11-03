use super::*;
use etas_effects::facts::{CallableEnvironmentId, MetadataSpecMethodDefinition};

impl Relocation<'_> {
    pub(super) fn metadata_method_environment(
        &self,
        invocation: CodeFingerprint,
        environment: CallableEnvironmentId,
    ) -> Result<(CodeFingerprint, CallableEnvironmentId), ProjectionError> {
        let invocation = self.metadata_method(invocation, environment)?;
        let id = CallableEnvironmentId {
            origin: CodeFingerprint::of_parts(
                "etas.effects.frozen-method-environment.v1",
                [
                    invocation.as_bytes().as_slice(),
                    environment.origin.as_bytes().as_slice(),
                ],
            ),
            application: environment.application,
        };
        Ok((invocation, id))
    }

    pub(super) fn metadata_method_formals(
        &self,
        invocation: CodeFingerprint,
        owner: HirItemId,
    ) -> Result<
        BTreeMap<
            etas_effects::facts::CallableGenericParameter,
            etas_effects::facts::CallableGenericValue,
        >,
        ProjectionError,
    > {
        use etas_effects::facts::{CallableGenericParameter as P, CallableGenericValue as V};
        let signature = super::super::super::collect::callable_signature(self.target, owner)
            .ok_or_else(|| invalid("frozen method binding has no checked generic scope"))?;
        let definitions = self.method_evidence.borrow();
        let definition = definitions
            .get(&invocation)
            .ok_or_else(|| invalid("frozen method binding has no relocated descriptor"))?;
        if definition.generics
            != signature
                .generic_params
                .iter()
                .map(|parameter| (parameter.name.clone(), parameter.kind))
                .collect::<Vec<_>>()
        {
            return Err(invalid(
                "frozen method binding has an inconsistent generic interface",
            ));
        }
        Ok(signature
            .generic_params
            .iter()
            .enumerate()
            .map(|(position, parameter)| {
                let value = match parameter.kind {
                    etas_types::CallableGenericParamKind::Type => V::Type(parameter.subject),
                    etas_types::CallableGenericParamKind::Effect => V::EffectRow(EffectRow {
                        effects: EffectSet::new(),
                        open: Some(etas_effects::effect_var_id_from_name(&parameter.name)),
                    }),
                };
                (
                    P::Graph {
                        graph: invocation,
                        scope: etas_package_metadata::CallableValueScope::Root,
                        position: position as u32,
                    },
                    value,
                )
            })
            .collect())
    }

    pub(super) fn metadata_method(
        &self,
        invocation: CodeFingerprint,
        environment: CallableEnvironmentId,
    ) -> Result<CodeFingerprint, ProjectionError> {
        if let Some(key) = self.method_targets.borrow().get(&(invocation, environment)) {
            return Ok(*key);
        }
        let snapshot = self
            .original
            .effects
            .callable_environments
            .get(&environment)
            .ok_or_else(|| invalid("frozen method has no checked capture environment"))?;
        let owner = *self
            .returned_items
            .get(&snapshot.owner)
            .ok_or_else(|| invalid("frozen method capture has no canonical target owner"))?;
        let signature = super::super::super::collect::callable_signature(self.target, owner)
            .ok_or_else(|| invalid("frozen method capture has no checked target signature"))?;
        // Project the captured binding into its lexical generic scope before
        // lowering it against the candidate's checked store. Import-local TypeIds
        // and names from the dependency's generic scope never cross this boundary.
        let method = self
            .metadata_projection
            .external_method_invocation(invocation, environment)
            .map_err(|error| invalid(&error.to_string()))?;
        let target = etas_package_metadata::CallableRequestTarget::SpecMethod(Box::new(method));
        let ExternalCallableRequestTarget::SpecMethod(method) =
            crate::project::lower_callable_request_target_in_scope(
                &target,
                self.target_output,
                &signature.generic_params,
            )
            .map_err(|error| invalid(&error))?
        else {
            return Err(invalid("frozen method projection changed its target kind"));
        };
        let parameters = unit_parameters(self.target.hir, EffectUnit::Item(owner))
            .iter()
            .map(|symbol| {
                let parameter =
                    self.target.hir.symbols.get(*symbol).ok_or_else(|| {
                        invalid("frozen method interface has no checked parameter")
                    })?;
                Ok((parameter.name.clone(), ParameterCallTarget::Native(*symbol)))
            })
            .collect::<Result<_, ProjectionError>>()?;
        let definition = MetadataSpecMethodDefinition {
            invocation: method,
            generics: signature
                .generic_params
                .iter()
                .map(|parameter| (parameter.name.clone(), parameter.kind))
                .collect(),
            parameters,
            span: self
                .target
                .hir
                .items
                .get(owner)
                .ok_or_else(|| invalid("frozen method owner has no diagnostic anchor"))?
                .span(),
        };
        let key = definition.identity().map_err(|error| invalid(&error))?;
        let mut definitions = self.method_evidence.borrow_mut();
        if let Some(previous) = definitions.get(&key) {
            if previous.as_ref() != &definition {
                return Err(invalid(
                    "frozen method evidence has conflicting checked definitions",
                ));
            }
        } else {
            if definitions.len() >= etas_package_metadata::MAX_METADATA_GRAPH_NODES {
                return Err(invalid("frozen method evidence exceeds the graph budget"));
            }
            definitions.insert(key, std::sync::Arc::new(definition));
        }
        self.method_targets
            .borrow_mut()
            .insert((invocation, environment), key);
        Ok(key)
    }

    pub(in crate::code::definition::frozen_calls) fn into_method_evidence(
        self,
    ) -> BTreeMap<CodeFingerprint, std::sync::Arc<MetadataSpecMethodDefinition>> {
        self.method_evidence.into_inner()
    }
}
