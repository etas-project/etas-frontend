use crate::{Diagnostic, TokenStream};

#[derive(Clone, Debug)]
pub struct Parse<T> {
    pub value: T,
    pub diagnostics: Vec<Diagnostic>,
    pub tokens: TokenStream,
}
