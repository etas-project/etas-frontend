use std::collections::HashMap;

use crate::{SpecKind, TraceSpecConformanceTarget, TypeFacts, TypeId};
use etas_hir::SymbolId;

#[derive(Clone, Copy)]
pub struct AgentMethodSpecBinding<'a> {
    pub spec: SymbolId,
    pub kind: SpecKind,
    pub args: &'a [TypeId],
}

/// One index over checked conformance facts, shared by reflection selection
/// and portable definition projection. It does not infer new satisfactions.
pub struct AgentMethodSpecIndex<'a> {
    bindings: HashMap<SymbolId, Vec<AgentMethodSpecBinding<'a>>>,
}

impl<'a> AgentMethodSpecIndex<'a> {
    pub fn new(facts: &'a TypeFacts) -> Self {
        let mut bindings = HashMap::<_, Vec<_>>::new();
        let mut bodies = HashMap::<_, Vec<_>>::new();
        for method in facts.agent_methods.values() {
            bodies.entry(method.body).or_default().push(method.symbol);
        }
        for implementation in &facts.spec_impls {
            for member in &implementation.methods {
                if facts
                    .agent_methods
                    .get(&member.symbol)
                    .is_some_and(|method| method.component == implementation.self_type)
                {
                    bindings
                        .entry(member.symbol)
                        .or_default()
                        .push(AgentMethodSpecBinding {
                            spec: implementation.spec_symbol,
                            kind: SpecKind::TypeSpec,
                            args: &implementation.args,
                        });
                }
            }
        }
        for fact in &facts.callable_spec_satisfactions {
            for symbol in bodies.get(&fact.item).into_iter().flatten() {
                bindings
                    .entry(*symbol)
                    .or_default()
                    .push(AgentMethodSpecBinding {
                        spec: fact.spec_symbol,
                        kind: SpecKind::CallableSpec,
                        args: &fact.args,
                    });
            }
        }
        for fact in &facts.trace_spec_conformances {
            if let TraceSpecConformanceTarget::Named { spec_symbol, args } = &fact.target {
                for symbol in bodies.get(&fact.item).into_iter().flatten() {
                    bindings
                        .entry(*symbol)
                        .or_default()
                        .push(AgentMethodSpecBinding {
                            spec: *spec_symbol,
                            kind: SpecKind::TraceSpec,
                            args,
                        });
                }
            }
        }
        Self { bindings }
    }
    pub fn bindings(&self, method: SymbolId) -> &[AgentMethodSpecBinding<'a>] {
        self.bindings.get(&method).map_or(&[], Vec::as_slice)
    }
}
