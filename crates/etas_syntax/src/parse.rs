use crate::{Diagnostic, TokenStream};

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct Parse<T> {
    pub value: T,
    pub diagnostics: Vec<Diagnostic>,
    pub tokens: TokenStream,
}
