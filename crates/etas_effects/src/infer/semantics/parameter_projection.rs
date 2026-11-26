use crate::infer::analysis::dependencies::InputFact;
use std::collections::{BTreeMap, BTreeSet};

use etas_code::{CodeFingerprint, descriptor::ValueProjectionStep as Step};
use etas_hir::{HirItemId, SymbolDef, SymbolId};

use super::{engine::EffectSemantics, specialize::EffectSpecializationError, state::LatentSource};
use crate::facts::NativeParameterProjection;

#[derive(Default)]
pub(super) struct NativeParameterProjectionIndex {
    pub(super) nodes: usize,
    pub(super) call_inputs: BTreeMap<u32, std::sync::Arc<crate::facts::NativeCallInputs>>,
    pub(super) input_ids: BTreeMap<CodeFingerprint, u32>,
    pub(super) joined_inputs: BTreeMap<
        (
            Option<crate::EffectUnit>,
            Option<CodeFingerprint>,
            etas_hir::HirExprId,
        ),
        u32,
    >,
    pub(super) input_users: BTreeMap<u32, BTreeSet<CodeFingerprint>>,
}

impl NativeParameterProjectionIndex {
    pub(super) fn new(projections: &BTreeMap<CodeFingerprint, NativeParameterProjection>) -> Self {
        let mut index = Self::default();
        for (projection_id, projection) in projections {
            index.insert(Step::nodes(&projection.path));
            for (id, inputs) in &projection.call_inputs {
                if index.call_inputs.insert(*id, inputs.clone()).is_none() {
                    index.insert(inputs.nodes());
                }
                index
                    .input_users
                    .entry(*id)
                    .or_default()
                    .insert(*projection_id);
                if inputs.joined {
                    index
                        .joined_inputs
                        .insert((inputs.scope, inputs.context, inputs.call), *id);
                } else {
                    let bytes = serde_json::to_vec(inputs).expect("checked call inputs serialize");
                    index.input_ids.insert(
                        CodeFingerprint::of_bytes("etas.effects.call-inputs.v1", &bytes),
                        *id,
                    );
                }
            }
        }
        index
    }

    pub(super) fn insert(&mut self, path_length: usize) {
        self.nodes = self.nodes.saturating_add(1 + path_length);
    }
}

