use etas_hir::{HirExpr, HirExprId};

use super::Traversal;
use crate::intraprocedural::{Control, HirAnalysisSemantics};

enum Task<D> {
    Enter(HirExprId),
    Complete(HirExprId),
    Merge(Box<Control<D>>),
}

impl<S, F, E> Traversal<'_, S, F>
where
    S: HirAnalysisSemantics,
    F: Fn() -> Result<(), E>,
{
    /// The root's hooks belong to expr_control. Nested structural expressions
    /// keep their own control channels until their completion hook has run.
    pub(super) fn structural_expr_control(
        &mut self,
        root: HirExprId,
        state: S::Domain,
    ) -> Result<Control<S::Domain>, E> {
        (self.checkpoint)()?;
        let mut tasks = Vec::new();
        let mut control = Control::normal(state);
        let Some(operands) = children(&self.context().program().exprs[root]) else {
            return Ok(control.map_states(|state| self.semantics.incomplete_facts(state)));
        };
        tasks.extend(operands.into_iter().rev().map(Task::Enter));
        while let Some(task) = tasks.pop() {
            (self.checkpoint)()?;
            match task {
                Task::Enter(expr) => {
                    let Some(state) = control.take_normal_state() else {
                        continue;
                    };
                    let inherited = std::mem::take(&mut control);
                    match children(&self.context().program().exprs[expr]) {
                        Some(children) => {
                            control = Control::normal(self.semantics.before_expr(expr, state));
                            if !inherited.is_empty() {
                                tasks.push(Task::Merge(Box::new(inherited)));
                            }
                            tasks.push(Task::Complete(expr));
                            tasks.extend(children.into_iter().rev().map(Task::Enter));
                        }
                        None => {
                            control = self.expr_control(expr, state)?;
                            control.join_assign(&inherited);
                        }
                    }
                }
                Task::Complete(expr) => control = self.semantics.after_expr_control(expr, control),
                Task::Merge(inherited) => {
                    control.join_assign(&inherited);
                }
            }
        }
        Ok(control)
    }
}

fn children(expr: &HirExpr) -> Option<Vec<HirExprId>> {
    Some(match expr {
        HirExpr::ListCons { head, tail, .. } => vec![*head, *tail],
        HirExpr::Range { start, end, .. } => vec![*start, *end],
        HirExpr::Binary { lhs, rhs, .. } => vec![*lhs, *rhs],
        HirExpr::Field { base, .. } | HirExpr::Unary { expr: base, .. } => vec![*base],
        HirExpr::Index { base, index, .. } => vec![*base, *index],
        HirExpr::Slice {
            base, start, end, ..
        } => vec![*base, *start, *end],
        _ => return None,
    })
}
