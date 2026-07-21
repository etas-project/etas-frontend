mod config;
mod context;
mod domain;
mod facts;
mod interprocedural;
mod oracle;
mod place;
mod semantics;
mod solver;
mod summary;

#[cfg(test)]
mod tests;

pub use config::{
    AliasConstraintModel, AliasPrecisionConfig, AllocationSensitivity, ContextSensitivity,
    FieldSensitivity, FlowSensitivity, HeapModel, IndexSensitivity, UpdatePolicy,
};
pub use context::{AliasContext, ContextualAliasUnit};
pub use domain::{AliasDomain, AliasSet, AliasValue};
pub use facts::{AliasAnalysisOutput, AliasFacts};
pub use interprocedural::{AliasAnalysisInput, analyze_aliases};
pub use oracle::{
    AliasCallTarget, AliasIntrinsicSummary, AliasOracle, AllocationKind, NoAliasOracle,
};
pub use place::{AliasTarget, AllocationSiteKind, Place, Projection};
pub use solver::{
    AliasConstraint, AliasConstraintFrame, AliasSolution, AliasSolver, AliasVar, HybridAliasSolver,
    InclusionAliasSolver, UnificationAliasSolver,
};
pub use summary::{AliasParamSummary, AliasSummary, AliasValueExpr, AliasWriteEffect};
