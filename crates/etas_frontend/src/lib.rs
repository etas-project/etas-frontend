mod artifact;
mod incremental;
mod output;
mod package_metadata;
mod passes;
mod pipeline;
mod project;
mod session;

pub use artifact::{
    ArtifactDependencySummary, ArtifactReuseStatsSummary, DiskCacheAccess,
    FRONTEND_ARTIFACT_SCHEMA_VERSION, FRONTEND_COMPILER_VERSION, FrontendArtifactKey,
    FrontendArtifactKind, FrontendArtifactManifest, FrontendDiskArtifactStore, FrontendUnitKey,
    ModuleImportExportSummary, SourceFingerprintSummary,
};
pub use etas_cache::ProjectRevision;
pub use incremental::{
    ChangeSummary, CheckMode, CheckRequest, CheckResponse, CheckScope, CompilerOptionChange,
    DependencyOverlayChange, DiagnosticSet, EnvironmentChange, MemoryArtifactReuse,
    ProjectChangeSet, SnapshotDetailLevel, SourceChange, SourceVersion, TextEdit,
};
pub use output::{
    BLOCK_UNIT_KIND, BODY_UNIT_KIND, CheckOutput, CheckedProgram, CheckedProject,
    DiagnosticDocument, DiagnosticDumpOptions, DiagnosticLabelRecord, DiagnosticRecord,
    DiagnosticSpanRecord, DiagnosticSuggestionRecord, DiagnosticTextEditRecord,
    EXPRESSION_UNIT_KIND, EntryPolicy, ExternalModuleId, ExternalPackageId, ExternalSymbolId,
    ITEM_UNIT_KIND, MODULE_PART_UNIT_KIND, MODULE_UNIT_KIND, ModuleId, ModulePartId, ModulePath,
    PROJECT_UNIT_KIND, ProjectCompileOptions, ProjectEntry, ProjectEntryFact,
    ProjectEntryResolution, ProjectEnvironmentInput, ProjectExternalActionArgKindInput,
    ProjectExternalActionSignatureInput, ProjectExternalActionSummaryInput,
    ProjectExternalAgentSignatureInput, ProjectExternalCallableSpecSatisfactionInput,
    ProjectExternalEffectArgInput, ProjectExternalEffectRefInput, ProjectExternalEffectRowInput,
    ProjectExternalEffectSummaryInput, ProjectExternalExportInput,
    ProjectExternalFlowSignatureInput, ProjectExternalLatentFlowSummaryInput,
    ProjectExternalModuleInput, ProjectExternalNamedSignatureInput, ProjectExternalPackageInput,
    ProjectExternalPublicMetadataInput, ProjectExternalReExportInput,
    ProjectExternalRecordFieldInput, ProjectExternalSpecBoundInput, ProjectExternalSpecImplInput,
    ProjectExternalSpecKindInput, ProjectExternalSpecMethodInput,
    ProjectExternalSpecSignatureInput, ProjectExternalToolSchemaInput,
    ProjectExternalToolSignatureInput, ProjectExternalTraceSpecClauseInput,
    ProjectExternalTraceSpecClauseKindInput, ProjectExternalTraceSpecConformanceInput,
    ProjectExternalTraceSpecConformanceTargetInput, ProjectExternalTraceSpecSummaryInput,
    ProjectExternalTypeInput, ProjectExternalTypeSpecSatisfactionInput, ProjectId, ProjectInput,
    ProjectOutput, ProjectToolBindingInput, SOURCE_FILE_UNIT_KIND, TopLevelLetFact,
    TopLevelLetFacts, UnitId,
};
pub use package_metadata::{
    PackageMetadataArtifact, PackageMetadataBinInput, PackageMetadataBuildInput,
    PackageMetadataDependencyInput, PackageMetadataError, PackageMetadataHeader,
    PackageMetadataToolBindingInput, build_package_metadata_artifact,
    build_package_metadata_artifact_from_checked, emit_package_metadata_artifact,
};
pub(crate) use project::ProjectContext;
pub use project::{
    AffectedModuleSet, AstBodyRef, AstImportRef, AstItemKind, AstItemRef, CatalogModuleOrigin,
    CatalogModuleOriginKind, ExportTable, ExportedItem, FsSourceLoader, HirBodyBindings,
    HirExprPathResolution, HirItemBindings, HirOutput, HirPathResolution, ImportEdge, ImportGraph,
    ImportTarget, LoadedProjectInput, ModuleCatalog, ModuleExport, ModuleExportTable,
    ModuleExportTarget, ModuleIndex, ModuleInfo, ModuleKey, ModuleNamespaceNode,
    ModuleNamespaceTree, ModuleOrigin, ModulePart, ModuleRecord, ModuleTopoOrder, ParseOutput,
    ParsedSource, ProjectSourceLoadOptions, ProjectSourceLoadScope, ProjectSourceLoader, ReExport,
    ReachabilityFacts, ResolvedImport, ResolvedImports, ResolvedModulePath, ResolvedModuleTarget,
    ResolvedPaths, ResolvedWildcardImport, RuntimeSourceReason, RuntimeSourceReasonKind,
    RuntimeSourceRequirement, RuntimeSourceRequirements, SourceBundle, SourceFile, SourceInput,
    SourceKind, SourceLoader, SourceSet, UnitKind, UnitNode, UnitTarget, UnitTree,
};
pub use session::{
    DefinitionTarget, DiagnosticDelta, FrontendArtifactStore, FrontendCacheConfig, FrontendSession,
    FrontendSessionError, FrontendSessionOptions, FrontendSessionStore, ProjectSemanticDelta,
    ProjectSemanticSnapshot, ProjectSessionId, SnapshotDefinitionTargets,
};

