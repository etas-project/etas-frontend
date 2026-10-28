use std::collections::BTreeSet;

use etas_core::{Diagnostic, DiagnosticCode, EffectDiagnosticCode, Severity, TypeDiagnosticCode};
use etas_hir::HirExprId;
use etas_types::{ResolvedEffectRowInstantiation, materialize_effect_row_instantiations};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{
    ANALYSIS_REACHABILITY_FACTS, DIAGNOSTICS, EFFECT_OUTPUT, GENERIC_EFFECT_BINDINGS, HIR_OUTPUT,
    TYPE_OUTPUT,
};
use crate::{CheckScope, ProjectContext, project::instantiation};

pub struct MaterializeGenericEffectRowsPass;

impl Pass<ProjectContext> for MaterializeGenericEffectRowsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("MaterializeGenericEffectRowsPass", PassKind::Transform)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                TYPE_OUTPUT,
                EFFECT_OUTPUT,
                ANALYSIS_REACHABILITY_FACTS,
                DIAGNOSTICS,
            ]))
            .produces(ArtifactSet::from([
                TYPE_OUTPUT,
                GENERIC_EFFECT_BINDINGS,
                DIAGNOSTICS,
            ]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        if context.diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == Severity::Error
                && diagnostic.code
                    != DiagnosticCode::Effect(EffectDiagnosticCode::RuntimeRequiredInPhase1)
        }) {
            return PassResult::changed(PreservedArtifacts::All, self.descriptor().produces);
        }
        let (Some(hir), Some(types), Some(effects), Some(artifacts), Some(reachability)) = (
            &context.hir,
            &mut context.types,
            &context.effects,
            &context.effect_pipeline_artifacts,
            &context.reachability,
        ) else {
            return PassResult::failed(
                "generic row materialization requires checked type, effect and reachability artifacts",
            );
        };
        let index = hir.hir.index();
        let mut calls = types
            .facts
            .generic_instantiations
            .iter()
            .filter(|(_, fact)| !fact.deferred_effect_row_obligations.is_empty())
            .map(|(call, _)| *call)
            .collect::<Vec<_>>();
        calls.sort();
        let mut patches = Vec::new();
        for call in calls {
            if let Some(control) = manager.execution_control()
                && let Err(error) = control.checkpoint()
            {
                return PassResult::interrupted(error);
            }
            let Some(owner) = index.expr_item.get(&call) else {
                return PassResult::failed("deferred generic row has no checked expression owner");
            };
            if context.check_scope == CheckScope::EntryReachable
                && !reachability.analysis_items.contains(owner)
            {
                continue;
            }
            let fact = &types.facts.generic_instantiations[&call];
            let patch = (|| -> Result<_, String> {
                let solved = effects
                    .facts
                    .generic_effect_rows
                    .get(&call)
                    .ok_or("deferred generic row has no solved Effect fact")?;
                let mut bindings = Vec::new();
                for name in fact
                    .deferred_effect_row_obligations
                    .iter()
                    .map(|obligation| obligation.param.as_str())
                    .collect::<BTreeSet<_>>()
                {
                    let mut matches = solved.iter().filter(|(parameter, _)| parameter == name);
                    let (_, row) = matches
                        .next()
                        .ok_or("deferred generic binder has no solved Effect row")?;
                    if matches.next().is_some() {
                        return Err(
                            "deferred generic binder has multiple solved Effect rows".into()
                        );
                    }
                    bindings.push((
                        name.to_owned(),
                        instantiation::checked_row(
                            row,
                            &artifacts.registry,
                            instantiation::signature(&types.facts, *owner),
                        )?,
                    ));
                }
                Ok(ResolvedEffectRowInstantiation { call, bindings })
            })();
            match patch {
                Ok(patch) => patches.push(patch),
                Err(reason) => return reject(context, call, reason),
            }
        }
        if let Err(error) = materialize_effect_row_instantiations(types, &patches) {
            return reject(context, error.call, error.to_string());
        }
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}

