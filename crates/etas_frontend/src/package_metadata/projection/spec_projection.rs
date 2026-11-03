use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use etas_code::{DefinitionKey, DefinitionKind};
use etas_types::{
    CheckedSpecBound, CheckedSpecRef, SpecKind, TypeId, TypeStore, TypeSubstitutionEngine,
    facts::specs::{
        CanonicalSpecFacts, ExternalSpecFacts, ExternalSpecIdentity, ExternalSpecSignatureFact,
    },
    ty::{CanonicalTypeParameterIdentity, TypeSubstitutionBudget},
};

use crate::{
    ProjectExternalDefinitionOriginsInput, ProjectExternalModuleInput,
    ProjectExternalPublicMetadataInput, package_metadata::PackageMetadataError,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SpecIdentity {
    Standard(Vec<String>),
    Declaration(DefinitionKey),
}

type BoundIdentity = (SpecIdentity, Vec<TypeId>);

fn invalid(reason: &'static str) -> PackageMetadataError {
    PackageMetadataError::InvalidMetadataType {
        reason: reason.into(),
    }
}

struct Proof<'a> {
    origins: &'a [ProjectExternalDefinitionOriginsInput],
    canonical: &'a CanonicalSpecFacts,
    external: &'a ExternalSpecFacts,
    metadata: &'a [ProjectExternalPublicMetadataInput],
    modules: &'a [ProjectExternalModuleInput],
    marker_proof: &'a dyn Fn(&DefinitionKey) -> bool,
    remaining: usize,
}

