use std::{
    cell::{OnceCell, RefCell},
    collections::HashMap,
};

use etas_code::{CodeFingerprint, descriptor::ValueProjectionStep};
use etas_hir::{HirItemId, SymbolId};
use etas_types::{TypeId, TypeSubstitutionError, ty::TypeLookupIndex};

use super::engine::EffectSemantics;

/// Queries over one immutable checked type graph, reused across solver rounds.
#[derive(Default)]
pub(crate) struct EffectTypeQueries {
    pub(super) index: OnceCell<TypeLookupIndex>,
    representations: RefCell<HashMap<TypeId, Option<TypeId>>>,
    projections: RefCell<HashMap<TypeId, ValueProjections>>,
    // Successful projection IDs remain in the fact table across solver rounds.
    pub(super) parameter_projection_ids: RefCell<ParameterProjectionIds>,
}

type ValueProjections = HashMap<Vec<ValueProjectionStep>, Option<TypeId>>;
type ParameterProjectionIds =
    HashMap<(HirItemId, SymbolId), HashMap<Vec<ValueProjectionStep>, CodeFingerprint>>;

impl EffectSemantics<'_> {
    pub(super) fn applied_representation(
        &self,
        ty: TypeId,
    ) -> Result<Option<TypeId>, TypeSubstitutionError> {
        if let Some(result) = self.type_queries.representations.borrow().get(&ty) {
            return Ok(*result);
        }
        let result = etas_types::ty::applied_representation_in_store_indexed(
            &self.types.store,
            self.type_lookup(),
            ty,
        )?;
        self.type_queries
            .representations
            .borrow_mut()
            .insert(ty, result);
        Ok(result)
    }

    pub(super) fn project_value(
        &self,
        ty: TypeId,
        path: &[ValueProjectionStep],
    ) -> Result<Option<TypeId>, TypeSubstitutionError> {
        if let Some(result) = self
            .type_queries
            .projections
            .borrow()
            .get(&ty)
            .and_then(|projections| projections.get(path))
        {
            return Ok(*result);
        }
        let result = etas_types::value_projection::project_indexed(
            &self.types.store,
            &self.types.facts,
            self.type_lookup(),
            ty,
            path,
        )?;
        self.type_queries
            .projections
            .borrow_mut()
            .entry(ty)
            .or_default()
            .insert(path.to_vec(), result);
        Ok(result)
    }
}
