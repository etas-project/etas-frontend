use super::*;
use etas_types::method::{MethodReceiver, MethodTarget};

impl ProjectMetadataProjection<'_> {
    pub(super) fn methods(
        &self,
        identities: Option<&crate::code::identity::DefinitionIdentityIndex>,
    ) -> Result<Vec<etas_package_metadata::PublishedMethod>, PackageMetadataError> {
        let declarations = self
            .checked
            .types
            .methods
            .entries()
            .filter(|method| {
                method.public
                    && matches!(
                        method.target,
                        MethodTarget::Source { .. } | MethodTarget::Agent { .. }
                    )
            })
            .collect::<Vec<_>>();
        if declarations.is_empty() {
            return Ok(Vec::new());
        }
        let mut methods = BTreeMap::new();
        let selection = self.exported_definition_items()?;
        let head = |ty| match self.checked.type_store.get(ty) {
            Some(Type::Applied { constructor, .. }) => TypeId(constructor.0),
            _ => ty,
        };
        let subjects = selection
            .iter()
            .filter_map(|item| {
                let symbol = self.checked.items.get(*item)?.symbol()?;
                match self.checked.types.symbol_types.get(&symbol)? {
                    etas_types::SymbolTypeFact::Type { constructor }
                    | etas_types::SymbolTypeFact::NominalType { constructor, .. } => {
                        Some(TypeId(constructor.0))
                    }
                    etas_types::SymbolTypeFact::TypeAlias { target, .. } => Some(head(*target)),
                    etas_types::SymbolTypeFact::Agent { signature } => {
                        Some(head(signature.component))
                    }
                    _ => None,
                }
            })
            .collect::<BTreeSet<_>>();
        for method in declarations
            .into_iter()
            .filter(|method| subjects.contains(&head(method.receiver_type)))
        {
            let (symbol, callable) = match method.target {
                MethodTarget::Source {
                    symbol, callable, ..
                }
                | MethodTarget::Agent { symbol, callable } => (symbol, callable),
                _ => continue,
            };
            let identities = identities
                .ok_or_else(|| invalid("public method requires canonical package origins"))?;
            if !crate::code::definition::selected_callable(
                self.checked,
                identities,
                symbol,
                callable,
                Some(&selection),
            )
            .map_err(|error| invalid(&error.to_string()))?
            {
                continue;
            }
            let identity = identities
                .method(symbol)
                .cloned()
                .ok_or_else(|| invalid("public method requires a canonical method identity"))?;
            let names = method
                .owner_params
                .iter()
                .filter(|param| param.kind == etas_types::CallableGenericParamKind::Type)
                .map(|param| param.name.clone())
                .collect();
            let published = etas_package_metadata::PublishedMethod {
                identity: identity.clone(),
                self_type: self.metadata_type_in_generic_scope(method.receiver_type, &names)?,
                receiver: match method.receiver {
                    MethodReceiver::Value { slot } => {
                        etas_package_metadata::MethodReceiver::Value { slot }
                    }
                    MethodReceiver::Associated => etas_package_metadata::MethodReceiver::Associated,
                    MethodReceiver::Component => etas_package_metadata::MethodReceiver::Component,
                },
                owner_params: self.callable_generic_params_metadata(&method.owner_params)?,
                requirements: self.method_requirements(callable)?,
                callable: self.implementation_callable_graph(callable, Some(identities))?,
            };
            published.validate()?;
            if let Some(previous) = methods.insert(identity, published.clone())
                && previous != published
            {
                return Err(invalid(
                    "one method identity has conflicting checked declarations",
                ));
            }
        }
        Ok(methods.into_values().collect())
    }

    fn method_requirements(
        &self,
        item: HirItemId,
    ) -> Result<etas_package_metadata::MethodRequirements, PackageMetadataError> {
        use etas_effects::{LimitKind as K, LimitValue as V, RequirementFact};
        use etas_package_metadata::{MethodLimitKind as M, MethodLimitValue as W};
        let summary = self
            .checked
            .effects
            .unit_effects
            .get(&etas_effects::EffectUnit::Item(item))
            .ok_or_else(|| invalid("published method has no solved behavior requirements"))?;
        let mut requirements = etas_package_metadata::MethodRequirements::default();
        for requirement in summary.requirements.iter() {
            match requirement {
                RequirementFact::Limit(limit) => {
                    requirements
                        .limits
                        .push(etas_package_metadata::MethodLimitRequirement {
                            kind: match limit.kind {
                                K::Iterations => M::Iterations,
                                K::Tokens => M::Tokens,
                                K::ContextTokens => M::ContextTokens,
                                K::Cost => M::Cost,
                                K::WallTime => M::WallTime,
                                K::Attempts => M::Attempts,
                            },
                            value: limit.value.as_ref().map(|value| match value {
                                V::Count(count) => W::Count(*count),
                                V::DurationMillis(ms) => W::DurationMillis(*ms),
                                V::MoneyMicros { amount, currency } => W::MoneyMicros {
                                    amount: *amount,
                                    currency: currency.clone(),
                                },
                            }),
                        })
                }
                RequirementFact::TraceSpec(clause) => requirements
                    .trace_requirements
                    .push(self.method_trace_requirement(clause)?),
            }
        }
        for requirement in summary.trace_spec_obligations.iter() {
            let RequirementFact::TraceSpec(clause) = requirement else {
                return Err(invalid(
                    "trace obligation unexpectedly contains an execution limit",
                ));
            };
            requirements
                .trace_obligations
                .push(self.method_trace_requirement(clause)?);
        }
        Ok(requirements)
    }

    fn method_trace_requirement(
        &self,
        clause: &TraceSpecClauseFact,
    ) -> Result<etas_package_metadata::MethodTraceRequirement, PackageMetadataError> {
        use etas_package_metadata::MethodTraceRequirement as R;
        match clause {
            TraceSpecClauseFact::PendingTypedTraceSpecModel => {
                Err(invalid("published method still has pending trace facts"))
            }
            TraceSpecClauseFact::TraceSpecReference { name } => Ok(R::Reference(name.clone())),
            TraceSpecClauseFact::Limit { count } => Ok(R::Limit(*count as u64)),
            _ => self
                .trace_spec_clause_metadata(clause)?
                .map(R::Clause)
                .ok_or_else(|| invalid("published method trace requirement is not representable")),
        }
    }
}
