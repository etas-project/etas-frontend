use etas_hir::HirExprId;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum InterproceduralDiagnostic<U> {
    InvalidCondensationGraph,
    MissingBody { unit: U },
    MissingSummary { call: HirExprId, callee: U },
    IterationLimitReached { component: Vec<U> },
}
