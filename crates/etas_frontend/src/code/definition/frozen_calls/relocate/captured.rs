use super::*;
use etas_effects::facts::{CallableEnvironment, CallableEnvironmentId, CallableValueOrigin};
use std::collections::BTreeSet;

impl Relocation<'_> {
    pub fn captured_environments(
        &self,
        summary: &EffectSummary,
    ) -> Result<BTreeMap<CallableEnvironmentId, CallableEnvironment>, ProjectionError> {
        let mut pending = Vec::new();
        for exit in summary.exits.iter().flat_map(|exits| &exits.exits) {
            if let CallableOutcome::Returned {
                observation: ReturnObservation::CallableSources(sources),
                ..
            } = &exit.outcome
            {
                for source in sources {
                    match source {
                        CallableReturnSource::FrozenContract { environment, .. } => {
                            pending.extend(environment)
                        }
                        CallableReturnSource::Captured { environment, .. } => {
                            pending.push(*environment)
                        }
                        CallableReturnSource::Metadata(source) => {
                            pending.extend(source.environment)
                        }
                        CallableReturnSource::Reference(reference) => {
                            pending.extend(reference.environment)
                        }
                        _ => {}
                    }
                }
            }
        }
        let mut environments = BTreeMap::new();
        while let Some(id) = pending.pop() {
            if environments.contains_key(&id) {
                continue;
            }
            if environments.len() >= etas_package_metadata::MAX_METADATA_GRAPH_NODES {
                return Err(invalid("frozen captured environment exceeds graph budget"));
            }
            let source = self
                .original
                .effects
                .callable_environments
                .get(&id)
                .ok_or_else(|| invalid("frozen callable's capture environment is missing"))?;
            let owner = *self
                .returned_items
                .get(&source.owner)
                .ok_or_else(|| invalid("captured environment has no canonical target owner"))?;
            let mut parameters = BTreeMap::new();
            for (parameter, origins) in &source.parameters {
                let parameter = self.parameter_target(*parameter)?;
                let mut relocated = BTreeSet::new();
                for origin in origins {
                    let origin = match *origin {
                        CallableValueOrigin::FrozenContract {
                            unit,
                            reference,
                            environment,
                        } => {
                            pending.extend(environment);
                            CallableValueOrigin::FrozenContract {
                                unit: self.captured_unit(unit)?,
                                reference: reference
                                    .map(|value| self.captured_expr(value))
                                    .transpose()?,
                                environment,
                            }
                        }
                        CallableValueOrigin::Unit(unit) => {
                            CallableValueOrigin::Unit(self.captured_unit(unit)?)
                        }
                        CallableValueOrigin::Captured { unit, environment } => {
                            pending.push(environment);
                            CallableValueOrigin::Captured {
                                unit: self.captured_unit(unit)?,
                                environment,
                            }
                        }
                        CallableValueOrigin::Metadata(source) => {
                            pending.extend(source.environment);
                            CallableValueOrigin::Metadata(source)
                        }
                        CallableValueOrigin::Parameter { owner, symbol } => {
                            CallableValueOrigin::Parameter {
                                owner: *self.returned_items.get(&owner).ok_or_else(|| {
                                    invalid("captured parameter has no target owner")
                                })?,
                                symbol: *self.parameters.get(&symbol).ok_or_else(|| {
                                    invalid("captured parameter has no target symbol")
                                })?,
                            }
                        }
                        CallableValueOrigin::EnumConstructor(symbol) => {
                            CallableValueOrigin::EnumConstructor(self.variant(symbol)?)
                        }
                        CallableValueOrigin::StdReference { value, environment } => {
                            pending.extend(environment);
                            CallableValueOrigin::StdReference {
                                value: self.captured_expr(value)?,
                                environment,
                            }
                        }
                        CallableValueOrigin::Reference {
                            unit,
                            value,
                            environment,
                        } => {
                            pending.extend(environment);
                            CallableValueOrigin::Reference {
                                unit: self.captured_unit(unit)?,
                                value: self.captured_expr(value)?,
                                environment,
                            }
                        }
                        CallableValueOrigin::Unavailable { expr } => {
                            CallableValueOrigin::Unavailable {
                                expr: self.captured_expr(expr)?,
                            }
                        }
                    };
                    relocated.insert(origin);
                }
                parameters.insert(parameter, relocated);
            }
            let generic_scope = self
                .from
                .callables
                .iter()
                .find(|(_, item)| **item == source.owner)
                .map(|(id, _)| super::projection::owner(id))
                .transpose()?;
            let mut generics = BTreeMap::new();
            for (parameter, value) in &source.generics {
                use etas_effects::facts::{
                    CallableGenericParameter as P, CallableGenericValue as V,
                };
                let parameter = match parameter {
                    P::Native { owner, position } => P::Native {
                        owner: *self.returned_items.get(owner).ok_or_else(|| {
                            invalid("captured generic has no canonical target owner")
                        })?,
                        position: *position,
                    },
                    P::Graph { .. } => *parameter,
                };
                let scope = generic_scope.as_ref().ok_or_else(|| {
                    invalid("captured generic value has no checked relocation scope")
                })?;
                let value = match value {
                    V::Type(ty) => V::Type(self.ty(scope, *ty)?),
                    V::EffectRow(row) => V::EffectRow(self.row(row, scope)?),
                };
                generics.insert(parameter, value);
            }
            environments.insert(
                id,
                CallableEnvironment {
                    owner,
                    parameters,
                    generics,
                },
            );
        }
        Ok(environments)
    }

    pub(super) fn captured_unit(&self, unit: EffectUnit) -> Result<EffectUnit, ProjectionError> {
        match unit {
            EffectUnit::Item(item) => self
                .returned_items
                .get(&item)
                .copied()
                .map(EffectUnit::Item),
            EffectUnit::AnonymousFlow { .. } => self.returned_anonymous.get(&unit).copied(),
            EffectUnit::HandlerArm { .. } => None,
        }
        .ok_or_else(|| invalid("captured callable has no unchanged canonical target unit"))
    }

    fn captured_expr(
        &self,
        expr: etas_hir::HirExprId,
    ) -> Result<etas_hir::HirExprId, ProjectionError> {
        self.native_exprs.get(&expr).copied().ok_or_else(|| {
            invalid("captured callable reference has no unchanged source expression")
        })
    }
}

