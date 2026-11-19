use std::collections::BTreeSet;

use etas_code::{CodeFingerprint, descriptor::ValueProjectionStep as Step};
use etas_hir::{HirItemId, SymbolDef, SymbolId};

use super::{engine::EffectSemantics, specialize::EffectSpecializationError, state::LatentSource};
use crate::facts::NativeParameterProjection;

impl EffectSemantics<'_> {
    pub(crate) fn parameter_projection_target(
        &self,
        target: crate::ParameterCallTarget,
    ) -> Option<(crate::ParameterCallTarget, Vec<Step>)> {
        match target {
            crate::ParameterCallTarget::ExternalProjection(id) => {
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
            || path.len() > etas_package_metadata::MAX_METADATA_GRAPH_DEPTH
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
        let mut projections = self.external_parameter_projections.borrow_mut();
        if !projections.contains_key(&id) {
            let nodes = projections
                .values()
                .map(|entry| 1 + entry.path.len())
                .sum::<usize>();
            if nodes.saturating_add(1 + path.len())
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
        path: Vec<Step>,
    ) -> Result<CodeFingerprint, EffectSpecializationError> {
        let invalid = || EffectSpecializationError::InvalidCallableEnvironment {
            reason: "parameter projection has no checked value contract".into(),
        };
        if !matches!(self.hir.symbols.get(parameter).map(|data| &data.def), Some(SymbolDef::Param { owner: actual, .. }) if *actual == owner)
            || path.is_empty()
            || path.len() > etas_package_metadata::MAX_METADATA_GRAPH_DEPTH
        {
            return Err(invalid());
        }
        let bytes = serde_json::to_vec(&(owner, parameter, &path)).map_err(|_| invalid())?;
        let id = CodeFingerprint::of_bytes("etas.effects.parameter-projection.v1", &bytes);
        if self.parameter_projections.borrow().contains_key(&id) {
            return Ok(id);
        }
        let ty = self.symbol_value_type(parameter).ok_or_else(invalid)?;
        let lookup = self
            .projection_type_lookup
            .get_or_init(|| etas_types::ty::TypeLookupIndex::new(&self.types.store));
        let ty = etas_types::value_projection::project_indexed(
            &self.types.store,
            &self.types.facts,
            lookup,
            ty,
            &path,
        )
        .map_err(|_| invalid())?
        .ok_or_else(invalid)?;
        // Session-local IDs are retained only with their checked HIR revision.
        // Public metadata exports the scoped root and path, never this ID.
        let mut projections = self.parameter_projections.borrow_mut();
        if !projections.contains_key(&id) {
            let used = projections
                .values()
                .map(|entry| 1 + entry.path.len())
                .sum::<usize>();
            if used.saturating_add(1 + path.len()) > etas_package_metadata::MAX_METADATA_GRAPH_NODES
            {
                return Err(invalid());
            }
            std::sync::Arc::make_mut(&mut projections).insert(
                id,
                NativeParameterProjection {
                    owner,
                    parameter,
                    path,
                    ty,
                },
            );
        }
        Ok(id)
    }
}
