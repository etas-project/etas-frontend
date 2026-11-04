use std::collections::HashMap;

use etas_types::{
    CheckedSpecBound, CheckedSpecRef, TypeId, TypeSubstitutionBudget, TypeSubstitutionEngine,
};
use etas_utils::PipelineExecutionControl;

use crate::{CheckedProgramData, code::descriptor::ProjectionError};

fn type_spec_bounds(
    source: &CheckedProgramData,
    ty: TypeId,
) -> impl Iterator<Item = &CheckedSpecBound> {
    let object_bounds = match source.type_store.get(ty) {
        Some(etas_types::Type::Existential { bounds }) => bounds.as_slice(),
        _ => &[],
    };
    source
        .types
        .type_generics
        .get(&ty)
        .into_iter()
        .flatten()
        .flat_map(|parameter| &parameter.bounds)
        .chain(object_bounds)
}

/// Map standard spec references only after both checked registries agree on
/// their canonical declarations. User and package specs need their own proof.
pub(crate) fn checked_std_spec_correspondence(
    source: &CheckedProgramData,
    target: &etas_std::StdRegistry,
    reachable: &[TypeId],
    control: &dyn PipelineExecutionControl,
) -> Result<HashMap<CheckedSpecRef, CheckedSpecRef>, ProjectionError> {
    let mut mapped = HashMap::new();
    for ty in reachable {
        checkpoint(control)?;
        for bound in type_spec_bounds(source, *ty) {
            checkpoint(control)?;
            let path = match &bound.spec {
                CheckedSpecRef::Std(path) => Some(path),
                CheckedSpecRef::Source(symbol) => source.types.std_spec_aliases.get(symbol),
                CheckedSpecRef::External(_) => None,
                CheckedSpecRef::Canonical(identity) => {
                    if identity.kind() != etas_code::DefinitionKind::Spec
                        || !source
                            .types
                            .canonical_specs
                            .signatures
                            .contains_key(identity)
                    {
                        return Err(ProjectionError::MissingIdentity(
                            "canonical checked spec declaration".into(),
                        ));
                    }
                    mapped.insert(bound.spec.clone(), bound.spec.clone());
                    continue;
                }
            };
            let Some(path) = path else { continue };
            if source.std_registry.version() != target.version() {
                return Err(ProjectionError::Invalid(
                    "standard registry versions differ".into(),
                ));
            }
            let source_symbol = source.std_registry.lookup_qualified(path).ok_or_else(|| {
                ProjectionError::Invalid(
                    "checked standard spec is absent in source registry".into(),
                )
            })?;
            let target_symbol = target.lookup_qualified(path).ok_or_else(|| {
                ProjectionError::Invalid(
                    "checked standard spec is absent in target registry".into(),
                )
            })?;
            if source_symbol.qualified_path != target_symbol.qualified_path
                || source_symbol.decl != target_symbol.decl
                || !matches!(&source_symbol.decl, etas_std::StdDecl::Type(decl) if decl.kind == etas_std::TypeDeclKind::Spec)
            {
                return Err(ProjectionError::Invalid(
                    "standard spec declarations differ".into(),
                ));
            }
            let replacement = CheckedSpecRef::Std(target_symbol.qualified_path.clone());
            if mapped
                .insert(bound.spec.clone(), replacement.clone())
                .is_some_and(|old| old != replacement)
            {
                return Err(ProjectionError::Invalid(
                    "conflicting standard spec correspondence".into(),
                ));
            }
        }
    }
    Ok(mapped)
}

fn checkpoint(control: &dyn PipelineExecutionControl) -> Result<(), ProjectionError> {
    control.checkpoint().map_err(|error| {
        ProjectionError::Syntax(etas_code::syntax::SyntaxOperationError::Interrupted(error))
    })
}

#[derive(Clone)]
struct PlannedSignature {
    identity: etas_code::DefinitionKey,
    kind: etas_types::SpecKind,
    parameters: Vec<String>,
    parameter_kinds: Vec<etas_types::CallableGenericParamKind>,
    parameter_bounds: Vec<Vec<CheckedSpecBound>>,
    source_binders: Vec<Option<TypeId>>,
    supers: Vec<CheckedSpecBound>,
}

#[derive(Clone)]
struct PlannedImpl {
    generic_params: Vec<etas_types::CallableGenericParam>,
    self_type: TypeId,
    spec: CheckedSpecRef,
    args: Vec<TypeId>,
}

