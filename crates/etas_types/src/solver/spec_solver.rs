use std::collections::{HashMap, HashSet};
mod external;

use crate::{
    CheckedSpecRef, NamedTypeRef, SpecFacts, SpecObligation, Type, TypeId, TypeStore, TypeUnifier,
    solver::report::{SolverFailure, SolverReport},
};

pub fn solve_spec_obligations(
    store: &TypeStore,
    facts: &SpecFacts,
    obligations: &[SpecObligation],
    named_substitutions: &HashMap<String, TypeId>,
) -> SolverReport {
    let mut report = SolverReport::default();
    for obligation in obligations {
        let ty = resolve_named(store, obligation.ty, named_substitutions);
        let args = obligation
            .args
            .iter()
            .copied()
            .map(|arg| resolve_named(store, arg, named_substitutions))
            .collect::<Vec<_>>();
        let mut visited = HashSet::new();
        let satisfied = match &obligation.spec {
            CheckedSpecRef::Source(spec_symbol) => {
                type_satisfies_spec(store, facts, ty, *spec_symbol, &args, &mut visited)
            }
            CheckedSpecRef::Std(path) => std_type_satisfies_spec(store, facts, ty, path, &args),
            CheckedSpecRef::External(identity) => {
                external::type_satisfies(store, facts, ty, identity, &args)
            }
            CheckedSpecRef::Canonical(identity) => {
                external::canonical_type_satisfies(store, facts, ty, identity, &args)
            }
        };
        if !satisfied {
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                span: obligation.span,
                message: format!(
                    "{} does not satisfy spec bound `{}`",
                    display_type_name(store, ty),
                    checked_spec_name(facts, &obligation.spec)
                ),
            });
        }
    }
    report
}

pub fn satisfies_spec(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec_symbol: etas_hir::SymbolId,
    args: &[TypeId],
) -> bool {
    type_satisfies_spec(store, facts, ty, spec_symbol, args, &mut HashSet::new())
}

fn type_satisfies_spec(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec_symbol: etas_hir::SymbolId,
    args: &[TypeId],
    visited: &mut HashSet<(TypeId, etas_hir::SymbolId)>,
) -> bool {
    if let Some(identity) = facts.external_aliases.get(&spec_symbol) {
        return external::type_satisfies(store, facts, ty, identity, args);
    }
    if !visited.insert((ty, spec_symbol)) {
        return false;
    }
    if type_param_bound_satisfies_spec(store, facts, ty, spec_symbol, args) {
        return true;
    }
    if let Some(path) = facts.std_spec_aliases.get(&spec_symbol)
        && std_type_satisfies_spec(store, facts, ty, path, args)
    {
        return true;
    }
    if external::evidence_satisfies(store, facts, ty, &CheckedSpecRef::Source(spec_symbol), args) {
        return true;
    }
    facts.impls.iter().any(|impl_fact| {
        type_same(store, impl_fact.self_type, ty)
            && external::entails(
                store,
                facts,
                CheckedSpecRef::Source(impl_fact.spec_symbol),
                &impl_fact.args,
                &CheckedSpecRef::Source(spec_symbol),
                args,
            )
    })
}

pub(crate) fn std_type_satisfies_spec(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec: &[String],
    args: &[TypeId],
) -> bool {
    if std_type_param_bound_satisfies_spec(store, facts, ty, spec, args) {
        return true;
    }
    if external::evidence_satisfies(store, facts, ty, &CheckedSpecRef::Std(spec.to_vec()), args) {
        return true;
    }
    facts.std_impls.iter().any(|implementation| {
        implementation.spec == spec
            && type_same(store, implementation.self_type, ty)
            && args_match(store, &implementation.args, args)
    }) || facts.impls.iter().any(|implementation| {
        facts
            .std_spec_aliases
            .get(&implementation.spec_symbol)
            .is_some_and(|path| path == spec)
            && type_same(store, implementation.self_type, ty)
            && args_match(store, &implementation.args, args)
    })
}

