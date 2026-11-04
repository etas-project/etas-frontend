use std::collections::HashMap;

use etas_code::DefinitionKey;

use super::{
    Type, TypeId, TypeInterner, TypeStore, TypeSubstitutionBudget, TypeSubstitutionEngine,
    TypeSubstitutionError, substitution::TypeRelocationMaterializationInput,
};
use crate::{CallableGenericParam, CheckedSpecRef, EnumLayoutFact, TypeFacts};

/// Descriptive inputs for a checked-program bridge. Callers must derive the
/// identity maps from checked declarations; this operation grants no access.
pub struct ForeignTypeGraphInput<'a> {
    pub source: &'a TypeStore,
    pub facts: &'a TypeFacts,
    pub target_facts: &'a TypeFacts,
    pub reachable: &'a [TypeId],
    pub identities: &'a HashMap<TypeId, DefinitionKey>,
    pub declarations: &'a HashMap<TypeId, TypeId>,
    pub effect_names: &'a HashMap<String, String>,
    pub specs: &'a HashMap<CheckedSpecRef, CheckedSpecRef>,
    pub max_depth: usize,
    pub budget: &'a TypeSubstitutionBudget<'a>,
}

pub struct ForeignTypeGraph {
    pub interner: TypeInterner,
    pub declarations: HashMap<TypeId, TypeId>,
    pub type_generics: HashMap<TypeId, Vec<CallableGenericParam>>,
    pub enum_layouts: HashMap<TypeId, EnumLayoutFact>,
}