pub(crate) struct CheckedSpecPlan {
    pub mapping: HashMap<CheckedSpecRef, CheckedSpecRef>,
    signatures: Vec<PlannedSignature>,
    implementations: Vec<PlannedImpl>,
}

pub(crate) struct SpecMaterializationInput<'a> {
    pub source: &'a etas_types::TypeStore,
    pub target: &'a mut etas_types::TypeInterner,
    pub declarations: &'a HashMap<TypeId, TypeId>,
    pub effect_names: &'a HashMap<String, String>,
    pub reachable: &'a [TypeId],
    pub max_depth: usize,
    pub budget: &'a TypeSubstitutionBudget<'a>,
}

impl CheckedSpecPlan {
    fn type_roots(&self) -> impl Iterator<Item = TypeId> + '_ {
        self.signatures
            .iter()
            .flat_map(|signature| {
                signature
                    .parameter_bounds
                    .iter()
                    .flatten()
                    .chain(&signature.supers)
                    .flat_map(|bound| bound.args.iter().copied())
                    .chain(signature.source_binders.iter().flatten().copied())
            })
            .chain(self.implementations.iter().flat_map(|implementation| {
                std::iter::once(implementation.self_type)
                    .chain(implementation.args.iter().copied())
                    .chain(implementation.generic_params.iter().flat_map(|parameter| {
                        std::iter::once(parameter.subject).chain(
                            parameter
                                .bounds
                                .iter()
                                .flat_map(|bound| bound.args.iter().copied()),
                        )
                    }))
            }))
    }
    pub(crate) fn materialize(
        &self,
        input: SpecMaterializationInput<'_>,
    ) -> Result<etas_types::facts::RelocatedSpecFacts, etas_types::TypeSubstitutionError> {
        use etas_types::facts::specs::{
            CanonicalSpecFacts, CanonicalSpecImplFact, CanonicalSpecSignatureFact,
        };
        let SpecMaterializationInput {
            source,
            target,
            declarations,
            effect_names,
            reachable,
            max_depth,
            budget,
        } = input;
        let mut facts = CanonicalSpecFacts::default();
        let mut std_impls = Vec::new();
        let mut canonical_binders = None;
        for signature in &self.signatures {
            budget.charge_node()?;
            budget.charge_edges(signature.parameters.len())?;
            let mut scoped = HashMap::with_capacity(signature.parameters.len());
            for (index, name) in signature.parameters.iter().enumerate() {
                budget.charge_node()?;
                let index = u32::try_from(index).map_err(|_| {
                    etas_types::TypeSubstitutionError::MissingSpecIdentity(
                        CheckedSpecRef::Canonical(signature.identity.clone()),
                    )
                })?;
                let identity = etas_types::ty::CanonicalTypeParameterIdentity::new(
                    signature.identity.clone(),
                    index,
                )
                .map_err(|_| {
                    etas_types::TypeSubstitutionError::MissingSpecIdentity(
                        CheckedSpecRef::Canonical(signature.identity.clone()),
                    )
                })?;
                let mut source_binder = signature.source_binders[index as usize];
                if source_binder.is_none() {
                    if canonical_binders.is_none() {
                        budget.charge_edges(reachable.len())?;
                        let mut index = HashMap::new();
                        for ty in reachable {
                            budget.charge_node()?;
                            if let Some(key) = source.parameter_identity(*ty)
                                && index.insert(key.clone(), *ty).is_some_and(|old| old != *ty)
                            {
                                return Err(
                                    etas_types::TypeSubstitutionError::MissingTypeIdentity(*ty),
                                );
                            }
                        }
                        canonical_binders = Some(index);
                    }
                    source_binder = canonical_binders
                        .as_ref()
                        .and_then(|index| index.get(&identity))
                        .copied();
                }
                if let Some(source_binder) = source_binder {
                    let mapped = target
                        .intern_canonical_parameter(identity, name.clone())
                        .map_err(|_| {
                            etas_types::TypeSubstitutionError::MissingTypeIdentity(source_binder)
                        })?;
                    if declarations
                        .get(&source_binder)
                        .is_some_and(|old| *old != mapped)
                        || scoped
                            .insert(source_binder, mapped)
                            .is_some_and(|old| old != mapped)
                    {
                        return Err(etas_types::TypeSubstitutionError::MissingTypeIdentity(
                            source_binder,
                        ));
                    }
                }
            }
            let mut relocation = TypeSubstitutionEngine::relocating_materialized_with_overrides(
                etas_types::ty::TypeRelocationMaterializationInput {
                    source,
                    target,
                    declaration_types: declarations,
                    effect_names,
                },
                &scoped,
            )
            .with_spec_identities(&self.mapping)
            .with_expansion_depth(max_depth)
            .with_budget(budget);
            let mut parameter_bounds = Vec::new();
            for bounds in &signature.parameter_bounds {
                let mut mapped = Vec::new();
                for bound in bounds {
                    budget.charge_edges(1)?;
                    let spec = self.mapping.get(&bound.spec).cloned().ok_or_else(|| {
                        etas_types::TypeSubstitutionError::MissingSpecIdentity(bound.spec.clone())
                    })?;
                    let mut args = Vec::new();
                    for ty in &bound.args {
                        budget.charge_edges(1)?;
                        args.push(relocation.substitute(*ty)?);
                    }
                    mapped.push(CheckedSpecBound { spec, args });
                }
                parameter_bounds.push(mapped);
            }
            let mut supers = Vec::new();
            for bound in &signature.supers {
                budget.charge_edges(1)?;
                let spec = self.mapping.get(&bound.spec).cloned().ok_or_else(|| {
                    etas_types::TypeSubstitutionError::MissingSpecIdentity(bound.spec.clone())
                })?;
                let mut args = Vec::new();
                for ty in &bound.args {
                    budget.charge_edges(1)?;
                    args.push(relocation.substitute(*ty)?);
                }
                supers.push(CheckedSpecBound { spec, args });
            }
            let value = CanonicalSpecSignatureFact {
                kind: signature.kind,
                param_names: signature.parameters.clone(),
                param_kinds: signature.parameter_kinds.clone(),
                param_bounds: parameter_bounds,
                super_specs: supers,
            };
            if facts
                .signatures
                .insert(signature.identity.clone(), value.clone())
                .is_some_and(|old| old != value)
            {
                return Err(etas_types::TypeSubstitutionError::MissingSpecIdentity(
                    CheckedSpecRef::Canonical(signature.identity.clone()),
                ));
            }
        }
        let mut relocation = TypeSubstitutionEngine::relocating_materialized(
            etas_types::ty::TypeRelocationMaterializationInput {
                source,
                target,
                declaration_types: declarations,
                effect_names,
            },
        )
        .with_spec_identities(&self.mapping)
        .with_expansion_depth(max_depth)
        .with_budget(budget);
        for implementation in &self.implementations {
            budget.charge_node()?;
            let mut generic_params = implementation.generic_params.clone();
            for parameter in &mut generic_params {
                budget.charge_node()?;
                parameter.subject = relocation.substitute(parameter.subject)?;
                for bound in &mut parameter.bounds {
                    bound.spec = self.mapping.get(&bound.spec).cloned().ok_or_else(|| {
                        etas_types::TypeSubstitutionError::MissingSpecIdentity(bound.spec.clone())
                    })?;
                    for ty in &mut bound.args {
                        *ty = relocation.substitute(*ty)?;
                    }
                }
            }
            let self_type = relocation.substitute(implementation.self_type)?;
            let mut args = Vec::new();
            for ty in &implementation.args {
                budget.charge_edges(1)?;
                args.push(relocation.substitute(*ty)?);
            }
            match self.mapping.get(&implementation.spec) {
                Some(CheckedSpecRef::Canonical(spec)) => facts.impls.push(CanonicalSpecImplFact {
                    generic_params,
                    self_type,
                    spec: spec.clone(),
                    args,
                }),
                Some(CheckedSpecRef::Std(spec)) => {
                    std_impls.push(etas_types::CheckedStdSpecImplFact {
                        generic_params,
                        self_type,
                        spec: spec.clone(),
                        args,
                    })
                }
                _ => {
                    return Err(etas_types::TypeSubstitutionError::MissingSpecIdentity(
                        implementation.spec.clone(),
                    ));
                }
            }
        }
        Ok(etas_types::facts::RelocatedSpecFacts {
            canonical: facts,
            std_impls,
        })
    }
}

