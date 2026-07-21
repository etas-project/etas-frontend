//! Lexical scope storage and construction.
//!
//! `ScopeTree` is the canonical lexical binding graph used by name resolution:
//! it owns scopes, parent links, namespaces, and symbol bindings. The structural
//! HIR view in `crate::view` may derive read-only scope indexes from this data,
//! but it is not a second source of truth for lexical lookup.

mod builder;
pub mod lexical;

pub use builder::ScopeBuilder;
pub use lexical::*;
