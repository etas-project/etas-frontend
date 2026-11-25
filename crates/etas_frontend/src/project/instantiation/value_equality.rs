use std::collections::HashMap;

use etas_types::{TypeFacts, TypeId, TypeStore, TypeSubstitutionEngine, TypeSubstitutionError};

use super::{ForeignMethodInstantiation, InstantiationError, NativeInstantiation};

pub(super) enum ValueEqualityApplication {
    Native(NativeInstantiation),
    Foreign(ForeignMethodInstantiation),
}

pub(super) struct ValueEqualityApplications<'a> {
    methods: Vec<&'a etas_types::method::MethodSignatureFact>,
}

impl<'a> ValueEqualityApplications<'a> {
    pub fn new(facts: &'a TypeFacts) -> Self {
        Self {
            methods: facts
                .value_equality_methods()
                .map(|(method, _)| method)
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.methods.is_empty()
    }

    pub fn collect(
        &self,
        store: &TypeStore,
        types: impl IntoIterator<Item = TypeId>,
    ) -> Result<Vec<ValueEqualityApplication>, InstantiationError> {
        let names = HashMap::new();
        let validation = TypeSubstitutionEngine::existing(store, &names);
        let mut applications = Vec::new();
        for receiver in types {
            for method in &self.methods {
                let Some(bindings) = etas_types::solver::spec_solver::match_implementation_subject(
                    store,
                    &method.owner_params,
                    method.receiver_type,
                    receiver,
                ) else {
                    continue;
                };
                match validation.validate_closed_argument(receiver) {
                    // A function reference can expose its generic template;
                    // only its instantiated layouts require execution proofs.
                    Err(
                        TypeSubstitutionError::UnresolvedType(_)
                        | TypeSubstitutionError::OpenEffectRow(_),
                    ) => continue,
                    result => result?,
                }
                let owner_bindings = method
                    .owner_params
                    .iter()
                    .map(|parameter| {
                        bindings
                            .get(&parameter.subject)
                            .copied()
                            .map(|argument| (parameter.subject, argument))
                            .ok_or("value equality has an unbound implementation parameter")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let application = match &method.target {
                    etas_types::method::MethodTarget::Source { callable, .. } => {
                        ValueEqualityApplication::Native(NativeInstantiation {
                            owner: *callable,
                            arguments: Vec::new(),
                            owner_bindings,
                        })
                    }
                    etas_types::method::MethodTarget::External { identity } => {
                        ValueEqualityApplication::Foreign(ForeignMethodInstantiation {
                            identity: identity.clone(),
                            arguments: Vec::new(),
                            owner_bindings,
                        })
                    }
                    _ => return Err("value equality must have a checked Etas body".into()),
                };
                applications.push(application);
            }
        }
        Ok(applications)
    }
}
