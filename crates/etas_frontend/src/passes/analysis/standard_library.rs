use std::collections::HashMap;
use std::sync::Arc;

use etas_core::SourceId;
use etas_hir::{HirItem, HirItemId, SymbolId};
use etas_std::{
    StdImplementationDeterminism, StdImplementationRef, StdIntrinsicId, StdLibrarySourceId,
    StdSymbolId,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::ProjectContext;
use crate::passes::artifacts::{
    ADMITTED_STD_LIBRARY_SOURCES, CHECKED_STD_LIBRARY_IMPLEMENTATIONS, EFFECT_OUTPUT, HIR_OUTPUT,
    TYPE_OUTPUT,
};

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub(crate) struct CheckedStandardLibraryImplementation {
    pub declaration: StdSymbolId,
    pub source: StdLibrarySourceId,
    pub item: HirItemId,
    pub symbol: SymbolId,
    pub optimized_intrinsic: Option<StdIntrinsicId>,
    pub signature: etas_types::CallableSignature,
    pub effects: etas_effects::EffectRow,
    pub requested_actions: etas_effects::EffectRow,
    pub determinism: etas_effects::Determinism,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, etas_core::RetainedSize)]
pub(crate) struct CheckedStandardLibraryImplementations {
    pub implementations: Vec<CheckedStandardLibraryImplementation>,
}

pub struct CheckStandardLibraryImplementationsPass;

impl Pass<ProjectContext> for CheckStandardLibraryImplementationsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new(
            "CheckStandardLibraryImplementationsPass",
            PassKind::Analysis,
        )
        .requires(ArtifactSet::from([
            ADMITTED_STD_LIBRARY_SOURCES,
            HIR_OUTPUT,
            TYPE_OUTPUT,
            EFFECT_OUTPUT,
        ]))
        .produces(ArtifactSet::one(CHECKED_STD_LIBRARY_IMPLEMENTATIONS))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let (Some(admitted), Some(hir), Some(types), Some(effects), Some(effect_artifacts)) = (
            context.admitted_std_library_sources.as_ref(),
            context.hir.as_ref(),
            context.types.as_ref(),
            context.effects.as_ref(),
            context.effect_pipeline_artifacts.as_ref(),
        ) else {
            return PassResult::failed(
                "standard library implementation checking requires finalized type and effect facts",
            );
        };

        let admitted_by_source = admitted
            .sources
            .iter()
            .map(|source| (source.source, source.library_source))
            .collect::<HashMap<SourceId, StdLibrarySourceId>>();
        let mut source_items =
            HashMap::<(StdLibrarySourceId, String), Vec<(HirItemId, SymbolId)>>::new();
        for (item, declaration) in hir.hir.items.iter() {
            let HirItem::Flow(flow) = declaration else {
                continue;
            };
            let Some(source) = admitted_by_source.get(&flow.span.source).copied() else {
                continue;
            };
            let Some(symbol) = hir.hir.symbols.get(flow.symbol) else {
                return PassResult::failed(
                    "standard library source flow is missing its checked symbol",
                );
            };
            source_items
                .entry((source, symbol.name.clone()))
                .or_default()
                .push((item, flow.symbol));
        }

        let mut checked = CheckedStandardLibraryImplementations::default();
        for declaration in context.std_registry.symbols() {
            let Some(StdImplementationRef::Source {
                target,
                optimized_intrinsic,
            }) = declaration.implementation
            else {
                continue;
            };
            if !admitted
                .sources
                .iter()
                .any(|source| source.library_source == target.source)
            {
                continue;
            }
            let Some(items) = source_items.get(&(target.source, target.item.to_owned())) else {
                return PassResult::failed(format!(
                    "standard declaration `{}` has no checked source implementation",
                    declaration.qualified_path.join(".")
                ));
            };
            let [(item, symbol)] = items.as_slice() else {
                return PassResult::failed(format!(
                    "standard declaration `{}` has multiple checked source implementations",
                    declaration.qualified_path.join(".")
                ));
            };
            let Some(public_signature) = types.facts.std_callable_signatures.get(&declaration.id)
            else {
                return PassResult::failed(format!(
                    "standard declaration `{}` has no checked public signature",
                    declaration.qualified_path.join(".")
                ));
            };
            let Some(etas_types::ItemSignature::Flow(source_signature)) =
                types.facts.item_signatures.get(item)
            else {
                return PassResult::failed(format!(
                    "standard source implementation `{}` has no checked flow signature",
                    target.item
                ));
            };
            if !signatures_match(source_signature, public_signature) {
                return PassResult::failed(format!(
                    "standard source implementation `{}` does not match declaration `{}`: source={source_signature:?}, declaration={public_signature:?}",
                    target.item,
                    declaration.qualified_path.join(".")
                ));
            }
            let Some(summary) = effects.facts.item_effects.get(item) else {
                return PassResult::failed(format!(
                    "standard source implementation `{}` has no solved effect summary",
                    target.item
                ));
            };
            let expected_effects = match public_signature.effects.as_ref() {
                Some(row) => etas_effects::validate::resolve_checked_type_row(
                    &types.store,
                    &effect_artifacts.registry,
                    row,
                ),
                None => Ok(etas_effects::EffectRow::empty()),
            };
            let expected_effects = match expected_effects {
                Ok(row) => row,
                Err(reason) => return PassResult::failed(reason),
            };
            let expected_requests = match public_signature.requested_actions.as_ref() {
                Some(row) => etas_effects::validate::resolve_checked_type_row(
                    &types.store,
                    &effect_artifacts.registry,
                    row,
                ),
                None => Ok(etas_effects::EffectRow::empty()),
            };
            let expected_requests = match expected_requests {
                Ok(row) => row,
                Err(reason) => return PassResult::failed(reason),
            };
            let expected_determinism = match target.determinism {
                StdImplementationDeterminism::Deterministic => {
                    etas_effects::Determinism::Deterministic
                }
                StdImplementationDeterminism::RuntimeMediated => {
                    etas_effects::Determinism::RuntimeMediated
                }
                StdImplementationDeterminism::NonDeterministic => {
                    etas_effects::Determinism::NonDeterministic
                }
            };
            if !row_is_allowed(&summary.escaping_effects, &expected_effects)
                || !row_is_allowed(&summary.requested_actions, &expected_requests)
                || summary.determinism != expected_determinism
                || matches!(
                    summary.support,
                    etas_effects::InterpreterSupport::Rejected(_)
                )
            {
                return PassResult::failed(format!(
                    "standard source implementation `{}` has behavior incompatible with declaration `{}`: source effects={:?}, source requests={:?}, source determinism={:?}; declaration effects={expected_effects:?}, declaration requests={expected_requests:?}, declaration determinism={expected_determinism:?}",
                    target.item,
                    declaration.qualified_path.join("."),
                    summary.escaping_effects,
                    summary.requested_actions,
                    summary.determinism,
                ));
            }
            checked
                .implementations
                .push(CheckedStandardLibraryImplementation {
                    declaration: declaration.id,
                    source: target.source,
                    item: *item,
                    symbol: *symbol,
                    optimized_intrinsic,
                    signature: source_signature.clone(),
                    effects: summary.escaping_effects.clone(),
                    requested_actions: summary.requested_actions.clone(),
                    determinism: summary.determinism,
                });
        }
        checked
            .implementations
            .sort_by_key(|implementation| implementation.declaration);
        context.checked_std_library_implementations = Some(Arc::new(checked));
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(CHECKED_STD_LIBRARY_IMPLEMENTATIONS),
        )
    }
}

