use std::collections::HashMap;

use etas_code::descriptor::{
    GenericOwner, GenericParameterInfo, GenericParameterKind, GenericParameterRef,
};
use etas_types::TypeId;

use super::{DefinitionDescriptorProjector, ProjectionError, project::Work};
use crate::code::identity::GenericDeclaration;

pub(super) struct Scope {
    pub owner: GenericOwner,
    pub binders: HashMap<String, GenericParameterRef>,
    pub type_parameters: HashMap<TypeId, GenericParameterRef>,
    pub own_binders: Vec<GenericParameterRef>,
    parameters: Vec<GenericDeclaration>,
}

impl DefinitionDescriptorProjector<'_> {
    pub(super) fn callable_scope(
        &mut self,
        owner: GenericOwner,
        parameters: &[GenericDeclaration],
    ) -> Result<usize, ProjectionError> {
        let inherited = if let GenericOwner::Method(method) = &owner {
            self.identities
                .method_owner_parameters(method)
                .map(|parameters| {
                    (
                        GenericOwner::Implementation(method.implementation().clone()),
                        parameters.to_vec(),
                    )
                })
        } else {
            None
        };
        let inherited = inherited
            .map(|(owner, parameters)| {
                let parent = if let Some(scope) =
                    self.scopes.iter().position(|scope| scope.owner == owner)
                {
                    if self.scopes[scope].parameters.len() != parameters.len()
                        || self.scopes[scope]
                            .parameters
                            .iter()
                            .zip(&parameters)
                            .any(|(a, b)| a.effect != b.effect)
                    {
                        return Err(ProjectionError::Invalid(
                            "implementation has inconsistent checked binders".into(),
                        ));
                    }
                    scope
                } else {
                    self.scope(owner, &parameters)?
                };
                Ok((parent, parameters))
            })
            .transpose()?;
        let scope = self.scope(owner, parameters)?;
        if let Some((parent, parameters)) = inherited {
            for (parameter, binder) in parameters
                .iter()
                .zip(self.scopes[parent].own_binders.clone())
            {
                if let Some(subject) = parameter.subject {
                    self.scopes[scope].type_parameters.insert(subject, binder);
                }
            }
        }
        Ok(scope)
    }

    pub(super) fn scope(
        &mut self,
        owner: GenericOwner,
        parameters: &[GenericDeclaration],
    ) -> Result<usize, ProjectionError> {
        if let Some(scope) = self.scopes.iter().position(|scope| scope.owner == owner) {
            let previous = self.scopes[scope].parameters.clone();
            if previous.len() != parameters.len()
                || previous
                    .iter()
                    .zip(parameters)
                    .any(|(a, b)| a.name != b.name || a.effect != b.effect)
            {
                return Err(ProjectionError::Invalid(
                    "one definition identity has conflicting generic binders".into(),
                ));
            }
            if previous
                .iter()
                .zip(parameters)
                .any(|(a, b)| a.bounds != b.bounds)
                && self.binder_contract(owner.clone(), &previous)?
                    != self.binder_contract(owner.clone(), parameters)?
            {
                return Err(ProjectionError::Invalid(
                    "one definition identity has conflicting generic bounds".into(),
                ));
            }
            // A declaration can have both an imported signature and a linked
            // source signature. Their local parameter IDs share one portable
            // owner/position, while each checked type needs its scoped origin.
            for (parameter, binder) in parameters
                .iter()
                .zip(self.scopes[scope].own_binders.clone())
            {
                if let Some(subject) = parameter.subject {
                    if !matches!(
                        self.checked.type_store.get(subject),
                        Some(etas_types::Type::Var(_) | etas_types::Type::Named(_))
                    ) {
                        return Err(ProjectionError::Invalid(format!(
                            "generic binder subject is not a parameter: {:?}",
                            self.checked.type_store.get(subject)
                        )));
                    }
                    if self.scopes[scope]
                        .type_parameters
                        .insert(subject, binder)
                        .is_some_and(|old| old != binder)
                    {
                        return Err(ProjectionError::Invalid(
                            "conflicting checked generic parameter identity".into(),
                        ));
                    }
                }
            }
            return Ok(scope);
        }
        self.ensure_nodes(parameters.len())?;
        let scope = self.scopes.len();
        self.scopes.push(Scope {
            owner: owner.clone(),
            binders: HashMap::new(),
            type_parameters: HashMap::new(),
            own_binders: Vec::with_capacity(parameters.len()),
            parameters: parameters.to_vec(),
        });
        for (position, parameter) in parameters.iter().enumerate() {
            self.node()?;
            self.text(&parameter.name)?;
            let target = GenericParameterRef(self.binders.len() as u32);
            if let Some(subject) = parameter.subject
                && self.scopes[scope]
                    .type_parameters
                    .insert(subject, target)
                    .is_some()
            {
                return Err(ProjectionError::Invalid(
                    "duplicate checked generic parameter identity".into(),
                ));
            }
            if self.scopes[scope]
                .binders
                .insert(parameter.name.clone(), target)
                .is_some()
            {
                return Err(ProjectionError::Invalid(
                    "duplicate checked generic binder name".into(),
                ));
            }
            self.scopes[scope].own_binders.push(target);
            self.binders.push(GenericParameterInfo {
                owner: owner.clone(),
                position: position as u32,
                name: parameter.name.clone(),
                kind: if parameter.effect {
                    GenericParameterKind::EffectRow
                } else {
                    GenericParameterKind::Type
                },
                bounds: Vec::new(),
            });
            self.work.push(Work::Bounds {
                declaration: parameter.clone(),
                scope,
                target,
            });
        }
        Ok(scope)
    }

    fn binder_contract(
        &self,
        owner: GenericOwner,
        parameters: &[GenericDeclaration],
    ) -> Result<etas_code::CodeFingerprint, ProjectionError> {
        use etas_code::descriptor::{PrimitiveInfo, SignatureInfo, TypeInfo};
        // Compare portable declarations: linked Source and imported External
        // spec references can name the same canonical bound.
        let mut projection = Self::from_input(self.checked, self.identities, self.limits);
        let scope = projection.callable_scope(owner, parameters)?;
        let unit = projection.reserve_type()?;
        projection.types[unit.0 as usize] = Some(TypeInfo::Primitive(PrimitiveInfo::Unit));
        let root = projection.reserve_type()?;
        projection.types[root.0 as usize] = Some(TypeInfo::Function(SignatureInfo {
            generics: projection.scopes[scope].own_binders.clone(),
            parameters: Vec::new(),
            output: unit,
            declared_output: None,
            effects: None,
        }));
        Ok(projection.finish()?.type_fingerprint(root)?)
    }
}