impl Proof<'_> {
    fn charge(&mut self, amount: usize) -> Result<(), PackageMetadataError> {
        self.remaining = self
            .remaining
            .checked_sub(amount)
            .ok_or_else(|| invalid("canonical spec metadata proof exceeds its resource limit"))?;
        Ok(())
    }

    fn external_identity(
        &mut self,
        spec: &ExternalSpecIdentity,
        arity: usize,
    ) -> Result<DefinitionKey, PackageMetadataError> {
        let signature = self
            .external
            .signatures
            .get(spec)
            .ok_or_else(|| invalid("external spec bound has no checked declaration"))?;
        if signature.kind != SpecKind::TypeSpec
            || signature.param_names.len() != arity
            || signature.generic_params.len() != arity
        {
            return Err(invalid(
                "external spec bound has an incompatible checked declaration",
            ));
        }
        self.require_public_spec(spec, signature)?;
        let mut found = None;
        for origin in self.origins {
            self.charge(1)?;
            if origin.package.0 != spec.package {
                continue;
            }
            for binding in &origin.origins.declarations {
                self.charge(1)?;
                if binding.path() == spec.path
                    && (binding.definition.kind() != DefinitionKind::Spec
                        || found.replace(binding.definition.clone()).is_some())
                {
                    return Err(invalid(
                        "external spec bound has ambiguous or invalid origin",
                    ));
                }
            }
        }
        found.ok_or_else(|| invalid("external spec bound has no verified package origin"))
    }

    fn require_public_spec(
        &mut self,
        spec: &ExternalSpecIdentity,
        signature: &ExternalSpecSignatureFact,
    ) -> Result<(), PackageMetadataError> {
        let mut public_signature = false;
        for package in self.metadata {
            self.charge(1)?;
            if package.package.0 != spec.package {
                continue;
            }
            for item in &package.spec_signatures {
                self.charge(1)?;
                if item.path == spec.path && item.visibility == "public" {
                    if item.callable.is_some()
                        || !item.methods.is_empty()
                        || !signature.methods.is_empty()
                    {
                        return Err(invalid(
                            "behavioral spec requires checked method correspondence",
                        ));
                    }
                    if item.kind != crate::ProjectExternalSpecKindInput::Type
                        || item.param_names != signature.param_names
                        || item.generic_params.len() != signature.generic_params.len()
                        || item.generic_params.iter().zip(&signature.generic_params).any(|(input, checked)| {
                            input.name != checked.name || !matches!((input.kind, checked.kind),
                                (crate::ProjectExternalCallableGenericParamKindInput::Type, etas_types::CallableGenericParamKind::Type)
                                | (crate::ProjectExternalCallableGenericParamKindInput::Effect, etas_types::CallableGenericParamKind::Effect))
                        })
                    {
                        return Err(invalid("public spec metadata disagrees with checked signature"));
                    }
                    if public_signature {
                        return Err(invalid("canonical spec has duplicate public declarations"));
                    }
                    public_signature = true;
                }
            }
        }
        let mut public_export = false;
        for module in self.modules {
            self.charge(1)?;
            if module.package.map(|package| package.0) != Some(spec.package)
                || spec.path.len() != module.path.segments.len() + 1
                || !spec.path.starts_with(&module.path.segments)
            {
                continue;
            }
            for item in &module.exports {
                self.charge(1)?;
                if spec.path.last() == Some(&item.name)
                    && item.visibility == etas_hir::Visibility::Public
                {
                    if public_export {
                        return Err(invalid("canonical spec has duplicate public exports"));
                    }
                    public_export = true;
                }
            }
        }
        if !public_signature || !public_export {
            return Err(invalid("canonical spec has no public external declaration"));
        }
        Ok(())
    }

    fn bound_identity(
        &mut self,
        bound: &CheckedSpecBound,
    ) -> Result<SpecIdentity, PackageMetadataError> {
        self.charge(1)?;
        match &bound.spec {
            CheckedSpecRef::Std(path) => Ok(SpecIdentity::Standard(path.clone())),
            CheckedSpecRef::Canonical(key) if key.kind() == DefinitionKind::Spec => {
                let signature =
                    self.canonical.signatures.get(key).ok_or_else(|| {
                        invalid("canonical spec bound has no checked declaration")
                    })?;
                if signature.kind != SpecKind::TypeSpec
                    || signature.param_names.len() != bound.args.len()
                    || signature.param_kinds.len() != bound.args.len()
                    || signature.param_bounds.len() != bound.args.len()
                {
                    return Err(invalid(
                        "canonical spec bound has an incompatible checked declaration",
                    ));
                }
                Ok(SpecIdentity::Declaration(key.clone()))
            }
            CheckedSpecRef::External(spec) => self
                .external_identity(spec, bound.args.len())
                .map(SpecIdentity::Declaration),
            _ => Err(invalid("spec bound has no portable checked identity")),
        }
    }

    fn bounds(
        &mut self,
        bounds: &[CheckedSpecBound],
        mut substitution: Option<&mut TypeSubstitutionEngine<'_>>,
        pending: &mut VecDeque<(DefinitionKey, ExternalSpecIdentity)>,
    ) -> Result<Vec<BoundIdentity>, PackageMetadataError> {
        let external_side = substitution.is_some();
        self.charge(bounds.len())?;
        let mut result = Vec::with_capacity(bounds.len());
        for bound in bounds {
            self.charge(bound.args.len())?;
            let spec = self.bound_identity(bound)?;
            match (&bound.spec, &spec, external_side) {
                (CheckedSpecRef::External(external), SpecIdentity::Declaration(identity), true) => {
                    pending.push_back((identity.clone(), external.clone()));
                }
                (CheckedSpecRef::Canonical(_), _, true)
                | (CheckedSpecRef::External(_), _, false) => {
                    return Err(invalid(
                        "spec bound belongs to the wrong checked identity space",
                    ));
                }
                _ => {}
            }
            let mut args = Vec::with_capacity(bound.args.len());
            for ty in &bound.args {
                args.push(match substitution.as_deref_mut() {
                    Some(substitution) => substitution.substitute(*ty)
                        .map_err(|_| invalid("external spec bound type cannot be checked against its canonical declaration"))?,
                    None => *ty,
                });
            }
            result.push((spec, args));
        }
        result.sort();
        Ok(result)
    }
}

