use std::collections::HashSet;

use etas_types::{
    CallableGenericParam, CallableGenericParamKind, Type, TypeId, TypeStore, TypeSubstitutionEngine,
};

/// Checked data types that must be concrete and freezable before CPU admission.
#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct CpuFreezeContract {
    pub parameters: Vec<TypeId>,
    pub output: TypeId,
    pub captures: Vec<CpuCaptureContract>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct CpuCaptureContract {
    pub symbol: etas_hir::SymbolId,
    pub ty: TypeId,
}

impl CpuFreezeContract {
    pub(super) fn supports_template(
        &self,
        store: &TypeStore,
        generics: &[CallableGenericParam],
    ) -> bool {
        let parameters = generics
            .iter()
            .filter(|parameter| parameter.kind == CallableGenericParamKind::Type)
            .map(|parameter| parameter.subject)
            .collect();
        self.parameters
            .iter()
            .chain(std::iter::once(&self.output))
            .chain(self.captures.iter().map(|capture| &capture.ty))
            .all(|ty| freezable_type(store, *ty, &parameters, &mut HashSet::new()))
    }

    pub fn is_materialized(
        &self,
        store: &TypeStore,
        substitution: &mut TypeSubstitutionEngine<'_>,
    ) -> bool {
        self.parameters
            .iter()
            .chain(std::iter::once(&self.output))
            .chain(self.captures.iter().map(|capture| &capture.ty))
            .all(|ty| {
                substitution
                    .substitute(*ty)
                    .is_ok_and(|ty| freezable_type(store, ty, &HashSet::new(), &mut HashSet::new()))
            })
    }
}

fn freezable_type(
    store: &TypeStore,
    ty: TypeId,
    parameters: &HashSet<TypeId>,
    visited: &mut HashSet<TypeId>,
) -> bool {
    if parameters.contains(&ty) {
        return true;
    }
    if !visited.insert(ty) {
        return true;
    }
    match store.get(ty) {
        Some(Type::Primitive(_) | Type::IntegerLiteral { .. }) => true,
        Some(Type::Array(inner) | Type::Option(inner)) => {
            freezable_type(store, *inner, parameters, visited)
        }
        Some(Type::Tuple(parts)) => parts
            .iter()
            .all(|ty| freezable_type(store, *ty, parameters, visited)),
        Some(Type::Record(record)) => record
            .fields
            .iter()
            .all(|field| freezable_type(store, field.ty, parameters, visited)),
        Some(Type::Nominal(_) | Type::Applied { .. }) => {
            etas_types::ty::applied_representation_in_store(store, ty)
                .ok()
                .flatten()
                .is_some_and(|ty| freezable_type(store, ty, parameters, visited))
        }
        _ => false,
    }
}
