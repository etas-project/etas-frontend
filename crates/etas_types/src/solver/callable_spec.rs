use std::collections::HashMap;

use super::{SolverFailure, TypeUnifier};
use crate::{
    CallableSignature, EffectRowRef, Type, TypeId, TypeInterner, TypeSubstitutionEngine, TypeVarId,
};

pub struct CallableSpecBinding {
    pub args: Vec<TypeId>,
    pub effect_bound: Option<EffectRowRef>,
}

pub fn solve(
    interner: &mut TypeInterner,
    parameters: &[(String, bool)],
    explicit_args: &[TypeId],
    expected: &CallableSignature,
    actual: &CallableSignature,
    span: etas_core::Span,
) -> Result<CallableSpecBinding, SolverFailure> {
    let failure = |message: &str| SolverFailure {
        code: etas_core::TypeDiagnosticCode::TypeMismatch,
        span,
        message: message.into(),
    };
    let type_parameters = parameters
        .iter()
        .filter(|(_, effect)| !effect)
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    if !explicit_args.is_empty() && explicit_args.len() != type_parameters.len() {
        return Err(failure(
            "callable spec type argument count does not match its declared binders",
        ));
    }
    let mut named = HashMap::new();
    for (index, name) in type_parameters.iter().enumerate() {
        let ty = explicit_args.get(index).copied().unwrap_or_else(|| {
            let var = TypeVarId(interner.store().len() as u32);
            interner.intern(Type::Var(var))
        });
        named.insert((*name).clone(), ty);
    }
    let mut engine = TypeSubstitutionEngine::materializing(interner, &named, None);
    let expected_inputs = expected
        .params
        .iter()
        .map(|ty| engine.substitute(*ty))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| failure("callable spec input substitution is incomplete"))?;
    let expected_output = engine
        .substitute(expected.output)
        .map_err(|_| failure("callable spec output substitution is incomplete"))?;
    let expected_inputs = match expected_inputs.as_slice() {
        [input] => match interner.store().get(*input) {
            Some(Type::Primitive(crate::PrimitiveType::Unit)) => Vec::new(),
            Some(Type::Tuple(inputs)) => inputs.clone(),
            _ => expected_inputs,
        },
        _ => expected_inputs,
    };
    if expected_inputs.len() != actual.params.len() {
        return Err(failure(
            "callable spec input arity does not match the implementation",
        ));
    }
    let mut unifier = TypeUnifier::new(interner.store());
    for (expected, actual) in expected_inputs.iter().zip(&actual.params) {
        unifier
            .unify(*expected, *actual)
            .map_err(|_| failure("implementation input type does not satisfy the callable spec"))?;
    }
    unifier
        .unify(expected_output, actual.output)
        .map_err(|_| failure("implementation output type does not satisfy the callable spec"))?;
    let substitutions = unifier.into_substitution();
    let mut engine = TypeSubstitutionEngine::materializing(interner, &named, Some(&substitutions));
    let args = type_parameters
        .iter()
        .map(|name| engine.substitute(named[*name]))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| failure("callable spec generic binding is incomplete"))?;
    let effect_bound = expected
        .effects
        .clone()
        .map(|row| engine.substitute_effect_row(row))
        .transpose()
        .map_err(|_| failure("callable spec effect type substitution is incomplete"))?;
    Ok(CallableSpecBinding { args, effect_bound })
}
