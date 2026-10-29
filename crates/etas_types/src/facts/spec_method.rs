use crate::{
    CallableGenericParam, CallableSignature, GenericInstantiationFact, SpecMethodFact,
    SymbolTypeFact, TypeFacts,
};

impl SpecMethodFact {
    pub fn signature_in<'a>(&'a self, facts: &'a TypeFacts) -> Option<&'a CallableSignature> {
        match (&self.signature, self.source_symbol()) {
            (Some(signature), _) => Some(signature),
            (None, Some(symbol)) => match facts.symbol_types.get(&symbol) {
                Some(SymbolTypeFact::Flow { signature }) => Some(signature),
                _ => None,
            },
            (None, None) => None,
        }
    }

    pub fn implementation_generic_parameters<'a>(
        &self,
        declaration: &'a CallableSignature,
        implementation: &'a CallableSignature,
    ) -> Result<Vec<(&'a CallableGenericParam, &'a CallableGenericParam)>, String> {
        if let Some(index) = self.receiver_param {
            let receiver = declaration
                .generic_params
                .get(index as usize)
                .ok_or("spec receiver generic parameter is out of bounds")?;
            if receiver.kind != crate::CallableGenericParamKind::Type
                || declaration.params.first() != Some(&receiver.subject)
            {
                return Err("spec receiver does not bind the first input".into());
            }
        }
        let parameters = declaration
            .generic_params
            .iter()
            .enumerate()
            .filter(|(index, _)| Some(*index as u32) != self.receiver_param)
            .map(|(_, parameter)| parameter)
            .collect::<Vec<_>>();
        if parameters.len() != implementation.generic_params.len() {
            return Err("spec implementation generic parameter arity is inconsistent".into());
        }
        parameters
            .into_iter()
            .zip(&implementation.generic_params)
            .map(|(declared, actual)| {
                if declared.kind != actual.kind {
                    return Err("spec implementation generic parameter kind is inconsistent".into());
                }
                Ok((declared, actual))
            })
            .collect()
    }

    pub fn implementation_instantiation(
        &self,
        declaration: &CallableSignature,
        implementation: &CallableSignature,
        arguments: &GenericInstantiationFact,
    ) -> Result<GenericInstantiationFact, String> {
        let mut result = GenericInstantiationFact {
            type_bindings: Vec::new(),
            effect_row_bindings: Vec::new(),
            deferred_effect_row_obligations: Vec::new(),
        };
        for (declared, actual) in
            self.implementation_generic_parameters(declaration, implementation)?
        {
            match declared.kind {
                crate::CallableGenericParamKind::Type => {
                    let matches = arguments
                        .type_bindings
                        .iter()
                        .filter(|(name, _)| name == &declared.name)
                        .collect::<Vec<_>>();
                    let [(_, ty)] = matches.as_slice() else {
                        return Err(
                            "spec method requires one checked type argument per parameter".into(),
                        );
                    };
                    result.type_bindings.push((actual.name.clone(), *ty));
                }
                crate::CallableGenericParamKind::Effect => {
                    let rows = arguments
                        .effect_row_bindings
                        .iter()
                        .filter(|(name, _)| name == &declared.name)
                        .collect::<Vec<_>>();
                    if rows.len() > 1 {
                        return Err("spec method has duplicate checked effect arguments".into());
                    }
                    if let Some((_, row)) = rows.first() {
                        result
                            .effect_row_bindings
                            .push((actual.name.clone(), row.clone()));
                    }
                    let start = result.deferred_effect_row_obligations.len();
                    for obligation in arguments
                        .deferred_effect_row_obligations
                        .iter()
                        .filter(|obligation| obligation.param == declared.name)
                    {
                        let mut obligation = obligation.clone();
                        obligation.param = actual.name.clone();
                        result.deferred_effect_row_obligations.push(obligation);
                    }
                    if rows.is_empty() && start == result.deferred_effect_row_obligations.len() {
                        return Err("spec method is missing its checked effect argument".into());
                    }
                }
            }
        }
        if result.type_bindings.len() != arguments.type_bindings.len()
            || result.effect_row_bindings.len() != arguments.effect_row_bindings.len()
            || result.deferred_effect_row_obligations.len()
                != arguments.deferred_effect_row_obligations.len()
        {
            return Err("spec method arguments do not match its declaration".into());
        }
        Ok(result)
    }
}
