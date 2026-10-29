use etas_types::{Type, TypeId, facts::reflection::CheckedReflectionTarget};

use super::{engine::EffectSemantics, shared::*};

impl EffectSemantics<'_> {
    pub(super) fn symbolic_contract_expr(&self, state: &EffectState, expr: HirExprId) -> bool {
        self.types.facts.expr_types.get(&expr).is_some_and(|ty| {
            self.latent_sources_for_expr(state, expr)
                .is_some_and(|sources| self.symbolic_contract_value(*ty, &sources))
        })
    }

    pub(super) fn symbolic_contract_value(&self, ty: TypeId, sources: &[LatentSource]) -> bool {
        !sources.is_empty()
            && sources.iter().all(|source| match source {
                LatentSource::Parameter { symbol, .. } => {
                    self.symbol_value_type(*symbol).is_some_and(|parameter| {
                        self.descriptor_contract_type(parameter) == Some(ty)
                            || (parameter == ty && self.symbolic_callable_parameter(*symbol))
                    })
                }
                _ => false,
            })
    }

    pub(super) fn symbolic_callable_parameter(&self, symbol: etas_hir::SymbolId) -> bool {
        let Some(SymbolDef::Param { owner, .. }) =
            self.hir.symbols.get(symbol).map(|symbol| &symbol.def)
        else {
            return false;
        };
        let Some(ty) = self.symbol_value_type(symbol) else {
            return false;
        };
        let signature = match self.types.facts.item_signatures.get(owner) {
            Some(
                etas_types::ItemSignature::Flow(signature)
                | etas_types::ItemSignature::Tool(signature),
            ) => Some(signature),
            Some(etas_types::ItemSignature::Agent(signature)) => signature.callable(),
            _ => None,
        };
        let Some(signature) = signature else {
            return false;
        };
        if !signature.generic_params.iter().any(|parameter| {
            parameter.kind == etas_types::CallableGenericParamKind::Type && parameter.subject == ty
        }) {
            return false;
        }
        // A typed descriptor proves the generic parameter's callable kind, not
        // that another value of F shares that descriptor's behavioral contract.
        self.unit_params(EffectUnit::Item(*owner))
            .is_some_and(|parameters| {
                parameters.into_iter().any(|parameter| {
                    self.symbol_value_type(parameter)
                        .and_then(|ty| self.descriptor_contract_type(ty))
                        == Some(ty)
                })
            })
    }

    pub(super) fn definition_contract_sources(
        &self,
        state: &EffectState,
        expr: HirExprId,
    ) -> Option<Vec<LatentSource>> {
        if let Some(recipe) = self.types.facts.definition_operations.get(&expr) {
            // Both operations preserve the frozen contract. The compile request
            // itself contributes only its ordinary service effects elsewhere.
            return self
                .latent_sources_for_expr(state, recipe.input)
                .map(|sources| {
                    sources
                        .into_iter()
                        .map(|source| freeze(source, expr))
                        .collect()
                });
        }
        let fact = self.types.facts.reflections.get(&expr)?;
        match fact.target {
            CheckedReflectionTarget::Callable { declaration }
            | CheckedReflectionTarget::Agent { declaration, .. }
            | CheckedReflectionTarget::AgentDefaultEntry { declaration, .. } => {
                let symbol = self.type_symbols.canonical_symbol(self.hir, declaration)?;
                let SymbolDef::Item { item } = self.hir.symbols.get(symbol)?.def else {
                    return None;
                };
                let unit = match self.hir.items.get(item)? {
                    etas_hir::HirItem::Flow(_) | etas_hir::HirItem::Tool(_) => {
                        EffectUnit::Item(item)
                    }
                    etas_hir::HirItem::Agent(_) => match self.agent_default_call_target(item) {
                        CallTarget::Direct(unit) => unit,
                        _ => return None,
                    },
                    _ => return None,
                };
                Some(vec![LatentSource::FrozenContract {
                    unit,
                    reference: None,
                    environment: None,
                }])
            }
            CheckedReflectionTarget::CallableValue { binding, .. }
            | CheckedReflectionTarget::AgentValue { binding, .. } => self
                .latent_sources_for_symbol(state, binding)
                .map(|sources| {
                    sources
                        .into_iter()
                        .map(|source| freeze(source, expr))
                        .collect()
                }),
            CheckedReflectionTarget::ExternalCallable { .. } => None,
        }
    }

    pub(super) fn descriptor_contract_type(&self, ty: TypeId) -> Option<TypeId> {
        let Type::Applied { constructor, args } = self.types.store.get(ty)? else {
            return None;
        };
        let code = &self.types.facts.known_std_types.code;
        if code.callable_definition != Some(TypeId(constructor.0))
            && code.agent_definition != Some(TypeId(constructor.0))
        {
            return None;
        }
        let [contract] = args.as_slice() else {
            return None;
        };
        Some(*contract)
    }
}

fn freeze(source: LatentSource, expr: HirExprId) -> LatentSource {
    match source {
        LatentSource::Unit(unit) => LatentSource::FrozenContract {
            unit,
            reference: None,
            environment: None,
        },
        LatentSource::Captured { unit, environment } => LatentSource::FrozenContract {
            unit,
            reference: None,
            environment: Some(environment),
        },
        LatentSource::Reference {
            unit,
            value,
            environment,
        } => LatentSource::FrozenContract {
            unit,
            reference: Some(value),
            environment,
        },
        LatentSource::Metadata(mut source) => {
            source.analysis = crate::CallableAnalysisKind::FrozenContract;
            LatentSource::Metadata(source)
        }
        // A generic descriptor retains its symbolic parameter until application.
        source @ (LatentSource::FrozenContract { .. }
        | LatentSource::Parameter { .. }
        | LatentSource::Unavailable { .. }) => source,
        _ => LatentSource::Unavailable { expr },
    }
}
