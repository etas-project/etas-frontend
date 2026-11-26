//! Recursive functions denote finite systems of ordered trace equations.
//! Their least fixed points are interpreted in each spec's finite relation
//! domain, rather than expanded into an ever-growing sequence tree.
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use etas_code::CodeFingerprint;

use crate::{ActionTraceDomain as Trace, EffectSummary, EffectUnit};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TraceFact {
    Expr(etas_hir::HirExprId),
    Stmt(etas_hir::HirStmtId),
    Parameter(etas_hir::SymbolId),
}

impl TraceFact {
    pub(crate) fn summary_mut(
        self,
        inputs: &mut crate::EffectMaterializationInputs,
    ) -> &mut EffectSummary {
        match self {
            Self::Expr(id) => inputs.expr_effects.get_mut(&id),
            Self::Stmt(id) => inputs.stmt_effects.get_mut(&id),
            Self::Parameter(id) => inputs.inferred_parameter_effects.get_mut(&id),
        }
        .expect("recorded trace fact")
    }
}

#[derive(Default)]
pub(crate) struct TraceEquations {
    active: BTreeSet<EffectUnit>,
    equation_owners: BTreeMap<CodeFingerprint, EffectUnit>,
    bodies: BTreeMap<CodeFingerprint, Trace>,
    pending_facts: HashSet<TraceFact>,
    waiting_facts: HashMap<CodeFingerprint, HashSet<TraceFact>>,
}

pub(crate) fn identity(value: &impl serde::Serialize) -> CodeFingerprint {
    let bytes = serde_json::to_vec(value).expect("checked trace equation identity serializes");
    CodeFingerprint::of_bytes("etas.effects.recursive-trace.v1", &bytes)
}

/// A recursive application with new type/callback bindings is a different
/// equation. Until that contextual equation is materialized, keep an explicit
/// unresolved edge at the call position; binding it to the template would omit
/// possible behavior (for example a recursive call that swaps two callbacks).
pub(crate) fn defer_context(summary: &mut EffectSummary, call: etas_hir::HirExprId) {
    fn map(trace: &Trace, call: etas_hir::HirExprId) -> Trace {
        match trace {
            Trace::Recursive { id, body: None } => Trace::Recursive {
                id: identity(&(id, "application", call)),
                body: None,
            },
            Trace::Recursive { body: Some(_), .. } | Trace::Shared(_) => trace.clone(),
            _ => trace.map_children(|child| map(child, call)),
        }
    }
    summary.action_trace = map(&summary.action_trace, call);
    if let Some(exits) = &mut summary.exits {
        for exit in &mut exits.exits {
            exit.trace = map(&exit.trace, call);
        }
        for exit in &mut exits.parameter_exits {
            exit.prefix = map(&exit.prefix, call);
        }
    }
}

impl TraceEquations {
    pub(crate) fn reuse_closed_equations(
        &mut self,
        mut old: Self,
        dirty: &HashSet<etas_hir::HirItemId>,
    ) {
        old.equation_owners
            .retain(|_, unit| unit.owner().is_some_and(|i| !dirty.contains(&i)));
        old.bodies
            .retain(|key, _| old.equation_owners.contains_key(key));
        self.bodies = old.bodies;
        self.equation_owners = old.equation_owners;
    }

    pub(crate) fn has_active_component(&self) -> bool {
        !self.active.is_empty()
    }

    pub(crate) fn contains(&self, unit: EffectUnit) -> bool {
        self.active.contains(&unit)
    }
    pub(crate) fn begin(&mut self, units: &[EffectUnit], cyclic: bool) {
        self.active.clear();
        if cyclic {
            self.active.extend(units);
        }
    }

    pub(crate) fn record_fact(&mut self, fact: TraceFact) {
        self.pending_facts.insert(fact);
    }

    pub(crate) fn record(&mut self, id: CodeFingerprint, trace: &mut Trace) {
        if let Some(waiting) = self.waiting_facts.remove(&id) {
            self.pending_facts.extend(waiting);
        }
        if let Some(unit) = self.active.first() {
            self.equation_owners.insert(id, *unit);
        }
        self.bodies.insert(id, std::mem::take(trace));
        *trace = Trace::Recursive { id, body: None };
    }

    pub(crate) fn summarize(&mut self, unit: EffectUnit, summary: &mut EffectSummary) {
        if !self.active.contains(&unit) {
            return;
        }
        self.summarize_with_id(identity(&unit), summary);
    }

    pub(crate) fn summarize_trace(&mut self, unit: EffectUnit, trace: &mut Trace) {
        if self.active.contains(&unit) {
            self.record(identity(&(unit, "spec-actions")), trace);
        }
    }

    pub(crate) fn summarize_with_id(
        &mut self,
        owner: CodeFingerprint,
        summary: &mut EffectSummary,
    ) {
        self.record(identity(&(owner, "actions")), &mut summary.action_trace);
        if let Some(exits) = &mut summary.exits {
            for exit in &mut exits.exits {
                self.record(identity(&(owner, "exit", &exit.outcome)), &mut exit.trace);
            }
            for exit in &mut exits.parameter_exits {
                self.record(
                    identity(&(
                        owner,
                        "prefix",
                        exit.target,
                        exit.output,
                        &exit.source,
                        &exit.arguments,
                    )),
                    &mut exit.prefix,
                );
            }
        }
    }

