//! Recursive functions denote finite systems of ordered trace equations.
//! Their least fixed points are interpreted in each spec's finite relation
//! domain, rather than expanded into an ever-growing sequence tree.
use std::collections::{BTreeMap, BTreeSet};

use etas_code::CodeFingerprint;

use crate::{ActionTraceDomain as Trace, EffectSummary, EffectUnit};

#[derive(Default)]
pub(crate) struct TraceEquations {
    active: BTreeSet<EffectUnit>,
    bodies: BTreeMap<CodeFingerprint, Trace>,
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
            Trace::Recursive { body: Some(_), .. } => trace.clone(),
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

    pub(crate) fn record(&mut self, id: CodeFingerprint, trace: &mut Trace) {
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

    pub(crate) fn close_summary(&self, summary: &mut EffectSummary) {
        self.close_trace(&mut summary.action_trace);
        if let Some(exits) = &mut summary.exits {
            for exit in &mut exits.exits {
                self.close_trace(&mut exit.trace);
            }
            for exit in &mut exits.parameter_exits {
                self.close_trace(&mut exit.prefix);
            }
        }
    }

    fn close_trace(&self, trace: &mut Trace) {
        if !self.bodies.is_empty()
            && trace
                .free_references()
                .iter()
                .any(|id| self.bodies.contains_key(id))
        {
            *trace = self.close(trace).shared();
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