pub(crate) struct CheckedTypeAndSpecPlanInput<'a> {
    pub source: &'a CheckedProgramData,
    pub target: &'a etas_std::StdRegistry,
    pub roots: &'a [TypeId],
    pub rows: &'a [&'a etas_types::EffectRowRef],
    pub declarations: &'a HashMap<TypeId, TypeId>,
    pub identity: &'a dyn Fn(&CheckedSpecRef) -> Result<etas_code::DefinitionKey, ProjectionError>,
    pub max_depth: usize,
    pub control: &'a dyn PipelineExecutionControl,
    pub budget: &'a TypeSubstitutionBudget<'a>,
}

pub(crate) fn checked_type_and_spec_plan(
    input: CheckedTypeAndSpecPlanInput<'_>,
) -> Result<(Vec<TypeId>, CheckedSpecPlan), ProjectionError> {
    let CheckedTypeAndSpecPlanInput {
        source,
        target,
        roots,
        rows,
        declarations,
        identity,
        max_depth,
        control,
        budget,
    } = input;
    let mut reachable = etas_types::ty::checked_type_graph(
        &source.type_store,
        &source.types,
        roots,
        rows,
        declarations,
        max_depth,
        budget,
    )
    .map_err(spec_budget_error)?;
    let mut seen = reachable
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    loop {
        checkpoint(control)?;
        let plan = checked_spec_plan(source, target, &reachable, identity, control, budget)?;
        let mut missing = Vec::new();
        for ty in plan.type_roots() {
            charge_edges(budget, 1)?;
            if !seen.contains(&ty) {
                missing.push(ty);
            }
        }
        if missing.is_empty() {
            return Ok((reachable, plan));
        }
        let additional = etas_types::ty::checked_type_graph(
            &source.type_store,
            &source.types,
            &missing,
            &[],
            declarations,
            max_depth,
            budget,
        )
        .map_err(spec_budget_error)?;
        let before = seen.len();
        for ty in additional {
            charge_node(budget)?;
            if seen.insert(ty) {
                reachable.push(ty);
            }
        }
        if before == seen.len() {
            return Err(ProjectionError::Invalid(
                "checked spec type closure did not progress".into(),
            ));
        }
    }
}