    pub(crate) fn close_facts(&mut self, inputs: &mut crate::EffectMaterializationInputs) {
        let mut pending = std::mem::take(&mut self.pending_facts);
        let mut missing = HashSet::new();
        for fact in pending.drain() {
            self.close_summary_with(fact.summary_mut(inputs), |id| {
                missing.insert(id);
            });
            for id in missing.drain() {
                self.waiting_facts.entry(id).or_default().insert(fact);
            }
        }
        self.pending_facts = pending;
    }

    pub(crate) fn close_summary(&self, summary: &mut EffectSummary) {
        self.close_summary_with(summary, |_| {});
    }

    fn close_summary_with(
        &self,
        summary: &mut EffectSummary,
        mut missing: impl FnMut(CodeFingerprint),
    ) {
        let mut close_trace = |trace: &mut Trace| {
            if trace.has_free_reference(|id| {
                if self.bodies.contains_key(id) {
                    return true;
                }
                missing(*id);
                false
            }) {
                *trace = self.close(trace).shared();
                // Expanding this equation may expose a different forward reference.
                trace.has_free_reference(|id| {
                    missing(*id);
                    false
                });
            }
        };
        close_trace(&mut summary.action_trace);
        if let Some(exits) = &mut summary.exits {
            for exit in &mut exits.exits {
                close_trace(&mut exit.trace);
            }
            for exit in &mut exits.parameter_exits {
                close_trace(&mut exit.prefix);
            }
        }
    }

    pub(crate) fn close(&self, trace: &Trace) -> Trace {
        if self.bodies.is_empty() {
            return trace.clone();
        }
        self.close_in(trace, &mut BTreeSet::new())
    }

    fn close_in(&self, trace: &Trace, bound: &mut BTreeSet<CodeFingerprint>) -> Trace {
        match trace {
            Trace::Shared(_) => trace.clone(),
            Trace::Recursive { id, body: None } if !bound.contains(id) => {
                let Some(body) = self.bodies.get(id) else {
                    // A dangling equation remains explicit and unprovable.
                    return trace.clone();
                };
                bound.insert(*id);
                let body = self.close_in(body, bound);
                bound.remove(id);
                Trace::Recursive {
                    id: *id,
                    body: Some(Box::new(body)),
                }
            }
            Trace::Recursive {
                id,
                body: Some(body),
            } => {
                let fresh = bound.insert(*id);
                let body = self.close_in(body, bound);
                if fresh {
                    bound.remove(id);
                }
                Trace::Recursive {
                    id: *id,
                    body: Some(Box::new(body)),
                }
            }
            _ => trace.map_children(|child| self.close_in(child, bound)),
        }
    }
}

impl Trace {
    pub(crate) fn map_children(&self, mut map: impl FnMut(&Trace) -> Trace) -> Trace {
        match self {
            Trace::Seq(parts) => Trace::Seq(parts.iter().map(map).collect()),
            Trace::Choice(parts) => Trace::Choice(parts.iter().map(map).collect()),
            Trace::Parallel(parts) => Trace::Parallel(parts.iter().map(map).collect()),
            Trace::Repeat(body) => Trace::Repeat(Box::new(map(body))),
            Trace::Shared(body) => map(&body.body).shared(),
            Trace::Recursive { id, body } => Trace::Recursive {
                id: *id,
                body: body.as_ref().map(|body| Box::new(map(body))),
            },
            Trace::DefaultHandler { guards, body } => Trace::DefaultHandler {
                guards: guards.clone(),
                body: Box::new(map(body)),
            },
            Trace::TaskStart { task_site, body } => Trace::TaskStart {
                task_site: *task_site,
                body: Box::new(map(body)),
            },
            leaf => leaf.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fact_follows_a_second_equation_exposed_by_the_first() {
        let mut equations = TraceEquations::default();
        let mut inputs = crate::EffectMaterializationInputs::default();
        let expr = etas_hir::HirExprId(1);
        let caller = identity(&"caller");
        let callee = identity(&"callee");
        let reference = |id| Trace::Recursive { id, body: None };
        inputs.expr_effects.entry(expr).or_default().action_trace = reference(caller);
        equations.record_fact(TraceFact::Expr(expr));
        equations.close_facts(&mut inputs);

        equations.record(caller, &mut reference(callee));
        equations.close_facts(&mut inputs);
        let action = Trace::TaskAwait { task_site: expr };
        equations.record(callee, &mut action.clone());
        equations.close_facts(&mut inputs);

        let equation = |id, body| Trace::Recursive {
            id,
            body: Some(Box::new(body)),
        };
        assert_eq!(
            inputs.expr_effects[&expr].action_trace.structure(),
            &equation(caller, equation(callee, action)),
        );
    }
}
