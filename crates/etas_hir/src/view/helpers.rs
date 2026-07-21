use etas_core::Span;

use crate::{
    HirArg, HirElseBranch, HirExpr, HirExprId, HirFieldInit, HirItem, HirMapEntry, HirMatchArm,
    HirPat, HirProgram, HirRecordPatField, HirStage, HirStmt, HirStmtId, ScopeId,
};

use super::model::{HirBodyKind, HirBodyRef, HirTreeChild};

pub(super) struct HirTreeChildren<'hir> {
    state: HirTreeChildIterState<'hir>,
}

enum HirTreeChildIterState<'hir> {
    Empty,
    Fixed {
        children: [Option<HirTreeChild>; 4],
        index: usize,
    },
    ExprSlice {
        exprs: &'hir [HirExprId],
        index: usize,
    },
    PatSlice {
        pats: &'hir [crate::HirPatId],
        index: usize,
    },
    Args {
        first: Option<HirExprId>,
        args: &'hir [HirArg],
        index: usize,
    },
    RecordFields {
        fields: &'hir [HirFieldInit],
        index: usize,
    },
    MapEntries {
        entries: &'hir [HirMapEntry],
        index: usize,
        value_next: bool,
    },
    HandlerArms {
        arms: &'hir [crate::HirHandlerArmId],
        index: usize,
    },
    Stages {
        input: Option<HirExprId>,
        stages: &'hir [HirStage],
        index: usize,
    },
    Match {
        first: Option<HirTreeChild>,
        arms: &'hir [HirMatchArm],
        index: usize,
    },
    StmtFor {
        phase: u8,
        pat: crate::HirPatId,
        iter: HirExprId,
        limits: &'hir [HirExprId],
        limit_index: usize,
        body: crate::HirBlockId,
    },
    StmtWhile {
        phase: u8,
        cond: HirExprId,
        limits: &'hir [HirExprId],
        limit_index: usize,
        body: crate::HirBlockId,
    },
    StmtRetry {
        limits: &'hir [HirExprId],
        limit_index: usize,
        body: Option<crate::HirBlockId>,
    },
    RecordPatFields {
        fields: &'hir [HirRecordPatField],
        index: usize,
    },
}

impl<'hir> HirTreeChildren<'hir> {
    fn empty() -> Self {
        Self {
            state: HirTreeChildIterState::Empty,
        }
    }

    fn fixed(children: [Option<HirTreeChild>; 4]) -> Self {
        Self {
            state: HirTreeChildIterState::Fixed { children, index: 0 },
        }
    }
}