impl EffectSemantics<'_> {
    pub(crate) fn parameter_projection_target(
        &self,
        target: crate::ParameterCallTarget,
    ) -> Option<(crate::ParameterCallTarget, Vec<Step>)> {
        match target {
            crate::ParameterCallTarget::ExternalProjection(id) => {
                self.dependencies.read(InputFact::ExternalProjection(id));
                let projections = self.external_parameter_projections.borrow();
                let projection = projections.get(&id)?;
                Some((projection.root, projection.path.clone()))
            }
            crate::ParameterCallTarget::Projection(id) => {
                let projection = self.parameter_projection(id)?;
                Some((
                    crate::ParameterCallTarget::Native(projection.parameter),
                    projection.path,
                ))
            }
            crate::ParameterCallTarget::Graph {
                graph,
                mut parameter,
            } if parameter.projection.is_some() => {
                let (metadata, _, _, _) = self.callable_graph(graph)?;
                let path = metadata.projection_path(parameter).ok()?.to_vec();
                parameter.projection = None;
                Some((crate::ParameterCallTarget::Graph { graph, parameter }, path))
            }
            _ => None,
        }
    }

    pub(crate) fn intern_external_parameter_projection(
        &self,
        mut root: crate::ParameterCallTarget,
        mut path: Vec<Step>,
    ) -> Option<crate::ParameterCallTarget> {
        if let Some((base, mut prefix)) = self.parameter_projection_target(root) {
            prefix.extend(path);
            path = prefix;
            root = base;
        }
        if path.is_empty()
            || !Step::well_formed(
                &path,
                etas_package_metadata::MAX_METADATA_GRAPH_DEPTH,
                etas_package_metadata::MAX_METADATA_GRAPH_NODES,
            )
            || matches!(
                root,
                crate::ParameterCallTarget::Projection(_)
                    | crate::ParameterCallTarget::ExternalProjection(_)
            )
        {
            return None;
        }
        let bytes = serde_json::to_vec(&(root, &path)).ok()?;
        let id = CodeFingerprint::of_bytes("etas.effects.external-parameter-projection.v1", &bytes);
        self.dependencies.read(InputFact::ExternalProjection(id));
        let mut projections = self.external_parameter_projections.borrow_mut();
        if !projections.contains_key(&id) {
            let nodes = projections
                .values()
                .map(|entry| 1 + Step::nodes(&entry.path))
                .sum::<usize>();
            if nodes.saturating_add(1 + Step::nodes(&path))
                > etas_package_metadata::MAX_METADATA_GRAPH_NODES
            {
                return None;
            }
            std::sync::Arc::make_mut(&mut projections)
                .insert(id, crate::facts::ExternalParameterProjection { root, path });
        }
        Some(crate::ParameterCallTarget::ExternalProjection(id))
    }

    pub(crate) fn parameter_projection(
        &self,
        id: CodeFingerprint,
    ) -> Option<NativeParameterProjection> {
        self.dependencies.read(InputFact::Projection(id));
        self.parameter_projections.borrow().get(&id).cloned()
    }

    pub(crate) fn project_parameter_origins(
        &self,
        sources: impl IntoIterator<Item = LatentSource>,
        path: &[Step],
    ) -> Option<BTreeSet<LatentSource>> {
        let mut result = BTreeSet::new();
        for source in sources {
            if path.is_empty() {
                result.insert(source);
                continue;
            }
            let (owner, parameter, mut full_path) = match source {
                LatentSource::Parameter { owner, symbol } => (owner, symbol, Vec::new()),
                LatentSource::Projection(id) => {
                    let projection = self.parameter_projection(id)?;
                    (projection.owner, projection.parameter, projection.path)
                }
                _ => return None,
            };
            full_path.extend_from_slice(path);
            result.insert(LatentSource::Projection(
                self.intern_parameter_projection(owner, parameter, full_path)
                    .ok()?,
            ));
        }
        Some(result)
    }

    pub(crate) fn intern_parameter_projection(
        &self,
        owner: HirItemId,
        parameter: SymbolId,
        mut path: Vec<Step>,
    ) -> Result<CodeFingerprint, EffectSpecializationError> {
        let mut index = 0;
        while index < path.len() {
            if let Step::Repeat { alternatives } = &path[index] {
                if let Some(length) = alternatives.iter().find_map(|alternative| {
                    path[index + 1..]
                        .starts_with(alternative)
                        .then_some(alternative.len())
                }) {
                    path.drain(index + 1..index + 1 + length);
                    continue;
                }
                if path.get(index + 1) == Some(&path[index]) {
                    path.remove(index + 1);
                    continue;
                }
            }
            index += 1;
        }
        let invalid = || EffectSpecializationError::InvalidCallableEnvironment {
            reason: format!(
                "parameter projection has no checked value contract: owner {owner:?}, parameter {parameter:?}, type {:?}, path {path:?}",
                self.symbol_value_type(parameter)
            ),
        };
        if !matches!(self.hir.symbols.get(parameter).map(|data| &data.def), Some(SymbolDef::Param { owner: actual, .. }) if *actual == owner)
            || path.is_empty()
            || !Step::well_formed(
                &path,
                etas_package_metadata::MAX_METADATA_GRAPH_DEPTH,
                etas_package_metadata::MAX_METADATA_GRAPH_NODES,
            )
        {
            return Err(invalid());
        }
        let mut identities = self.type_queries.parameter_projection_ids.borrow_mut();
        let paths = identities.entry((owner, parameter)).or_default();
        let id = match paths.get(&path) {
            Some(id) => {
                self.dependencies.read(InputFact::Projection(*id));
                return Ok(*id);
            }
            None => NativeParameterProjection::path_identity(owner, parameter, &path)
                .map_err(|_| invalid())?,
        };
        self.dependencies.read(InputFact::Projection(id));
        if self.parameter_projections.borrow().contains_key(&id) {
            paths.insert(path, id);
            return Ok(id);
        }
        let ty = match self.agent_receiver_parameter(owner) {
            Some((receiver, layout)) if parameter == receiver => layout,
            _ => self.symbol_value_type(parameter).ok_or_else(invalid)?,
        };
        let ty = self
            .project_value(ty, &path)
            .map_err(|_| invalid())?
            .ok_or_else(invalid)?;
        // Session-local IDs are retained only with their checked HIR revision.
        // Public metadata exports the scoped root and path, never this ID.
        let mut index = self.parameter_projection_index.borrow_mut();
        if index.nodes.saturating_add(1 + Step::nodes(&path))
            > etas_package_metadata::MAX_METADATA_GRAPH_NODES
        {
            return Err(EffectSpecializationError::InvalidCallableEnvironment {
                reason: format!(
                    "parameter projection graph exceeds compiler node budget ({} retained nodes, path length {})",
                    index.nodes,
                    path.len()
                ),
            });
        }
        index.insert(Step::nodes(&path));
        let call_inputs = Step::call_inputs(&path)
            .into_iter()
            .map(|id| {
                index
                    .call_inputs
                    .get(&id)
                    .cloned()
                    .map(|inputs| (id, inputs))
                    .ok_or_else(invalid)
            })
            .collect::<Result<_, _>>()?;
        for input in Step::call_inputs(&path) {
            index.input_users.entry(input).or_default().insert(id);
        }
        paths.insert(path.clone(), id);
        std::sync::Arc::make_mut(&mut self.parameter_projections.borrow_mut()).insert(
            id,
            NativeParameterProjection {
                owner,
                parameter,
                path,
                ty,
                call_inputs,
            },
        );
        Ok(id)
    }
}