fn std_type_param_bound_satisfies_spec(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec: &[String],
    args: &[TypeId],
) -> bool {
    let Some(Type::Named(NamedTypeRef {
        source_parameter: Some(parameter),
        ..
    })) = store.get(ty)
    else {
        return false;
    };
    facts
        .type_param_bounds
        .get(parameter)
        .into_iter()
        .flatten()
        .any(|bound| {
            facts
                .std_spec_aliases
                .get(&bound.spec_symbol)
                .is_some_and(|path| path == spec && args_match(store, &bound.args, args))
        })
}

fn type_param_bound_satisfies_spec(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec_symbol: etas_hir::SymbolId,
    args: &[TypeId],
) -> bool {
    let Some(Type::Named(NamedTypeRef {
        source_parameter: Some(parameter),
        ..
    })) = store.get(ty)
    else {
        return false;
    };
    facts
        .type_param_bounds
        .get(parameter)
        .into_iter()
        .flatten()
        .any(|bound| {
            external::entails(
                store,
                facts,
                CheckedSpecRef::Source(bound.spec_symbol),
                &bound.args,
                &CheckedSpecRef::Source(spec_symbol),
                args,
            )
        })
}

fn args_match(store: &TypeStore, actual: &[TypeId], expected: &[TypeId]) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .copied()
            .zip(expected.iter().copied())
            .all(|(actual, expected)| arg_matches(store, actual, expected))
}

fn arg_matches(store: &TypeStore, actual: TypeId, expected: TypeId) -> bool {
    if type_same(store, actual, expected) {
        return true;
    }
    matches!(
        store.get(expected),
        Some(Type::Named(NamedTypeRef { name, source_parameter: None })) if store.parameter_identity(expected).is_none() && is_schematic_type_param(name)
    )
}

fn type_same(store: &TypeStore, lhs: TypeId, rhs: TypeId) -> bool {
    if lhs == rhs {
        return true;
    }
    let mut unifier = TypeUnifier::new(store);
    unifier.unify(lhs, rhs).is_ok()
}

fn resolve_named(store: &TypeStore, ty: TypeId, substitutions: &HashMap<String, TypeId>) -> TypeId {
    match store.get(ty) {
        Some(Type::Named(NamedTypeRef { name, .. })) if store.parameter_identity(ty).is_none() => {
            substitutions.get(name).copied().unwrap_or(ty)
        }
        _ => ty,
    }
}

fn spec_name(facts: &SpecFacts, symbol: etas_hir::SymbolId) -> String {
    facts
        .signatures
        .get(&symbol)
        .map(|signature| signature.name.clone())
        .unwrap_or_else(|| format!("symbol{}", symbol.0))
}

fn checked_spec_name(facts: &SpecFacts, spec: &CheckedSpecRef) -> String {
    match spec {
        CheckedSpecRef::Source(symbol) => spec_name(facts, *symbol),
        CheckedSpecRef::Std(path) => path.join("."),
        CheckedSpecRef::External(identity) => identity.path.join("."),
        CheckedSpecRef::Canonical(identity) => identity
            .module()
            .iter()
            .chain(identity.declaration())
            .cloned()
            .collect::<Vec<_>>()
            .join("."),
    }
}

fn display_type_name(store: &TypeStore, ty: TypeId) -> String {
    match store.get(ty) {
        Some(Type::Named(name)) => name.name.clone(),
        Some(Type::Nominal(nominal)) => nominal.name.clone(),
        Some(Type::Primitive(primitive)) => format!("{primitive:?}"),
        _ => format!("type{}", ty.0),
    }
}

