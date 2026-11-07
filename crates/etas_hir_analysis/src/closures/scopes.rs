use etas_hir::{HirExprId, ScopeId, ScopeOwner, ScopeTree};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClosureScope {
    pub scope: ScopeId,
    pub expr: HirExprId,
}

#[derive(Clone, Copy)]
struct Entry {
    start: usize,
    end: usize,
    nearest: Option<ClosureScope>,
    outer: Option<ClosureScope>,
}

/// Derived for one planning pass; ScopeTree remains the canonical binding graph.
/// DFS intervals answer lexical containment without walking non-closure scopes.
pub struct ClosureScopes {
    entries: Vec<Option<Entry>>,
}

enum Visit {
    Enter(ScopeId, Option<ClosureScope>),
    Exit(ScopeId),
}

impl ClosureScopes {
    pub fn build(scopes: &ScopeTree) -> Result<Self, String> {
        let count = scopes.iter().count();
        let mut children = vec![Vec::new(); count];
        let mut lambdas = vec![None; count];
        let mut pending = Vec::new();
        for scope in scopes.iter() {
            if let ScopeOwner::Lambda(expr) = scope.owner {
                *lambdas
                    .get_mut(scope.id.index())
                    .ok_or("invalid closure scope identity")? = Some(expr);
            }
            if let Some(parent) = scope.parent {
                children
                    .get_mut(parent.index())
                    .ok_or("missing closure scope parent")?
                    .push(scope.id);
            } else {
                pending.push(Visit::Enter(scope.id, None));
            }
        }
        let mut entries: Vec<Option<Entry>> = vec![None; count];
        let mut clock = 0;
        while let Some(visit) = pending.pop() {
            match visit {
                Visit::Enter(scope, outer) => {
                    let entry = entries
                        .get_mut(scope.index())
                        .ok_or("invalid closure scope identity")?;
                    if entry.is_some() {
                        return Err("repeated closure scope traversal".into());
                    }
                    let nearest = lambdas[scope.index()]
                        .map(|expr| ClosureScope { scope, expr })
                        .or(outer);
                    *entry = Some(Entry {
                        start: clock,
                        end: clock,
                        nearest,
                        outer,
                    });
                    clock += 1;
                    pending.push(Visit::Exit(scope));
                    pending.extend(
                        children[scope.index()]
                            .iter()
                            .rev()
                            .map(|child| Visit::Enter(*child, nearest)),
                    );
                }
                Visit::Exit(scope) => {
                    entries
                        .get_mut(scope.index())
                        .and_then(Option::as_mut)
                        .ok_or("missing entered closure scope")?
                        .end = clock;
                }
            }
        }
        // Every node has at most one parent. Unvisited nodes therefore belong to
        // a component with a parent cycle, not an additional valid tree root.
        if entries.iter().any(Option::is_none) {
            return Err("cyclic closure scope ancestry".into());
        }
        Ok(Self { entries })
    }

    fn entry(&self, scope: ScopeId) -> Result<&Entry, String> {
        self.entries
            .get(scope.index())
            .and_then(Option::as_ref)
            .ok_or_else(|| format!("missing closure scope {scope:?}"))
    }

    pub fn contains(&self, ancestor: ScopeId, scope: ScopeId) -> Result<bool, String> {
        let ancestor = self.entry(ancestor)?;
        let scope = self.entry(scope)?;
        Ok(ancestor.start <= scope.start && scope.start < ancestor.end)
    }

    pub fn nearest(&self, scope: ScopeId) -> Result<Option<ClosureScope>, String> {
        Ok(self.entry(scope)?.nearest)
    }

    pub fn outer(&self, closure: ClosureScope) -> Result<Option<ClosureScope>, String> {
        Ok(self.entry(closure.scope)?.outer)
    }
}