use etas_core::{Diagnostic, SourceFile as CoreSourceFile};
pub use etas_hir::HirDumpOptions;
use etas_hir::dump_hir as dump_hir_text;

pub use etas_syntax::DumpOptions;
use etas_syntax::dump_parse;

pub struct Frontend;

impl Frontend {
    pub fn parse(&self, input: SourceInput) -> ParseOutput {
        let source = CoreSourceFile::new(input.id, input.path, input.text);
        let parsed = etas_syntax::parse_program(source.clone());
        ParseOutput { source, parsed }
    }

    pub fn lower(&self, parsed: &ParseOutput) -> HirOutput {
        let hir = etas_hir::lower_program(&parsed.parsed.value);
        let tree_index = etas_hir::HirTreeIndex::build(&hir);
        HirOutput { hir, tree_index }
    }

    pub fn check(&self, input: SourceInput) -> CheckOutput {
        self.check_project(ProjectInput::single_source(input))
            .into_check_output()
    }

    pub fn check_project(&self, input: ProjectInput) -> ProjectOutput {
        let mut session = FrontendSession::new();
        let project = session.open_project(input);
        session
            .check(project, CheckRequest::default())
            .expect("one-shot frontend check uses a valid project session")
            .output
    }
}

pub fn dump_ast(parse_output: &ParseOutput, options: DumpOptions) -> String {
    dump_parse(&parse_output.parsed, options)
}

pub fn dump_hir(check_output: &CheckOutput, options: HirDumpOptions) -> String {
    let hir = check_output
        .hir
        .as_ref()
        .expect("dump_hir requires a CheckOutput produced by the frontend check pipeline");
    dump_hir_text(&hir.hir, options)
}

pub fn dump_diagnostics(
    diagnostics: &[Diagnostic],
    options: DiagnosticDumpOptions,
) -> DiagnosticDocument {
    let mut records = diagnostics
        .iter()
        .map(|diagnostic| diagnostic_record(diagnostic, options))
        .collect::<Vec<_>>();
    records.sort_by(|left, right| {
        (
            left.primary.source.0,
            left.primary.start,
            left.primary.end,
            left.message.as_str(),
        )
            .cmp(&(
                right.primary.source.0,
                right.primary.start,
                right.primary.end,
                right.message.as_str(),
            ))
    });
    DiagnosticDocument {
        diagnostics: records,
    }
}

pub fn diagnostics(output: &CheckOutput) -> &[Diagnostic] {
    &output.diagnostics
}

pub fn source_file(input: SourceInput) -> CoreSourceFile {
    CoreSourceFile::new(input.id, input.path, input.text)
}

fn diagnostic_record(diagnostic: &Diagnostic, options: DiagnosticDumpOptions) -> DiagnosticRecord {
    DiagnosticRecord {
        code: format!("{:?}", diagnostic.code),
        phase: format!("{:?}", diagnostic.phase),
        severity: format!("{:?}", diagnostic.severity),
        message: diagnostic.message.clone(),
        primary: DiagnosticSpanRecord {
            source: diagnostic.primary.span.source,
            start: diagnostic.primary.span.range.start.0,
            end: diagnostic.primary.span.range.end.0,
            label: diagnostic.primary.label.clone(),
        },
        labels: if options.include_labels {
            diagnostic
                .labels
                .iter()
                .map(|label| DiagnosticLabelRecord {
                    source: label.span.source,
                    start: label.span.range.start.0,
                    end: label.span.range.end.0,
                    style: format!("{:?}", label.style),
                    message: label.message.clone(),
                })
                .collect()
        } else {
            Vec::new()
        },
        notes: if options.include_notes {
            diagnostic.notes.clone()
        } else {
            Vec::new()
        },
        help: options
            .include_notes
            .then(|| diagnostic.help.clone())
            .flatten(),
        suggestions: if options.include_suggestions {
            diagnostic
                .suggestions
                .iter()
                .map(|suggestion| DiagnosticSuggestionRecord {
                    title: suggestion.title.clone(),
                    applicability: format!("{:?}", suggestion.applicability),
                    edits: suggestion
                        .edits
                        .iter()
                        .map(|edit| DiagnosticTextEditRecord {
                            start: edit.range.start.0,
                            end: edit.range.end.0,
                            replacement: edit.replacement.clone(),
                        })
                        .collect(),
                })
                .collect()
        } else {
            Vec::new()
        },
    }
}
