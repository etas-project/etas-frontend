//! Inputs actually consulted by each item's effect transfers. The scheduling
//! unit includes its handler arms, parameter defaults and anonymous flows,
//! which share expression facts and lexical captures.
use super::EffectAnalysisInput;
use crate::{EffectUnit, facts::CallableEnvironmentId};
use etas_hir::{HirExprId, HirItemId, SymbolId};
use etas_hir_analysis::interprocedural::CallGraph;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum InputFact {
    Owner(HirItemId),
    BodyFacts(HirItemId),
    Parameter(SymbolId),
    Global(SymbolId),
    GlobalProjections(SymbolId),
    Value(HirExprId),
    ValueProjections(HirExprId),
    Projection(etas_code::CodeFingerprint),
    ExternalProjection(etas_code::CodeFingerprint),
    CallInput(u32),
    MethodEvidence,
}

#[derive(Default)]
pub(crate) struct RoundDependencies {
    current: Option<HirItemId>,
    /// Environments are published during a sweep. Only a reader which saw an
    /// earlier version needs another transfer when a writer refines the frame.
    frame_readers: RefCell<HashMap<CallableEnvironmentId, HashSet<HirItemId>>>,
    changed_frames: RefCell<HashSet<HirItemId>>,
    reads: RefCell<HashMap<HirItemId, HashSet<InputFact>>>,
    /// A diagnostic may be anchored in a callee even though it was emitted
    /// when applying that callee at this caller. Revisit its actual producer.
    revisit: RefCell<HashSet<HirItemId>>,
    message_counts: (usize, usize, usize),
}

impl RoundDependencies {
    pub(crate) fn read(&self, fact: InputFact) {
        if let Some(item) = self.current {
            if !matches!(fact, InputFact::Owner(owner) | InputFact::BodyFacts(owner) if owner == item)
            {
                self.reads
                    .borrow_mut()
                    .entry(item)
                    .or_default()
                    .insert(fact);
            }
        }
    }

    pub(crate) fn read_frame(&self, id: CallableEnvironmentId) {
        if let Some(owner) = self.current {
            self.frame_readers
                .borrow_mut()
                .entry(id)
                .or_default()
                .insert(owner);
        }
    }

    pub(crate) fn wrote_frame(&self, id: CallableEnvironmentId) {
        if let Some(readers) = self.frame_readers.borrow_mut().remove(&id) {
            self.changed_frames.borrow_mut().extend(readers);
        }
        self.read_frame(id);
    }

    pub(crate) fn revisit_current(&self) {
        if let Some(item) = self.current {
            self.revisit.borrow_mut().insert(item);
        }
    }

    pub(crate) fn select(&mut self, item: Option<HirItemId>, counts: (usize, usize, usize)) {
        if counts != self.message_counts {
            self.revisit_current();
            self.message_counts = counts;
        }
        self.current = item;
    }

    pub(crate) fn invalidated(
        &self,
        old: &EffectAnalysisInput<'_>,
        new: &EffectAnalysisInput<'_>,
        graph: &CallGraph<EffectUnit>,
        previous_dirty: &HashSet<HirItemId>,
        changed_summaries: &HashSet<HirItemId>,
        components: &[etas_hir_analysis::interprocedural::ComponentConvergence<EffectUnit>],
    ) -> (HashSet<HirItemId>, bool) {
        let mut changed = HashSet::new();
        let mut dirty = self.revisit.borrow().clone();
        macro_rules! changed_entries {
            ($old:expr, $new:expr, |$binding:pat_param| $fact:expr) => {{
                let before = &$old;
                let after = &$new;
                for (key, value) in after.iter() {
                    if before.get(key) != Some(value) {
                        let $binding = key;
                        changed.insert($fact);
                    }
                }
                for key in before.keys().filter(|key| !after.contains_key(*key)) {
                    let $binding = key;
                    changed.insert($fact);
                }
            }};
        }
        changed_entries!(old.parameter_effects, new.parameter_effects, |s| {
            InputFact::Parameter(*s)
        });
        dirty.extend(self.changed_frames.borrow().iter().copied());
        changed_entries!(old.global_values.symbols, new.global_values.symbols, |s| {
            InputFact::Global(*s)
        });
        changed_entries!(
            old.global_values.symbol_projections,
            new.global_values.symbol_projections,
            |(s, _)| InputFact::GlobalProjections(*s)
        );
        changed_entries!(old.call_results.values, new.call_results.values, |id| {
            InputFact::Value(*id)
        });
        changed_entries!(
            old.call_results.value_projections,
            new.call_results.value_projections,
            |(id, _)| InputFact::ValueProjections(*id)
        );
        changed_entries!(old.parameter_projections, new.parameter_projections, |id| {
            InputFact::Projection(*id)
        });
        changed_entries!(
            old.external_parameter_projections,
            new.external_parameter_projections,
            |id| InputFact::ExternalProjection(*id)
        );
        // The next round restores recipes reachable from a retained projection.
        // An unreferenced recipe has a sweep-local ID and must be interned again
        // by its producers before a cached trace can refer to it.
        let recipes = |input: &EffectAnalysisInput<'_>| {
            input
                .parameter_projections
                .values()
                .flat_map(|p| p.call_inputs.iter())
                .map(|(id, value)| (*id, value.clone()))
                .collect::<HashMap<_, _>>()
        };
        let old_recipes = recipes(old);
        let new_recipes = recipes(new);
        changed_entries!(old_recipes, new_recipes, |id| InputFact::CallInput(*id));
        if old.method_evidence != new.method_evidence {
            changed.insert(InputFact::MethodEvidence);
        }
        let mut topology = HashSet::new();
        for (unit, captures) in new.captured_values.iter() {
            let previous = old.captured_values.get(unit);
            if previous == Some(captures) {
                continue;
            }
            dirty.extend(unit.owner());
            // Capture contents feed transfers; only captured callable values can
            // add a call-graph edge. Empty frames do not merge components.
            if captures.values.values().any(|sources| {
                sources.iter().any(|source| {
                    source
                        .captured_call_target()
                        .is_some_and(|target| !graph.has_call(*unit, target))
                })
            }) {
                topology.extend(unit.owner());
            }
        }
        for (owner, targets) in new.spec_unit_targets.iter() {
            if old.spec_unit_targets.get(owner) != Some(targets) {
                topology.insert(*owner);
            }
        }
        for (expr, targets) in new.spec_method_targets.iter() {
            if old.spec_method_targets.get(expr) != Some(targets) {
                topology.extend(new.hir.index().expr_item.get(expr));
            }
        }

