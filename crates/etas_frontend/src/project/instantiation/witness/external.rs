use super::*;
use etas_types::{CallableGenericParamKind, CheckedSpecRef, GenericInstantiationFact};

impl WitnessResolver<'_> {
    pub(in crate::project::instantiation) fn bound_bodies(
        &self,
        signature: &etas_types::CallableSignature,
        fact: &GenericInstantiationFact,
        substitution: &mut TypeSubstitutionEngine<'_>,
    ) -> Result<PackBodies, InstantiationError> {
        let names = fact
            .type_bindings
            .iter()
            .map(|(name, ty)| Ok((name.clone(), substitution.substitute(*ty)?)))
            .collect::<Result<HashMap<_, _>, etas_types::TypeSubstitutionError>>()?;
        let parameters = signature
            .generic_params
            .iter()
            .filter_map(|parameter| {
                names
                    .get(&parameter.name)
                    .map(|ty| (parameter.subject, *ty))
            })
            .collect();
        let mut bindings = TypeSubstitutionEngine::existing(substitution.input_store(), &names)
            .with_type_parameters(&parameters);
        let mut result = PackBodies {
            bodies: BTreeSet::new(),
            applications: Vec::new(),
        };
        // A metadata callable receives the selected witness dictionary, including
        // methods captured by returned flows. This does not select other impls or
        // grant invocation authority to the dictionary's members.
        for parameter in &signature.generic_params {
            if parameter.kind != CallableGenericParamKind::Type {
                continue;
            }
            let receiver = bindings.substitute(parameter.subject)?;
            for bound in &parameter.bounds {
                let methods = match &bound.spec {
                    CheckedSpecRef::Source(symbol) => {
                        if let Some(signature) = self.types.facts.spec_signatures.get(symbol) {
                            &signature.methods
                        } else if let Some(identity) =
                            self.types.facts.external_specs.aliases.get(symbol)
                            && let Some(signature) =
                                self.types.facts.external_specs.signatures.get(identity)
                        {
                            &signature.methods
                        } else {
                            return Err(
                                "external callable bound has no checked spec declaration".into()
                            );
                        }
                    }
                    CheckedSpecRef::External(identity) => {
                        let signature = self
                            .types
                            .facts
                            .external_specs
                            .signatures
                            .get(identity)
                            .ok_or("external callable bound has no checked spec declaration")?;
                        &signature.methods
                    }
                    CheckedSpecRef::Std(_) | CheckedSpecRef::Canonical(_) => continue,
                };
                let args = bound
                    .args
                    .iter()
                    .map(|ty| bindings.substitute(*ty))
                    .collect::<Result<Vec<_>, _>>()?;
                for member in methods {
                    let Some(witness) = self.resolve_in_store(
                        bindings.input_store(),
                        receiver,
                        &args,
                        &member.identity,
                    )?
                    else {
                        continue;
                    };
                    let (MethodTarget::Source { callable, .. }
                    | MethodTarget::Agent { callable, .. }) = witness.declaration.target
                    else {
                        continue;
                    };
                    result.bodies.insert(callable);
                    if witness.declaration.signature.generic_params.is_empty() {
                        let mut owner_bindings =
                            witness.owner_bindings.into_iter().collect::<Vec<_>>();
                        owner_bindings.sort_unstable();
                        result.applications.push(NativeInstantiation {
                            owner: callable,
                            arguments: Vec::new(),
                            owner_bindings,
                        });
                    }
                }
            }
        }
        Ok(result)
    }
}
