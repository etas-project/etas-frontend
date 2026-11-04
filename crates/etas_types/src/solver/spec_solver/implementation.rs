use std::collections::{HashMap, HashSet};

use crate::{
    CallableGenericParam, CallableGenericParamKind, TypeId, TypeStore, TypeSubstitutionEngine,
    TypeUnifier,
};

pub fn match_subject(
    store: &TypeStore,
    parameters: &[CallableGenericParam],
    subject: TypeId,
    actual: TypeId,
) -> Option<HashMap<TypeId, TypeId>> {
    if parameters
        .iter()
        .any(|parameter| parameter.kind != CallableGenericParamKind::Type)
    {
        return None;
    }
    let parameters = parameters
        .iter()
        .filter(|param| param.kind == CallableGenericParamKind::Type)
        .map(|param| param.subject)
        .collect::<HashSet<_>>();
    let mut unifier = TypeUnifier::new(store).with_pattern_parameters(&parameters);
    unifier.unify(subject, actual).ok()?;
    let bindings = unifier.into_parameter_bindings();
    parameters
        .iter()
        .all(|parameter| bindings.contains_key(parameter))
        .then_some(bindings)
}

pub(crate) fn substitute(
    store: &TypeStore,
    bindings: &HashMap<TypeId, TypeId>,
    arguments: &[TypeId],
) -> Option<Vec<TypeId>> {
    let names = HashMap::new();
    let mut substitution =
        TypeSubstitutionEngine::existing(store, &names).with_type_parameters(bindings);
    arguments
        .iter()
        .map(|ty| substitution.substitute(*ty).ok())
        .collect()
}
