use std::collections::{HashMap, HashSet, VecDeque};

use super::{
    EffectArgRef, EffectRowRef, HandlerProducedEffects, ResourceHandleType, Type, TypeId,
    TypeStore, TypeSubstitutionBudget, TypeSubstitutionError,
};
use crate::TypeFacts;

/// Enumerate the checked type graph needed by a type or effect-row application.
/// Nominal and enum identities are retained as nodes; recursive declarations
/// are visited once instead of expanding their values indefinitely.
/// `checked_declarations` must come from the caller's verified identity bridge;
/// this descriptive traversal does not validate or authorize that bridge.
pub fn checked_type_graph(
    store: &TypeStore,
    facts: &TypeFacts,
    roots: &[TypeId],
    rows: &[&EffectRowRef],
    checked_declarations: &HashMap<TypeId, TypeId>,
    max_depth: usize,
    budget: &TypeSubstitutionBudget<'_>,
) -> Result<Vec<TypeId>, TypeSubstitutionError> {
    let mut walker = Walker {
        queue: VecDeque::new(),
        seen: HashSet::new(),
        visited: Vec::new(),
        budget,
    };
    for root in roots {
        walker.edge(*root, 0)?;
    }
    for row in rows {
        walker.row(row, 0)?;
    }
    while let Some((id, depth)) = walker.queue.pop_front() {
        if !walker.seen.insert(id) {
            continue;
        }
        if depth > max_depth {
            return Err(TypeSubstitutionError::ExpansionDepth(max_depth));
        }
        budget.node()?;
        let ty = store
            .get(id)
            .ok_or(TypeSubstitutionError::MissingType(id))?;
        walker.visited.push(id);
        if checked_declarations.contains_key(&id) {
            continue;
        }
        let params = facts.type_generics.get(&id).map(Vec::as_slice);
        if let Type::Nominal(nominal) = ty {
            validate_parameters(id, &nominal.params, params, budget)?;
        }
        let child_depth = depth.saturating_add(1);
        if let Some(params) = params {
            for param in params {
                budget.edges(1)?;
                walker.edge(param.subject, child_depth)?;
                for bound in &param.bounds {
                    budget.edges(1)?;
                    for arg in &bound.args {
                        walker.edge(*arg, child_depth)?;
                    }
                }
            }
        }
        match ty {
            Type::Existential { bounds } => {
                for bound in bounds {
                    budget.edges(1)?;
                    for arg in &bound.args {
                        walker.edge(*arg, child_depth)?;
                    }
                }
            }
            Type::Array(inner)
            | Type::List(inner)
            | Type::Set(inner)
            | Type::Range { index: inner }
            | Type::Slice(inner)
            | Type::Option(inner)
            | Type::Refined { base: inner, .. }
            | Type::Trust { inner, .. }
            | Type::Schema(inner)
            | Type::Message(inner)
            | Type::MemorySelection(inner)
            | Type::MemoryRegion(inner) => walker.edge(*inner, child_depth)?,
            Type::Map { key, value }
            | Type::Store { key, value }
            | Type::Result {
                ok: key,
                err: value,
            } => {
                walker.edge(*key, child_depth)?;
                walker.edge(*value, child_depth)?;
            }
            Type::Record(record) => {
                for field in &record.fields {
                    walker.edge(field.ty, child_depth)?;
                }
            }
            Type::Tuple(elements) => {
                for element in elements {
                    walker.edge(*element, child_depth)?;
                }
            }
            Type::Enum(_) => {
                let layout = facts
                    .enum_layouts
                    .get(&id)
                    .ok_or(TypeSubstitutionError::MissingEnumLayout(id))?;
                validate_parameters(id, &layout.type_params, params, budget)?;
                for variant in &layout.variants {
                    budget.edges(1)?;
                    for field in &variant.fields {
                        walker.edge(*field, child_depth)?;
                    }
                }
            }
            Type::Nominal(nominal) => {
                if let Some(representation) = nominal.representation {
                    walker.edge(representation, child_depth)?;
                }
            }
            Type::Function(flow) => {
                for input in &flow.input {
                    walker.edge(*input, child_depth)?;
                }
                walker.edge(flow.output, child_depth)?;
                if let Some(row) = &flow.effects {
                    walker.row(row, child_depth)?;
                }
            }
            Type::Handler(handler) => {
                walker.row(&handler.handled, child_depth)?;
                if let HandlerProducedEffects::Explicit(row) = &handler.produced {
                    walker.row(row, child_depth)?;
                }
                if let Some(result) = handler.result {
                    walker.edge(result, child_depth)?;
                }
            }
            Type::Applied { constructor, args } => {
                walker.edge(TypeId(constructor.0), child_depth)?;
                for arg in args {
                    walker.edge(*arg, child_depth)?;
                }
            }
            Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => {
                walker.edge(*schema, child_depth)?;
            }
            Type::ResourceHandle(ResourceHandleType::ExternalTool { signature }) => {
                walker.edge(*signature, child_depth)?;
            }
            Type::ResourceHandle(ResourceHandleType::Other { args, .. }) => {
                for arg in args {
                    walker.edge(*arg, child_depth)?;
                }
            }
            Type::Primitive(_)
            | Type::IntegerLiteral { .. }
            | Type::Var(_)
            | Type::Named(_)
            | Type::Prompt
            | Type::PromptPart
            | Type::MemoryPlace(_) => {}
        }
    }
    Ok(walker.visited)
}

