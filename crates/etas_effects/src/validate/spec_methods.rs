use std::collections::HashMap;

use super::*;

impl ContractValidator<'_, '_> {
    pub(super) fn validate_external_spec_method_effects(
        &mut self,
    ) -> Result<(), EffectPipelineError> {
        for implementation in &self.types.facts.external_specs.methods {
            let declaration = match &implementation.spec {
                etas_types::CheckedSpecRef::Source(symbol) => self
                    .types
                    .facts
                    .spec_signatures
                    .get(symbol)
                    .map(|spec| (&spec.param_names, &spec.methods)),
                etas_types::CheckedSpecRef::External(identity) => self
                    .types
                    .facts
                    .external_specs
                    .signatures
                    .get(identity)
                    .map(|spec| (&spec.param_names, &spec.methods)),
                etas_types::CheckedSpecRef::Std(_) => None,
            }
            .ok_or_else(|| invalid("external method has no checked spec declaration"))?;
            let method = declaration
                .1
                .iter()
                .find(|method| method.name == implementation.identity.name())
                .ok_or_else(|| invalid("external method has no checked member declaration"))?;
            let expected = method
                .signature_in(&self.types.facts)
                .ok_or_else(|| invalid("external spec method signature is missing"))?;
            let Some(bound) = &expected.effects else {
                continue;
            };
            let mut bindings = declaration
                .0
                .iter()
                .cloned()
                .zip(implementation.args.iter().copied())
                .collect::<HashMap<_, _>>();
            if let Some(position) = method.receiver_param {
                let receiver = expected
                    .generic_params
                    .get(position as usize)
                    .ok_or_else(|| invalid("external spec method receiver binder is missing"))?;
                bindings.insert(receiver.name.clone(), implementation.self_type);
            }
            let mut rows = HashMap::new();
            for (declared, actual) in method
                .implementation_generic_parameters(expected, &implementation.signature)
                .map_err(|_| invalid("external method generic signature disagrees with its spec"))?
            {
                match declared.kind {
                    etas_types::CallableGenericParamKind::Type => {
                        bindings.insert(declared.name.clone(), actual.subject);
                    }
                    etas_types::CallableGenericParamKind::Effect => {
                        rows.insert(
                            declared.name.clone(),
                            etas_types::EffectRowRef {
                                effects: Vec::new(),
                                tail: Some(actual.name.clone()),
                            },
                        );
                    }
                }
            }
            let bound = etas_types::TypeSubstitutionEngine::existing(&self.types.store, &bindings)
                .with_effect_arguments(&rows)
                .substitute_effect_row(bound.clone())
                .map_err(|_| invalid("external spec method bound cannot be specialized"))?;
            let actual =
                implementation.signature.effects.as_ref().ok_or_else(|| {
                    invalid("external method behavior lacks a solved escaping row")
                })?;
            let bound = resolve_checked_type_row(&self.types.store, self.registry, &bound)
                .map_err(|_| {
                    invalid("external spec method bound has no checked effect identity")
                })?;
            let actual = resolve_checked_type_row(&self.types.store, self.registry, actual)
                .map_err(|_| {
                    invalid("external method escaping row has no checked effect identity")
                })?;
            if !(crate::EffectCoverage {
                registry: self.registry,
                types: &self.types.store,
            })
            .row_covers(&declared_public_boundary(&bound), &actual)
            {
                self.effects.diagnostics.push(etas_core::Diagnostic::effect_check(
                    EffectDiagnosticCode::EffectOutsideDeclaredRow, implementation.span,
                    "external implementation escaping effects exceed the behavioral spec method bound",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn validate_spec_method_effects(&mut self) -> Result<(), EffectPipelineError> {
        for implementation in &self.types.facts.spec_impls {
            let spec = self
                .types
                .facts
                .spec_signatures
                .get(&implementation.spec_symbol)
                .ok_or_else(|| invalid("spec implementation has no checked declaration"))?;
            for method in &spec.methods {
                let Some(provided) = implementation
                    .methods
                    .iter()
                    .find(|candidate| candidate.name == method.name)
                else {
                    // Missing members are rejected by the signature pipeline.
                    continue;
                };
                let Some(etas_hir::SymbolDef::Item { item }) = self
                    .hir
                    .symbols
                    .get(provided.symbol)
                    .map(|symbol| &symbol.def)
                else {
                    return Err(invalid(
                        "spec implementation method is not a checked callable item",
                    ));
                };
                let item = *item;
                if !self.item_is_reachable(item) {
                    continue;
                }
                let declaration = method
                    .signature_in(&self.types.facts)
                    .ok_or_else(|| invalid("spec method has no checked callable signature"))?;
                let Some(bound) = &declaration.effects else {
                    continue;
                };
                let Some(etas_types::ItemSignature::Flow(actual)) =
                    self.types.facts.item_signatures.get(&item)
                else {
                    return Err(invalid("spec implementation method signature is missing"));
                };
                let span = self
                    .hir
                    .items
                    .get(item)
                    .ok_or_else(|| invalid("spec implementation item is missing"))?
                    .span();
                let result = (|| -> Result<crate::EffectRow, String> {
                    let mut types = spec
                        .param_names
                        .iter()
                        .cloned()
                        .zip(implementation.args.iter().copied())
                        .collect::<HashMap<_, _>>();
                    if let Some(index) = method.receiver_param {
                        let receiver = declaration
                            .generic_params
                            .get(index as usize)
                            .ok_or("spec receiver binder is missing")?;
                        types.insert(receiver.name.clone(), implementation.self_type);
                    }
                    let mut rows = HashMap::new();
                    for (declared, actual) in
                        method.implementation_generic_parameters(declaration, actual)?
                    {
                        match declared.kind {
                            etas_types::CallableGenericParamKind::Type => {
                                types.insert(declared.name.clone(), actual.subject);
                            }
                            etas_types::CallableGenericParamKind::Effect => {
                                rows.insert(
                                    declared.name.clone(),
                                    etas_types::EffectRowRef {
                                        effects: Vec::new(),
                                        tail: Some(actual.name.clone()),
                                    },
                                );
                            }
                        }
                    }
                    let bound =
                        etas_types::TypeSubstitutionEngine::existing(&self.types.store, &types)
                            .with_effect_arguments(&rows)
                            .substitute_effect_row(bound.clone())
                            .map_err(|error| error.to_string())?;
                    resolve_checked_type_row(&self.types.store, self.registry, &bound)
                })();
                let bound = match result {
                    Ok(bound) => bound,
                    Err(reason) => {
                        self.reject_item(
                            item,
                            span,
                            EffectDiagnosticCode::IncompleteEffectFacts,
                            format!("spec method effect bound is inconsistent: {reason}"),
                        );
                        continue;
                    }
                };
                let Some(summary) = self.effects.facts.item_effects.get(&item) else {
                    self.reject_item(
                        item,
                        span,
                        EffectDiagnosticCode::IncompleteEffectFacts,
                        "spec method effect validation requires a solved body summary",
                    );
                    continue;
                };
                let coverage = crate::EffectCoverage {
                    registry: self.registry,
                    types: &self.types.store,
                };
                if !coverage
                    .row_covers(&declared_public_boundary(&bound), &summary.escaping_effects)
                {
                    self.reject_item(
                        item,
                        span,
                        EffectDiagnosticCode::EffectOutsideDeclaredRow,
                        "implementation escaping effects exceed the behavioral spec method bound",
                    );
                }
            }
        }
        Ok(())
    }
}

fn invalid(reason: &str) -> EffectPipelineError {
    EffectPipelineError::InvalidCallableFacts {
        reason: reason.into(),
    }
}