impl Iterator for HirTreeChildren<'_> {
    type Item = HirTreeChild;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.state {
            HirTreeChildIterState::Empty => None,
            HirTreeChildIterState::Fixed { children, index } => {
                while *index < children.len() {
                    let child = children[*index].take();
                    *index += 1;
                    if child.is_some() {
                        return child;
                    }
                }
                None
            }
            HirTreeChildIterState::ExprSlice { exprs, index } => {
                let expr = exprs.get(*index).copied()?;
                *index += 1;
                Some(HirTreeChild::Expr(expr))
            }
            HirTreeChildIterState::PatSlice { pats, index } => {
                let pat = pats.get(*index).copied()?;
                *index += 1;
                Some(HirTreeChild::Pattern(pat))
            }
            HirTreeChildIterState::Args { first, args, index } => {
                if let Some(expr) = first.take() {
                    return Some(HirTreeChild::Expr(expr));
                }
                while let Some(arg) = args.get(*index) {
                    *index += 1;
                    if let Some(expr) = arg_expr(arg) {
                        return Some(HirTreeChild::Expr(expr));
                    }
                }
                None
            }
            HirTreeChildIterState::RecordFields { fields, index } => {
                while let Some(field) = fields.get(*index) {
                    *index += 1;
                    if let HirFieldInit::Named { value, .. } = field {
                        return Some(HirTreeChild::Expr(*value));
                    }
                }
                None
            }
            HirTreeChildIterState::MapEntries {
                entries,
                index,
                value_next,
            } => {
                let entry = entries.get(*index)?;
                if *value_next {
                    *value_next = false;
                    *index += 1;
                    Some(HirTreeChild::Expr(entry.value))
                } else {
                    *value_next = true;
                    Some(HirTreeChild::Expr(entry.key))
                }
            }
            HirTreeChildIterState::HandlerArms { arms, index } => {
                let arm = arms.get(*index).copied()?;
                *index += 1;
                Some(HirTreeChild::HandlerArm(arm))
            }
            HirTreeChildIterState::Stages {
                input,
                stages,
                index,
            } => {
                if let Some(input) = input.take() {
                    return Some(HirTreeChild::Expr(input));
                }
                let stage = stages.get(*index)?;
                *index += 1;
                Some(HirTreeChild::Expr(stage.expr))
            }
            HirTreeChildIterState::Match { first, arms, index } => {
                if let Some(first) = first.take() {
                    return Some(first);
                }
                let arm = arms.get(*index)?;
                *index += 1;
                Some(HirTreeChild::Pattern(arm.pat))
            }
            HirTreeChildIterState::StmtFor {
                phase,
                pat,
                iter,
                limits,
                limit_index,
                body,
            } => match *phase {
                0 => {
                    *phase = 1;
                    Some(HirTreeChild::Pattern(*pat))
                }
                1 => {
                    *phase = 2;
                    Some(HirTreeChild::Expr(*iter))
                }
                2 => {
                    if let Some(limit) = limits.get(*limit_index).copied() {
                        *limit_index += 1;
                        Some(HirTreeChild::Expr(limit))
                    } else {
                        *phase = 3;
                        Some(HirTreeChild::Block(*body))
                    }
                }
                _ => None,
            },
            HirTreeChildIterState::StmtWhile {
                phase,
                cond,
                limits,
                limit_index,
                body,
            } => match *phase {
                0 => {
                    *phase = 1;
                    Some(HirTreeChild::Expr(*cond))
                }
                1 => {
                    if let Some(limit) = limits.get(*limit_index).copied() {
                        *limit_index += 1;
                        Some(HirTreeChild::Expr(limit))
                    } else {
                        *phase = 2;
                        Some(HirTreeChild::Block(*body))
                    }
                }
                _ => None,
            },
            HirTreeChildIterState::StmtRetry {
                limits,
                limit_index,
                body,
            } => {
                if let Some(limit) = limits.get(*limit_index).copied() {
                    *limit_index += 1;
                    Some(HirTreeChild::Expr(limit))
                } else {
                    body.take().map(HirTreeChild::Block)
                }
            }
            HirTreeChildIterState::RecordPatFields { fields, index } => {
                while let Some(field) = fields.get(*index) {
                    *index += 1;
                    if let Some(child) = record_pat_field_child(field) {
                        return Some(child);
                    }
                }
                None
            }
        }
    }
}

pub(super) fn body_scope(program: &HirProgram, body: HirBodyRef) -> Option<ScopeId> {
    match program.items.get(body.item)? {
        HirItem::Flow(flow) if body.kind == HirBodyKind::Flow => Some(flow.scope),
        HirItem::Tool(tool) if body.kind == HirBodyKind::Tool => Some(tool.scope),
        HirItem::Agent(agent) if body.kind == HirBodyKind::Agent => Some(agent.scope),
        HirItem::Impl(item) => Some(item.scope),
        HirItem::TopLevelLet(_) => None,
        _ => None,
    }
}

pub(super) fn body_debug_id(body: HirBodyRef) -> u32 {
    body.item.0
}

pub(super) fn item_scope(item: &HirItem) -> Option<ScopeId> {
    match item {
        HirItem::TypeAlias(item) => Some(item.scope),
        HirItem::Type(item) => Some(item.scope),
        HirItem::Enum(item) => Some(item.scope),
        HirItem::Spec(item) => Some(item.scope),
        HirItem::Impl(item) => Some(item.scope),
        HirItem::Effect(item) => Some(item.scope),
        HirItem::Tool(item) => Some(item.scope),
        HirItem::Agent(item) => Some(item.scope),
        HirItem::Protocol(item) => Some(item.scope),
        HirItem::Flow(item) => Some(item.scope),
        HirItem::TopLevelLet(_) | HirItem::Error { .. } => None,
    }
}

pub(super) fn stmt_span(program: &HirProgram, stmt: &HirStmt) -> Option<Span> {
    match stmt {
        HirStmt::Let { span, .. }
        | HirStmt::Var { span, .. }
        | HirStmt::Assign { span, .. }
        | HirStmt::For { span, .. }
        | HirStmt::While { span, .. }
        | HirStmt::Retry { span, .. }
        | HirStmt::Resume { span, .. }
        | HirStmt::Finish { span, .. }
        | HirStmt::Return { span, .. }
        | HirStmt::Break { span }
        | HirStmt::Continue { span }
        | HirStmt::Expr { span, .. }
        | HirStmt::Error { span } => Some(*span),
        HirStmt::If(expr) | HirStmt::Match(expr) => program
            .exprs
            .get(*expr)
            .map(|expr| expr.span(&program.blocks)),
    }
}

