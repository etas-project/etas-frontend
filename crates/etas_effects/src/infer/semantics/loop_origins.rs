//! Finite regular paths for loop-carried value dependencies. The loop header
//! keeps its initial origins and closes each type-preserving back-edge.
use super::{
    engine::EffectSemantics,
    state::{LatentEnv, LatentSource},
};
use etas_code::descriptor::ValueProjectionStep as Step;
use etas_hir::{HirItemId, SymbolId};
use etas_types::TypeId;
use std::collections::{BTreeMap, BTreeSet};

type Root = (HirItemId, SymbolId);
type Closure = (Root, Vec<Step>, Vec<Step>);

impl EffectSemantics<'_> {
    fn minimal_loop_alternatives(
        &self,
        ty: TypeId,
        alternatives: BTreeSet<Vec<Step>>,
    ) -> Option<BTreeSet<Vec<Step>>> {
        let mut candidates = BTreeSet::new();
        for path in alternatives {
            let mut primitive = path.clone();
            for length in 1..=path.len() {
                if path.len() % length == 0
                    && path.chunks(length).all(|part| part == &path[..length])
                    && self.project_value(ty, &path[..length]).ok()?? == ty
                {
                    primitive.truncate(length);
                    break;
                }
            }
            candidates.insert(primitive);
        }
        let mut candidates = candidates.into_iter().collect::<Vec<_>>();
        candidates.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
        let mut result = BTreeSet::<Vec<Step>>::new();
        for path in candidates {
            // A generator already expressible by shorter generators adds no
            // edge to their transitive closure (for example A A beside A).
            let mut reached = vec![false; path.len() + 1];
            reached[0] = true;
            for position in 0..path.len() {
                if reached[position] {
                    for word in &result {
                        if path[position..].starts_with(word) {
                            reached[position + word.len()] = true;
                        }
                    }
                }
            }
            if !reached[path.len()] {
                result.insert(path);
            }
        }
        Some(result)
    }

    fn origin_position(&self, source: LatentSource) -> Option<(Root, Vec<Step>)> {
        match source {
            LatentSource::Parameter { owner, symbol } => Some(((owner, symbol), Vec::new())),
            LatentSource::Projection(id) => self
                .parameter_projection(id)
                .map(|projection| ((projection.owner, projection.parameter), projection.path)),
            _ => None,
        }
    }

    fn loop_alternatives(&self, ty: TypeId, path: &[Step]) -> Option<BTreeSet<Vec<Step>>> {
        if path.is_empty() || self.project_value(ty, path).ok()?? != ty {
            return None;
        }
        if !path.iter().any(|step| matches!(step, Step::Repeat { .. })) {
            return self.minimal_loop_alternatives(ty, BTreeSet::from([path.to_vec()]));
        }
        let mut alternatives = BTreeSet::new();
        let mut run = Vec::new();
        for step in path {
            if let Step::Repeat {
                alternatives: repeated,
            } = step
            {
                if !run.is_empty() {
                    if self.project_value(ty, &run).ok()?? != ty {
                        return None;
                    }
                    alternatives.insert(std::mem::take(&mut run));
                }
                if self.project_value(ty, std::slice::from_ref(step)).ok()?? != ty {
                    return None;
                }
                alternatives.extend(repeated.iter().cloned());
            } else {
                run.push(step.clone());
            }
        }
        if !run.is_empty() {
            if self.project_value(ty, &run).ok()?? != ty {
                return None;
            }
            alternatives.insert(run);
        }
        self.minimal_loop_alternatives(ty, alternatives)
    }

    pub(super) fn close_origin_set(
        &self,
        previous: &BTreeSet<LatentSource>,
        current: &mut BTreeSet<LatentSource>,
    ) {
        let mut roots = previous
            .iter()
            .filter_map(|source| self.origin_position(*source))
            .collect::<Vec<_>>();
        roots.sort_by_key(|(_, path)| path.len());
        let mut closures = BTreeMap::<Closure, BTreeSet<Vec<Step>>>::new();
        let mut replaced = Vec::new();
        for source in current.difference(previous).copied() {
            let Some((root, path)) = self.origin_position(source) else {
                continue;
            };
            let Some(root_type) = self.symbol_value_type(root.1) else {
                continue;
            };
            'candidate: for (prior_root, prior) in &roots {
                if *prior_root != root || path.len() <= prior.len() {
                    continue;
                }
                for position in 0..=prior.len() {
                    let (prefix, suffix) = prior.split_at(position);
                    if !path.starts_with(prefix) || !path.ends_with(suffix) {
                        continue;
                    }
                    let inserted = &path[position..path.len() - suffix.len()];
                    let Some(base) = self.project_value(root_type, prefix).ok().flatten() else {
                        continue;
                    };
                    let Some(alternatives) = self.loop_alternatives(base, inserted) else {
                        continue;
                    };
                    closures
                        .entry((root, prefix.to_vec(), suffix.to_vec()))
                        .or_default()
                        .extend(alternatives);
                    replaced.push(source);
                    break 'candidate;
                }
            }
        }
        let mut closed = Vec::new();
        for ((root, mut prefix, suffix), alternatives) in closures {
            let Some(ty) = self
                .symbol_value_type(root.1)
                .and_then(|ty| self.project_value(ty, &prefix).ok().flatten())
            else {
                return;
            };
            let Some(alternatives) = self.minimal_loop_alternatives(ty, alternatives) else {
                return;
            };
            prefix.push(Step::Repeat {
                alternatives: alternatives.into_iter().collect(),
            });
            prefix.extend(suffix);
            if let Ok(id) = self.intern_parameter_projection(root.0, root.1, prefix) {
                closed.push(LatentSource::Projection(id));
            } else {
                // Leave original checked origins intact if specialization needs
                // a type that the materialization pass has not supplied yet.
                return;
            }
        }
        for source in replaced {
            current.remove(&source);
        }
        current.extend(closed);
    }

    pub(super) fn close_loop_origins(&self, previous: &LatentEnv, current: &mut LatentEnv) {
        for (key, sources) in &mut current.symbols {
            if let Some(prior) = previous.symbols.get(key) {
                self.close_origin_set(prior, sources);
            }
        }
        for (key, sources) in &mut current.values {
            if let Some(prior) = previous.values.get(key) {
                self.close_origin_set(prior, sources);
            }
        }
        for (key, sources) in &mut current.symbol_projections {
            if let Some(prior) = previous.symbol_projections.get(key) {
                self.close_origin_set(prior, sources);
            }
        }
        for (key, sources) in &mut current.value_projections {
            if let Some(prior) = previous.value_projections.get(key) {
                self.close_origin_set(prior, sources);
            }
        }
    }
}
