use std::collections::{BTreeSet, HashMap};

use super::super::{
    engine::EffectSemantics,
    specialize::{CheckedGenericBindings, EffectSpecializationError},
    state::LatentSource,
};
use crate::{
    ParameterCallTarget,
    facts::{CallableEnvironment, CallableEnvironmentId},
    infer::unit::EffectUnit,
};
use etas_types::{Type, TypeId, TypeSubstitutionEngine, method::MethodTarget};

fn invalid(reason: &str) -> EffectSpecializationError {
    EffectSpecializationError::InvalidCallableEnvironment {
        reason: reason.into(),
    }
}

pub(super) struct ObjectMethodInput<'a> {
    pub call: etas_hir::HirExprId,
    pub invocation: CallableEnvironmentId,
    pub snapshot: &'a CallableEnvironment,
    pub method: &'a etas_types::SpecMethodFact,
    pub declaration: &'a etas_types::CallableSignature,
    pub arguments: &'a CheckedGenericBindings,
    pub spec_args: &'a [TypeId],
}

impl EffectSemantics<'_> {
    pub(super) fn existential_method_sources(
        &self,
        input: ObjectMethodInput<'_>,
    ) -> Result<Option<BTreeSet<LatentSource>>, EffectSpecializationError> {
        let ObjectMethodInput {
            call,
            invocation,
            snapshot,
            method,
            declaration,
            arguments,
            spec_args,
        } = input;
        let mut pending = snapshot
            .parameters
            .get(&ParameterCallTarget::Interface(0))
            .ok_or_else(|| invalid("existential method receiver has no checked value origins"))?
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let mut visited = BTreeSet::new();
        let mut result = BTreeSet::new();
        while let Some(origin) = pending.pop() {
            if !visited.insert(origin) {
                continue;
            }
            let LatentSource::Existential { pack, environment } = origin else {
                if matches!(origin, LatentSource::Parameter { .. }) {
                    return Ok(None);
                }
                return Err(invalid(
                    "existential method receiver is missing checked packing evidence",
                ));
            };
            let pack_fact = self
                .types
                .facts
                .existential_packs
                .get(&pack)
                .ok_or_else(|| invalid("existential value references a missing checked pack"))?;
            let packed = self.callable_environment(environment)?;
            let owner = *self
                .context
                .view()
                .index()
                .expr_item
                .get(&pack)
                .ok_or_else(|| invalid("existential pack has no lexical owner"))?;
            let (types, rows) = self.native_environment_generics(owner, &packed.generics)?;
            let names = types.into_iter().collect::<HashMap<_, _>>();
            let rows = self.type_row_arguments(&rows, Some(packed.owner))?;
            let mut substitution = TypeSubstitutionEngine::existing(&self.types.store, &names)
                .with_effect_arguments(&rows);
            let source_type = substitution.substitute(pack_fact.source_type)?;
            let mut selected = None;
            for witness in &pack_fact.witnesses {
                if witness.member != method.identity {
                    continue;
                }
                let args = witness
                    .bound
                    .args
                    .iter()
                    .map(|arg| substitution.substitute(*arg))
                    .collect::<Result<Vec<_>, _>>()?;
                if args == spec_args && selected.replace(witness).is_some() {
                    return Err(invalid(
                        "existential value has ambiguous checked member evidence",
                    ));
                }
            }
            let witness = selected.ok_or_else(|| {
                invalid("existential value has no evidence for the selected spec member")
            })?;
            let target = if matches!(witness.target, MethodTarget::BoundSpec { .. }) {
                if matches!(
                    self.types.store.get(source_type),
                    Some(Type::Existential { .. })
                ) {
                    pending.extend(
                        packed
                            .parameters
                            .get(&ParameterCallTarget::Interface(0))
                            .ok_or_else(|| {
                                invalid("repacked existential lost its producer evidence")
                            })?,
                    );
                    continue;
                }
                if matches!(self.types.store.get(source_type), Some(Type::Named(named)) if named.source_parameter.is_some())
                {
                    return Ok(None);
                }
                &self
                    .types
                    .facts
                    .methods
                    .witness(source_type, spec_args, &method.identity)
                    .ok_or_else(|| {
                        invalid("existential generic source has no checked member witness")
                    })?
                    .target
            } else {
                &witness.target
            };
            let mut actual = snapshot.clone();
            actual.parameters.insert(
                ParameterCallTarget::Interface(0),
                packed
                    .parameters
                    .get(&ParameterCallTarget::Interface(0))
                    .cloned()
                    .ok_or_else(|| invalid("existential producer capture is missing"))?,
            );
            let arguments = etas_types::GenericInstantiationFact {
                type_bindings: arguments.types.clone(),
                effect_row_bindings: arguments.effect_rows.clone(),
                deferred_effect_row_obligations: Vec::new(),
            };
            let source = match target {
                MethodTarget::Source { callable, .. } => {
                    self.discovered_spec_targets
                        .borrow_mut()
                        .entry(call)
                        .or_default()
                        .insert(EffectUnit::Item(*callable));
                    self.capture_spec_implementation(
                        *callable,
                        invocation,
                        actual,
                        declaration,
                        method,
                        arguments,
                    )?
                }
                MethodTarget::External { .. } => {
                    let implementation = self
                        .types
                        .facts
                        .methods
                        .witness(source_type, spec_args, &method.identity)
                        .ok_or_else(|| {
                            invalid("existential external member declaration is missing")
                        })?;
                    self.capture_external_spec_implementation(
                        implementation,
                        invocation,
                        actual,
                        declaration,
                        method,
                        arguments,
                    )?
                }
                _ => {
                    return Err(invalid(
                        "existential method has no executable checked target",
                    ));
                }
            };
            result.insert(source);
        }
        if result.is_empty() {
            return Err(invalid(
                "existential method has no checked implementation alternatives",
            ));
        }
        Ok(Some(result))
    }
}
