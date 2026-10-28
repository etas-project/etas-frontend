use etas_hir::SymbolId;

use crate::TypeId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReflectionBinding {
    AgentDeclaration(SymbolId),
    CallableDeclaration(SymbolId),
    AgentMethod(SymbolId),
    SpecMethod {
        receiver: Box<ReflectionBinding>,
        spec: SymbolId,
        args: Vec<TypeId>,
        method: String,
    },
    Value {
        binding: SymbolId,
        ty: TypeId,
    },
    Invalid {
        reason: String,
    },
}