pub(super) fn validate_parameters(
    owner: TypeId,
    names: &[String],
    params: Option<&[crate::CallableGenericParam]>,
    budget: &TypeSubstitutionBudget<'_>,
) -> Result<(), TypeSubstitutionError> {
    budget.edges(names.len())?;
    match params {
        Some(params)
            if names.len() == params.len()
                && names
                    .iter()
                    .zip(params)
                    .all(|(name, param)| name == &param.name) =>
        {
            Ok(())
        }
        None if names.is_empty() => Ok(()),
        _ => Err(TypeSubstitutionError::InvalidApplication(owner)),
    }
}

struct Walker<'a, 'b> {
    queue: VecDeque<(TypeId, usize)>,
    seen: HashSet<TypeId>,
    visited: Vec<TypeId>,
    budget: &'a TypeSubstitutionBudget<'b>,
}

impl Walker<'_, '_> {
    fn edge(&mut self, id: TypeId, depth: usize) -> Result<(), TypeSubstitutionError> {
        self.budget.edges(1)?;
        if !self.seen.contains(&id) {
            self.queue.push_back((id, depth));
        }
        Ok(())
    }

    fn row(&mut self, row: &EffectRowRef, depth: usize) -> Result<(), TypeSubstitutionError> {
        for effect in &row.effects {
            self.budget.edges(1)?;
            for arg in &effect.args {
                self.budget.edges(1)?;
                if let EffectArgRef::Type(id) = arg {
                    self.edge(*id, depth)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnumLayoutFact, EnumVariantLayoutFact, FieldType, RecordType, TypeInterner};

    #[test]
    fn visits_recursive_nominal_and_enum_layout_once() {
        let mut interner = TypeInterner::new();
        let nominal = interner.intern(Type::Nominal(super::super::NominalTypeRef {
            name: "Directory".into(),
            params: vec![],
            representation: None,
            read_only_erased_view: false,
        }));
        let enumeration = interner.intern(Type::Enum(super::super::EnumTypeRef {
            name: "Entry".into(),
        }));
        let representation = interner.intern(Type::Record(RecordType {
            fields: vec![FieldType {
                name: "entry".into(),
                ty: enumeration,
            }],
        }));
        interner.define_nominal(nominal, Some(representation));
        let mut facts = TypeFacts::default();
        facts.enum_layouts.insert(
            enumeration,
            EnumLayoutFact {
                type_params: vec![],
                variants: vec![EnumVariantLayoutFact {
                    name: "Dir".into(),
                    fields: vec![nominal],
                    field_names: None,
                }],
            },
        );
        let graph = checked_type_graph(
            interner.store(),
            &facts,
            &[nominal],
            &[],
            &HashMap::new(),
            10,
            &TypeSubstitutionBudget::new(10, 10, None),
        )
        .unwrap();
        assert_eq!(graph, vec![nominal, representation, enumeration]);
    }

    #[test]
    fn traverses_effect_arguments_and_enforces_checked_layout_and_limits() {
        let mut interner = TypeInterner::new();
        let enumeration = interner.intern(Type::Enum(super::super::EnumTypeRef {
            name: "Error".into(),
        }));
        let row = EffectRowRef {
            effects: vec![super::super::EffectRef {
                name: "Error".into(),
                args: vec![EffectArgRef::Type(enumeration)],
            }],
            tail: None,
        };
        let facts = TypeFacts::default();
        assert_eq!(
            checked_type_graph(
                interner.store(),
                &facts,
                &[],
                &[&row],
                &HashMap::new(),
                10,
                &TypeSubstitutionBudget::new(10, 10, None),
            ),
            Err(TypeSubstitutionError::MissingEnumLayout(enumeration))
        );
        assert_eq!(
            checked_type_graph(
                interner.store(),
                &facts,
                &[],
                &[&row],
                &HashMap::from([(enumeration, TypeId(100))]),
                10,
                &TypeSubstitutionBudget::new(10, 10, None),
            )
            .unwrap(),
            vec![enumeration]
        );
        assert!(matches!(
            checked_type_graph(
                interner.store(),
                &facts,
                &[],
                &[&row],
                &HashMap::new(),
                10,
                &TypeSubstitutionBudget::new(10, 1, None),
            ),
            Err(TypeSubstitutionError::WorkLimit {
                resource: "edges",
                ..
            })
        ));
    }

    #[test]
    fn enforces_depth_and_propagates_cancellation() {
        struct Cancelled;
        impl etas_utils::PipelineExecutionControl for Cancelled {
            fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
                Err(etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    reason: "cancelled".into(),
                })
            }
        }
        let mut types = TypeInterner::new();
        let inner = types.primitive(crate::PrimitiveType::Bool);
        let outer = types.intern(Type::Array(inner));
        let facts = TypeFacts::default();
        assert_eq!(
            checked_type_graph(
                types.store(),
                &facts,
                &[outer],
                &[],
                &HashMap::new(),
                0,
                &TypeSubstitutionBudget::new(10, 10, None),
            ),
            Err(TypeSubstitutionError::ExpansionDepth(0))
        );
        assert!(matches!(
            checked_type_graph(
                types.store(),
                &facts,
                &[outer],
                &[],
                &HashMap::new(),
                10,
                &TypeSubstitutionBudget::new(10, 10, Some(&Cancelled)),
            ),
            Err(TypeSubstitutionError::Interrupted(_))
        ));
    }

    #[test]
    fn includes_generic_parameter_and_bound_arguments() {
        let mut types = TypeInterner::new();
        let owner = types.intern(Type::Nominal(super::super::NominalTypeRef {
            name: "Box".into(),
            params: vec!["T".into()],
            representation: None,
            read_only_erased_view: false,
        }));
        let subject = types.intern(Type::Named(super::super::NamedTypeRef {
            name: "T".into(),
            source_parameter: None,
        }));
        let bound_arg = types.primitive(crate::PrimitiveType::String);
        let mut facts = TypeFacts::default();
        assert_eq!(
            checked_type_graph(
                types.store(),
                &facts,
                &[owner],
                &[],
                &HashMap::new(),
                10,
                &TypeSubstitutionBudget::new(10, 10, None),
            ),
            Err(TypeSubstitutionError::InvalidApplication(owner))
        );
        facts.type_generics.insert(
            owner,
            vec![crate::CallableGenericParam {
                kind: crate::CallableGenericParamKind::Type,
                name: "T".into(),
                subject,
                bounds: vec![crate::CheckedSpecBound {
                    spec: crate::CheckedSpecRef::Std(vec!["std".into(), "Spec".into()]),
                    args: vec![bound_arg],
                }],
            }],
        );
        assert_eq!(
            checked_type_graph(
                types.store(),
                &facts,
                &[owner],
                &[],
                &HashMap::new(),
                10,
                &TypeSubstitutionBudget::new(10, 10, None),
            )
            .unwrap(),
            vec![owner, subject, bound_arg]
        );
    }

    #[test]
    fn empty_enum_variants_consume_work_budget() {
        let mut types = TypeInterner::new();
        let owner = types.intern(Type::Enum(super::super::EnumTypeRef {
            name: "Many".into(),
        }));
        let mut facts = TypeFacts::default();
        facts.enum_layouts.insert(
            owner,
            EnumLayoutFact {
                type_params: vec![],
                variants: (0..8)
                    .map(|index| EnumVariantLayoutFact {
                        name: format!("V{index}"),
                        fields: vec![],
                        field_names: None,
                    })
                    .collect(),
            },
        );
        assert!(matches!(
            checked_type_graph(
                types.store(),
                &facts,
                &[owner],
                &[],
                &HashMap::new(),
                10,
                &TypeSubstitutionBudget::new(10, 4, None),
            ),
            Err(TypeSubstitutionError::WorkLimit {
                resource: "edges",
                ..
            })
        ));
    }
}
