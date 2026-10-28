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

    pub(crate) fn facts(&self) -> &HashMap<HirExprId, CheckedReflectionFact> {
        &self.types.reflections
    }

    pub(crate) fn site(&self, expression: HirExprId) -> Option<(HirItemId, Span)> {
        self.sites
            .get(&expression)
            .map(|site| (site.owner, site.span))
    }
}
