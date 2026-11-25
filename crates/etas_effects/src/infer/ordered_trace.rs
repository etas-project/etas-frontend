//! A loop header is an ordered equation reference throughout dataflow. Its
//! body and completion prefixes share that reference instead of copying the
//! preceding iterations. Linear equations become Repeat; other equations keep
//! their scoped recursive structure for the finite spec monitor.
use std::collections::BTreeMap;

use etas_code::CodeFingerprint;
use etas_hir::HirBlockId;

use crate::{ActionTraceDomain as Trace, CallableExitSummary, EffectSummary, EffectUnit};

type LoopKey = (Option<EffectUnit>, HirBlockId, Option<CodeFingerprint>);

#[derive(Default)]
pub(crate) struct LoopTraces {
    loops: BTreeMap<LoopKey, LoopTrace>,
}

struct LoopTrace {
    id: CodeFingerprint,
    entry: Trace,
    back: Option<Trace>,
    solution: std::cell::OnceCell<Trace>,
}

impl LoopTraces {
    pub(crate) fn begin(&mut self, key: LoopKey, entry: &Trace) -> Trace {
        let id = super::recursive_trace::identity(&("loop-header", key));
        self.loops.insert(
            key,
            LoopTrace {
                id,
                entry: entry.clone(),
                back: None,
                solution: Default::default(),
            },
        );
        Trace::Recursive { id, body: None }
    }

    pub(crate) fn join(&mut self, key: LoopKey, back: &Trace) -> Trace {
        let equation = self.loops.get_mut(&key).expect("entered loop equation");
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

    pub(crate) fn finish(&self, key: LoopKey, trace: &Trace) -> Trace {
        let equation = self.loops.get(&key).expect("entered loop equation");
        let solution = equation.solution.get_or_init(|| {
            let reference = Trace::Recursive {
                id: equation.id,
                body: None,
            };
            match &equation.back {
                None => equation.entry.clone(),
                Some(back) => match after_prefix(back, &reference) {
                    Some(iteration) => {
                        let mut trace = equation.entry.clone();
                        trace.seq_assign(iteration.repeat());
                        trace.shared()
                    }
                    None => {
                        let mut body = equation.entry.clone();
                        body.choice_assign(back.clone());
                        Trace::Recursive {
                            id: equation.id,
                            body: Some(Box::new(body)),
                        }
                    }
                },
            }
        });
        substitute(trace, equation.id, solution)
    }

    pub(crate) fn finish_summary(&self, key: LoopKey, summary: &mut EffectSummary) {
        let id = self.loops[&key].id;
        if references(&summary.action_trace, id) {
            summary.action_trace = self.finish(key, &summary.action_trace);
        }
        if let Some(exits) = &mut summary.exits {
            self.finish_exits(key, exits);
        }
    }

    pub(crate) fn finish_exits(&self, key: LoopKey, exits: &mut CallableExitSummary) {
        let id = self.loops[&key].id;
        for exit in &mut exits.exits {
            if references(&exit.trace, id) {
                exit.trace = self.finish(key, &exit.trace);
            }
        }
        for exit in &mut exits.parameter_exits {
            if references(&exit.prefix, id) {
                exit.prefix = self.finish(key, &exit.prefix);
            }
        }
    }
}

fn references(trace: &Trace, variable: CodeFingerprint) -> bool {
    let mut work = vec![trace];
    while let Some(trace) = work.pop() {
        match trace {
            Trace::Recursive { id, body: None } if *id == variable => return true,
            Trace::Recursive { id, body: Some(_) } if *id == variable => {}
            Trace::Recursive {
                body: Some(body), ..
            }
            | Trace::Repeat(body)
            | Trace::DefaultHandler { body, .. }
            | Trace::TaskStart { body, .. } => work.push(body),
            Trace::Seq(parts) | Trace::Choice(parts) | Trace::Parallel(parts) => work.extend(parts),
            _ => {}
        }
    }
    false
}

fn substitute(trace: &Trace, variable: CodeFingerprint, solution: &Trace) -> Trace {
    match trace {
        Trace::Recursive { id, body: None } if *id == variable => solution.clone(),
        Trace::Recursive { id, body: Some(_) } if *id == variable => trace.clone(),
        Trace::Shared(_) => trace.clone(),
        _ => trace.map_children(|child| substitute(child, variable, solution)),
    }
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
