use std::collections::HashMap;

use etas_code::{MethodId, descriptor::GenericOwner};
use etas_types::{
    CallableGenericParamKind, CheckedSpecRef, TypeId, TypeInterner, TypeSubstitutionEngine,
    method::{MethodEvidence, MethodOwner, MethodSignatureFact, MethodTarget},
};

use crate::{CheckedProgramData, code::definition::DefinitionCatalog};

pub(super) struct Import<'a> {
    pub declaration: &'a MethodSignatureFact,
    pub identity: MethodId,
}

pub(super) fn select<'a>(
    program: &'a CheckedProgramData,
    catalog: &DefinitionCatalog,
    target: &DefinitionCatalog,
    arguments: &[TypeId],
) -> Result<Vec<Import<'a>>, String> {
    let mut selected = Vec::new();
    let target_specs = target
        .checked_program()
        .types
        .spec_signatures
        .keys()
        .map(|symbol| {
            target
                .native_spec_identity(&CheckedSpecRef::Source(*symbol))
                .cloned()
                .map_err(|e| e.to_string())
        })
        .collect::<Result<std::collections::HashSet<_>, _>>()?;
    for declaration in program.types.methods.entries() {
        let MethodTarget::Source { callable, .. } = declaration.target else {
            continue;
        };
        if !matches!(declaration.evidence, MethodEvidence::Implementation { .. })
            || !arguments.iter().any(|argument| {
                etas_types::solver::spec_solver::match_implementation_subject(
                    &program.type_store,
                    &declaration.owner_params,
                    declaration.receiver_type,
                    *argument,
                )
                .is_some()
            })
        {
            continue;
        }
        let MethodEvidence::Implementation { spec, .. } = &declaration.evidence else {
            unreachable!()
        };
        let canonical = match spec {
            CheckedSpecRef::Source(symbol) => program
                .types
                .std_spec_aliases
                .get(symbol)
                .map(|path| CheckedSpecRef::Std(path.clone()))
                .unwrap_or_else(|| spec.clone()),
            _ => spec.clone(),
        };
        if !target_specs.contains(
            catalog
                .native_spec_identity(&canonical)
                .map_err(|e| e.to_string())?,
        ) {
            continue;
        }
        let identity = catalog
            .native_callables()
            .find_map(|(identity, item)| {
                (item == callable)
                    .then(|| identity.as_method().cloned())
                    .flatten()
            })
            .ok_or("caller implementation has no canonical method identity")?;
        selected.push(Import {
            declaration,
            identity,
        });
    }
    selected.sort_by(|a, b| a.identity.cmp(&b.identity));
    Ok(selected)
}

