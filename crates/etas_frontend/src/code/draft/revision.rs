use std::sync::Arc;

use crate::AstBodyRef;
use etas_code::syntax::SyntaxBlockRef;
use etas_core::Span;
use etas_syntax::RelocatedBodySyntax;

/// Body-only revision data. It does not retain an earlier checked candidate or
/// a caller frame, so publishing successive revisions cannot create a chain
/// of complete ProjectOutput snapshots.
#[derive(Clone, Debug, etas_core::RetainedSize)]
pub(crate) struct NativeBodyRevision {
    pub slot: AstBodyRef,
    pub declaration: Span,
    pub syntax: SyntaxBlockRef,
    pub relocated: Arc<RelocatedBodySyntax>,
    pub unchanged: bool,
}