/// Construct foreign declarations transactionally in a separate interner.
/// Recursive references are resolved through canonical identity reservations;
/// source-local type and symbol IDs are never copied as target identities.
pub fn relocate_foreign_type_graph(
    mut target: TypeInterner,
    input: ForeignTypeGraphInput<'_>,
) -> Result<ForeignTypeGraph, TypeSubstitutionError> {
    input.budget.edges(input.declarations.len())?;
    let mut declarations = HashMap::with_capacity(input.declarations.len());
    for (source, target) in input.declarations {
        input.budget.node()?;
        declarations.insert(*source, *target);
    }
    for source_id in input.reachable {
        input.budget.node()?;
        if declarations.contains_key(source_id)
            || !matches!(
                input.source.get(*source_id),
                Some(Type::Nominal(_) | Type::Enum(_))
            )
        {
            continue;
        }
        let identity = input
            .identities
            .get(source_id)
            .ok_or(TypeSubstitutionError::MissingTypeIdentity(*source_id))?;
        if let Some(params) = input.facts.type_generics.get(source_id) {
            for (index, param) in params.iter().enumerate() {
                input.budget.edges(1)?;
                if !matches!(input.source.get(param.subject), Some(Type::Named(name)) if name.name == param.name)
                {
                    return Err(TypeSubstitutionError::InvalidApplication(*source_id));
                }
                let index = u32::try_from(index)
                    .map_err(|_| TypeSubstitutionError::InvalidApplication(*source_id))?;
                let key = super::CanonicalTypeParameterIdentity::new(identity.clone(), index)
                    .map_err(|_| TypeSubstitutionError::InvalidApplication(*source_id))?;
                if let Some(mapped) = declarations.get(&param.subject) {
                    if !input.declarations.contains_key(&param.subject)
                        && target.store().parameter_identity(*mapped) != Some(&key)
                    {
                        return Err(TypeSubstitutionError::InvalidApplication(*source_id));
                    }
                    continue;
                }
                let mapped = target
                    .intern_canonical_parameter(key, param.name.clone())
                    .map_err(|_| TypeSubstitutionError::InvalidApplication(*source_id))?;
                declarations.insert(param.subject, mapped);
            }
        }
    }
    let mut fresh = Vec::new();
    for source_id in input.reachable {
        input.budget.node()?;
        if declarations.contains_key(source_id) {
            continue;
        }
        let source_ty = input
            .source
            .get(*source_id)
            .ok_or(TypeSubstitutionError::MissingType(*source_id))?;
        let reservation = match source_ty {
            Type::Nominal(decl) => {
                let mut decl = decl.clone();
                decl.representation = None;
                Type::Nominal(decl)
            }
            Type::Enum(decl) => Type::Enum(decl.clone()),
            _ => continue,
        };
        let identity = input
            .identities
            .get(source_id)
            .ok_or(TypeSubstitutionError::MissingTypeIdentity(*source_id))?;
        let existing = target.canonical_nominal(identity);
        let target_id = match existing {
            Some(id) => {
                let actual = target
                    .store()
                    .get(id)
                    .ok_or(TypeSubstitutionError::MissingType(id))?;
                let compatible = match (&reservation, actual) {
                    (Type::Nominal(expected), Type::Nominal(actual)) => {
                        expected.params == actual.params
                            && expected.read_only_erased_view == actual.read_only_erased_view
                    }
                    (Type::Enum(_), Type::Enum(_)) => true,
                    _ => false,
                };
                if !compatible {
                    return Err(TypeSubstitutionError::InvalidApplication(*source_id));
                }
                id
            }
            None => target
                .intern_canonical_nominal(identity.clone(), reservation)
                .map_err(|_| TypeSubstitutionError::InvalidApplication(*source_id))?,
        };
        declarations.insert(*source_id, target_id);
        fresh.push((*source_id, target_id, existing.is_none()));
    }

    let mut relocation =
        TypeSubstitutionEngine::relocating_materialized(TypeRelocationMaterializationInput {
            source: input.source,
            target: &mut target,
            declaration_types: &declarations,
            effect_names: input.effect_names,
        })
        .with_spec_identities(input.specs)
        .with_expansion_depth(input.max_depth)
        .with_budget(input.budget);
    let mut completed = Vec::new();
    let mut type_generics = HashMap::new();
    let mut enum_layouts = HashMap::new();
    for (source_id, target_id, is_new) in &fresh {
        input.budget.node()?;
        let source_ty = input
            .source
            .get(*source_id)
            .ok_or(TypeSubstitutionError::MissingType(*source_id))?;
        let source_params = input.facts.type_generics.get(source_id).map(Vec::as_slice);
        match source_ty {
            Type::Nominal(decl) => super::graph::validate_parameters(
                *source_id,
                &decl.params,
                source_params,
                input.budget,
            )?,
            Type::Enum(_) => {
                let layout = input
                    .facts
                    .enum_layouts
                    .get(source_id)
                    .ok_or(TypeSubstitutionError::MissingEnumLayout(*source_id))?;
                super::graph::validate_parameters(
                    *source_id,
                    &layout.type_params,
                    source_params,
                    input.budget,
                )?;
            }
            _ => return Err(TypeSubstitutionError::InvalidApplication(*source_id)),
        }
        if let Type::Nominal(decl) = source_ty {
            let representation = decl
                .representation
                .map(|ty| relocation.substitute(ty))
                .transpose()?;
            completed.push((*source_id, *target_id, *is_new, representation));
        }
        if let Some(params) = input.facts.type_generics.get(source_id) {
            let mut mapped = Vec::new();
            for param in params {
                input.budget.edges(1)?;
                let subject = relocation.substitute(param.subject)?;
                let mut bounds = Vec::new();
                for bound in &param.bounds {
                    input.budget.edges(1)?;
                    let spec = input.specs.get(&bound.spec).cloned().ok_or_else(|| {
                        TypeSubstitutionError::MissingSpecIdentity(bound.spec.clone())
                    })?;
                    let mut args = Vec::new();
                    for arg in &bound.args {
                        input.budget.edges(1)?;
                        args.push(relocation.substitute(*arg)?);
                    }
                    bounds.push(crate::CheckedSpecBound { spec, args });
                }
                mapped.push(CallableGenericParam {
                    kind: param.kind,
                    name: param.name.clone(),
                    subject,
                    bounds,
                });
            }
            if !is_new && input.target_facts.type_generics.get(target_id) != Some(&mapped) {
                return Err(TypeSubstitutionError::InvalidApplication(*source_id));
            }
            type_generics.insert(*target_id, mapped);
        } else {
            if !is_new
                && input
                    .target_facts
                    .type_generics
                    .get(target_id)
                    .is_some_and(|params| !params.is_empty())
            {
                return Err(TypeSubstitutionError::InvalidApplication(*source_id));
            }
            type_generics.insert(*target_id, Vec::new());
        }
        if matches!(source_ty, Type::Enum(_)) {
            let source_layout = input
                .facts
                .enum_layouts
                .get(source_id)
                .ok_or(TypeSubstitutionError::MissingEnumLayout(*source_id))?;
            input.budget.edges(source_layout.type_params.len())?;
            let mut variants = Vec::new();
            for variant in &source_layout.variants {
                input.budget.edges(1)?;
                let mut fields = Vec::new();
                for field in &variant.fields {
                    input.budget.edges(1)?;
                    fields.push(relocation.substitute(*field)?);
                }
                if let Some(names) = &variant.field_names {
                    input.budget.edges(names.len())?;
                    if names.len() != fields.len() {
                        return Err(TypeSubstitutionError::InvalidApplication(*source_id));
                    }
                }
                variants.push(crate::EnumVariantLayoutFact {
                    name: variant.name.clone(),
                    fields,
                    field_names: variant.field_names.clone(),
                });
            }
            let layout = EnumLayoutFact {
                type_params: source_layout.type_params.clone(),
                variants,
            };
            if !is_new && input.target_facts.enum_layouts.get(target_id) != Some(&layout) {
                return Err(TypeSubstitutionError::InvalidApplication(*source_id));
            }
            enum_layouts.insert(*target_id, layout);
        }
    }
    drop(relocation);
    for (source_id, target_id, is_new, representation) in completed {
        if is_new {
            target.define_nominal(target_id, representation);
        } else if !matches!(target.store().get(target_id), Some(Type::Nominal(decl)) if decl.representation == representation)
        {
            return Err(TypeSubstitutionError::InvalidApplication(source_id));
        }
    }
    Ok(ForeignTypeGraph {
        interner: target,
        declarations,
        type_generics,
        enum_layouts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CallableGenericParamKind, CheckedSpecBound, EnumVariantLayoutFact, FieldType,
        NominalTypeRef, PrimitiveType, RecordType,
    };
    use etas_code::{DefinitionKind, PackageKey};

    fn identity(name: &str) -> DefinitionKey {
        DefinitionKey::new(
            PackageKey::from_name_and_version("caller", "1.0.0").unwrap(),
            vec!["app".into()],
            vec![name.into()],
            DefinitionKind::Type,
        )
        .unwrap()
    }

    #[test]
    fn relocates_recursive_nominal_and_enum_without_reusing_source_ids() {
        let mut source = TypeInterner::new();
        let nominal = source.intern(Type::Nominal(NominalTypeRef {
            name: "Directory".into(),
            params: vec![],
            representation: None,
            read_only_erased_view: false,
        }));
        let enumeration = source.intern(Type::Enum(super::super::EnumTypeRef {
            name: "Entry".into(),
        }));
        let representation = source.intern(Type::Record(RecordType {
            fields: vec![FieldType {
                name: "entry".into(),
                ty: enumeration,
            }],
        }));
        source.define_nominal(nominal, Some(representation));
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
        let mut target = TypeInterner::new();
        target.primitive(PrimitiveType::Bool);
        target.primitive(PrimitiveType::I32);
        target.primitive(PrimitiveType::String);
        let identities = HashMap::from([
            (nominal, identity("Directory")),
            (enumeration, identity("Entry")),
        ]);
        let result = relocate_foreign_type_graph(
            target,
            ForeignTypeGraphInput {
                source: source.store(),
                facts: &facts,
                target_facts: &TypeFacts::default(),
                reachable: &[nominal, representation, enumeration],
                identities: &identities,
                declarations: &HashMap::new(),
                effect_names: &HashMap::new(),
                specs: &HashMap::new(),
                max_depth: 16,
                budget: &TypeSubstitutionBudget::new(100, 100, None),
            },
        )
        .unwrap();
        let mapped = result.declarations[&nominal];
        let mapped_enum = result.declarations[&enumeration];
        assert_ne!(mapped, nominal);
        assert_eq!(
            result.interner.store().nominal_identity(mapped),
            Some(&identity("Directory"))
        );
        let Some(Type::Nominal(decl)) = result.interner.store().get(mapped) else {
            panic!("missing nominal")
        };
        let Some(Type::Record(record)) = decl
            .representation
            .and_then(|id| result.interner.store().get(id))
        else {
            panic!("missing representation")
        };
        assert_eq!(record.fields[0].ty, mapped_enum);
        assert_eq!(
            result.enum_layouts[&mapped_enum].variants[0].fields,
            vec![mapped]
        );
    }

    #[test]
    fn generic_bounds_require_checked_binder_and_spec_mappings() {
        let mut source = TypeInterner::new();
        let generic = source.intern(Type::Named(super::super::NamedTypeRef {
            name: "T".into(),
            source_parameter: None,
        }));
        let nominal = source.intern(Type::Nominal(NominalTypeRef {
            name: "Box".into(),
            params: vec!["T".into()],
            representation: Some(generic),
            read_only_erased_view: false,
        }));
        let spec = CheckedSpecRef::Std(vec!["std".into(), "Allowed".into()]);
        let mut facts = TypeFacts::default();
        facts.type_generics.insert(
            nominal,
            vec![CallableGenericParam {
                kind: CallableGenericParamKind::Type,
                name: "T".into(),
                subject: generic,
                bounds: vec![CheckedSpecBound {
                    spec: spec.clone(),
                    args: vec![],
                }],
            }],
        );
        let mut target = TypeInterner::new();
        target.primitive(PrimitiveType::Bool);
        let mapped_generic = target.intern(Type::Named(super::super::NamedTypeRef {
            name: "T".into(),
            source_parameter: None,
        }));
        let identities = HashMap::from([(nominal, identity("Box"))]);
        let declarations = HashMap::from([(generic, mapped_generic)]);
        let missing_spec = relocate_foreign_type_graph(
            target.clone(),
            ForeignTypeGraphInput {
                source: source.store(),
                facts: &facts,
                target_facts: &TypeFacts::default(),
                reachable: &[nominal, generic],
                identities: &identities,
                declarations: &declarations,
                effect_names: &HashMap::new(),
                specs: &HashMap::new(),
                max_depth: 16,
                budget: &TypeSubstitutionBudget::new(100, 100, None),
            },
        );
        assert!(matches!(
            missing_spec,
            Err(TypeSubstitutionError::MissingSpecIdentity(_))
        ));
        let specs = HashMap::from([(spec.clone(), spec)]);
        let result = relocate_foreign_type_graph(
            target,
            ForeignTypeGraphInput {
                source: source.store(),
                facts: &facts,
                target_facts: &TypeFacts::default(),
                reachable: &[nominal, generic],
                identities: &identities,
                declarations: &declarations,
                effect_names: &HashMap::new(),
                specs: &specs,
                max_depth: 16,
                budget: &TypeSubstitutionBudget::new(100, 100, None),
            },
        )
        .unwrap();
        assert_eq!(
            result.type_generics[&result.declarations[&nominal]][0].subject,
            mapped_generic
        );
    }

    #[test]
    fn nominal_representation_relocates_its_existential_spec_identity() {
        let mut source = TypeInterner::new();
        let from = CheckedSpecRef::Source(etas_hir::SymbolId(17));
        let to = CheckedSpecRef::Canonical(
            DefinitionKey::new(
                PackageKey::from_name_and_version("caller", "1.0.0").unwrap(),
                vec!["app".into()],
                vec!["Marker".into()],
                DefinitionKind::Spec,
            )
            .unwrap(),
        );
        let object = source.intern(Type::Existential {
            bounds: vec![CheckedSpecBound {
                spec: from.clone(),
                args: vec![],
            }],
        });
        let nominal = source.intern(Type::Nominal(NominalTypeRef {
            name: "Holder".into(),
            params: vec![],
            representation: Some(object),
            read_only_erased_view: false,
        }));
        let result = relocate_foreign_type_graph(
            TypeInterner::new(),
            ForeignTypeGraphInput {
                source: source.store(),
                facts: &TypeFacts::default(),
                target_facts: &TypeFacts::default(),
                reachable: &[nominal, object],
                identities: &HashMap::from([(nominal, identity("Holder"))]),
                declarations: &HashMap::new(),
                effect_names: &HashMap::new(),
                specs: &HashMap::from([(from, to.clone())]),
                max_depth: 32,
                budget: &TypeSubstitutionBudget::new(100, 100, None),
            },
        )
        .unwrap();
        let Some(Type::Nominal(holder)) =
            result.interner.store().get(result.declarations[&nominal])
        else {
            panic!("holder")
        };
        let Some(Type::Existential { bounds }) =
            result.interner.store().get(holder.representation.unwrap())
        else {
            panic!("existential representation")
        };
        assert_eq!(bounds[0].spec, to);
    }

    #[test]
    fn checked_identity_cannot_hide_a_conflicting_nominal_layout() {
        let mut source = TypeInterner::new();
        let string = source.primitive(PrimitiveType::String);
        let nominal = source.intern(Type::Nominal(NominalTypeRef {
            name: "Token".into(),
            params: vec![],
            representation: Some(string),
            read_only_erased_view: false,
        }));
        let mut target = TypeInterner::new();
        let integer = target.primitive(PrimitiveType::I32);
        target
            .intern_canonical_nominal(
                identity("Token"),
                Type::Nominal(NominalTypeRef {
                    name: "Token".into(),
                    params: vec![],
                    representation: Some(integer),
                    read_only_erased_view: false,
                }),
            )
            .unwrap();
        let result = relocate_foreign_type_graph(
            target,
            ForeignTypeGraphInput {
                source: source.store(),
                facts: &TypeFacts::default(),
                target_facts: &TypeFacts::default(),
                reachable: &[nominal, string],
                identities: &HashMap::from([(nominal, identity("Token"))]),
                declarations: &HashMap::new(),
                effect_names: &HashMap::new(),
                specs: &HashMap::new(),
                max_depth: 16,
                budget: &TypeSubstitutionBudget::new(100, 100, None),
            },
        );
        assert!(
            matches!(result, Err(TypeSubstitutionError::InvalidApplication(id)) if id == nominal)
        );
    }
}
