use etas_code::PackageKey;
use etas_types::Type;

use super::{BoundDeclaration, DefinitionIdentityIndex, GenericDeclaration, TypeDeclaration};
use crate::code::descriptor::{DescriptorInput, ProjectionError};

impl DefinitionIdentityIndex {
    pub(super) fn bind_external_types(
        &mut self,
        checked: DescriptorInput<'_>,
        standard_package: PackageKey,
    ) -> Result<(), ProjectionError> {
        let external_identities = checked
            .types
            .reflection_declaration_identities
            .values()
            .collect::<std::collections::HashSet<_>>();
        let bound_identities = self
            .paths
            .values()
            .collect::<std::collections::HashSet<_>>();
        for (ty, identity) in &checked.types.reflection_type_identities {
            if identity.package() == standard_package {
                continue;
            }
            if let Some(declaration) = self.types.get(ty) {
                if declaration.identity != *identity {
                    return Err(ProjectionError::Invalid(
                        "checked type has conflicting source and external identities".into(),
                    ));
                }
                continue;
            }
            if !external_identities.contains(identity) || !bound_identities.contains(identity) {
                return Err(ProjectionError::MissingIdentity(format!(
                    "external type declaration {identity:?}"
                )));
            }
            let name = match checked.type_store.get(*ty) {
                Some(Type::Nominal(nominal)) => &nominal.name,
                Some(Type::Enum(enumeration)) => &enumeration.name,
                _ => return Err(ProjectionError::MissingType(*ty)),
            };
            let parameters = checked.types.type_generics.get(ty).ok_or_else(|| {
                ProjectionError::MissingIdentity(format!(
                    "checked external type generics {identity:?}"
                ))
            })?;
            let declaration = TypeDeclaration {
                identity: identity.clone(),
                parameters: parameters
                    .iter()
                    .map(|parameter| GenericDeclaration {
                        name: parameter.name.clone(),
                        effect: parameter.kind == etas_types::CallableGenericParamKind::Effect,
                        bounds: parameter
                            .bounds
                            .iter()
                            .cloned()
                            .map(BoundDeclaration::Checked)
                            .collect(),
                    })
                    .collect(),
                alias: None,
                checked_type: Some(*ty),
                standard: None,
            };
            if self
                .type_names
                .get(name)
                .is_some_and(|previous| previous.identity != *identity)
            {
                return Err(ProjectionError::Invalid(
                    "external type name has conflicting checked identities".into(),
                ));
            }
            self.type_names.insert(name.clone(), declaration.clone());
            self.types.insert(*ty, declaration);
        }
        Ok(())
    }
}
