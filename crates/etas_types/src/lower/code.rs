use crate::{Type, TypeConstructorId, TypeId, TypeInterner, facts::symbols::KnownCodeTypes};

pub(crate) fn agent_definition_type(
    interner: &mut TypeInterner,
    code: &KnownCodeTypes,
    component: TypeId,
) -> Option<TypeId> {
    let constructor = code.agent_definition?;
    Some(interner.intern(Type::Applied {
        constructor: TypeConstructorId(constructor.0),
        args: vec![component],
    }))
}