type CheckedExternalSpecParameters = (
    Vec<etas_types::CallableGenericParamKind>,
    Vec<Vec<CheckedSpecBound>>,
    Vec<Option<TypeId>>,
);

fn checked_external_spec_parameters(
    source: &CheckedProgramData,
    signature: &etas_types::facts::specs::ExternalSpecSignatureFact,
    budget: &TypeSubstitutionBudget<'_>,
) -> Result<CheckedExternalSpecParameters, ProjectionError> {
    if signature.generic_params.len() != signature.param_names.len() {
        return Err(ProjectionError::MissingIdentity(
            "external spec parameter constraints".into(),
        ));
    }
    let mut kinds = Vec::new();
    let mut bounds = Vec::new();
    let mut binders = Vec::new();
    for (name, param) in signature.param_names.iter().zip(&signature.generic_params) {
        charge_node(budget)?;
        if param.name != *name || param.kind != etas_types::CallableGenericParamKind::Type {
            return Err(ProjectionError::Invalid(
                "external spec generic parameter identity or kind differs".into(),
            ));
        }
        if !matches!(source.type_store.get(param.subject), Some(etas_types::Type::Named(named)) if named.source_parameter.is_none() && named.name == *name)
        {
            return Err(ProjectionError::Invalid(
                "external spec generic binder fact differs".into(),
            ));
        }
        charge_edges(budget, param.bounds.len())?;
        kinds.push(param.kind);
        bounds.push(param.bounds.clone());
        binders.push(Some(param.subject));
    }
    Ok((kinds, bounds, binders))
}