pub(super) fn expression_bindings(
    original: &crate::CheckedProgram,
    target: DescriptorInput<'_>,
    items: &HashMap<HirItemId, HirItemId>,
    overlay: &crate::code::draft::DefaultBodyOverlay,
) -> HashMap<etas_hir::HirExprId, etas_hir::HirExprId> {
    let old_default = overlay.frozen().default_body();
    let target_default = old_default.and_then(|owner| items.get(&owner).copied());
    let mut targets = HashMap::new();
    for (expr, data) in target.hir.exprs.iter() {
        let Some(owner) = target.hir.index().expr_item.get(&expr).copied() else {
            continue;
        };
        if Some(owner) == target_default && !overlay.native_revision().unchanged {
            continue;
        }
        let mut span = data.span(&target.hir.blocks);
        if Some(owner) == target_default && span.source == overlay.relocated().source().id {
            span.source = overlay.slot().source;
        }
        targets
            .entry((owner, span, std::mem::discriminant(data)))
            .and_modify(|entry| *entry = None)
            .or_insert(Some(expr));
    }
    original
        .hir
        .exprs
        .iter()
        .filter_map(|(expr, data)| {
            let owner = items.get(original.hir.index().expr_item.get(&expr)?)?;
            targets
                .get(&(
                    *owner,
                    data.span(&original.hir.blocks),
                    std::mem::discriminant(data),
                ))
                .copied()
                .flatten()
                .map(|target| (expr, target))
        })
        .collect()
}