pub(super) fn for_each_stmt_child(
    program: &HirProgram,
    stmt: HirStmtId,
    mut f: impl FnMut(HirTreeChild),
) {
    for child in stmt_children(program, stmt) {
        f(child);
    }
}

pub(super) fn stmt_children(program: &HirProgram, stmt: HirStmtId) -> HirTreeChildren<'_> {
    let Some(stmt) = program.stmts.get(stmt) else {
        return HirTreeChildren::empty();
    };
    match stmt {
        HirStmt::Let { pat, value, .. } | HirStmt::Var { pat, value, .. } => {
            HirTreeChildren::fixed([
                Some(HirTreeChild::Pattern(*pat)),
                Some(HirTreeChild::Expr(*value)),
                None,
                None,
            ])
        }
        HirStmt::Assign { target, value, .. } => HirTreeChildren::fixed([
            Some(HirTreeChild::Expr(*target)),
            Some(HirTreeChild::Expr(*value)),
            None,
            None,
        ]),
        HirStmt::If(expr) | HirStmt::Match(expr) => {
            HirTreeChildren::fixed([Some(HirTreeChild::Expr(*expr)), None, None, None])
        }
        HirStmt::For {
            pat,
            iter,
            limits,
            body,
            ..
        } => HirTreeChildren {
            state: HirTreeChildIterState::StmtFor {
                phase: 0,
                pat: *pat,
                iter: *iter,
                limits,
                limit_index: 0,
                body: *body,
            },
        },
        HirStmt::While {
            cond, limits, body, ..
        } => HirTreeChildren {
            state: HirTreeChildIterState::StmtWhile {
                phase: 0,
                cond: *cond,
                limits,
                limit_index: 0,
                body: *body,
            },
        },
        HirStmt::Retry { limits, body, .. } => HirTreeChildren {
            state: HirTreeChildIterState::StmtRetry {
                limits,
                limit_index: 0,
                body: Some(*body),
            },
        },
        HirStmt::Resume { value, .. } | HirStmt::Return { value, .. } => {
            HirTreeChildren::fixed([value.map(HirTreeChild::Expr), None, None, None])
        }
        HirStmt::Finish { value, .. } => {
            HirTreeChildren::fixed([Some(HirTreeChild::Expr(*value)), None, None, None])
        }
        HirStmt::Expr { expr, .. } => {
            HirTreeChildren::fixed([Some(HirTreeChild::Expr(*expr)), None, None, None])
        }
        HirStmt::Break { .. } | HirStmt::Continue { .. } | HirStmt::Error { .. } => {
            HirTreeChildren::empty()
        }
    }
}

pub(super) fn for_each_expr_child(
    program: &HirProgram,
    expr: HirExprId,
    mut f: impl FnMut(HirTreeChild),
) {
    for child in expr_children(program, expr) {
        f(child);
    }
}