pub(crate) fn checked_spec_plan(
    source: &CheckedProgramData,
    target: &etas_std::StdRegistry,
    reachable: &[TypeId],
    identity: &dyn Fn(&CheckedSpecRef) -> Result<etas_code::DefinitionKey, ProjectionError>,
    control: &dyn PipelineExecutionControl,
    budget: &TypeSubstitutionBudget<'_>,
) -> Result<CheckedSpecPlan, ProjectionError> {
    use std::collections::{HashSet, VecDeque};
    let mut mapping = checked_std_spec_correspondence(source, target, reachable, control)?;
    let mut pending = VecDeque::new();
    let mut implementations = Vec::new();
    for ty in reachable {
        charge_node(budget)?;
        for bound in type_spec_bounds(source, *ty) {
            charge_edges(budget, 1)?;
            pending.push_back(bound.spec.clone());
        }
    }
    let mut add_impl = |self_type: TypeId,
                        spec: CheckedSpecRef,
                        args: &[TypeId],
                        generic_params: &[etas_types::CallableGenericParam]|
     -> Result<(), ProjectionError> {
        let mut relevant = false;
        for ty in reachable {
            charge_edges(budget, 1)?;
            if etas_types::solver::spec_solver::match_implementation_subject(
                &source.type_store,
                generic_params,
                self_type,
                *ty,
            )
            .is_some()
            {
                relevant = true;
                break;
            }
        }
        if relevant {
            charge_edges(budget, args.len())?;
            pending.push_back(spec.clone());
            pending.extend(
                generic_params
                    .iter()
                    .flat_map(|parameter| parameter.bounds.iter().map(|bound| bound.spec.clone())),
            );
            implementations.push(PlannedImpl {
                generic_params: generic_params.to_vec(),
                self_type,
                spec,
                args: args.to_vec(),
            });
        }
        Ok(())
    };
    for item in &source.types.spec_impls {
        charge_node(budget)?;
        add_impl(
            item.self_type,
            CheckedSpecRef::Source(item.spec_symbol),
            &item.args,
            &item.generic_params,
        )?;
    }
    for item in &source.types.external_specs.impls {
        charge_node(budget)?;
        add_impl(
            item.self_type,
            CheckedSpecRef::External(item.spec.clone()),
            &item.args,
            &item.generic_params,
        )?;
    }
    for item in &source.types.canonical_specs.impls {
        charge_node(budget)?;
        add_impl(
            item.self_type,
            CheckedSpecRef::Canonical(item.spec.clone()),
            &item.args,
            &item.generic_params,
        )?;
    }
    for item in &source.types.std_spec_impls {
        charge_node(budget)?;
        add_impl(
            item.self_type,
            CheckedSpecRef::Std(item.spec.clone()),
            &item.args,
            &item.generic_params,
        )?;
    }
    let mut seen = HashSet::new();
    let mut signatures = Vec::new();
    while let Some(spec) = pending.pop_front() {
        charge_node(budget)?;
        if !seen.insert(spec.clone()) {
            continue;
        }
        if let Some(mapped) = checked_standard_ref(source, target, &spec)? {
            mapping.insert(spec, mapped);
            continue;
        }
        let canonical = identity(&spec)?;
        if canonical.kind() != etas_code::DefinitionKind::Spec {
            return Err(ProjectionError::Invalid(
                "checked spec identity has the wrong kind".into(),
            ));
        }
        let (kind, parameters, parameter_kinds, parameter_bounds, source_binders, supers) =
            match &spec {
                CheckedSpecRef::Source(symbol)
                    if source.types.external_specs.aliases.contains_key(symbol) =>
                {
                    let external = &source.types.external_specs.aliases[symbol];
                    let sig = source
                        .types
                        .external_specs
                        .signatures
                        .get(external)
                        .ok_or_else(|| {
                            ProjectionError::MissingIdentity(
                                "checked external spec signature".into(),
                            )
                        })?;
                    validate_external_package(source, external, &canonical, budget)?;
                    let (kinds, bounds, binders) =
                        checked_external_spec_parameters(source, sig, budget)?;
                    (
                        sig.kind,
                        sig.param_names.clone(),
                        kinds,
                        bounds,
                        binders,
                        sig.super_specs.clone(),
                    )
                }
                CheckedSpecRef::Source(symbol) => {
                    let sig = source
                        .types
                        .spec_signatures
                        .get(symbol)
                        .ok_or(ProjectionError::MissingDeclaration(*symbol))?;
                    if sig.params.len() != sig.param_names.len() {
                        return Err(ProjectionError::Invalid(
                            "checked spec parameter facts disagree".into(),
                        ));
                    }
                    let mut kinds = Vec::new();
                    let mut parameter_bounds = Vec::new();
                    let mut binders = Vec::new();
                    for (symbol, name) in sig.params.iter().zip(&sig.param_names) {
                        charge_node(budget)?;
                        let data = source
                            .hir
                            .symbols
                            .get(*symbol)
                            .ok_or(ProjectionError::MissingDeclaration(*symbol))?;
                        if data.name != *name {
                            return Err(ProjectionError::Invalid(
                                "checked spec binder name differs".into(),
                            ));
                        }
                        let etas_hir::SymbolDef::TypeParam { bounds, .. } = &data.def else {
                            return Err(ProjectionError::Invalid(
                                "spec effect binder requires checked row provenance".into(),
                            ));
                        };
                        let checked = source
                            .types
                            .type_param_bounds
                            .get(symbol)
                            .map(Vec::as_slice)
                            .unwrap_or_default();
                        if checked.len() != bounds.len() {
                            return Err(ProjectionError::Invalid(
                                "checked spec parameter bounds are incomplete".into(),
                            ));
                        }
                        let Some(etas_types::SymbolTypeFact::Param { ty }) =
                            source.types.symbol_types.get(symbol)
                        else {
                            return Err(ProjectionError::MissingDeclaration(*symbol));
                        };
                        if !matches!(source.type_store.get(*ty), Some(etas_types::Type::Named(named)) if named.source_parameter == Some(*symbol))
                        {
                            return Err(ProjectionError::Invalid(
                                "checked spec binder identity differs".into(),
                            ));
                        }
                        kinds.push(etas_types::CallableGenericParamKind::Type);
                        binders.push(Some(*ty));
                        parameter_bounds.push(
                            checked
                                .iter()
                                .map(|bound| CheckedSpecBound {
                                    spec: CheckedSpecRef::Source(bound.spec_symbol),
                                    args: bound.args.clone(),
                                })
                                .collect(),
                        );
                    }
                    let supers = sig
                        .super_specs
                        .iter()
                        .map(|bound| CheckedSpecBound {
                            spec: bound.super_spec.clone(),
                            args: bound.args.clone(),
                        })
                        .collect();
                    (
                        sig.kind,
                        sig.param_names.clone(),
                        kinds,
                        parameter_bounds,
                        binders,
                        supers,
                    )
                }
                CheckedSpecRef::External(external) => {
                    let sig = source
                        .types
                        .external_specs
                        .signatures
                        .get(external)
                        .ok_or_else(|| {
                            ProjectionError::MissingIdentity(
                                "checked external spec signature".into(),
                            )
                        })?;
                    validate_external_package(source, external, &canonical, budget)?;
                    let (kinds, bounds, binders) =
                        checked_external_spec_parameters(source, sig, budget)?;
                    (
                        sig.kind,
                        sig.param_names.clone(),
                        kinds,
                        bounds,
                        binders,
                        sig.super_specs.clone(),
                    )
                }
                CheckedSpecRef::Canonical(key) => {
                    let sig = source
                        .types
                        .canonical_specs
                        .signatures
                        .get(key)
                        .ok_or_else(|| {
                            ProjectionError::MissingIdentity(
                                "canonical checked spec declaration".into(),
                            )
                        })?;
                    if sig.param_names.len() != sig.param_kinds.len()
                        || sig.param_names.len() != sig.param_bounds.len()
                    {
                        return Err(ProjectionError::Invalid(
                            "canonical spec parameter facts are incomplete".into(),
                        ));
                    }
                    (
                        sig.kind,
                        sig.param_names.clone(),
                        sig.param_kinds.clone(),
                        sig.param_bounds.clone(),
                        vec![None; sig.param_names.len()],
                        sig.super_specs.clone(),
                    )
                }
                CheckedSpecRef::Std(_) => {
                    return Err(ProjectionError::Invalid(
                        "standard spec was not resolved".into(),
                    ));
                }
            };
        if kind != etas_types::SpecKind::TypeSpec {
            return Err(ProjectionError::Invalid(
                "type bound does not identify a type spec".into(),
            ));
        }
        charge_edges(budget, parameters.len() + supers.len())?;
        for bound in parameter_bounds.iter().flatten().chain(&supers) {
            charge_edges(budget, bound.args.len())?;
            pending.push_back(bound.spec.clone());
        }
        mapping.insert(spec, CheckedSpecRef::Canonical(canonical.clone()));
        signatures.push(PlannedSignature {
            identity: canonical,
            kind,
            parameters,
            parameter_kinds,
            parameter_bounds,
            source_binders,
            supers,
        });
    }
    Ok(CheckedSpecPlan {
        mapping,
        signatures,
        implementations,
    })
}