fn is_schematic_type_param(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

#[cfg(test)]
mod canonical_parameter_tests {
    use super::*;

    #[test]
    fn foreign_binder_is_not_a_wildcard_or_unrelated_named_binding() {
        let owner = etas_code::DefinitionKey::new(
            etas_code::PackageKey::from_name_and_version("caller", "1.0.0").unwrap(),
            vec!["app".into()],
            vec!["Tree".into()],
            etas_code::DefinitionKind::Type,
        )
        .unwrap();
        let mut types = crate::TypeInterner::new();
        let parameter = types
            .intern_canonical_parameter(
                crate::ty::CanonicalTypeParameterIdentity::new(owner, 0).unwrap(),
                "T".into(),
            )
            .unwrap();
        let boolean = types.primitive(crate::PrimitiveType::Bool);
        assert!(!arg_matches(types.store(), boolean, parameter));
        assert_eq!(
            resolve_named(
                types.store(),
                parameter,
                &HashMap::from([("T".into(), boolean)])
            ),
            parameter
        );
    }

    #[test]
    fn canonical_spec_requires_checked_impl_and_super_spec_facts() {
        use crate::facts::specs::{CanonicalSpecImplFact, CanonicalSpecSignatureFact};
        use etas_code::{DefinitionKey, DefinitionKind, PackageKey};
        let key = |package: &str, name: &str| {
            DefinitionKey::new(
                PackageKey::from_name_and_version(package, "1.0.0").unwrap(),
                vec!["app".into()],
                vec![name.into()],
                DefinitionKind::Spec,
            )
            .unwrap()
        };
        let base = key("first", "Base");
        let derived = key("first", "Derived");
        let same_name_other_package = key("other", "Base");
        let mut types = crate::TypeInterner::new();
        let actual = types.primitive(crate::PrimitiveType::Bool);
        let unrelated = types.primitive(crate::PrimitiveType::I32);
        let mut facts = SpecFacts::default();
        facts.canonical.signatures.insert(
            base.clone(),
            CanonicalSpecSignatureFact {
                kind: crate::SpecKind::TypeSpec,
                param_names: vec![],
                param_kinds: vec![],
                param_bounds: vec![],
                super_specs: vec![],
            },
        );
        facts.canonical.signatures.insert(
            derived.clone(),
            CanonicalSpecSignatureFact {
                kind: crate::SpecKind::TypeSpec,
                param_names: vec![],
                param_kinds: vec![],
                param_bounds: vec![],
                super_specs: vec![crate::CheckedSpecBound {
                    spec: CheckedSpecRef::Canonical(base.clone()),
                    args: vec![],
                }],
            },
        );
        facts.canonical.impls.push(CanonicalSpecImplFact {
            self_type: actual,
            spec: derived,
            args: vec![],
        });
        let obligation = |ty, spec| SpecObligation {
            ty,
            spec: CheckedSpecRef::Canonical(spec),
            args: vec![],
            span: etas_core::Span::new(etas_core::SourceId(0), etas_core::TextRange::default()),
        };
        assert!(
            solve_spec_obligations(
                types.store(),
                &facts,
                &[obligation(actual, base.clone())],
                &HashMap::new()
            )
            .failures
            .is_empty()
        );
        assert!(
            !solve_spec_obligations(
                types.store(),
                &facts,
                &[obligation(unrelated, base)],
                &HashMap::new()
            )
            .failures
            .is_empty()
        );
        assert!(
            !solve_spec_obligations(
                types.store(),
                &facts,
                &[obligation(actual, same_name_other_package)],
                &HashMap::new()
            )
            .failures
            .is_empty()
        );
    }

    #[test]
    fn canonical_super_spec_checks_intermediate_generic_bounds() {
        use crate::facts::specs::{CanonicalSpecImplFact, CanonicalSpecSignatureFact};
        use etas_code::{DefinitionKey, DefinitionKind, PackageKey};
        let package = PackageKey::from_name_and_version("caller", "1.0.0").unwrap();
        let key = |name: &str| {
            DefinitionKey::new(
                package,
                vec!["app".into()],
                vec![name.into()],
                DefinitionKind::Spec,
            )
            .unwrap()
        };
        let base = key("Base");
        let derived = key("Derived");
        let marker = key("Marker");
        let mut types = crate::TypeInterner::new();
        let subject = types.primitive(crate::PrimitiveType::String);
        let argument = types.primitive(crate::PrimitiveType::Bool);
        let binder = types
            .intern_canonical_parameter(
                crate::ty::CanonicalTypeParameterIdentity::new(derived.clone(), 0).unwrap(),
                "T".into(),
            )
            .unwrap();
        let mut facts = SpecFacts::default();
        let signature = |names: Vec<String>, bounds, supers| CanonicalSpecSignatureFact {
            kind: crate::SpecKind::TypeSpec,
            param_kinds: vec![crate::CallableGenericParamKind::Type; names.len()],
            param_names: names,
            param_bounds: bounds,
            super_specs: supers,
        };
        facts
            .canonical
            .signatures
            .insert(marker.clone(), signature(vec![], vec![], vec![]));
        facts.canonical.signatures.insert(
            base.clone(),
            signature(vec!["T".into()], vec![vec![]], vec![]),
        );
        facts.canonical.signatures.insert(
            derived.clone(),
            signature(
                vec!["T".into()],
                vec![vec![crate::CheckedSpecBound {
                    spec: CheckedSpecRef::Canonical(marker.clone()),
                    args: vec![],
                }]],
                vec![crate::CheckedSpecBound {
                    spec: CheckedSpecRef::Canonical(base.clone()),
                    args: vec![binder],
                }],
            ),
        );
        facts.canonical.impls.push(CanonicalSpecImplFact {
            self_type: subject,
            spec: derived,
            args: vec![argument],
        });
        let obligation = SpecObligation {
            ty: subject,
            spec: CheckedSpecRef::Canonical(base),
            args: vec![argument],
            span: etas_core::Span::new(etas_core::SourceId(0), etas_core::TextRange::default()),
        };
        assert!(
            !solve_spec_obligations(
                types.store(),
                &facts,
                std::slice::from_ref(&obligation),
                &HashMap::new()
            )
            .failures
            .is_empty()
        );
        facts.canonical.impls.push(CanonicalSpecImplFact {
            self_type: argument,
            spec: marker,
            args: vec![],
        });
        assert!(
            solve_spec_obligations(types.store(), &facts, &[obligation], &HashMap::new())
                .failures
                .is_empty()
        );
    }

    #[test]
    fn mixed_external_canonical_generic_bound_cycles_fail_closed() {
        use crate::facts::specs::{
            CanonicalSpecSignatureFact, ExternalSpecIdentity, ExternalSpecSignatureFact,
        };
        use etas_code::{DefinitionKey, DefinitionKind, PackageKey};
        let canonical = DefinitionKey::new(
            PackageKey::from_name_and_version("caller", "1.0.0").unwrap(),
            vec!["app".into()],
            vec!["A".into()],
            DefinitionKind::Spec,
        )
        .unwrap();
        let external = ExternalSpecIdentity {
            package: 7,
            path: vec!["dep".into(), "B".into()],
        };
        let mut types = crate::TypeInterner::new();
        let argument = types.primitive(crate::PrimitiveType::Bool);
        let canonical_t = types
            .intern_canonical_parameter(
                crate::ty::CanonicalTypeParameterIdentity::new(canonical.clone(), 0).unwrap(),
                "T".into(),
            )
            .unwrap();
        let external_t = types.intern(crate::Type::Named(crate::NamedTypeRef {
            name: "T".into(),
            source_parameter: None,
        }));
        let mut facts = SpecFacts::default();
        facts.canonical.signatures.insert(
            canonical.clone(),
            CanonicalSpecSignatureFact {
                kind: crate::SpecKind::TypeSpec,
                param_names: vec!["T".into()],
                param_kinds: vec![crate::CallableGenericParamKind::Type],
                param_bounds: vec![vec![crate::CheckedSpecBound {
                    spec: CheckedSpecRef::External(external.clone()),
                    args: vec![canonical_t],
                }]],
                super_specs: vec![],
            },
        );
        facts.external_signatures.insert(
            external,
            ExternalSpecSignatureFact {
                kind: crate::SpecKind::TypeSpec,
                param_names: vec!["T".into()],
                generic_params: vec![crate::CallableGenericParam {
                    kind: crate::CallableGenericParamKind::Type,
                    name: "T".into(),
                    subject: external_t,
                    bounds: vec![crate::CheckedSpecBound {
                        spec: CheckedSpecRef::Canonical(canonical.clone()),
                        args: vec![external_t],
                    }],
                }],
                methods: vec![],
                super_specs: vec![],
            },
        );
        let obligation = SpecObligation {
            ty: argument,
            spec: CheckedSpecRef::Canonical(canonical),
            args: vec![argument],
            span: etas_core::Span::new(etas_core::SourceId(0), etas_core::TextRange::default()),
        };
        assert!(
            !solve_spec_obligations(types.store(), &facts, &[obligation], &HashMap::new())
                .failures
                .is_empty()
        );
    }

    #[test]
    fn canonical_spec_closure_checks_arguments_and_terminates_on_cycles() {
        use crate::facts::specs::{CanonicalSpecImplFact, CanonicalSpecSignatureFact};
        use etas_code::{DefinitionKey, DefinitionKind, PackageKey};
        let key = |name: &str| {
            DefinitionKey::new(
                PackageKey::from_name_and_version("caller", "1.0.0").unwrap(),
                vec!["app".into()],
                vec![name.into()],
                DefinitionKind::Spec,
            )
            .unwrap()
        };
        let base = key("Base");
        let derived = key("Derived");
        let unrelated = key("Unrelated");
        let mut types = crate::TypeInterner::new();
        let subject = types.primitive(crate::PrimitiveType::String);
        let boolean = types.primitive(crate::PrimitiveType::Bool);
        let integer = types.primitive(crate::PrimitiveType::I32);
        let mut facts = SpecFacts::default();
        facts.canonical.signatures.insert(
            base.clone(),
            CanonicalSpecSignatureFact {
                kind: crate::SpecKind::TypeSpec,
                param_names: vec!["T".into()],
                param_kinds: vec![crate::CallableGenericParamKind::Type],
                param_bounds: vec![vec![]],
                super_specs: vec![crate::CheckedSpecBound {
                    spec: CheckedSpecRef::Canonical(derived.clone()),
                    args: vec![],
                }],
            },
        );
        facts.canonical.signatures.insert(
            derived.clone(),
            CanonicalSpecSignatureFact {
                kind: crate::SpecKind::TypeSpec,
                param_names: vec![],
                param_kinds: vec![],
                param_bounds: vec![],
                super_specs: vec![crate::CheckedSpecBound {
                    spec: CheckedSpecRef::Canonical(base.clone()),
                    args: vec![boolean],
                }],
            },
        );
        facts.canonical.signatures.insert(
            unrelated.clone(),
            CanonicalSpecSignatureFact {
                kind: crate::SpecKind::TypeSpec,
                param_names: vec![],
                param_kinds: vec![],
                param_bounds: vec![],
                super_specs: vec![],
            },
        );
        facts.canonical.impls.push(CanonicalSpecImplFact {
            self_type: subject,
            spec: derived,
            args: vec![],
        });
        let obligation = |spec, args| SpecObligation {
            ty: subject,
            spec: CheckedSpecRef::Canonical(spec),
            args,
            span: etas_core::Span::new(etas_core::SourceId(0), etas_core::TextRange::default()),
        };
        assert!(
            solve_spec_obligations(
                types.store(),
                &facts,
                &[obligation(base.clone(), vec![boolean])],
                &HashMap::new()
            )
            .failures
            .is_empty()
        );
        assert!(
            !solve_spec_obligations(
                types.store(),
                &facts,
                &[obligation(base, vec![integer])],
                &HashMap::new()
            )
            .failures
            .is_empty()
        );
        assert!(
            !solve_spec_obligations(
                types.store(),
                &facts,
                &[obligation(unrelated, vec![])],
                &HashMap::new()
            )
            .failures
            .is_empty()
        );
    }
}