pub(super) fn reserve_parameters(
    imports: &[Import<'_>],
    target: &mut TypeInterner,
    declarations: &mut HashMap<TypeId, TypeId>,
) -> Result<(), String> {
    for import in imports {
        for (parameters, owner) in [
            (
                &import.declaration.owner_params,
                GenericOwner::Implementation(import.identity.implementation().clone()),
            ),
            (
                &import.declaration.signature.generic_params,
                GenericOwner::Method(import.identity.clone()),
            ),
        ] {
            for (position, parameter) in parameters.iter().enumerate() {
                if parameter.kind != CallableGenericParamKind::Type {
                    continue;
                }
                let identity = etas_types::ty::CanonicalTypeParameterIdentity::new_scoped(
                    owner.clone(),
                    u32::try_from(position)
                        .map_err(|_| "method parameter index exceeds its checked ABI")?,
                )
                .map_err(str::to_owned)?;
                let ty = target
                    .intern_canonical_parameter(identity, parameter.name.clone())
                    .map_err(str::to_owned)?;
                if declarations
                    .insert(parameter.subject, ty)
                    .is_some_and(|old| old != ty)
                {
                    return Err("method parameter has conflicting checked scopes".into());
                }
            }
        }
    }
    Ok(())
}

pub(super) fn relocate(
    imports: &[Import<'_>],
    source: &CheckedProgramData,
    target: &CheckedProgramData,
    specs: &HashMap<CheckedSpecRef, CheckedSpecRef>,
    interner: &mut TypeInterner,
    declarations: &HashMap<TypeId, TypeId>,
    effect_names: &HashMap<String, String>,
) -> Result<Vec<MethodSignatureFact>, String> {
    let mut methods = Vec::new();
    for import in imports {
        let effect_parameters = import
            .declaration
            .signature
            .generic_params
            .iter()
            .filter(|p| p.kind == CallableGenericParamKind::Effect)
            .map(|p| (p.name.clone(), p.name.clone()))
            .collect::<HashMap<_, _>>();
        let mut relocation = TypeSubstitutionEngine::relocating_materialized(
            etas_types::ty::TypeRelocationMaterializationInput {
                source: &source.type_store,
                target: interner,
                declaration_types: declarations,
                effect_names,
            },
        )
        .with_spec_identities(specs)
        .with_relocated_effect_parameters(&effect_parameters);
        let mut declaration = import.declaration.clone();
        let MethodTarget::Source { callable, .. } = declaration.target else {
            unreachable!()
        };
        if let Some(contract) = source
            .effects
            .public_contracts
            .iter()
            .find(|contract| contract.item == callable)
        {
            declaration.signature.effects = Some(super::instantiation::checked_row(
                &contract.public_row,
                &source.effect_registry,
                Some(&declaration.signature),
                std::iter::empty(),
            )?);
        }
        let source_effects = declaration.signature.effects.clone();
        declaration
            .try_map_types(|ty| relocation.substitute(ty))
            .map_err(|e| e.to_string())?;
        declaration.signature.effects = source_effects
            .map(|row| relocation.substitute_effect_row(row))
            .transpose()
            .map_err(|e| e.to_string())?;
        let MethodEvidence::Implementation { spec, .. } = &mut declaration.evidence else {
            unreachable!()
        };
        let original_spec = spec.clone();
        *spec = specs
            .get(spec)
            .cloned()
            .ok_or("caller method has no checked spec correspondence")?;
        declaration.owner = MethodOwner::Spec(spec.clone());
        for parameter in declaration
            .owner_params
            .iter_mut()
            .chain(&mut declaration.signature.generic_params)
        {
            for bound in &mut parameter.bounds {
                bound.spec = specs
                    .get(&bound.spec)
                    .cloned()
                    .ok_or("caller method bound has no checked correspondence")?;
            }
        }
        let required = match &original_spec {
            CheckedSpecRef::Source(symbol) => source
                .types
                .spec_signatures
                .get(symbol)
                .map(|s| &s.methods)
                .or_else(|| {
                    source
                        .types
                        .external_specs
                        .aliases
                        .get(symbol)
                        .and_then(|key| source.types.external_specs.signatures.get(key))
                        .map(|s| &s.methods)
                }),
            CheckedSpecRef::External(key) => source
                .types
                .external_specs
                .signatures
                .get(key)
                .map(|s| &s.methods),
            _ => None,
        };
        if let CheckedSpecRef::Source(symbol) = spec {
            let required = target
                .types
                .spec_signatures
                .get(symbol)
                .ok_or("SDK method spec declaration is missing")?;
            declaration.spec_member = Some(
                required
                    .methods
                    .iter()
                    .find(|method| method.name == declaration.name)
                    .ok_or("SDK spec has no matching checked member")?
                    .identity
                    .clone(),
            );
        } else if required.is_none() {
            return Err("caller method has no checked member declaration".into());
        }
        declaration.target = MethodTarget::External {
            identity: import.identity.clone(),
        };
        declaration.defining_module = None;
        let flow = etas_types::FlowType {
            input: declaration.signature.params.clone(),
            output: declaration.signature.output,
            effects: declaration.signature.effects.clone(),
        };
        drop(relocation);
        interner.intern(etas_types::Type::Function(flow));
        methods.push(declaration);
    }
    Ok(methods)
}