fn checked_standard_ref(
    source: &CheckedProgramData,
    target: &etas_std::StdRegistry,
    spec: &CheckedSpecRef,
) -> Result<Option<CheckedSpecRef>, ProjectionError> {
    let path = match spec {
        CheckedSpecRef::Std(path) => Some(path),
        CheckedSpecRef::Source(symbol) => source.types.std_spec_aliases.get(symbol),
        CheckedSpecRef::External(_) | CheckedSpecRef::Canonical(_) => None,
    };
    let Some(path) = path else { return Ok(None) };
    if source.std_registry.version() != target.version() {
        return Err(ProjectionError::Invalid(
            "standard registry versions differ".into(),
        ));
    }
    let left = source.std_registry.lookup_qualified(path).ok_or_else(|| {
        ProjectionError::Invalid("checked standard spec is absent in source registry".into())
    })?;
    let right = target.lookup_qualified(path).ok_or_else(|| {
        ProjectionError::Invalid("checked standard spec is absent in target registry".into())
    })?;
    if left.qualified_path != right.qualified_path
        || left.decl != right.decl
        || !matches!(&left.decl, etas_std::StdDecl::Type(decl) if decl.kind == etas_std::TypeDeclKind::Spec)
    {
        return Err(ProjectionError::Invalid(
            "standard spec declarations differ".into(),
        ));
    }
    Ok(Some(CheckedSpecRef::Std(right.qualified_path.clone())))
}