fn reject(context: &mut ProjectContext, call: HirExprId, reason: String) -> PassResult {
    let Some(hir) = context.hir.as_ref() else {
        return PassResult::failed("generic row diagnostic has no checked HIR");
    };
    let Some(expression) = hir.hir.exprs.get(call) else {
        return PassResult::failed("generic row diagnostic has no checked expression span");
    };
    context.diagnostics.push(Diagnostic::type_check(
        TypeDiagnosticCode::IncompleteTypeFacts,
        expression.span(&hir.hir.blocks),
        reason,
    ));
    PassResult::changed(
        PreservedArtifacts::All,
        MaterializeGenericEffectRowsPass.descriptor().produces,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use etas_utils::{PassControl, Pipeline};
    use std::sync::Arc;

    #[test]
    fn missing_solved_rows_are_rejected_at_the_actual_call_span() {
        let mut source = crate::SourceInput::anonymous(
            "flow twice<effect E>(f: () -> unit ![E]) -> unit ![E] { f(); f(); } flow main() -> unit ![] { twice(() => { return; }); }",
        );
        source.id = etas_core::SourceId(417);
        let mut context = ProjectContext::new(crate::ProjectInput::single_source(source));
        let mut manager = PassManager::new();
        let run = manager.run_pipeline(&mut crate::pipeline::project_pipeline(), &mut context);
        assert!(matches!(run.control, PassControl::Continue));
        assert!(context.diagnostics.is_empty(), "{:?}", context.diagnostics);
        let effects = context.effects.as_mut().unwrap();
        let call = *effects.facts.generic_effect_rows.keys().next().unwrap();
        Arc::make_mut(&mut effects.facts)
            .generic_effect_rows
            .clear();
        let fact = Arc::make_mut(&mut context.types.as_mut().unwrap().facts)
            .generic_instantiations
            .get_mut(&call)
            .unwrap();
        assert!(fact.deferred_effect_row_obligations.is_empty());
        fact.effect_row_bindings.clear();
        fact.deferred_effect_row_obligations
            .push(etas_types::DeferredEffectRowObligation {
                param: "E".into(),
                source: call,
            });
        let before = context.types.as_ref().unwrap().facts.clone();
        let run = manager.run_pipeline(
            &mut Pipeline::new("test.materialize_rows").pass(MaterializeGenericEffectRowsPass),
            &mut context,
        );
        assert!(matches!(run.control, PassControl::Continue));
        assert!(Arc::ptr_eq(&before, &context.types.as_ref().unwrap().facts));
        assert_eq!(context.diagnostics.len(), 1);
        let diagnostic = &context.diagnostics[0];
        let hir = &context.hir.as_ref().unwrap().hir;
        assert_eq!(diagnostic.primary.span, hir.exprs[call].span(&hir.blocks));
        assert_eq!(diagnostic.primary.span.source, etas_core::SourceId(417));
        assert_eq!(
            diagnostic.code,
            DiagnosticCode::Type(TypeDiagnosticCode::IncompleteTypeFacts)
        );
        assert!(diagnostic.message.contains("no solved Effect fact"));
    }

    #[test]
    fn checked_publication_requires_the_completed_generic_binding_artifact() {
        let mut manager = PassManager::new();
        let mut context = ProjectContext::new(crate::ProjectInput::single_source(
            crate::SourceInput::anonymous("flow main() -> unit ![] { return; }"),
        ));
        let descriptor = crate::passes::BuildCheckedProjectPass.descriptor();
        for artifact in descriptor.requires.iter_refs() {
            if artifact.key != GENERIC_EFFECT_BINDINGS {
                manager.mark_artifact_ref_available(artifact);
            }
        }
        let run = manager.run_pipeline(
            &mut Pipeline::new("test.checked_publication")
                .pass(crate::passes::BuildCheckedProjectPass),
            &mut context,
        );
        assert!(matches!(run.control, PassControl::Failed(_)));
        assert!(context.checked.is_none());
    }
}