pub(super) fn expr_children(program: &HirProgram, expr: HirExprId) -> HirTreeChildren<'_> {
    let Some(expr) = program.exprs.get(expr) else {
        return HirTreeChildren::empty();
    };
    match expr {
        HirExpr::Literal(_)
        | HirExpr::Path(_)
        | HirExpr::EmptyRecordOrMap { .. }
        | HirExpr::EmptySequence { .. }
        | HirExpr::Error { .. } => HirTreeChildren::empty(),
        HirExpr::Record(record) => HirTreeChildren {
            state: HirTreeChildIterState::RecordFields {
                fields: &record.fields,
                index: 0,
            },
        },
        HirExpr::Tuple { elems, .. }
        | HirExpr::Array { elems, .. }
        | HirExpr::List { elems, .. }
        | HirExpr::Set { elems, .. } => HirTreeChildren {
            state: HirTreeChildIterState::ExprSlice {
                exprs: elems,
                index: 0,
            },
        },
        HirExpr::ListCons { head, tail, .. } => HirTreeChildren::fixed([
            Some(HirTreeChild::Expr(*head)),
            Some(HirTreeChild::Expr(*tail)),
            None,
            None,
        ]),
        HirExpr::Map { entries, .. } => HirTreeChildren {
            state: HirTreeChildIterState::MapEntries {
                entries,
                index: 0,
                value_next: false,
            },
        },
        HirExpr::Range { start, end, .. } => HirTreeChildren::fixed([
            Some(HirTreeChild::Expr(*start)),
            Some(HirTreeChild::Expr(*end)),
            None,
            None,
        ]),
        HirExpr::Call { callee, args, .. } => HirTreeChildren {
            state: HirTreeChildIterState::Args {
                first: Some(*callee),
                args,
                index: 0,
            },
        },
        HirExpr::MethodCall { receiver, args, .. } => HirTreeChildren {
            state: HirTreeChildIterState::Args {
                first: Some(*receiver),
                args,
                index: 0,
            },
        },
        HirExpr::SpecMethodCall { receiver, args, .. } => HirTreeChildren {
            state: HirTreeChildIterState::Args {
                first: Some(*receiver),
                args,
                index: 0,
            },
        },
        HirExpr::Perform { args, .. } => HirTreeChildren {
            state: HirTreeChildIterState::Args {
                first: None,
                args,
                index: 0,
            },
        },
        HirExpr::Handler { handlers, .. } => HirTreeChildren {
            state: HirTreeChildIterState::HandlerArms {
                arms: handlers,
                index: 0,
            },
        },
        HirExpr::Handle { body, handler, .. } => HirTreeChildren::fixed([
            Some(HirTreeChild::Expr(*body)),
            Some(HirTreeChild::Expr(*handler)),
            None,
            None,
        ]),
        HirExpr::StageCompose { stages, .. } => HirTreeChildren {
            state: HirTreeChildIterState::Stages {
                input: None,
                stages,
                index: 0,
            },
        },
        HirExpr::Pipeline { input, stages, .. } => HirTreeChildren {
            state: HirTreeChildIterState::Stages {
                input: Some(*input),
                stages,
                index: 0,
            },
        },
        HirExpr::Field { base, .. }
        | HirExpr::Try { expr: base, .. }
        | HirExpr::Unary { expr: base, .. } => {
            HirTreeChildren::fixed([Some(HirTreeChild::Expr(*base)), None, None, None])
        }
        HirExpr::Index { base, index, .. } => HirTreeChildren::fixed([
            Some(HirTreeChild::Expr(*base)),
            Some(HirTreeChild::Expr(*index)),
            None,
            None,
        ]),
        HirExpr::Slice {
            base, start, end, ..
        } => HirTreeChildren::fixed([
            Some(HirTreeChild::Expr(*base)),
            Some(HirTreeChild::Expr(*start)),
            Some(HirTreeChild::Expr(*end)),
            None,
        ]),
        HirExpr::Binary { lhs, rhs, .. } => HirTreeChildren::fixed([
            Some(HirTreeChild::Expr(*lhs)),
            Some(HirTreeChild::Expr(*rhs)),
            None,
            None,
        ]),
        HirExpr::If {
            cond,
            then_block,
            else_branch,
            ..
        } => {
            let else_child = else_branch.as_ref().map(|branch| match branch {
                HirElseBranch::If(expr) => HirTreeChild::Expr(*expr),
                HirElseBranch::Block(block) => HirTreeChild::Block(*block),
            });
            HirTreeChildren::fixed([
                Some(HirTreeChild::Expr(*cond)),
                Some(HirTreeChild::Block(*then_block)),
                else_child,
                None,
            ])
        }
        HirExpr::Match {
            scrutinee, arms, ..
        } => {
            let first = Some(HirTreeChild::Expr(*scrutinee));
            if arms.is_empty() {
                HirTreeChildren::fixed([first, None, None, None])
            } else {
                HirTreeChildren {
                    state: HirTreeChildIterState::Match {
                        first,
                        arms,
                        index: 0,
                    },
                }
            }
        }
        HirExpr::Lambda { .. } => HirTreeChildren::empty(),
        HirExpr::Block(block) => {
            HirTreeChildren::fixed([Some(HirTreeChild::Block(*block)), None, None, None])
        }
    }
}

pub(super) fn for_each_pat_child(pat: &HirPat, mut f: impl FnMut(HirTreeChild)) {
    for child in pat_children(pat) {
        f(child);
    }
}

pub(super) fn pat_children(pat: &HirPat) -> HirTreeChildren<'_> {
    match pat {
        HirPat::Tuple { elems, .. } => HirTreeChildren {
            state: HirTreeChildIterState::PatSlice {
                pats: elems,
                index: 0,
            },
        },
        HirPat::Record { fields, .. } => HirTreeChildren {
            state: HirTreeChildIterState::RecordPatFields { fields, index: 0 },
        },
        HirPat::Variant { args, .. } => HirTreeChildren {
            state: HirTreeChildIterState::PatSlice {
                pats: args,
                index: 0,
            },
        },
        HirPat::Binding { .. }
        | HirPat::Wildcard { .. }
        | HirPat::Literal(_)
        | HirPat::Error { .. } => HirTreeChildren::empty(),
    }
}

fn record_pat_field_child(field: &HirRecordPatField) -> Option<HirTreeChild> {
    field.pat.map(HirTreeChild::Pattern)
}

fn arg_expr(arg: &HirArg) -> Option<HirExprId> {
    match arg {
        HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => Some(*expr),
    }
}