        let mut callers = HashMap::<HirItemId, Vec<HirItemId>>::new();
        let mut notifications = HashSet::new();
        let mut peers = HashMap::<HirItemId, Vec<HirItemId>>::new();
        for (owner, reads) in self.reads.borrow().iter() {
            if !changed.is_disjoint(reads) || reads.iter().any(|fact| matches!(fact, InputFact::CallInput(id) if !new_recipes.contains_key(id))) {
                dirty.insert(*owner);
            }
            for fact in reads {
                if let InputFact::Owner(dependency) | InputFact::BodyFacts(dependency) = fact {
                    callers.entry(*dependency).or_default().push(*owner);
                    if matches!(fact, InputFact::BodyFacts(_)) {
                        // These readers consult in-sweep expression or handler facts in
                        // addition to the public summary. Refresh both owners together.
                        peers.entry(*dependency).or_default().push(*owner);
                        peers.entry(*owner).or_default().push(*dependency);
                    } else if changed_summaries.contains(dependency)
                        && !previous_dirty.contains(owner)
                    {
                        notifications.insert(*owner);
                    }
                }
            }
        }
        for unit in graph.units() {
            if let Some(owner) = unit.owner() {
                let dependents = graph.dependents(unit);
                if changed_summaries.contains(&owner) {
                    notifications.extend(
                        dependents
                            .iter()
                            .filter_map(|u| u.owner())
                            .filter(|caller| !previous_dirty.contains(caller)),
                    );
                }
                callers
                    .entry(owner)
                    .or_default()
                    .extend(dependents.into_iter().filter_map(EffectUnit::owner));
            }
        }
        let pending_summaries = !notifications.is_empty();
        dirty.extend(notifications);
        // New dispatch edges can merge previously separate components.
        let mut pending: Vec<_> = topology.iter().copied().collect();
        while let Some(item) = pending.pop() {
            for caller in callers.get(&item).into_iter().flatten() {
                if topology.insert(*caller) {
                    pending.push(*caller);
                }
            }
        }
        dirty.extend(topology);
        // Lexically grouped units can belong to several recursive components.
        for component in components.iter().filter(|c| c.units.len() > 1) {
            let owners: Vec<_> = component.units.iter().filter_map(|u| u.owner()).collect();
            if let Some(first) = owners.first() {
                for owner in &owners[1..] {
                    peers.entry(*first).or_default().push(*owner);
                    peers.entry(*owner).or_default().push(*first);
                }
            }
        }
        let mut pending: Vec<_> = dirty.iter().copied().collect();
        while let Some(owner) = pending.pop() {
            for peer in peers.get(&owner).into_iter().flatten() {
                if dirty.insert(*peer) {
                    pending.push(*peer);
                }
            }
        }
        (dirty, pending_summaries)
    }

    pub(crate) fn retain(&mut self, dirty: &HashSet<HirItemId>) {
        self.reads.get_mut().retain(|item, _| !dirty.contains(item));
        self.frame_readers.get_mut().retain(|_, readers| {
            readers.retain(|owner| !dirty.contains(owner));
            !readers.is_empty()
        });
        self.changed_frames.get_mut().clear();
        self.revisit.get_mut().clear();
        self.message_counts = (0, 0, 0);
        self.current = None;
    }
}
