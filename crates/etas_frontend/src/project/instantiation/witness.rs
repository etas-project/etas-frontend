use std::collections::{BTreeSet, HashMap};

use etas_hir::HirItemId;
use etas_types::{
    SpecMethodFact, SpecMethodIdentity, Type, TypeOutput, TypeSubstitutionEngine,
    facts::CheckedMethodCallFact,
    method::{MethodEvidence, MethodTarget, MethodWitness},
};

use super::{InstantiationError, NativeInstantiation};

mod external;

pub(super) struct WitnessResolver<'a> {
    types: &'a TypeOutput,
    specs: etas_types::SpecFacts,
    members: HashMap<SpecMethodIdentity, &'a SpecMethodFact>,
}

pub(super) struct PackBodies {
    pub bodies: BTreeSet<HirItemId>,
    pub applications: Vec<NativeInstantiation>,
}

pub(super) struct MethodApplication {
    pub body: HirItemId,
    pub instance: Option<NativeInstantiation>,
}

impl<'a> WitnessResolver<'a> {
    pub fn new(types: &'a TypeOutput) -> Self {
        Self {
            types,
            specs: etas_types::SpecFacts::from(types.facts.as_ref()),
            members: types
                .facts
                .spec_signatures
                .values()
                .flat_map(|signature| &signature.methods)
                .chain(
                    types
                        .facts
                        .external_specs
                        .signatures
                        .values()
                        .flat_map(|signature| &signature.methods),
                )
                .map(|member| (member.identity.clone(), member))
                .collect(),
        }
    }

    fn resolve(
        &self,
        receiver: etas_types::TypeId,
        args: &[etas_types::TypeId],
        member: &SpecMethodIdentity,
        substitution: &mut TypeSubstitutionEngine<'_>,
    ) -> Result<Option<MethodWitness<'_>>, InstantiationError> {
        let receiver = substitution.substitute(receiver)?;
        let args = args
            .iter()
            .map(|ty| substitution.substitute(*ty))
            .collect::<Result<Vec<_>, _>>()?;
        let store = substitution.input_store();
        self.resolve_in_store(store, receiver, &args, member)
    }

    fn resolve_in_store(
        &self,
        store: &etas_types::TypeStore,
        receiver: etas_types::TypeId,
        args: &[etas_types::TypeId],
        member: &SpecMethodIdentity,
    ) -> Result<Option<MethodWitness<'_>>, InstantiationError> {
        let witness =
            self.types
                .facts
                .methods
                .resolve_witness(store, &self.specs, receiver, args, member)?;
        let symbolic = matches!(store.get(receiver), Some(Type::Existential { .. }))
            || matches!(store.get(receiver), Some(Type::Named(named)) if named.source_parameter.is_some())
            || store.parameter_identity(receiver).is_some();
        if witness.is_none() && !symbolic {
            return Err("native invocation has no checked implementation witness".into());
        }
        Ok(witness)
    }

    pub fn method_application(
        &self,
        call: &CheckedMethodCallFact,
        substitution: &mut TypeSubstitutionEngine<'_>,
    ) -> Result<Option<MethodApplication>, InstantiationError> {
        let (MethodTarget::BoundSpec { member, .. }, MethodEvidence::Bound { args, .. }) =
            (&call.target, &call.evidence)
        else {
            return Err("bound method invocation has no checked witness recipe".into());
        };
        let Some(witness) = self.resolve(call.receiver_type, args, member, substitution)? else {
            return Ok(None);
        };
        let (MethodTarget::Source { callable, .. } | MethodTarget::Agent { callable, .. }) =
            witness.declaration.target
        else {
            return Ok(None);
        };
        let target = super::signature(&self.types.facts, callable)
            .ok_or("native witness has no callable signature")?;
        let mapped = self.instantiation(member, target, &call.instantiation)?;
        if !mapped.deferred_effect_row_obligations.is_empty() {
            return Ok(Some(MethodApplication {
                body: callable,
                instance: None,
            }));
        }
        let mut owner_bindings = witness.owner_bindings.into_iter().collect::<Vec<_>>();
        owner_bindings.sort_unstable();
        Ok(Some(MethodApplication {
            body: callable,
            instance: Some(NativeInstantiation {
                owner: callable,
                arguments: super::prepare::bind_call(substitution, target, &mapped)?,
                owner_bindings,
            }),
        }))
    }

    pub fn instantiation(
        &self,
        member: &SpecMethodIdentity,
        target: &etas_types::CallableSignature,
        arguments: &etas_types::GenericInstantiationFact,
    ) -> Result<etas_types::GenericInstantiationFact, InstantiationError> {
        let required = self
            .members
            .get(member)
            .ok_or("native witness has no checked member declaration")?;
        let declaration = required
            .signature_in(&self.types.facts)
            .ok_or("native witness has no member signature")?;
        required
            .implementation_instantiation(declaration, target, arguments)
            .map_err(InstantiationError::Invalid)
    }

    pub fn pack_bodies(
        &self,
        pack: &etas_types::facts::existentials::CheckedExistentialPackFact,
        substitution: &mut TypeSubstitutionEngine<'_>,
    ) -> Result<PackBodies, InstantiationError> {
        let mut result = PackBodies {
            bodies: BTreeSet::new(),
            applications: Vec::new(),
        };
        for witness in &pack.witnesses {
            let resolved = self.resolve(
                pack.source_type,
                &witness.bound.args,
                &witness.member,
                substitution,
            )?;
            let Some(resolved) = resolved else { continue };
            let (MethodTarget::Source { callable, .. } | MethodTarget::Agent { callable, .. }) =
                resolved.declaration.target
            else {
                continue;
            };
            result.bodies.insert(callable);
            if resolved.declaration.signature.generic_params.is_empty() {
                let mut owner_bindings = resolved.owner_bindings.into_iter().collect::<Vec<_>>();
                owner_bindings.sort_unstable();
                result.applications.push(NativeInstantiation {
                    owner: callable,
                    arguments: Vec::new(),
                    owner_bindings,
                });
            }
        }
        Ok(result)
    }
}
