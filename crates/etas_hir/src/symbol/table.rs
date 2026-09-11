use crate::{Symbol, SymbolData, SymbolId};

#[derive(Clone, Debug, Default)]
pub struct SymbolTable {
    symbols: Vec<Symbol>,
    members: std::collections::HashMap<(SymbolId, String), Vec<SymbolId>>,
}

impl SymbolTable {
    pub fn insert_member(&mut self, owner: SymbolId, name: String, member: SymbolId) {
        self.members.entry((owner, name)).or_default().push(member);
    }

    pub fn resolve_member(&self, owner: SymbolId, name: &str) -> crate::ResolveResult {
        match self
            .members
            .get(&(owner, name.to_owned()))
            .map(Vec::as_slice)
        {
            Some([symbol]) => crate::ResolveResult::Resolved(*symbol),
            Some(symbols) if !symbols.is_empty() => {
                crate::ResolveResult::Ambiguous(symbols.to_vec())
            }
            _ => crate::ResolveResult::Unresolved,
        }
    }

    pub fn alloc(&mut self, data: SymbolData) -> SymbolId {
        let id = SymbolId(self.symbols.len().min(u32::MAX as usize) as u32);
        self.symbols.push(Symbol {
            id,
            name: data.name,
            kind: data.kind,
            visibility: data.visibility,
            defining_module: data.defining_module,
            defining_item: data.defining_item,
            def: data.def,
            declared_type: data.declared_type,
            definition_span: data.definition_span,
        });
        id
    }

    pub fn get(&self, id: SymbolId) -> Option<&Symbol> {
        self.symbols.get(id.index())
    }

    pub fn get_mut(&mut self, id: SymbolId) -> Option<&mut Symbol> {
        self.symbols.get_mut(id.index())
    }

    pub fn find_by_name(&self, name: &str) -> Vec<SymbolId> {
        self.symbols
            .iter()
            .filter(|symbol| symbol.name == name)
            .map(|symbol| symbol.id)
            .collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Symbol> {
        self.symbols.iter()
    }

    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }
}
