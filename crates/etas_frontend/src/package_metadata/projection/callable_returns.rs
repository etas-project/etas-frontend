use super::*;
use etas_effects::{CallableReturnSource, EffectSummary, EffectUnit};
use etas_package_metadata::{
    CallableValueGraph, CallableValueNode, CallableValueParameter, CallableValueScope,
    CallableValueSource,
};

impl ProjectMetadataProjection<'_> {
    pub(in crate::package_metadata) fn returned_callables(
        &self,
        item: HirItemId,
        summary: &EffectSummary,
    ) -> Result<Option<CallableValueGraph>, PackageMetadataError> {
        Ok(self
            .returned_callable_projection(item, summary)?
            .map(|projection| std::sync::Arc::unwrap_or_clone(projection.graph)))
    }

    pub(in crate::package_metadata) fn returned_callable_projection(
        &self,
        item: HirItemId,
        summary: &EffectSummary,
    ) -> Result<Option<crate::package_metadata::CheckedCallableGraph>, PackageMetadataError> {
        let Some(sources) = sources(summary) else {
            return Ok(None);
        };
        let mut builder = Builder {
            projection: self,
            root: item,
            values: vec![],
            origins: vec![],
            units: BTreeMap::new(),
            foreign: BTreeMap::new(),
            references: BTreeMap::new(),
            pending: vec![],
            parameters: [(
                CallableValueScope::Root,
                self.callable_param_symbols(item)
                    .iter()
                    .enumerate()
                    .map(|(position, symbol)| (*symbol, position as u32))
                    .collect(),
            )]
            .into(),
            parents: BTreeMap::new(),
        };
        let Some(roots) = builder.sources(&sources, CallableValueScope::Root, None)? else {
            return Ok(None);
        };
        while let Some(pending) = builder.pending.pop() {
            let (index, node) = match pending {
                Pending::Native(index, unit) => (index, builder.node(index, unit)?),
                Pending::Metadata(index, instance) => {
                    (index, builder.foreign_node(index, instance)?)
                }
                Pending::Reference(index, reference) => {
                    (index, builder.reference_node(index, reference)?)
                }
            };
            let Some(node) = node else {
                return Ok(None);
            };
            // The worklist reserves slots before visiting return edges, so
            // recursive references never recursively embed a callable body.
            builder.values[index as usize] = Some(node);
        }
        let graph = CallableValueGraph {
            sources: roots,
            values: builder
                .values
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| invalid("callable graph contains an unmaterialized node"))?,
        };
        etas_package_metadata::validate_callable_value_graph(&graph)?;
        Ok(Some(crate::package_metadata::CheckedCallableGraph {
            graph: std::sync::Arc::new(graph),
            origins: builder.origins,
        }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Instance {
    unit: EffectUnit,
    environment: Option<etas_effects::facts::CallableEnvironmentId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ForeignInstance {
    source: etas_effects::MetadataCallableSource,
    root: CallableValueScope,
}

enum Pending {
    Native(u32, Instance),
    Metadata(u32, ForeignInstance),
    Reference(u32, etas_effects::CallableReturnReference),
}

struct Builder<'a, 'b> {
    projection: &'a ProjectMetadataProjection<'b>,
    root: HirItemId,
    values: Vec<Option<CallableValueNode>>,
    origins: Vec<crate::package_metadata::CallableGraphOrigin>,
    units: BTreeMap<Instance, u32>,
    foreign: BTreeMap<ForeignInstance, u32>,
    references: BTreeMap<etas_effects::CallableReturnReference, u32>,
    pending: Vec<Pending>,
    parameters: BTreeMap<CallableValueScope, BTreeMap<etas_hir::SymbolId, u32>>,
    parents: BTreeMap<u32, CallableValueScope>,
}

impl Builder<'_, '_> {
    fn parameter(
        &self,
        symbol: etas_hir::SymbolId,
        mut scope: CallableValueScope,
    ) -> Result<Option<CallableValueParameter>, PackageMetadataError> {
        let mut visited = BTreeSet::new();
        while visited.insert(scope) {
            if let Some(position) = self
                .parameters
                .get(&scope)
                .and_then(|params| params.get(&symbol))
            {
                return Ok(Some(CallableValueParameter {
                    scope,
                    position: *position,
                }));
            }
            scope = match scope {
                CallableValueScope::Root => return Ok(None),
                CallableValueScope::Value(value) => *self
                    .parents
                    .get(&value)
                    .ok_or_else(|| invalid("callable graph lexical parent is missing"))?,
            };
        }
        Err(invalid("callable graph lexical parameter lookup is cyclic"))
    }
    fn scope_contains(&self, mut scope: CallableValueScope, ancestor: CallableValueScope) -> bool {
        let mut visited = BTreeSet::new();
        while visited.insert(scope) {
            if scope == ancestor {
                return true;
            }
            scope = match scope {
                CallableValueScope::Root => return false,
                CallableValueScope::Value(value) => match self.parents.get(&value) {
                    Some(parent) => *parent,
                    None => return false,
                },
            };
        }
        false
    }

    fn node(
        &mut self,
        index: u32,
        instance: Instance,
    ) -> Result<Option<CallableValueNode>, PackageMetadataError> {
        let checked = self.projection.checked;
        let unit = instance.unit;
        let owner = unit
            .owner()
            .ok_or_else(|| invalid("returned callable has no declaration owner"))?;
        let signature = checked.types.item_signatures.get(&owner);
        let (type_names, effect_names) = signature
            .map(|signature| {
                (
                    callable_type_generic_names(signature),
                    callable_effect_generic_names(signature),
                )
            })
            .unwrap_or_default();
        let parameters = match signature {
            Some(ItemSignature::Flow(signature) | ItemSignature::Tool(signature)) => {
                signature.generic_params.as_slice()
            }
            Some(ItemSignature::Agent(signature)) => signature
                .callable()
                .map(|signature| signature.generic_params.as_slice())
                .unwrap_or_default(),
            _ => &[],
        };
        let generic_scope = if matches!(unit, EffectUnit::Item(_)) {
            Some(CallableValueScope::Value(index))
        } else if owner == self.root {
            Some(CallableValueScope::Root)
        } else {
            self.units
                .get(&Instance {
                    unit: EffectUnit::Item(owner),
                    environment: None,
                })
                .map(|parent| CallableValueScope::Value(*parent))
        };
        let captured = instance
            .environment
            .map(|environment| self.captured_projection(owner, parameters, environment))
            .transpose()?;
        let generic_bindings = if let Some(captured) = &captured {
            captured.bindings.clone()
        } else {
            generics::bindings(parameters, generic_scope)?
        };
        let (type_names, effect_names) = captured
            .as_ref()
            .map_or((type_names, effect_names), |captured| {
                (captured.type_names.clone(), captured.effect_names.clone())
            });
        let generic_params = if matches!(unit, EffectUnit::Item(_)) {
            self.projection
                .callable_generic_params_metadata(parameters)?
        } else {
            vec![]
        };
        let summary = if let Some(environment) = instance.environment {
            let key = etas_effects::facts::CapturedCallableId { unit, environment };
            match checked
                .effects
                .captured_callables
                .get(&key)
                .ok_or_else(|| invalid("exported closure has no materialized capture analysis"))?
            {
                etas_effects::facts::CapturedCallableAnalysis::Solved(summary) => summary.as_ref(),
                etas_effects::facts::CapturedCallableAnalysis::Unavailable { .. } => {
                    return Ok(None);
                }
            }
        } else {
            checked.effects.unit_effects.get(&unit).ok_or_else(|| {
                invalid("returned callable is missing its solved unit effect facts")
            })?
        };
        let (params, output, names, parameter_symbols) = match unit {
            EffectUnit::Item(item) => {
                let signature = match checked.types.item_signatures.get(&item) {
                    Some(ItemSignature::Flow(signature) | ItemSignature::Tool(signature)) => {
                        signature
                    }
                    _ => return Ok(None),
                };
                (
                    signature.params.clone(),
                    signature.output,
                    self.projection.callable_param_names(item)?,
                    self.projection.callable_param_symbols(item).to_vec(),
                )
            }
            EffectUnit::AnonymousFlow { value, .. } => {
                let Some(ty) = checked
                    .types
                    .expr_types
                    .get(&value)
                    .and_then(|ty| checked.type_store.get(*ty))
                else {
                    return Err(invalid("returned anonymous flow has no checked type fact"));
                };
                let Type::Function(flow) = ty else {
                    return Err(invalid("returned anonymous flow fact is not callable"));
                };
                let Some(etas_hir::HirExpr::Lambda { params, .. }) = checked.hir.exprs.get(value)
                else {
                    return Ok(None);
                };
                let names = params
                    .iter()
                    .map(|symbol| {
                        checked
                            .hir
                            .symbols
                            .get(*symbol)
                            .map(|symbol| symbol.name.clone())
                            .ok_or_else(|| {
                                invalid("returned anonymous parameter has no checked symbol")
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                (flow.input.clone(), flow.output, names, params.clone())
            }
            _ => return Ok(None),
        };
        self.parameters.insert(
            CallableValueScope::Value(index),
            parameter_symbols
                .iter()
                .enumerate()
                .map(|(position, symbol)| (*symbol, position as u32))
                .collect(),
        );
        let returned_sources = match sources(summary) {
            Some(returned) => {
                let Some(returned) = self.sources(
                    &returned,
                    CallableValueScope::Value(index),
                    instance.environment,
                )?
                else {
                    return Ok(None);
                };
                Some(returned)
            }
            None => None,
        };
        let trace_parameters = trace_parameters(&summary.action_trace);
        let mut parameter_bindings = vec![];
        let mut bound_parameters = BTreeSet::new();
        let mut trace_labels = BTreeMap::new();
        for target in trace_parameters {
            let etas_effects::ParameterCallTarget::Native(symbol) = target else {
                return Ok(None);
            };
            let Some(parameter) = self.parameter(symbol, CallableValueScope::Value(index))? else {
                return Ok(None);
            };
            if !self.scope_contains(CallableValueScope::Value(index), parameter.scope) {
                return Ok(None);
            }
            let label = match parameter.scope {
                CallableValueScope::Root => format!("rp{}", parameter.position),
                CallableValueScope::Value(value) => format!("v{value}p{}", parameter.position),
            };
            trace_labels.insert(
                etas_effects::ParameterCallTarget::Native(symbol),
                label.clone(),
            );
            if bound_parameters.insert(parameter) {
                parameter_bindings.push(etas_package_metadata::CallableValueParameterBinding {
                    label,
                    parameter,
                });
            }
        }
        let Some(projected) = summary::project(
            self.projection,
            summary,
            &type_names,
            &effect_names,
            &trace_labels,
        )?
        else {
            return Ok(None);
        };
        Ok(Some(CallableValueNode {
            handler_requests: projected.handler_requests,
            behavior: projected.behavior,
            parent_scope: self
                .parents
                .get(&index)
                .copied()
                .ok_or_else(|| invalid("callable value has no registered lexical parent"))?,
            generic_params,
            generic_bindings,
            param_names: names,
            params: params
                .into_iter()
                .map(|ty| {
                    if let Some(captured) = &captured {
                        return captured.metadata_type(self.projection, ty);
                    }
                    let mut ty = self.projection.metadata_type(ty)?;
                    normalize_generic_type(&mut ty, &type_names);
                    Ok::<_, PackageMetadataError>(ty)
                })
                .collect::<Result<_, _>>()?,
            output: {
                if let Some(captured) = &captured {
                    captured.metadata_type(self.projection, output)?
                } else {
                    let mut ty = self.projection.metadata_type(output)?;
                    normalize_generic_type(&mut ty, &type_names);
                    ty
                }
            },
            public_effects: projected.public_effects,
            requested_actions: projected.requested_actions,
            handled_requested_actions: projected.handled_requested_actions,
            default_handler_effects: projected.default_handler_effects,
            parameter_bindings,
            action_trace: projected.action_trace,
            returned_sources,
        }))
    }
}

fn sources(summary: &EffectSummary) -> Option<BTreeSet<CallableReturnSource>> {
    summary.exits.as_ref()?.callable_return_sources()
}

fn trace_parameters(trace: &ActionTraceDomain) -> BTreeSet<etas_effects::ParameterCallTarget> {
    let mut parameters = BTreeSet::new();
    let mut pending = vec![trace];
    while let Some(trace) = pending.pop() {
        match trace {
            ActionTraceDomain::ParameterCall { target, .. } => {
                parameters.insert(*target);
            }
            ActionTraceDomain::Widened {
                parameter_calls, ..
            } => {
                for call in parameter_calls {
                    parameters.insert(call.target);
                }
            }
            ActionTraceDomain::Seq(parts) | ActionTraceDomain::Choice(parts) => {
                pending.extend(parts)
            }
            ActionTraceDomain::Repeat(part) => pending.push(part),
            _ => {}
        }
    }
    parameters
}

mod behavior;
mod captured;
mod foreign;
mod generics;
mod references;
mod sources;
mod summary;
use behavior::behavior;

pub(super) fn invalid(reason: &str) -> PackageMetadataError {
    PackageMetadataError::InvalidMetadataType {
        reason: reason.into(),
    }
}
