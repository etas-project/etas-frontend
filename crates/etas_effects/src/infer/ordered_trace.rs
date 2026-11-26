//! A loop header is an ordered equation reference throughout dataflow. Its
//! body and completion prefixes share that reference instead of copying the
//! preceding iterations. Linear equations become Repeat; other equations keep
//! their scoped recursive structure for the finite spec monitor.
use std::{
    cell::{OnceCell, RefCell},
    collections::{HashMap, HashSet},
    sync::Arc,
};

use super::recursive_trace::TraceFact;
use etas_code::CodeFingerprint;
use etas_hir::HirBlockId;

use crate::facts::EffectMaterializationInputs;
use crate::{ActionTraceDomain as Trace, CallableExitSummary, EffectSummary, EffectUnit};

type LoopKey = (Option<EffectUnit>, HirBlockId, Option<CodeFingerprint>);

#[derive(Default)]
pub(crate) struct LoopTraces(Vec<LoopTrace>);

pub(crate) struct LoopTrace {
    id: CodeFingerprint,
    entry: Trace,
    back: Option<Trace>,
    solution: OnceCell<Trace>,
    facts: HashSet<TraceFact>,
    substitutions: RefCell<HashMap<usize, (Arc<super::domain::SharedActionTrace>, Trace)>>,
}

impl LoopTraces {
    pub(crate) fn begin(&mut self, key: LoopKey, entry: &Trace) -> Trace {
        let id = super::recursive_trace::identity(&("loop-header", key));
        self.0.push(LoopTrace {
            id,
            entry: entry.clone(),
            back: None,
            solution: OnceCell::new(),
            facts: HashSet::new(),
            substitutions: Default::default(),
        });
        Trace::Recursive { id, body: None }
    }

    pub(crate) fn join(&mut self, back: &Trace) -> Trace {
        let equation = self.0.last_mut().expect("entered loop equation");
        let changed = match &mut equation.back {
            Some(previous) => previous.choice_assign(back.clone()),
            None => {
                equation.back = Some(back.clone());
                true
            }
        };
        if changed {
            equation.solution.take();
        }
        Trace::Recursive {
            id: equation.id,
            body: None,
        }
    }

    pub(crate) fn facts_mut(&mut self) -> Option<&mut HashSet<TraceFact>> {
        self.0.last_mut().map(|equation| &mut equation.facts)
    }

    pub(crate) fn end(&mut self) -> LoopTrace {
        let equation = self.0.pop().expect("entered loop equation");
        // Facts written in an inner loop can still refer to an outer header.
        if let Some(parent) = self.facts_mut() {
            parent.extend(&equation.facts);
        }
        equation
    }
}

impl LoopTrace {
    pub(crate) fn finish_trace(&self, trace: &mut Trace) {
        if !references(trace, self.id) {
            return;
        }
        let solution = self.solution.get_or_init(|| {
            let reference = Trace::Recursive {
                id: self.id,
                body: None,
            };
            match &self.back {
                None => self.entry.clone(),
                Some(back) => match after_prefix(back, &reference) {
                    Some(iteration) => {
                        let mut trace = self.entry.clone();
                        trace.seq_assign(iteration.repeat());
                        trace.shared()
                    }
                    None => {
                        let mut body = self.entry.clone();
                        body.choice_assign(back.clone());
                        Trace::Recursive {
                            id: self.id,
                            body: Some(Box::new(body)),
                        }
                    }
                },
            }
        });
        *trace = self.substitute(trace, solution);
    }

    fn substitute(&self, trace: &Trace, solution: &Trace) -> Trace {
        match trace {
            Trace::Recursive { id, body: None } if *id == self.id => solution.clone(),
            Trace::Recursive { id, body: Some(_) } if *id == self.id => trace.clone(),
            Trace::Shared(shared) => {
                if !shared.free_references.contains(&self.id) {
                    return trace.clone();
                }
                let key = Arc::as_ptr(shared) as usize;
                if let Some((_, result)) = self.substitutions.borrow().get(&key) {
                    return result.clone();
                }
                let result = self.substitute(&shared.body, solution).shared();
                self.substitutions
                    .borrow_mut()
                    .insert(key, (shared.clone(), result.clone()));
                result
            }
            _ => trace
                .map_children(|child| self.substitute(child, solution))
                .shared(),
        }
    }

    pub(crate) fn finish_summary(&self, summary: &mut EffectSummary) {
        self.finish_trace(&mut summary.action_trace);
        if let Some(exits) = &mut summary.exits {
            self.finish_exits(exits);
        }
    }

    pub(crate) fn finish_exits(&self, exits: &mut CallableExitSummary) {
        for exit in &mut exits.exits {
            self.finish_trace(&mut exit.trace);
        }
        for exit in &mut exits.parameter_exits {
            self.finish_trace(&mut exit.prefix);
        }
    }

    pub(crate) fn finish_facts(&self, inputs: &mut EffectMaterializationInputs) {
        for fact in &self.facts {
            self.finish_summary(fact.summary_mut(inputs));
        }
    }
}

fn references(trace: &Trace, variable: CodeFingerprint) -> bool {
    trace.has_free_reference(|id| *id == variable)
}

/// An exact optimization for right-linear equations. Failure to factor is
/// represented by the recursive equation, never an unordered footprint.
fn after_prefix(trace: &Trace, prefix: &Trace) -> Option<Trace> {
    let trace = trace.structure();
    let prefix = prefix.structure();
    if prefix == &Trace::Empty {
        return Some(trace.clone());
    }
    if trace == prefix {
        return Some(Trace::Empty);
    }
    if let Trace::Choice(branches) = trace {
        let mut residual: Option<Trace> = None;
        for branch in branches {
            let branch = after_prefix(branch, prefix)?;
            match &mut residual {
                None => residual = Some(branch),
                Some(trace) => {
                    trace.choice_assign(branch);
                }
            }
        }
        return residual;
    }
    let Trace::Seq(parts) = trace else {
        return None;
    };
    let prefix = match prefix {
        Trace::Seq(parts) => parts.as_slice(),
        other => std::slice::from_ref(other),
    };
    let suffix = parts.strip_prefix(prefix)?;
    let mut result = Trace::Empty;
    for part in suffix {
        result.seq_assign(part.clone());
    }
    Some(result)
}
