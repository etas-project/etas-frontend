use super::*;
use etas_effects::{CallableReturnSource, EffectSummary, EffectUnit};
use etas_package_metadata::{
    CallableValueGraph, CallableValueNode, CallableValueParameter, CallableValueScope,
    CallableValueSource,
};

impl ProjectMetadataProjection<'_> {
    pub(in crate::package_metadata) fn invocation_callable_graph(
        &self,
        item: HirItemId,
        summary: &EffectSummary,
        identities: Option<&crate::code::identity::DefinitionIdentityIndex>,
    ) -> Result<Option<CallableValueGraph>, PackageMetadataError> {
        use etas_code::descriptor::ValueProjectionStep;
        use etas_effects::ParameterCallTarget as Target;
        let needs_inputs = summary_parameters(summary).into_iter().any(|target| {
            let path = match target {
                Target::Projection(id) => self
                    .checked
                    .effects
                    .parameter_projections
                    .get(&id)
                    .map(|p| &p.path),
                Target::ExternalProjection(id) => self
                    .checked
                    .effects
                    .external_parameter_projections
                    .get(&id)
                    .map(|p| &p.path),
                _ => None,
            };
            path.is_some_and(|path| !ValueProjectionStep::call_inputs(path).is_empty())
        });
        if needs_inputs {
            self.implementation_callable_graph(item, identities)
                .map(Some)
        } else {
            Ok(None)
        }
    }

    fn callable_trace_parameters(
        &self,
        summary: &EffectSummary,
    ) -> Result<BTreeSet<etas_effects::ParameterCallTarget>, PackageMetadataError> {
        use etas_effects::{ParameterCallTarget as Target, facts::CallableValueOrigin as Origin};
        let mut parameters = summary_parameters(summary);
        let mut pending = parameters
            .iter()
            .filter_map(|parameter| match parameter {
                Target::SpecMethod { environment, .. }
                | Target::MetadataSpecMethod {
                    environment: Some(environment),
                    ..
                } => Some(*environment),
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut visited = BTreeSet::new();
        while let Some(environment) = pending.pop() {
            if !visited.insert(environment) {
                continue;
            }
            let snapshot = self
                .checked
                .effects
                .callable_environments
                .get(&environment)
                .ok_or_else(|| invalid("method capture has no checked parameter environment"))?;
            for origin in snapshot.parameters.values().flatten() {
                match origin {
                    Origin::Projection(id) => {
                        parameters.insert(Target::Projection(*id));
                    }
                    Origin::Parameter { symbol, .. } => {
                        parameters.insert(Target::Native(*symbol));
                    }
                    Origin::Captured { environment, .. }
                    | Origin::SpecMethod { environment, .. }
                    | Origin::MetadataSpecMethod { environment, .. }
                    | Origin::Existential { environment, .. } => pending.push(*environment),
                    Origin::Metadata(source) => pending.extend(source.environment),
                    Origin::MetadataObject(source) => pending.extend(source.environment),
                    Origin::FrozenContract { environment, .. }
                    | Origin::Reference { environment, .. }
                    | Origin::StdReference { environment, .. } => pending.extend(environment),
                    _ => {}
                }
            }
        }
        Ok(parameters)
    }

    pub(in crate::package_metadata) fn implementation_callable_graph(
        &self,
        item: HirItemId,
        identities: Option<&crate::code::identity::DefinitionIdentityIndex>,
    ) -> Result<CallableValueGraph, PackageMetadataError> {
        let scoped = self.with_identities(identities);
        let mut builder = Builder::new(&scoped, item);
        builder.method_root = true;
        // Root carries the impl's type binders, with no runtime parameters.
        // Method parameters belong to the callable value reserved below.
        builder
            .parameters
            .insert(CallableValueScope::Root, BTreeMap::new());
        let roots = builder
            .sources(
                &[CallableReturnSource::SourceItem(item)].into(),
                CallableValueScope::Root,
                None,
            )?
            .ok_or_else(|| invalid("spec implementation callable origins are unavailable"))?;
        let graph = builder
            .finish(roots)?
            .ok_or_else(|| invalid("spec implementation callable analysis is unavailable"))?;
        Ok(std::sync::Arc::unwrap_or_clone(graph.graph))
    }

    pub(in crate::package_metadata) fn returned_callables(
        &self,
        item: HirItemId,
        summary: &EffectSummary,
        identities: Option<&crate::code::identity::DefinitionIdentityIndex>,
    ) -> Result<Option<CallableValueGraph>, PackageMetadataError> {
        Ok(self
            .returned_callable_projection(item, summary, identities)?
            .map(|projection| std::sync::Arc::unwrap_or_clone(projection.graph)))
    }

    pub(in crate::package_metadata) fn returned_callable_projection(
        &self,
        item: HirItemId,
        summary: &EffectSummary,
        identities: Option<&crate::code::identity::DefinitionIdentityIndex>,
    ) -> Result<Option<crate::package_metadata::CheckedCallableGraph>, PackageMetadataError> {
        let Some(sources) = sources(summary) else {
            return Ok(None);
        };
        let scoped = self.with_identities(identities);
        let mut builder = Builder::new(&scoped, item);
        let Some(roots) = builder.sources(&sources, CallableValueScope::Root, None)? else {
            return Ok(None);
        };
        if roots.is_empty() {
            // Checked absence of callable payload is not a callable graph.
            return Ok(None);
        }
        builder.root_projections =
            builder.return_projections(summary, CallableValueScope::Root, None)?;
        builder.finish(roots)
    }
}

impl<'a, 'b> Builder<'a, 'b> {
    fn invocation_parameters(
        &self,
        scope: CallableValueScope,
        bindings: &mut Vec<etas_package_metadata::CallableValueParameterBinding>,
    ) -> BTreeMap<etas_hir::SymbolId, u32> {
        let roots = bindings
            .iter()
            .map(|binding| (binding.parameter.scope, binding.parameter.position))
            .collect::<BTreeSet<_>>();
        let mut symbols = BTreeMap::new();
        for (owner, parameters) in &self.parameters {
            if !self.scope_contains(scope, *owner) {
                continue;
            }
            for (symbol, position) in parameters {
                if !roots.contains(&(*owner, *position)) {
                    continue;
                }
                let parameter = CallableValueParameter {
                    scope: *owner,
                    position: *position,
                    projection: None,
                };
                if !bindings
                    .iter()
                    .any(|binding| binding.parameter == parameter)
                {
                    bindings.push(etas_package_metadata::CallableValueParameterBinding {
                        label: self.parameter_label(parameter),
                        parameter,
                    });
                }
                symbols.insert(*symbol, parameter);
            }
        }
        bindings.sort_by(|left, right| {
            left.parameter
                .cmp(&right.parameter)
                .then(left.label.cmp(&right.label))
        });
        let positions = bindings
            .iter()
            .enumerate()
            .map(|(position, binding)| (binding.parameter, position as u32))
            .collect::<BTreeMap<_, _>>();
        symbols
            .into_iter()
            .map(|(symbol, parameter)| (symbol, positions[&parameter]))
            .collect()
    }

    fn new(projection: &'a ProjectMetadataProjection<'b>, root: HirItemId) -> Self {
        Self {
            projection,
            root,
            identities: projection.identities,
            parameter_projections: Default::default(),
            input_instances: Default::default(),
            pending_inputs: Default::default(),
            call_inputs: Vec::new(),
            values: vec![],
            objects: vec![],
            native_objects: BTreeMap::new(),
            foreign_objects: BTreeMap::new(),
            origins: vec![],
            units: BTreeMap::new(),
            foreign: BTreeMap::new(),
            references: BTreeMap::new(),
            pending: vec![],
            parameters: [(
                CallableValueScope::Root,
                projection
                    .callable_param_symbols(root)
                    .iter()
                    .enumerate()
                    .map(|(position, symbol)| (*symbol, position as u32))
                    .collect(),
            )]
            .into(),
            parents: BTreeMap::new(),
            method_root: false,
            root_projections: Vec::new(),
        }
    }

    fn finish(
        mut self,
        roots: Vec<CallableValueSource>,
    ) -> Result<Option<crate::package_metadata::CheckedCallableGraph>, PackageMetadataError> {
        loop {
            let pending = self.pending.pop();
            if pending.is_none() {
                let input = self.pending_inputs.borrow_mut().pop();
                let Some((index, input)) = input else { break };
                let node = self.input_node(input)?;
                if self.call_inputs.len() <= index as usize {
                    self.call_inputs.resize_with(index as usize + 1, || None);
                }
                self.call_inputs[index as usize] = Some(node);
                continue;
            }
            let pending = pending.unwrap();
            let (index, node) = match pending {
                Pending::ForeignObject(index, instance) => {
                    let Some(object) = self.foreign_object_node(instance)? else {
                        return Ok(None);
                    };
                    self.objects[index as usize] = Some(object);
                    continue;
                }
                Pending::Object(index, instance) => {
                    let Some(object) = self.object_node(instance)? else {
                        return Ok(None);
                    };
                    self.objects[index as usize] = Some(object);
                    continue;
                }
                Pending::Native(index, unit) => (index, self.node(index, unit)?),
                Pending::Metadata(index, instance) => (index, self.foreign_node(index, instance)?),
                Pending::Reference(index, reference) => {
                    (index, self.reference_node(index, reference)?)
                }
            };
            let Some(node) = node else {
                return Ok(None);
            };
            // The worklist reserves slots before visiting return edges, so
            // recursive references never recursively embed a callable body.
            self.values[index as usize] = Some(node);
        }
        let mut graph = CallableValueGraph {
            call_inputs: self
                .call_inputs
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| invalid("callback input graph contains an unmaterialized node"))?,
            source_projections: self.root_projections,
            parameter_projections: self.parameter_projections.into_inner(),
            objects: self
                .objects
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| invalid("value evidence contains an unmaterialized object"))?,
            sources: roots,
            values: self
                .values
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| invalid("callable graph contains an unmaterialized node"))?,
        };
        graph.canonicalize_parameter_projections()?;
        etas_package_metadata::validate_callable_value_graph(&graph)?;
        Ok(Some(crate::package_metadata::CheckedCallableGraph {
            graph: std::sync::Arc::new(graph),
            origins: self.origins,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct NativeObjectInstance {
    pack: etas_hir::HirExprId,
    environment: etas_effects::facts::CallableEnvironmentId,
    scope: CallableValueScope,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ForeignObjectInstance {
    source: etas_effects::MetadataObjectSource,
    root: CallableValueScope,
}

enum Pending {
    ForeignObject(u32, ForeignObjectInstance),
    Object(u32, NativeObjectInstance),
    Native(u32, Instance),
    Metadata(u32, ForeignInstance),
    Reference(u32, etas_effects::CallableReturnReference),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum InputInstance {
    Native { id: u32, scope: CallableValueScope },
    Foreign { id: u32, instance: ForeignInstance },
}

struct Builder<'a, 'b> {
    input_instances: std::cell::RefCell<BTreeMap<InputInstance, u32>>,
    pending_inputs: std::cell::RefCell<Vec<(u32, InputInstance)>>,
    call_inputs: Vec<Option<etas_package_metadata::CallableCallInputs>>,
    parameter_projections: std::cell::RefCell<Vec<Vec<etas_code::descriptor::ValueProjectionStep>>>,
    projection: &'a ProjectMetadataProjection<'b>,
    root: HirItemId,
    identities: Option<&'a crate::code::identity::DefinitionIdentityIndex>,
    values: Vec<Option<CallableValueNode>>,
    objects: Vec<Option<etas_package_metadata::ExistentialValueNode>>,
    native_objects: BTreeMap<NativeObjectInstance, u32>,
    foreign_objects: BTreeMap<ForeignObjectInstance, u32>,
    origins: Vec<crate::package_metadata::CallableGraphOrigin>,
    units: BTreeMap<Instance, u32>,
    foreign: BTreeMap<ForeignInstance, u32>,
    references: BTreeMap<etas_effects::CallableReturnReference, u32>,
    pending: Vec<Pending>,
    parameters: BTreeMap<CallableValueScope, BTreeMap<etas_hir::SymbolId, u32>>,
    parents: BTreeMap<u32, CallableValueScope>,
    method_root: bool,
    root_projections: Vec<etas_package_metadata::CallableReturnedProjection>,
}

impl Builder<'_, '_> {
    fn with_projection(
        &self,
        parameter: CallableValueParameter,
        suffix: &[etas_code::descriptor::ValueProjectionStep],
        scope: CallableValueScope,
    ) -> Result<CallableValueParameter, PackageMetadataError> {
        let mut path = suffix.to_vec();
        etas_code::descriptor::ValueProjectionStep::map_call_inputs(&mut path, &mut |id| {
            self.reserve_inputs(InputInstance::Native { id, scope })
        })?;
        self.with_portable_projection(parameter, &path)
    }

    fn with_portable_projection(
        &self,
        mut parameter: CallableValueParameter,
        suffix: &[etas_code::descriptor::ValueProjectionStep],
    ) -> Result<CallableValueParameter, PackageMetadataError> {
        if suffix.is_empty() {
            return Ok(parameter);
        }
        let mut paths = self.parameter_projections.borrow_mut();
        let mut path = match parameter.projection {
            Some(index) => paths
                .get(index as usize)
                .ok_or_else(|| invalid("composed parameter projection is missing"))?
                .clone(),
            None => Vec::new(),
        };
        path.extend_from_slice(suffix);
        let position = match paths.iter().position(|existing| *existing == path) {
            Some(position) => position,
            None => {
                let position = paths.len();
                paths.push(path);
                position
            }
        };
        parameter.projection = Some(
            u32::try_from(position)
                .map_err(|_| invalid("composed projection index exceeds u32"))?,
        );
        Ok(parameter)
    }

    fn target_parameter(
        &self,
        target: etas_effects::ParameterCallTarget,
        scope: CallableValueScope,
    ) -> Result<Option<CallableValueParameter>, PackageMetadataError> {
        match target {
            etas_effects::ParameterCallTarget::ExternalProjection(id) => {
                let projection = self
                    .projection
                    .checked
                    .effects
                    .external_parameter_projections
                    .get(&id)
                    .ok_or_else(|| invalid("exported external parameter selector is missing"))?;
                let Some(root) = self.target_parameter(projection.root, scope)? else {
                    return Ok(None);
                };
                self.with_projection(root, &projection.path, scope)
                    .map(Some)
            }
            etas_effects::ParameterCallTarget::Interface(position) => {
                Ok(Some(CallableValueParameter {
                    scope,
                    position,
                    projection: None,
                }))
            }
            etas_effects::ParameterCallTarget::Native(symbol) => self.parameter(symbol, scope),
            etas_effects::ParameterCallTarget::Projection(id) => {
                let projection = self
                    .projection
                    .checked
                    .effects
                    .parameter_projections
                    .get(&id)
                    .ok_or_else(|| {
                        invalid("native parameter projection has no checked source fact")
                    })?;
                let Some(parameter) = self.parameter(projection.parameter, scope)? else {
                    return Ok(None);
                };
                self.with_projection(parameter, &projection.path, scope)
                    .map(Some)
            }
            _ => Ok(None),
        }
    }

    fn parameter_label(&self, parameter: CallableValueParameter) -> String {
        let mut label = match parameter.scope {
            CallableValueScope::Root => format!("rp{}", parameter.position),
            CallableValueScope::Value(value) => format!("v{value}p{}", parameter.position),
        };
        if let Some(projection) = parameter.projection {
            let paths = self.parameter_projections.borrow();
            let path = &paths[projection as usize];
            let bytes = serde_json::to_vec(path).expect("checked projection path encoding");
            let identity = etas_code::CodeFingerprint::of_bytes(
                "etas.metadata.parameter-label-path.v1",
                &bytes,
            );
            label.push_str(&format!("q{identity}"));
        }
        label
    }

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
                    projection: None,
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

    fn unavailable_node(
        &self,
        index: u32,
        reason: &str,
    ) -> Result<Option<CallableValueNode>, PackageMetadataError> {
        if self.method_root {
            Err(invalid(&format!(
                "callable node {index} has unavailable analysis: {reason}"
            )))
        } else {
            Ok(None)
        }
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
        } else if owner == self.root && !self.method_root {
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
                etas_effects::facts::CapturedCallableAnalysis::Unavailable { reason } => {
                    return self.unavailable_node(
                        index,
                        &format!("captured callable analysis is unavailable: {reason}"),
                    );
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
                    Some(ItemSignature::Agent(signature)) => {
                        signature.callable().ok_or_else(|| {
                            invalid("published agent method has no checked callable signature")
                        })?
                    }
                    _ => {
                        return self.unavailable_node(
                            index,
                            "source item is not a flow, tool or agent callable",
                        );
                    }
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
                    return self
                        .unavailable_node(index, "anonymous flow does not refer to a lambda");
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
            _ => {
                return self
                    .unavailable_node(index, "effect unit has no callable value representation");
            }
        };
        self.parameters.insert(
            CallableValueScope::Value(index),
            parameter_symbols
                .iter()
                .enumerate()
                .map(|(position, symbol)| (*symbol, position as u32))
                .collect(),
        );
        // Unknown returned payload origins do not erase the callable's checked
        // behavior. Keep them unknown and retain independently known positions.
        let returned_sources = match sources(summary) {
            Some(returned) => self.sources(
                &returned,
                CallableValueScope::Value(index),
                instance.environment,
            )?,
            None => None,
        };
        let returned_projections = self.return_projections(
            summary,
            CallableValueScope::Value(index),
            instance.environment,
        )?;
        let trace_parameters = self.projection.callable_trace_parameters(summary)?;
        let mut parameter_bindings = vec![];
        let mut bound_parameters = BTreeSet::new();
        let mut trace_labels = BTreeMap::new();
        for target in trace_parameters {
            if matches!(
                target,
                etas_effects::ParameterCallTarget::SpecMethod { .. }
                    | etas_effects::ParameterCallTarget::MetadataSpecMethod { .. }
                    | etas_effects::ParameterCallTarget::AgentTools(_)
            ) {
                // These carry their checked receiver/member/generic interface
                // directly in the trace; they are not callable parameter labels.
                continue;
            }
            let Some(parameter) =
                self.target_parameter(target, CallableValueScope::Value(index))?
            else {
                return self.unavailable_node(
                    index,
                    &format!(
                        "trace parameter {target:?} has no exported lexical binding in {unit:?}"
                    ),
                );
            };
            if !self.scope_contains(CallableValueScope::Value(index), parameter.scope) {
                return self
                    .unavailable_node(index, "trace parameter escapes its exported lexical scope");
            }
            let label = self.parameter_label(parameter);
            trace_labels.insert(target, label.clone());
            if bound_parameters.insert(parameter) {
                parameter_bindings.push(etas_package_metadata::CallableValueParameterBinding {
                    label,
                    parameter,
                });
            }
        }
        let invocation_parameters =
            self.invocation_parameters(CallableValueScope::Value(index), &mut parameter_bindings);
        for (symbol, position) in &invocation_parameters {
            trace_labels
                .entry(etas_effects::ParameterCallTarget::Native(*symbol))
                .or_insert_with(|| parameter_bindings[*position as usize].label.clone());
        }
        let invocation_projection = self
            .projection
            .with_invocation_parameters(invocation_parameters);
        let Some(mut projected) = summary::project(
            &invocation_projection,
            summary,
            &type_names,
            &effect_names,
            &trace_labels,
        )?
        else {
            let reason = if !summary.residual_checks.is_empty() {
                "residual obligations need a checked callable representation"
            } else if summary.independent_determinism.is_none() {
                "independent determinism is unavailable"
            } else {
                "interpreter support is rejected"
            };
            return self.unavailable_node(index, reason);
        };
        projected.behavior.suspension =
            self.projection
                .checked
                .effects
                .suspension
                .get(&unit)
                .map(|summary| etas_code::descriptor::SuspensionInfo {
                    may_suspend: summary.may_suspend,
                    unknown: summary.unknown,
                });
        Ok(Some(CallableValueNode {
            residual_checks: summary.residual_checks.iter().cloned().collect(),
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
            returned_projections,
        }))
    }
}

fn sources(summary: &EffectSummary) -> Option<BTreeSet<CallableReturnSource>> {
    summary.exits.as_ref()?.callable_return_sources()
}

fn trace_parameters(trace: &ActionTraceDomain) -> BTreeSet<etas_effects::ParameterCallTarget> {
    trace
        .parameter_calls()
        .into_iter()
        .map(|call| call.target)
        .collect()
}

fn summary_parameters(summary: &EffectSummary) -> BTreeSet<etas_effects::ParameterCallTarget> {
    let mut targets = trace_parameters(&summary.action_trace);
    let mut requests = vec![&summary.handler_requests];
    if let Some(behavior) = &summary.default_behavior {
        requests.push(&behavior.direct.handler_requests);
        requests.extend(behavior.guarded.values().map(|part| &part.handler_requests));
    }
    for request in requests {
        if let etas_effects::RequestInstantiationSummary::Available { parameters, .. } = request {
            targets.extend(parameters.iter().map(|parameter| parameter.target));
        }
    }
    // Return origins have their own graph projection. Completion gaps belong
    // to descriptor outcome analysis, not this node's published input ABI.
    targets
}

mod behavior;
mod captured;
mod evidence;
mod foreign;
mod foreign_objects;
mod generics;
mod objects;
mod references;
mod sources;
mod summary;
pub(super) use behavior::behavior;

pub(super) fn invalid(reason: &str) -> PackageMetadataError {
    PackageMetadataError::InvalidMetadataType {
        reason: reason.into(),
    }
}

mod inputs;