fn validate_external_package(
    source: &CheckedProgramData,
    spec: &etas_types::facts::specs::ExternalSpecIdentity,
    identity: &etas_code::DefinitionKey,
    budget: &TypeSubstitutionBudget<'_>,
) -> Result<(), ProjectionError> {
    let mut found = false;
    for origin in source.external_definition_origins() {
        charge_edges(budget, 1)?;
        if origin.package.0 != spec.package {
            continue;
        }
        for declaration in &origin.origins.declarations {
            charge_edges(budget, 1)?;
            if declaration.path() == spec.path && &declaration.definition == identity {
                if found {
                    return Err(ProjectionError::Invalid(
                        "ambiguous external spec origin".into(),
                    ));
                }
                found = true;
            }
        }
    }
    if found {
        Ok(())
    } else {
        Err(ProjectionError::MissingIdentity(
            "package-bound external spec origin".into(),
        ))
    }
}

fn charge_node(budget: &TypeSubstitutionBudget<'_>) -> Result<(), ProjectionError> {
    budget.charge_node().map_err(spec_budget_error)
}
fn charge_edges(budget: &TypeSubstitutionBudget<'_>, amount: usize) -> Result<(), ProjectionError> {
    budget.charge_edges(amount).map_err(spec_budget_error)
}
fn spec_budget_error(error: etas_types::TypeSubstitutionError) -> ProjectionError {
    match error {
        etas_types::TypeSubstitutionError::Interrupted(interruption) => ProjectionError::Syntax(
            etas_code::syntax::SyntaxOperationError::Interrupted(interruption),
        ),
        etas_types::TypeSubstitutionError::WorkLimit { resource, limit } => {
            ProjectionError::Limit { resource, limit }
        }
        etas_types::TypeSubstitutionError::ExpansionDepth(limit) => ProjectionError::Limit {
            resource: "type expansion depth",
            limit,
        },
        etas_types::TypeSubstitutionError::RepresentationLimit(limit) => ProjectionError::Limit {
            resource: "type representation",
            limit,
        },
        other => ProjectionError::Invalid(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existential_bounds_relocate_through_the_checked_spec_plan() {
        struct Control;
        impl PipelineExecutionControl for Control {
            fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
                Ok(())
            }
        }
        let mut input = crate::ProjectInput::single_source(crate::SourceInput::anonymous(
            "module app; spec Marker; flow read(value: ? ~ Marker) -> unit ![] { return; }",
        ));
        input.options.entry_policy = crate::EntryPolicy::Optional;
        let output = crate::Frontend.check_project(input);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let checked = output.checked.unwrap();
        let package = etas_code::PackageKey::from_name_and_version("app", "1.0.0").unwrap();
        let origins = crate::code::identity::DefinitionOrigins {
            source_modules: crate::code::identity::DefinitionOrigins::declared_source_modules(
                &checked, package,
            )
            .unwrap(),
            standard_package: crate::code::identity::standard_package_key(&checked.std_registry)
                .unwrap(),
            external_definitions: Default::default(),
        };
        let catalog = crate::code::definition::DefinitionCatalog::build(
            checked.clone(),
            &origins,
            Default::default(),
        )
        .unwrap();
        let (object, bound) = checked
            .type_store
            .iter()
            .find_map(|(ty, node)| match node {
                etas_types::Type::Existential { bounds } => Some((ty, bounds[0].clone())),
                _ => None,
            })
            .unwrap();
        let budget = TypeSubstitutionBudget::new(1024, 1024, None);
        let (reachable, plan) = checked_type_and_spec_plan(CheckedTypeAndSpecPlanInput {
            source: &checked,
            target: &checked.std_registry,
            roots: &[object],
            rows: &[],
            declarations: &HashMap::new(),
            identity: &|spec| catalog.native_spec_identity(spec).cloned(),
            max_depth: 32,
            control: &Control,
            budget: &budget,
        })
        .unwrap();
        let expected =
            CheckedSpecRef::Canonical(catalog.native_spec_identity(&bound.spec).unwrap().clone());
        assert_eq!(plan.mapping[&bound.spec], expected);
        let mut target = etas_types::TypeInterner::new();
        let facts = plan
            .materialize(SpecMaterializationInput {
                source: &checked.type_store,
                target: &mut target,
                declarations: &HashMap::new(),
                effect_names: &HashMap::new(),
                reachable: &reachable,
                max_depth: 32,
                budget: &budget,
            })
            .unwrap();
        assert_eq!(facts.canonical.signatures.len(), 1);
        let mapped = TypeSubstitutionEngine::relocating_materialized(
            etas_types::ty::TypeRelocationMaterializationInput {
                source: &checked.type_store,
                target: &mut target,
                declaration_types: &HashMap::new(),
                effect_names: &HashMap::new(),
            },
        )
        .with_spec_identities(&plan.mapping)
        .substitute(object)
        .unwrap();
        let Some(etas_types::Type::Existential { bounds }) = target.store().get(mapped) else {
            panic!("existential type")
        };
        assert_eq!(bounds[0].spec, expected);
    }

    #[test]
    fn materializing_specs_does_not_copy_the_shared_declaration_map() {
        let mut source = etas_types::TypeStore::new();
        let mut target = etas_types::TypeInterner::new();
        let declarations = (0..1024)
            .map(|index| {
                let ty = etas_types::Type::Var(etas_types::TypeVarId(index));
                (source.intern(ty.clone()), target.intern(ty))
            })
            .collect::<HashMap<_, _>>();
        let package = etas_code::PackageKey::from_name_and_version("library", "1.0.0").unwrap();
        let signatures = (0..16)
            .map(|index| PlannedSignature {
                identity: etas_code::DefinitionKey::new(
                    package,
                    vec!["lib".into()],
                    vec![format!("S{index}")],
                    etas_code::DefinitionKind::Spec,
                )
                .unwrap(),
                kind: etas_types::SpecKind::TypeSpec,
                parameters: vec![],
                parameter_kinds: vec![],
                parameter_bounds: vec![],
                source_binders: vec![],
                supers: vec![],
            })
            .collect();
        let plan = CheckedSpecPlan {
            mapping: HashMap::new(),
            signatures,
            implementations: vec![],
        };
        let budget = TypeSubstitutionBudget::new(32, 32, None);
        let result = plan
            .materialize(SpecMaterializationInput {
                source: &source,
                target: &mut target,
                declarations: &declarations,
                effect_names: &HashMap::new(),
                reachable: &[],
                max_depth: 32,
                budget: &budget,
            })
            .unwrap();
        assert_eq!(result.canonical.signatures.len(), 16);
    }

    #[test]
    fn materializing_specs_indexes_canonical_binders_once() {
        let package = etas_code::PackageKey::from_name_and_version("library", "1.0.0").unwrap();
        let mut source = etas_types::TypeInterner::new();
        let mut signatures = Vec::new();
        let mut reachable = Vec::new();
        for index in 0..128 {
            let identity = etas_code::DefinitionKey::new(
                package,
                vec!["lib".into()],
                vec![format!("S{index}")],
                etas_code::DefinitionKind::Spec,
            )
            .unwrap();
            let binder = source
                .intern_canonical_parameter(
                    etas_types::ty::CanonicalTypeParameterIdentity::new(identity.clone(), 0)
                        .unwrap(),
                    "T".into(),
                )
                .unwrap();
            reachable.push(binder);
            if index < 16 {
                signatures.push(PlannedSignature {
                    identity,
                    kind: etas_types::SpecKind::TypeSpec,
                    parameters: vec!["T".into()],
                    parameter_kinds: vec![etas_types::CallableGenericParamKind::Type],
                    parameter_bounds: vec![vec![]],
                    source_binders: vec![None],
                    supers: vec![],
                });
            }
        }
        let plan = CheckedSpecPlan {
            mapping: HashMap::new(),
            signatures,
            implementations: vec![],
        };
        let budget = TypeSubstitutionBudget::new(200, 200, None);
        let mut target = etas_types::TypeInterner::new();
        let result = plan
            .materialize(SpecMaterializationInput {
                source: source.store(),
                target: &mut target,
                declarations: &HashMap::new(),
                effect_names: &HashMap::new(),
                reachable: &reachable,
                max_depth: 32,
                budget: &budget,
            })
            .unwrap();
        assert_eq!(result.canonical.signatures.len(), 16);
    }
}
