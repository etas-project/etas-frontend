use std::{collections::HashMap, sync::Arc};

use etas_core::Span;
use etas_hir::{HirExpr, HirExprId, HirItemId, HirRevision};
use etas_types::{CheckedReflectionFact, TypeFacts, TypeOutput};
use etas_utils::{PipelineExecutionControl, PipelineInterruption};

use crate::{HirPathResolution, ResolvedPaths};

/// Project-level provenance for reflection facts already solved by etas_types.
/// Keep the type-fact owner shared instead of copying its declaration universe.
#[derive(Clone, Debug, etas_core::RetainedSize)]
pub(crate) struct CheckedReflectionTargets {
    types: Arc<TypeFacts>,
    sites: HashMap<HirExprId, ReflectionSite>,
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
struct ReflectionSite {
    owner: HirItemId,
    span: Span,
    resolution: HirPathResolution,
}

pub(crate) enum ReflectionTargetError {
    Invalid(&'static str),
    Interrupted(PipelineInterruption),
}

impl From<&'static str> for ReflectionTargetError {
    fn from(reason: &'static str) -> Self {
        Self::Invalid(reason)
    }
}

fn checkpoint(control: Option<&dyn PipelineExecutionControl>) -> Result<(), ReflectionTargetError> {
    if let Some(control) = control {
        control
            .checkpoint()
            .map_err(ReflectionTargetError::Interrupted)?;
    }
    Ok(())
}

impl CheckedReflectionTargets {
    pub(crate) fn library_paths(&self, hir: &HirRevision) -> Result<ResolvedPaths, &'static str> {
        let mut expressions = self.sites.iter().collect::<Vec<_>>();
        expressions.sort_unstable_by_key(|(id, _)| **id);
        let expr_paths = expressions
            .into_iter()
            .map(|(id, site)| {
                let Some(HirExpr::Reflect(reflection)) = hir.exprs.get(*id) else {
                    return Err("reflection site has no HIR expression");
                };
                Ok(crate::HirExprPathResolution {
                    expr: *id,
                    path: reflection.target.root_path().clone(),
                    result: site.resolution.clone(),
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(ResolvedPaths {
            expr_paths,
            ..Default::default()
        })
    }

    /// Extend only declaration-layout facts while retaining the same checked
    /// reflection sites and their original HIR revision.
    pub(crate) fn extend_checked_types(
        self: &Arc<Self>,
        original: &Arc<TypeFacts>,
        store: &etas_types::TypeStore,
        generics: HashMap<etas_types::TypeId, Vec<etas_types::CallableGenericParam>>,
        layouts: HashMap<etas_types::TypeId, etas_types::EnumLayoutFact>,
        specs: etas_types::facts::RelocatedSpecFacts,
        control: Option<&dyn PipelineExecutionControl>,
    ) -> Result<(Arc<TypeFacts>, Arc<Self>), ReflectionTargetError> {
        if !Arc::ptr_eq(&self.types, original) {
            return Err("reflection targets belong to a different type-fact revision".into());
        }
        for (id, value) in &generics {
            checkpoint(control)?;
            let Some(owner) = store.nominal_identity(*id) else {
                return Err("foreign declaration has no canonical type identity".into());
            };
            for (index, parameter) in value.iter().enumerate() {
                checkpoint(control)?;
                let index = u32::try_from(index).map_err(|_| {
                    ReflectionTargetError::Invalid("foreign parameter index is invalid")
                })?;
                let identity =
                    etas_types::ty::CanonicalTypeParameterIdentity::new(owner.clone(), index)
                        .map_err(ReflectionTargetError::Invalid)?;
                if store.parameter_identity(parameter.subject) != Some(&identity) {
                    return Err("foreign parameter lacks its checked binder identity".into());
                }
            }
            if original
                .type_generics
                .get(id)
                .is_some_and(|old| old != value)
            {
                return Err("foreign generic declaration conflicts with checked facts".into());
            }
        }
        for (id, value) in &layouts {
            checkpoint(control)?;
            if store.nominal_identity(*id).is_none()
                || !matches!(store.get(*id), Some(etas_types::Type::Enum(_)))
            {
                return Err("foreign enum layout has no canonical identity".into());
            }
            if original
                .enum_layouts
                .get(id)
                .is_some_and(|old| old != value)
            {
                return Err("foreign enum layout conflicts with checked facts".into());
            }
        }
        specs
            .validate(original, store, control)
            .map_err(|error| match error {
                etas_types::facts::specs::RelocatedSpecValidationError::Invalid(reason) => {
                    ReflectionTargetError::Invalid(reason)
                }
                etas_types::facts::specs::RelocatedSpecValidationError::Interrupted(error) => {
                    ReflectionTargetError::Interrupted(error)
                }
            })?;
        if generics.is_empty()
            && layouts.is_empty()
            && specs.canonical.signatures.is_empty()
            && specs.canonical.impls.is_empty()
            && specs.std_impls.is_empty()
        {
            return Ok((original.clone(), self.clone()));
        }
        checkpoint(control)?;
        let mut extended = (**original).clone();
        extended.type_generics.extend(generics);
        extended.enum_layouts.extend(layouts);
        specs
            .merge_into(&mut extended, control)
            .map_err(ReflectionTargetError::Interrupted)?;
        let extended = Arc::new(extended);
        let mut sites = HashMap::with_capacity(self.sites.len());
        for (id, site) in &self.sites {
            checkpoint(control)?;
            sites.insert(*id, site.clone());
        }
        Ok((
            extended.clone(),
            Arc::new(Self {
                types: extended,
                sites,
            }),
        ))
    }

    pub(crate) fn resolve(
        hir: &HirRevision,
        paths: &ResolvedPaths,
        types: &TypeOutput,
        analyzed_items: &std::collections::BTreeSet<HirItemId>,
        has_source_errors: bool,
        control: Option<&dyn PipelineExecutionControl>,
    ) -> Result<Self, ReflectionTargetError> {
        let view = hir.view();
        let mut sites = HashMap::new();
        let mut path_index = HashMap::new();
        for path in &paths.expr_paths {
            checkpoint(control)?;
            if matches!(hir.exprs.get(path.expr), Some(HirExpr::Reflect(_)))
                && path_index.insert(path.expr, path).is_some()
            {
                return Err("reflection has duplicate resolved-path facts".into());
            }
        }
        for (expression, fact) in &types.facts.reflections {
            checkpoint(control)?;
            let Some(HirExpr::Reflect(reflection)) = hir.exprs.get(*expression) else {
                return Err(
                    "checked reflection fact does not identify a reflection expression".into(),
                );
            };
            let path = path_index
                .get(expression)
                .ok_or("checked reflection is missing its resolved-path provenance")?;
            if &path.path != reflection.target.root_path()
                || matches!(
                    path.result,
                    HirPathResolution::Unresolved | HirPathResolution::AmbiguousWildcard { .. }
                )
            {
                return Err("checked reflection disagrees with its resolved target path".into());
            }
            let owner = view
                .index()
                .expr_item
                .get(expression)
                .copied()
                .ok_or("checked reflection has no owning HIR item")?;
            if view.index().item_module.get(&owner).copied() != fact.access_module {
                return Err("checked reflection access scope differs from its HIR owner".into());
            }
            if types.facts.expr_types.get(expression) != Some(&fact.descriptor_type)
                || types.store.get(fact.descriptor_type).is_none()
            {
                return Err(
                    "checked reflection descriptor differs from its expression type".into(),
                );
            }
            sites.insert(
                *expression,
                ReflectionSite {
                    owner,
                    span: reflection.span,
                    resolution: path.result.clone(),
                },
            );
        }
        if !has_source_errors {
            for expression in path_index.keys() {
                checkpoint(control)?;
                let owner = view
                    .index()
                    .expr_item
                    .get(expression)
                    .ok_or("resolved reflection has no owning HIR item")?;
                if analyzed_items.contains(owner) && !sites.contains_key(expression) {
                    return Err("typed reflection expression has no checked target fact".into());
                }
            }
        }
        Ok(Self {
            types: types.facts.clone(),
            sites,
        })
    }

    pub(crate) fn validate_owner(&self, types: &Arc<TypeFacts>) -> Result<(), String> {
        if !Arc::ptr_eq(&self.types, types) {
            return Err("reflection targets belong to a different type-fact revision".into());
        }
        Ok(())
    }

    pub(crate) fn resolution(&self, expression: HirExprId) -> Option<&HirPathResolution> {
        self.sites.get(&expression).map(|site| &site.resolution)
    }

    pub(crate) fn facts(&self) -> &HashMap<HirExprId, CheckedReflectionFact> {
        &self.types.reflections
    }

    pub(crate) fn site(&self, expression: HirExprId) -> Option<(HirItemId, Span)> {
        self.sites
            .get(&expression)
            .map(|site| (site.owner, site.span))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_type_extension_preserves_the_reflection_revision() {
        let original = Arc::new(TypeFacts::default());
        let targets = Arc::new(CheckedReflectionTargets {
            types: original.clone(),
            sites: HashMap::new(),
        });
        let identity = etas_code::DefinitionKey::new(
            etas_code::PackageKey::from_name_and_version("caller", "1.0.0").unwrap(),
            vec!["app".into()],
            vec!["Token".into()],
            etas_code::DefinitionKind::Type,
        )
        .unwrap();
        let mut types = etas_types::TypeInterner::new();
        let id = types
            .intern_canonical_nominal(
                identity,
                etas_types::Type::Nominal(etas_types::NominalTypeRef {
                    name: "Token".into(),
                    params: vec![],
                    representation: None,
                    read_only_erased_view: false,
                }),
            )
            .unwrap();
        let generics = HashMap::from([(id, Vec::new())]);
        let Ok((extended, rebound)) = targets.extend_checked_types(
            &original,
            types.store(),
            generics,
            HashMap::new(),
            Default::default(),
            None,
        ) else {
            panic!("valid extension failed");
        };
        assert!(rebound.validate_owner(&extended).is_ok());
        assert!(rebound.validate_owner(&original).is_err());
        assert!(original.type_generics.is_empty());
        assert_eq!(extended.type_generics.get(&id), Some(&Vec::new()));
        assert!(matches!(
            targets.extend_checked_types(
                &extended,
                types.store(),
                HashMap::new(),
                HashMap::new(),
                Default::default(),
                None
            ),
            Err(ReflectionTargetError::Invalid(_))
        ));
    }
}
