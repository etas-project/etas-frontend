use std::ops::{Deref, DerefMut};

use crate::ScopeTree;

#[derive(Clone, Debug, Default)]
pub struct ScopeBuilder {
    tree: ScopeTree,
}

impl ScopeBuilder {
    pub fn finish(self) -> ScopeTree {
        self.tree
    }
}

impl Deref for ScopeBuilder {
    type Target = ScopeTree;

    fn deref(&self) -> &Self::Target {
        &self.tree
    }
}

impl DerefMut for ScopeBuilder {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.tree
    }
}
