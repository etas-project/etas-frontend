use std::ops::{Deref, DerefMut};

use crate::SymbolTable;

#[derive(Clone, Debug, Default)]
pub struct SymbolBuilder {
    table: SymbolTable,
}

impl SymbolBuilder {
    pub fn finish(self) -> SymbolTable {
        self.table
    }
}

impl Deref for SymbolBuilder {
    type Target = SymbolTable;

    fn deref(&self) -> &Self::Target {
        &self.table
    }
}

impl DerefMut for SymbolBuilder {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.table
    }
}
