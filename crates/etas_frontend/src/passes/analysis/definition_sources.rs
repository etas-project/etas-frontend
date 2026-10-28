use crate::{
    ProjectContext,
    code::definition::{DefinitionSourceCaptureInput, DefinitionSources},
    passes::artifacts::{
        CALLABLE_OUTPUT_FACTS, DEFINITION_SOURCES, HIR_OUTPUT, MODULE_CATALOG, PARSED_SOURCE_SET,
        RESOLVED_IMPORTS, global_with_diagnostics,
    },
};
use etas_core::{Diagnostic, SourceId, Span, TextRange, TextSize, TypeDiagnosticCode};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};
use std::sync::Arc;

pub struct RetainDefinitionSourcesPass;

impl Pass<ProjectContext> for RetainDefinitionSourcesPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("RetainDefinitionSourcesPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                PARSED_SOURCE_SET,
                CALLABLE_OUTPUT_FACTS,
                MODULE_CATALOG,
                RESOLVED_IMPORTS,
            ]))
            .produces(global_with_diagnostics([DEFINITION_SOURCES]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        _: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        if context.diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == etas_core::Severity::Error
                && !matches!(
                    diagnostic.code,
                    etas_core::DiagnosticCode::Effect(
                        etas_core::EffectDiagnosticCode::RuntimeRequiredInPhase1
                    )
                )
        }) {
            return PassResult::changed(
                PreservedArtifacts::All,
                ArtifactSet::one(DEFINITION_SOURCES),
            );
        }
        let (Some(hir), Some(types), Some(bindings), Some(sources), Some(items), Some(catalog)) = (
            &context.hir,
            &context.signature_types,
            &context.hir_body_bindings,
            &context.sources,
            &context.hir_item_bindings,
            &context.module_catalog,
        ) else {
            return PassResult::failed(
                "definition source retention requires HIR, type facts, AST body bindings, and source set",
            );
        };
        match DefinitionSources::capture(DefinitionSourceCaptureInput {
            parsed: &context.parsed_sources,
            sources,
            hir: &hir.hir,
            bodies: bindings,
            items,
            catalog,
            facts: &types.facts,
            options: &context.input.options.reflection,
            revisions: context
                .candidate
                .as_ref()
                .map(|candidate| &candidate.revisions)
                .unwrap_or(&Default::default()),
            original: context.candidate.as_ref().map(|candidate| {
                candidate
                    .overlay
                    .frozen()
                    .checked()
                    .definition_sources
                    .as_ref()
            }),
        }) {
            Ok(archive) => context.definition_sources = Some(Arc::new(archive)),
            Err(error) => {
                // Programmatic settings have no source span. Register their exact
                // representation as a virtual diagnostic source, never an Etas input.
                let text = format!("reflection body entry: {:?}\n", error.entry);
                let Some(id) = sources
                    .files
                    .iter()
                    .map(|source| source.id.0)
                    .max()
                    .and_then(|id| id.checked_add(1))
                else {
                    return PassResult::failed(
                        "reflection configuration diagnostic source identity space is exhausted",
                    );
                };
                let source = crate::SourceFile::from_input(&crate::SourceInput {
                    id: SourceId(id),
                    path: None,
                    text: text.clone(),
                    kind: crate::SourceKind::VirtualGenerated,
                });
                let mut sources = sources.clone();
                Arc::make_mut(&mut sources).files.push(source);
                context.sources = Some(sources);
                context.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::InvalidReflectionConfiguration,
                    Span::new(
                        SourceId(id),
                        TextRange::new(TextSize::ZERO, TextSize::new(text.len())),
                    ),
                    format!(
                        "invalid reflection body entry `{}`: {}",
                        error.entry, error.reason
                    ),
                ));
            }
        }
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(DEFINITION_SOURCES),
        )
    }
}