pub(super) fn checked_canonical_spec_path(
    identity: &DefinitionKey,
    canonical: &CanonicalSpecFacts,
    external: &ExternalSpecFacts,
    origins: &[ProjectExternalDefinitionOriginsInput],
    environment: (
        &[ProjectExternalPublicMetadataInput],
        &[ProjectExternalModuleInput],
    ),
    store: &TypeStore,
    marker_proof: &dyn Fn(&DefinitionKey) -> bool,
) -> Result<Vec<String>, PackageMetadataError> {
    if identity.kind() != DefinitionKind::Spec {
        return Err(invalid("canonical spec identity has the wrong kind"));
    }
    if !canonical.signatures.contains_key(identity) {
        return Err(invalid(
            "canonical spec declaration is missing checked facts",
        ));
    }
    let limit = etas_package_metadata::MAX_METADATA_GRAPH_NODES;
    let (metadata, modules) = environment;
    let mut proof = Proof {
        origins,
        canonical,
        external,
        metadata,
        modules,
        marker_proof,
        remaining: limit,
    };
    let mut selected: Option<ExternalSpecIdentity> = None;
    for origin in origins {
        proof.charge(1)?;
        if origin.origins.package != identity.package() {
            continue;
        }
        for binding in &origin.origins.declarations {
            proof.charge(1)?;
            if &binding.definition != identity {
                continue;
            }
            let path = binding.path();
            let key = ExternalSpecIdentity {
                package: origin.package.0,
                path: path.clone(),
            };
            let Some(signature) = external.signatures.get(&key) else {
                continue;
            };
            proof.require_public_spec(&key, signature)?;
            if selected.replace(key).is_some() {
                return Err(invalid(
                    "canonical spec has more than one checked external path",
                ));
            }
        }
    }
    let selected = selected
        .ok_or_else(|| invalid("canonical spec has no package-bound external declaration"))?;
    let path = selected.path.clone();
    proof.charge(store.len())?;
    let mut binders = BTreeMap::new();
    for (ty, _) in store.iter() {
        if let Some(key) = store.parameter_identity(ty) {
            if binders.insert(key.clone(), ty).is_some() {
                return Err(invalid("canonical spec binder identity is ambiguous"));
            }
        }
    }
    let budget = TypeSubstitutionBudget::new(limit, limit, None);
    let mut pending = VecDeque::from([(identity.clone(), selected)]);
    let mut seen = HashSet::new();
    while let Some((key, external_key)) = pending.pop_front() {
        proof.charge(1)?;
        if !seen.insert((key.clone(), external_key.clone())) {
            continue;
        }
        if !(proof.marker_proof)(&key) {
            return Err(invalid(
                "canonical spec is missing checked marker-spec proof",
            ));
        }
        let declaration = canonical
            .signatures
            .get(&key)
            .ok_or_else(|| invalid("canonical spec declaration is missing checked facts"))?;
        let signature = external
            .signatures
            .get(&external_key)
            .ok_or_else(|| invalid("external spec bound has no checked declaration"))?;
        let arity = signature.param_names.len();
        if declaration.kind != SpecKind::TypeSpec
            || declaration.param_names.len() != arity
            || declaration.param_kinds.len() != arity
            || declaration.param_bounds.len() != arity
            || proof.external_identity(&external_key, arity)? != key
        {
            return Err(invalid("canonical and external spec declarations differ"));
        }
        let mut names = HashMap::new();
        for (index, (name, parameter)) in signature
            .param_names
            .iter()
            .zip(&signature.generic_params)
            .enumerate()
        {
            proof.charge(1)?;
            if name != &parameter.name || declaration.param_kinds[index] != parameter.kind {
                return Err(invalid(
                    "canonical and external spec generic parameters differ",
                ));
            }
            if !matches!(store.get(parameter.subject), Some(etas_types::Type::Named(named)) if named.source_parameter.is_none() && named.name == *name)
                || store.parameter_identity(parameter.subject).is_some()
            {
                return Err(invalid(
                    "external spec generic binder lacks its checked identity",
                ));
            }
            let index = u32::try_from(index)
                .map_err(|_| invalid("canonical spec has too many generic parameters"))?;
            let binder = CanonicalTypeParameterIdentity::new(key.clone(), index)
                .map_err(|_| invalid("canonical spec generic parameter identity is invalid"))?;
            let ty = binders
                .get(&binder)
                .copied()
                .ok_or_else(|| invalid("canonical spec binder is missing"))?;
            if names.insert(name.clone(), ty).is_some() {
                return Err(invalid(
                    "external spec has duplicate generic parameter names",
                ));
            }
        }
        let mut substitution = TypeSubstitutionEngine::existing(store, &names)
            .with_expansion_depth(etas_package_metadata::MAX_METADATA_GRAPH_DEPTH)
            .with_budget(&budget);
        for (expected, actual) in declaration
            .param_bounds
            .iter()
            .zip(&signature.generic_params)
        {
            let expected = proof.bounds(expected, None, &mut pending)?;
            let actual = proof.bounds(&actual.bounds, Some(&mut substitution), &mut pending)?;
            if expected != actual {
                return Err(invalid("canonical and external spec generic bounds differ"));
            }
        }
        let expected = proof.bounds(&declaration.super_specs, None, &mut pending)?;
        let actual = proof.bounds(
            &signature.super_specs,
            Some(&mut substitution),
            &mut pending,
        )?;
        if expected != actual {
            return Err(invalid(
                "canonical and external super-spec declarations differ",
            ));
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ExternalModuleId, ExternalPackageId, ExternalSymbolId, ModulePath,
        ProjectExternalCallableGenericParamInput, ProjectExternalCallableGenericParamKindInput,
        ProjectExternalExportInput, ProjectExternalSpecBoundInput, ProjectExternalSpecKindInput,
        ProjectExternalSpecSignatureInput, ProjectExternalTypeInput,
    };
    use etas_code::{
        PackageKey,
        identity::{DefinitionBindingOrigin, DefinitionModuleOrigin, PackageDefinitionOrigins},
    };
    use etas_types::{
        CallableGenericParam, CallableGenericParamKind, NamedTypeRef, Type, TypeInterner,
        facts::specs::{CanonicalSpecSignatureFact, ExternalSpecSignatureFact},
    };

    #[test]
    fn canonical_spec_export_requires_identical_checked_signature_and_public_origin() {
        let package = PackageKey::from_name_and_version("library", "1.0.0").unwrap();
        let key = |name: &str| {
            DefinitionKey::new(
                package,
                vec!["lib".into()],
                vec![name.into()],
                DefinitionKind::Spec,
            )
            .unwrap()
        };
        let within = key("Within");
        let family = key("Family");
        let external_key = |name: &str| ExternalSpecIdentity {
            package: 1,
            path: vec!["dep".into(), name.into()],
        };
        let origins = vec![ProjectExternalDefinitionOriginsInput {
            package: ExternalPackageId(1),
            descriptors: None,
            origins: PackageDefinitionOrigins {
                package,
                modules: vec![DefinitionModuleOrigin {
                    path: vec!["dep".into()],
                    canonical_path: vec!["lib".into()],
                }],
                declarations: vec![
                    DefinitionBindingOrigin {
                        module: vec!["dep".into()],
                        declaration: vec!["Within".into()],
                        definition: within.clone(),
                    },
                    DefinitionBindingOrigin {
                        module: vec!["dep".into()],
                        declaration: vec!["Family".into()],
                        definition: family.clone(),
                    },
                ],
            },
        }];
        let mut types = TypeInterner::new();
        let parameter = types
            .intern_canonical_parameter(
                CanonicalTypeParameterIdentity::new(within.clone(), 0).unwrap(),
                "T".into(),
            )
            .unwrap();
        let canonical_family_parameter = types
            .intern_canonical_parameter(
                CanonicalTypeParameterIdentity::new(family.clone(), 0).unwrap(),
                "U".into(),
            )
            .unwrap();
        let external_parameter = types.intern(Type::Named(NamedTypeRef {
            name: "T".into(),
            source_parameter: None,
        }));
        let canonical_array = types.intern(Type::Array(parameter));
        let external_array = types.intern(Type::Array(external_parameter));
        let mut canonical = CanonicalSpecFacts::default();
        canonical.signatures.insert(
            within.clone(),
            CanonicalSpecSignatureFact {
                kind: SpecKind::TypeSpec,
                param_names: vec!["T".into()],
                param_kinds: vec![CallableGenericParamKind::Type],
                param_bounds: vec![vec![CheckedSpecBound {
                    spec: CheckedSpecRef::Canonical(family.clone()),
                    args: vec![canonical_array],
                }]],
                super_specs: vec![],
            },
        );
        canonical.signatures.insert(
            family.clone(),
            CanonicalSpecSignatureFact {
                kind: SpecKind::TypeSpec,
                param_names: vec!["U".into()],
                param_kinds: vec![CallableGenericParamKind::Type],
                param_bounds: vec![vec![]],
                super_specs: vec![],
            },
        );
        let mut external = ExternalSpecFacts::default();
        external.signatures.insert(
            external_key("Within"),
            ExternalSpecSignatureFact {
                kind: SpecKind::TypeSpec,
                param_names: vec!["T".into()],
                generic_params: vec![CallableGenericParam {
                    kind: CallableGenericParamKind::Type,
                    name: "T".into(),
                    subject: external_parameter,
                    bounds: vec![CheckedSpecBound {
                        spec: CheckedSpecRef::External(external_key("Family")),
                        args: vec![external_array],
                    }],
                }],
                methods: vec![],
                super_specs: vec![],
            },
        );
        let family_parameter = types.intern(Type::Named(NamedTypeRef {
            name: "U".into(),
            source_parameter: None,
        }));
        external.signatures.insert(
            external_key("Family"),
            ExternalSpecSignatureFact {
                kind: SpecKind::TypeSpec,
                param_names: vec!["U".into()],
                generic_params: vec![CallableGenericParam {
                    kind: CallableGenericParamKind::Type,
                    name: "U".into(),
                    subject: family_parameter,
                    bounds: vec![],
                }],
                methods: vec![],
                super_specs: vec![],
            },
        );
        let mut metadata = vec![ProjectExternalPublicMetadataInput {
            package: ExternalPackageId(1),
            types: vec![],
            values: vec![],
            enums: vec![],
            flows: vec![],
            agents: vec![],
            tools: vec![],
            tool_schemas: vec![],
            effects: vec![],
            actions: vec![],
            trace_specs: vec![],
            spec_signatures: vec![
                ProjectExternalSpecSignatureInput {
                    path: vec!["dep".into(), "Within".into()],
                    visibility: "public".into(),
                    kind: ProjectExternalSpecKindInput::Type,
                    param_names: vec!["T".into()],
                    generic_params: vec![ProjectExternalCallableGenericParamInput {
                        name: "T".into(),
                        kind: ProjectExternalCallableGenericParamKindInput::Type,
                        bounds: vec![ProjectExternalSpecBoundInput {
                            spec: vec!["dep".into(), "Family".into()],
                            args: vec![ProjectExternalTypeInput::Array(Box::new(
                                ProjectExternalTypeInput::Var("T".into()),
                            ))],
                        }],
                    }],
                    callable: None,
                    methods: vec![],
                    super_specs: vec![],
                },
                ProjectExternalSpecSignatureInput {
                    path: vec!["dep".into(), "Family".into()],
                    visibility: "public".into(),
                    kind: ProjectExternalSpecKindInput::Type,
                    param_names: vec!["U".into()],
                    generic_params: vec![ProjectExternalCallableGenericParamInput {
                        name: "U".into(),
                        kind: ProjectExternalCallableGenericParamKindInput::Type,
                        bounds: vec![],
                    }],
                    callable: None,
                    methods: vec![],
                    super_specs: vec![],
                },
            ],
            spec_impls: vec![],
            type_spec_satisfactions: vec![],
            callable_spec_satisfactions: vec![],
            trace_spec_conformances: vec![],
            effect_summaries: vec![],
            action_summaries: vec![],
            trace_spec_summaries: vec![],
            re_exports: vec![],
        }];
        let modules = vec![ProjectExternalModuleInput {
            package: Some(ExternalPackageId(1)),
            id: ExternalModuleId(1),
            path: ModulePath {
                segments: vec!["dep".into()],
            },
            exports: vec![
                ProjectExternalExportInput {
                    symbol: ExternalSymbolId(1),
                    name: "Within".into(),
                    visibility: etas_hir::Visibility::Public,
                },
                ProjectExternalExportInput {
                    symbol: ExternalSymbolId(2),
                    name: "Family".into(),
                    visibility: etas_hir::Visibility::Public,
                },
            ],
        }];
        let alpha_parameter = types.intern(Type::Named(NamedTypeRef {
            name: "X".into(),
            source_parameter: None,
        }));
        let alpha_array = types.intern(Type::Array(alpha_parameter));
        let mut alpha_external = external.clone();
        let alpha_signature = alpha_external
            .signatures
            .get_mut(&external_key("Within"))
            .unwrap();
        alpha_signature.param_names[0] = "X".into();
        alpha_signature.generic_params[0].name = "X".into();
        alpha_signature.generic_params[0].subject = alpha_parameter;
        alpha_signature.generic_params[0].bounds[0].args = vec![alpha_array];
        let mut alpha_metadata = metadata.clone();
        alpha_metadata[0].spec_signatures[0].param_names[0] = "X".into();
        alpha_metadata[0].spec_signatures[0].generic_params[0].name = "X".into();
        alpha_metadata[0].spec_signatures[0].generic_params[0].bounds[0].args =
            vec![ProjectExternalTypeInput::Array(Box::new(
                ProjectExternalTypeInput::Var("X".into()),
            ))];
        assert_eq!(
            checked_canonical_spec_path(
                &within,
                &canonical,
                &alpha_external,
                &origins,
                (&alpha_metadata, &modules),
                types.store(),
                &|_| true
            )
            .unwrap(),
            vec!["dep", "Within"]
        );
        let mut ambiguous_origins = origins.clone();
        let mut second_origin = origins[0].clone();
        second_origin.package = ExternalPackageId(2);
        second_origin.origins.modules[0].path = vec!["other".into()];
        for binding in &mut second_origin.origins.declarations {
            binding.module = vec!["other".into()];
        }
        ambiguous_origins.push(second_origin);
        let mut ambiguous_external = external.clone();
        for name in ["Within", "Family"] {
            let signature = external.signatures[&external_key(name)].clone();
            ambiguous_external.signatures.insert(
                ExternalSpecIdentity {
                    package: 2,
                    path: vec!["other".into(), name.into()],
                },
                signature,
            );
        }
        ambiguous_external
            .signatures
            .get_mut(&ExternalSpecIdentity {
                package: 2,
                path: vec!["other".into(), "Within".into()],
            })
            .unwrap()
            .generic_params[0]
            .bounds[0]
            .spec = CheckedSpecRef::External(ExternalSpecIdentity {
            package: 2,
            path: vec!["other".into(), "Family".into()],
        });
        let mut ambiguous_metadata = metadata.clone();
        let mut second_metadata = metadata[0].clone();
        second_metadata.package = ExternalPackageId(2);
        for signature in &mut second_metadata.spec_signatures {
            signature.path[0] = "other".into();
        }
        second_metadata.spec_signatures[0].generic_params[0].bounds[0].spec[0] = "other".into();
        ambiguous_metadata.push(second_metadata);
        let mut ambiguous_modules = modules.clone();
        let mut second_module = modules[0].clone();
        second_module.package = Some(ExternalPackageId(2));
        second_module.id = ExternalModuleId(2);
        second_module.path.segments[0] = "other".into();
        second_module.exports[0].symbol = ExternalSymbolId(3);
        second_module.exports[1].symbol = ExternalSymbolId(4);
        ambiguous_modules.push(second_module);
        assert!(
            checked_canonical_spec_path(
                &within,
                &canonical,
                &ambiguous_external,
                &ambiguous_origins,
                (&ambiguous_metadata, &ambiguous_modules),
                types.store(),
                &|_| true
            )
            .is_err()
        );
        let run = |canonical: &CanonicalSpecFacts,
                   metadata: &[ProjectExternalPublicMetadataInput]| {
            checked_canonical_spec_path(
                &within,
                canonical,
                &external,
                &origins,
                (metadata, &modules),
                types.store(),
                &|_| true,
            )
        };
        assert_eq!(run(&canonical, &metadata).unwrap(), vec!["dep", "Within"]);
        assert!(
            checked_canonical_spec_path(
                &within,
                &canonical,
                &external,
                &origins,
                (&metadata, &modules),
                types.store(),
                &|key| key != &family,
            )
            .is_err()
        );
        let mut behavioral_metadata = metadata.clone();
        behavioral_metadata[0].spec_signatures[1].methods.push(
            crate::ProjectExternalSpecMethodInput {
                name: "method".into(),
                path: vec!["dep".into(), "Family".into(), "method".into()],
                signature: None,
            },
        );
        assert!(run(&canonical, &behavioral_metadata).is_err());
        let mut behavioral_facts = external.clone();
        behavioral_facts
            .signatures
            .get_mut(&external_key("Family"))
            .unwrap()
            .methods
            .push(etas_types::SpecMethodFact {
                identity: etas_types::SpecMethodIdentity::External {
                    package: 1,
                    path: vec!["dep".into(), "Family".into(), "method".into()],
                },
                name: "method".into(),
                signature: None,
            });
        assert!(
            checked_canonical_spec_path(
                &within,
                &canonical,
                &behavioral_facts,
                &origins,
                (&metadata, &modules),
                types.store(),
                &|_| true,
            )
            .is_err()
        );
        let mut nested_external = external.clone();
        nested_external
            .signatures
            .get_mut(&external_key("Family"))
            .unwrap()
            .generic_params[0]
            .bounds
            .push(CheckedSpecBound {
                spec: CheckedSpecRef::External(external_key("Within")),
                args: vec![family_parameter],
            });
        let mut nested_metadata = metadata.clone();
        nested_metadata[0].spec_signatures[1].generic_params[0]
            .bounds
            .push(ProjectExternalSpecBoundInput {
                spec: vec!["dep".into(), "Within".into()],
                args: vec![ProjectExternalTypeInput::Var("U".into())],
            });
        assert!(
            checked_canonical_spec_path(
                &within,
                &canonical,
                &nested_external,
                &origins,
                (&nested_metadata, &modules),
                types.store(),
                &|_| true
            )
            .is_err()
        );
        let mut cyclic_canonical = canonical.clone();
        cyclic_canonical
            .signatures
            .get_mut(&family)
            .unwrap()
            .param_bounds[0]
            .push(CheckedSpecBound {
                spec: CheckedSpecRef::Canonical(within.clone()),
                args: vec![canonical_family_parameter],
            });
        assert_eq!(
            checked_canonical_spec_path(
                &within,
                &cyclic_canonical,
                &nested_external,
                &origins,
                (&nested_metadata, &modules),
                types.store(),
                &|_| true
            )
            .unwrap(),
            vec!["dep", "Within"]
        );
        let mut forged = canonical.clone();
        forged.signatures.get_mut(&within).unwrap().param_bounds[0][0]
            .args
            .clear();
        assert!(run(&forged, &metadata).is_err());
        forged = canonical.clone();
        forged.signatures.get_mut(&within).unwrap().param_bounds[0][0].spec =
            CheckedSpecRef::Canonical(key("Other"));
        assert!(run(&forged, &metadata).is_err());
        let mut wrong_origin = origins.clone();
        wrong_origin[0].package = ExternalPackageId(2);
        assert!(
            checked_canonical_spec_path(
                &within,
                &canonical,
                &external,
                &wrong_origin,
                (&metadata, &modules),
                types.store(),
                &|_| true
            )
            .is_err()
        );
        metadata[0].spec_signatures[0].param_names[0] = "Forged".into();
        assert!(run(&canonical, &metadata).is_err());
        metadata[0].spec_signatures[0].param_names[0] = "T".into();
        metadata[0].spec_signatures[0].visibility = "private".into();
        assert!(run(&canonical, &metadata).is_err());
    }
}
