use std::sync::Arc;

use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{
    CALLABLE_INSTANTIATIONS, DIAGNOSTICS, EFFECT_OUTPUT, GENERIC_EFFECT_BINDINGS, HIR_OUTPUT,
    TYPE_OUTPUT,
};
use crate::{
    ProjectContext,
    project::instantiation::{self, InstantiationError},
};

pub struct PrepareInvocationTypesPass;

/// Materializes types for statically resolved source calls before effect
/// analysis. Dynamic and deferred calls remain the responsibility of the
/// later, effect-informed invocation pass.
pub struct PrepareDirectInvocationTypesPass;

impl Pass<ProjectContext> for PrepareDirectInvocationTypesPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("PrepareDirectInvocationTypesPass", PassKind::Transform)
            .requires(ArtifactSet::from([HIR_OUTPUT, TYPE_OUTPUT, DIAGNOSTICS]))
            .produces(ArtifactSet::from([TYPE_OUTPUT, DIAGNOSTICS]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        if context
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error)
        {
            return PassResult::unchanged();
        }
        let (Some(hir), Some(types)) = (&context.hir, &mut context.types) else {
            return PassResult::failed(
                "direct invocation preparation requires checked HIR and types",
            );
        };
        let mut targets = std::collections::HashMap::new();
        for (expr, fact) in &types.facts.generic_instantiations {
            if !fact.deferred_effect_row_obligations.is_empty() {
                continue;
            }
            let Some(etas_hir::HirExpr::Call { callee, .. }) = hir.hir.exprs.get(*expr) else {
                continue;
            };
            let Some(etas_hir::HirExpr::Path(path)) = hir.hir.exprs.get(*callee) else {
                continue;
            };
            let etas_hir::ResolveResult::Resolved(symbol) = path.resolution else {
                continue;
            };
            let Some(symbol) = hir.hir.canonical_source_symbol(symbol) else {
                return PassResult::failed("direct generic call has an invalid source binding");
            };
            let Some(etas_hir::SymbolDef::Item { item }) =
                hir.hir.symbols.get(symbol).map(|symbol| &symbol.def)
            else {
                continue;
            };
            if instantiation::signature(&types.facts, *item).is_none() {
                continue;
            }
            targets
                .entry(*expr)
                .or_insert_with(std::collections::BTreeSet::new)
                .insert(*item);
        }
        if targets.is_empty() {
            return PassResult::unchanged();
        }
        let limits = context.candidate.as_ref().map_or_else(
            etas_code::descriptor::DescriptorLimits::default,
            |candidate| candidate.descriptor_limits,
        );
        let prepared = match instantiation::prepare_from_targets(
            &hir.hir,
            types,
            &targets,
            limits,
            manager.execution_control(),
            &[],
            None,
        ) {
            Ok(prepared) => prepared,
            Err(InstantiationError::Invalid(reason)) => return PassResult::failed(reason),
            Err(InstantiationError::Interrupted(interruption)) => {
                return PassResult::interrupted(interruption);
            }
            Err(InstantiationError::Resource { owner, reason }) => {
                if context.candidate.is_some() || manager.execution_control().is_some() {
                    return PassResult::interrupted(etas_utils::PipelineInterruption {
                        kind: etas_utils::PipelineInterruptionKind::ResourceLimit,
                        reason,
                    });
                }
                let Some(item) = hir.hir.items.get(owner) else {
                    return PassResult::failed("resource diagnostic has no checked callable owner");
                };
                context.diagnostics.push(etas_core::Diagnostic::type_check(
                    etas_core::TypeDiagnosticCode::CompilerResourceLimit,
                    item.span(),
                    reason,
                ));
                return PassResult::changed(PreservedArtifacts::All, self.descriptor().produces);
            }
        };
        types.store = prepared.store;
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}

impl Pass<ProjectContext> for PrepareInvocationTypesPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("PrepareInvocationTypesPass", PassKind::Transform)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                TYPE_OUTPUT,
                EFFECT_OUTPUT,
                GENERIC_EFFECT_BINDINGS,
                DIAGNOSTICS,
            ]))
            .produces(ArtifactSet::from([
                CALLABLE_INSTANTIATIONS,
                TYPE_OUTPUT,
                DIAGNOSTICS,
            ]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        if context
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error)
        {
            context.native_instantiations = Some(Arc::default());
            return PassResult::changed(PreservedArtifacts::All, self.descriptor().produces);
        }
        let (Some(hir), Some(types), Some(effects)) =
            (&context.hir, &mut context.types, &context.effects)
        else {
            return PassResult::failed(
                "invocation preparation requires checked HIR, type and effect artifacts",
            );
        };
        let prepared = match instantiation::prepare(
            instantiation::PreparationInput {
                hir: &hir.hir,
                types,
                effects: &effects.facts,
                limits: context.candidate.as_ref().map_or_else(
                    etas_code::descriptor::DescriptorLimits::default,
                    |candidate| candidate.descriptor_limits,
                ),
                control: manager.execution_control(),
            },
            &[],
            None,
        ) {
            Ok(prepared) => prepared,
            Err(InstantiationError::Invalid(reason)) => return PassResult::failed(reason),
            Err(InstantiationError::Interrupted(interruption)) => {
                return PassResult::interrupted(interruption);
            }
            Err(InstantiationError::Resource { owner, reason }) => {
                if context.candidate.is_some() || manager.execution_control().is_some() {
                    return PassResult::interrupted(etas_utils::PipelineInterruption {
                        kind: etas_utils::PipelineInterruptionKind::ResourceLimit,
                        reason,
                    });
                }
                let Some(item) = hir.hir.items.get(owner) else {
                    return PassResult::failed("resource diagnostic has no checked callable owner");
                };
                context.diagnostics.push(etas_core::Diagnostic::type_check(
                    etas_core::TypeDiagnosticCode::CompilerResourceLimit,
                    item.span(),
                    reason,
                ));
                context.native_instantiations = Some(Arc::default());
                return PassResult::changed(PreservedArtifacts::All, self.descriptor().produces);
            }
        };
        types.store = prepared.store;
        context.native_instantiations = Some(prepared.plan);
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}
