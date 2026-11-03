use etas_code::{
    descriptor::GenericParameterKind,
    residual::identity::{GenericParameterId, ResidualOwnerId, StaticBindingRecipe},
};
use etas_hir::{HirDeclarationConformanceTarget, HirItem, HirType, ResolveResult, SymbolDef};

use super::{TraceSpecMaterializer, callable_symbol_and_conformances};
use crate::EffectPipelineError;

impl TraceSpecMaterializer<'_> {
    pub(super) fn source_spec_bindings(
        &self,
        fact: &etas_types::TraceSpecConformanceFact,
        arguments: &[etas_types::TypeId],
    ) -> Result<Option<Vec<StaticBindingRecipe>>, EffectPipelineError> {
        let Some(reference) = self.checked_conformance_reference(fact) else {
            return Ok(None);
        };
        if reference.spec_args.len() != arguments.len() {
            return Ok(None);
        }
        if arguments.is_empty() {
            return Ok(Some(Vec::new()));
        }
        let mut bindings = Vec::with_capacity(arguments.len());
        for (source, checked_id) in reference.spec_args.iter().zip(arguments) {
            let Some(ty) = self.hir.types.get(*source) else {
                return Err(EffectPipelineError::MissingTypeFact { ty: *source });
            };
            let Some(checked) = self.types.store.get(*checked_id) else {
                return Err(EffectPipelineError::InvalidCallableFacts {
                    reason: "trace spec argument has no checked type".into(),
                });
            };
            let recipe = match (ty, checked) {
                (HirType::Primitive { kind, .. }, etas_types::Type::Primitive(primitive))
                    if etas_types::PrimitiveType::from_hir(*kind) == *primitive =>
                {
                    StaticBindingRecipe::ExplicitType(primitive.descriptor_fingerprint().map_err(
                        |error| EffectPipelineError::InvalidCallableFacts {
                            reason: format!("invalid primitive descriptor: {error:?}"),
                        },
                    )?)
                }
                (HirType::Path { path, args, .. }, etas_types::Type::Named(named))
                    if args.is_empty() =>
                {
                    let ResolveResult::Resolved(symbol) = path.resolution else {
                        return Ok(None);
                    };
                    if named.source_parameter != Some(symbol) {
                        return Ok(None);
                    }
                    let Some(SymbolDef::TypeParam {
                        owner, param_index, ..
                    }) = self.hir.symbols.get(symbol).map(|symbol| &symbol.def)
                    else {
                        return Ok(None);
                    };
                    if *owner != fact.item {
                        return Ok(None);
                    }
                    let Some(declaration) = self.hir.items.get(*owner) else {
                        return Ok(None);
                    };
                    if !matches!(declaration, HirItem::Flow(_) | HirItem::Tool(_)) {
                        return Ok(None);
                    }
                    let kind = super::checked_callable_owner_kind(self.hir, *owner)?;
                    let span = declaration.span();
                    let Some(snapshot) = self.source_snapshots.get(&span.source) else {
                        return Ok(None);
                    };
                    let location = snapshot
                        .location(u64::from(span.range.start.0), u64::from(span.range.end.0))
                        .map_err(|error| EffectPipelineError::InvalidSourceIdentity {
                            source: span.source,
                            reason: format!("invalid generic owner source range: {error:?}"),
                        })?;
                    let owner =
                        ResidualOwnerId::from_checked_source_declaration(snapshot, &location, kind)
                            .map_err(|error| EffectPipelineError::InvalidSourceIdentity {
                                source: span.source,
                                reason: format!("invalid generic owner: {error:?}"),
                            })?;
                    StaticBindingRecipe::TypeParameter(GenericParameterId::from_source_owner(
                        owner,
                        *param_index,
                        GenericParameterKind::Type,
                    ))
                }
                (HirType::Path { .. } | HirType::Tuple { .. } | HirType::Record { .. }, _)
                    if self.types.facts.type_refs.get(source) == Some(checked_id) =>
                {
                    let fingerprint = etas_types::ty::closed_structural_type_fingerprint(
                        &self.types.store,
                        *checked_id,
                        etas_code::descriptor::DescriptorLimits::default(),
                    )
                    .map_err(|error| {
                        EffectPipelineError::InvalidCallableFacts {
                            reason: format!("invalid closed structural spec argument: {error:?}"),
                        }
                    })?;
                    if let Some(fingerprint) = fingerprint {
                        StaticBindingRecipe::ExplicitType(fingerprint)
                    } else {
                        let Some(binders) = self.source_type_binders(fact.item)? else {
                            return Ok(None);
                        };
                        let Some(template) = etas_types::ty::symbolic_structural_type_template(
                            &self.types.store,
                            *checked_id,
                            &binders,
                            etas_code::residual::type_template::TypeTemplateLimits::default(),
                        )
                        .map_err(|error| {
                            EffectPipelineError::InvalidCallableFacts {
                                reason: format!("invalid symbolic spec argument: {error:?}"),
                            }
                        })?
                        else {
                            return Ok(None);
                        };
                        StaticBindingRecipe::TypeTemplate(template)
                    }
                }
                _ => return Ok(None),
            };
            bindings.push(recipe);
        }
        Ok(Some(bindings))
    }

    pub(super) fn checked_conformance_reference(
        &self,
        fact: &etas_types::TraceSpecConformanceFact,
    ) -> Option<&etas_hir::HirSpecRef> {
        let etas_types::TraceSpecConformanceTarget::Named { spec_symbol, .. } = &fact.target else {
            return None;
        };
        let matches = |reference: &&etas_hir::HirSpecRef| {
            let ResolveResult::Resolved(symbol) = reference.spec_path.resolution else {
                return false;
            };
            reference.span == fact.span
                && self.canonical_source_symbol(symbol) == Some(*spec_symbol)
        };
        let item = self.hir.items.get(fact.item)?;
        if let Some((_, conformances)) = callable_symbol_and_conformances(item) {
            let mut found = conformances
                .iter()
                .filter_map(|conformance| match &conformance.target {
                    HirDeclarationConformanceTarget::Path(reference) => Some(reference),
                    _ => None,
                })
                .filter(matches);
            if let Some(reference) = found.next() {
                return found.next().is_none().then_some(reference);
            }
        }
        let symbol = item.symbol()?;
        let method = self.types.facts.agent_methods.get(&symbol)?;
        if method.symbol != symbol || method.body != fact.item || method.name != "run" {
            return None;
        }
        let HirItem::Impl(implementation) = self.hir.items.get(method.implementation?)? else {
            return None;
        };
        let etas_hir::HirImplTarget::Agent { target, specs } = &implementation.target else {
            return None;
        };
        let HirItem::Agent(agent) = self.hir.items.get(method.agent)? else {
            return None;
        };
        let ResolveResult::Resolved(target_symbol) = target.resolution else {
            return None;
        };
        if self.canonical_source_symbol(target_symbol) != Some(agent.symbol)
            || !implementation.items.iter().any(|item| {
                matches!(item, etas_hir::HirImplItem::Callable { item } if *item == fact.item)
            })
        {
            return None;
        }
        let mut found = specs.iter().filter(matches);
        let reference = found.next()?;
        found.next().is_none().then_some(reference)
    }

    fn canonical_source_symbol(&self, symbol: etas_hir::SymbolId) -> Option<etas_hir::SymbolId> {
        let mut current = symbol;
        // Resolved source imports are the only permitted alias edges. A cycle
        // cannot terminate within the number of entries in this map.
        for _ in 0..=self.hir.source_import_targets.len() {
            self.hir.symbols.get(current)?;
            match self.hir.source_import_targets.get(&current) {
                Some(target) => current = *target,
                None => return Some(current),
            }
        }
        None
    }

    fn source_type_binders(
        &self,
        item: etas_hir::HirItemId,
    ) -> Result<
        Option<std::collections::BTreeMap<etas_hir::SymbolId, GenericParameterId>>,
        EffectPipelineError,
    > {
        let Some(declaration) = self.hir.items.get(item) else {
            return Ok(None);
        };
        let parameters = match declaration {
            HirItem::Flow(flow) => &flow.type_params,
            HirItem::Tool(tool) => &tool.type_params,
            _ => return Ok(None),
        };
        let kind = super::checked_callable_owner_kind(self.hir, item)?;
        let span = declaration.span();
        let Some(snapshot) = self.source_snapshots.get(&span.source) else {
            return Ok(None);
        };
        let location = snapshot
            .location(u64::from(span.range.start.0), u64::from(span.range.end.0))
            .map_err(|error| EffectPipelineError::InvalidSourceIdentity {
                source: span.source,
                reason: format!("invalid generic owner source range: {error:?}"),
            })?;
        let owner = ResidualOwnerId::from_checked_source_declaration(snapshot, &location, kind)
            .map_err(|error| EffectPipelineError::InvalidSourceIdentity {
                source: span.source,
                reason: format!("invalid generic owner: {error:?}"),
            })?;
        let mut binders = std::collections::BTreeMap::new();
        for symbol in parameters {
            let Some(definition) = self.hir.symbols.get(*symbol) else {
                return Ok(None);
            };
            if let SymbolDef::TypeParam {
                owner: parameter_owner,
                param_index,
                ..
            } = &definition.def
            {
                if *parameter_owner != item {
                    return Ok(None);
                }
                binders.insert(
                    *symbol,
                    GenericParameterId::from_source_owner(
                        owner,
                        *param_index,
                        GenericParameterKind::Type,
                    ),
                );
            }
        }
        Ok(Some(binders))
    }
}