fn row_is_allowed(actual: &etas_effects::EffectRow, declared: &etas_effects::EffectRow) -> bool {
    actual.open == declared.open
        && actual
            .effects
            .iter()
            .all(|effect| declared.effects.contains(effect))
}

fn signatures_match(
    source: &etas_types::CallableSignature,
    declaration: &etas_types::CallableSignature,
) -> bool {
    let empty = etas_types::EffectRowRef {
        effects: Vec::new(),
        tail: None,
    };
    source.generic_params == declaration.generic_params
        && source.params == declaration.params
        && source.output == declaration.output
        && source.effects.as_ref().unwrap_or(&empty)
            == declaration.effects.as_ref().unwrap_or(&empty)
        && source.requested_actions.as_ref().unwrap_or(&empty)
            == declaration.requested_actions.as_ref().unwrap_or(&empty)
}

#[cfg(test)]
mod tests {
    use etas_std::{StdImplementationRef, StdLibrarySourceId};
    use etas_utils::{PassControl, PassManager};

    use crate::{ProjectContext, ProjectInput, SourceInput, pipeline::project_pipeline};

    #[test]
    fn source_backed_text_implementation_is_admitted_and_checked() {
        let mut context = ProjectContext::new(ProjectInput::single_source(SourceInput::anonymous(
            r#"
module app;
import std.text.concat;
flow main() -> string { return concat(["a", "b"]); }
"#,
        )));
        let plan = crate::optimization::OptimizationPlan::from_options(&context.input.options);
        let mut pipeline = project_pipeline(&plan);
        let run = PassManager::new().run_pipeline(&mut pipeline, &mut context);

        assert_eq!(run.control, PassControl::Continue);
        assert!(context.diagnostics.is_empty(), "{:?}", context.diagnostics);
        assert!(
            context
                .admitted_std_library_sources
                .as_ref()
                .unwrap()
                .sources
                .iter()
                .any(|source| source.library_source == StdLibrarySourceId::TextAssembly)
        );
        let declaration = context
            .std_registry
            .lookup_qualified(&["std", "text", "concat"])
            .unwrap();
        let checked = context
            .checked_std_library_implementations
            .as_ref()
            .unwrap()
            .implementations
            .iter()
            .find(|implementation| implementation.declaration == declaration.id)
            .unwrap();
        let Some(StdImplementationRef::Source {
            optimized_intrinsic,
            ..
        }) = declaration.implementation
        else {
            panic!("source implementation");
        };
        assert_eq!(checked.optimized_intrinsic, optimized_intrinsic);
    }
}
