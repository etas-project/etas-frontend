use std::collections::{BTreeSet, HashMap, VecDeque};

use etas_code::descriptor::{
    BuiltinTypeInfo, DescriptorGraph, GenericParameterRef, TypeInfo, TypeInfoRef,
    VariantPayloadInfo,
};

use crate::code::descriptor::ProjectionError;
mod frozen_origin;
pub(super) use frozen_origin::preserves as preserves_frozen_origin;

// A type contains latent behavior either intrinsically or through a set of
// generic parameters. Union and binder substitution form a finite lattice, so
// recursive ADTs terminate without expanding their instantiated definitions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Latent {
    intrinsic: bool,
    parameters: BTreeSet<GenericParameterRef>,
}

impl Latent {
    fn join(&mut self, other: &Self) {
        self.intrinsic |= other.intrinsic;
        self.parameters.extend(&other.parameters);
    }
}

pub(super) fn has_latent_output(
    graph: &DescriptorGraph,
    signature: TypeInfoRef,
) -> Result<bool, ProjectionError> {
    let Some(TypeInfo::Function(signature)) = graph.type_info(signature) else {
        return Err(invalid("contract has no callable signature"));
    };
    let root = signature.output;
    let mut pending = vec![root];
    let mut states = HashMap::new();
    let mut dependents: HashMap<TypeInfoRef, Vec<TypeInfoRef>> = HashMap::new();
    while let Some(ty) = pending.pop() {
        if states.contains_key(&ty) {
            continue;
        }
        let node = graph
            .type_info(ty)
            .ok_or_else(|| invalid("contract output has a missing type descriptor"))?;
        states.insert(ty, Latent::default());
        for child in children(node) {
            dependents.entry(child).or_default().push(ty);
            pending.push(child);
        }
    }
    let mut pending: VecDeque<_> = states.keys().copied().collect();
    while let Some(ty) = pending.pop_front() {
        let node = graph
            .type_info(ty)
            .ok_or_else(|| invalid("contract output has a missing type descriptor"))?;
        let state = transfer(graph, node, &states)?;
        if states.get(&ty) != Some(&state) {
            states.insert(ty, state);
            if let Some(parents) = dependents.get(&ty) {
                pending.extend(parents);
            }
        }
    }
    let result = states
        .get(&root)
        .ok_or_else(|| invalid("contract output has no latent classification"))?;
    Ok(result.intrinsic || !result.parameters.is_empty())
}

fn children(node: &TypeInfo) -> Vec<TypeInfoRef> {
    match node {
        TypeInfo::Builtin { arguments, .. } | TypeInfo::Tuple(arguments) => arguments.clone(),
        TypeInfo::Record(fields) => fields.iter().map(|field| field.ty).collect(),
        TypeInfo::Nominal { representation, .. } => representation.iter().copied().collect(),
        TypeInfo::Alias { target, .. } => vec![*target],
        TypeInfo::Enum { variants, .. } => variants
            .iter()
            .flat_map(|variant| match &variant.payload {
                VariantPayloadInfo::None => Vec::new(),
                VariantPayloadInfo::Positional(fields) => fields.clone(),
                VariantPayloadInfo::Named(fields) => fields.iter().map(|field| field.ty).collect(),
            })
            .collect(),
        TypeInfo::Applied {
            constructor,
            arguments,
        } => std::iter::once(*constructor)
            .chain(arguments.iter().copied())
            .collect(),
        TypeInfo::Primitive(_)
        | TypeInfo::Parameter(_)
        | TypeInfo::Function(_)
        | TypeInfo::Handler { .. } => Vec::new(),
    }
}

fn transfer(
    graph: &DescriptorGraph,
    node: &TypeInfo,
    states: &HashMap<TypeInfoRef, Latent>,
) -> Result<Latent, ProjectionError> {
    let lookup = |ty: &TypeInfoRef| {
        states
            .get(ty)
            .ok_or_else(|| invalid("latent type dependency is absent"))
    };
    match node {
        TypeInfo::Function(_) | TypeInfo::Handler { .. } => Ok(Latent {
            intrinsic: true,
            ..Default::default()
        }),
        TypeInfo::Parameter(parameter) => Ok(Latent {
            parameters: [*parameter].into(),
            ..Default::default()
        }),
        TypeInfo::Applied {
            constructor,
            arguments,
        } => {
            let parameters = match graph.type_info(*constructor) {
                Some(
                    TypeInfo::Nominal { parameters, .. }
                    | TypeInfo::Enum { parameters, .. }
                    | TypeInfo::Alias { parameters, .. },
                ) => parameters,
                _ => return Err(invalid("applied output has no generic declaration")),
            };
            if parameters.len() != arguments.len() {
                return Err(invalid("applied output has inconsistent generic arity"));
            }
            let constructor = lookup(constructor)?;
            let mut result = Latent {
                intrinsic: constructor.intrinsic,
                ..Default::default()
            };
            for parameter in &constructor.parameters {
                if let Some(index) = parameters.iter().position(|item| item == parameter) {
                    result.join(lookup(&arguments[index])?);
                } else {
                    result.parameters.insert(*parameter);
                }
            }
            Ok(result)
        }
        _ => {
            let mut result = Latent::default();
            if matches!(node, TypeInfo::Nominal { definition, .. } if definition.kind() == etas_code::DefinitionKind::Agent)
                || matches!(
                    node,
                    TypeInfo::Builtin {
                        constructor: BuiltinTypeInfo::ExternalToolHandle,
                        ..
                    }
                )
            {
                result.intrinsic = true;
            }
            for child in children(node) {
                result.join(lookup(&child)?);
            }
            Ok(result)
        }
    }
}

fn invalid(reason: &str) -> ProjectionError {
    ProjectionError::Invalid(reason.into())
}
