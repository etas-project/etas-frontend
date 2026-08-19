use etas_cache::{
    ArtifactKey, ArtifactMeta, ArtifactStore, CacheNamespace, CachedArtifact, CompressionKind,
    DiskArtifactStorePolicy, DiskCacheBudgetPolicy, MemoryArtifactStore, TypedArtifactStore,
};
use etas_core::{DiagnosticCode, EffectDiagnosticCode, TypeDiagnosticCode};
use etas_effects::{
    DependencyEffectExtension, DependencyEffectMetadata, DependencyEffectTag, Effect,
    RuntimeRequirementReason,
};
use etas_frontend::EntryPolicy;
use etas_frontend::{
    AffectedModuleSet, ArtifactDependencySummary, ArtifactReuseStatsSummary, BODY_UNIT_KIND,
    CheckRequest, CheckScope, CompilerOptionChange, DefinitionTarget, DependencyOverlayChange,
    DiagnosticDumpOptions, DiagnosticSet, DiskCacheAccess, EnvironmentChange,
    FRONTEND_ARTIFACT_SCHEMA_VERSION, Frontend, FrontendArtifactKey, FrontendArtifactKind,
    FrontendArtifactManifest, FrontendDiskArtifactStore, FrontendSession, FrontendSessionError,
    FrontendSessionOptions, FrontendUnitKey, HirOutput, HirPathResolution, ITEM_UNIT_KIND,
    ImportGraph, ImportTarget, MemoryArtifactReuse, ModuleImportExportSummary, ModuleIndex,
    ModulePath, ModuleTopoOrder, ParsedSource, ProjectChangeSet, ProjectCompileOptions,
    ProjectEntry, ProjectEntryFact, ProjectEnvironmentInput, ProjectExternalActionArgKindInput,
    ProjectExternalActionSignatureInput, ProjectExternalCallableSpecSatisfactionInput,
    ProjectExternalEffectArgInput, ProjectExternalEffectRefInput, ProjectExternalEffectRowInput,
    ProjectExternalEffectSummaryInput, ProjectExternalExportInput,
    ProjectExternalFlowSignatureInput, ProjectExternalLatentFlowSummaryInput,
    ProjectExternalModuleInput, ProjectExternalNamedSignatureInput, ProjectExternalPackageInput,
    ProjectExternalPublicMetadataInput, ProjectExternalReExportInput,
    ProjectExternalRecordFieldInput, ProjectExternalSpecKindInput,
    ProjectExternalSpecSignatureInput, ProjectExternalToolSignatureInput,
    ProjectExternalTraceSpecClauseInput, ProjectExternalTraceSpecClauseKindInput,
    ProjectExternalTraceSpecConformanceInput, ProjectExternalTraceSpecConformanceTargetInput,
    ProjectExternalTraceSpecSummaryInput, ProjectExternalTypeInput, ProjectInput, ProjectOutput,
    ProjectRevision, ProjectSourceLoadOptions, ProjectSourceLoadScope, ProjectSourceLoader,
    ProjectToolBindingInput, ResolvedImports, ResolvedPaths, SnapshotDetailLevel, SourceChange,
    SourceFingerprintSummary, SourceInput, SourceKind, SourceSet, SourceVersion, TopLevelLetFacts,
    UnitId, UnitKind, UnitTarget, UnitTree, dump_ast, dump_diagnostics, dump_hir,
};
use etas_frontend::{ExternalModuleId, ExternalPackageId, ExternalSymbolId};
use etas_frontend::{PackageMetadataBuildInput, build_package_metadata_artifact_from_checked};
use etas_hir::HirDumpOptions;
use etas_package_metadata::{AnnotationValueKind, package_metadata_from_artifact};
use etas_syntax::DumpOptions;
use etas_types::{ItemSignature, PrimitiveType, SymbolTypeFact, Type, TypeOutput};
use etas_utils::UnitKey;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const FRONTEND_CACHE_HOLDER_ROOT_ENV: &str = "ETAS_FRONTEND_CACHE_HOLDER_ROOT";
const FRONTEND_CACHE_HOLDER_READY_ENV: &str = "ETAS_FRONTEND_CACHE_HOLDER_READY";
const FRONTEND_CACHE_HOLDER_STOP_ENV: &str = "ETAS_FRONTEND_CACHE_HOLDER_STOP";

#[test]
#[ignore = "subprocess helper invoked by frontend disk-store tests"]
fn frontend_disk_store_holder_helper() {
    let root = std::env::var(FRONTEND_CACHE_HOLDER_ROOT_ENV)
        .expect("frontend cache holder root path should be provided");
    let ready = std::env::var(FRONTEND_CACHE_HOLDER_READY_ENV)
        .expect("frontend cache holder ready path should be provided");
    let stop = std::env::var(FRONTEND_CACHE_HOLDER_STOP_ENV)
        .expect("frontend cache holder stop path should be provided");
    let _store =
        FrontendDiskArtifactStore::open(root).expect("frontend cache holder should open store");
    std::fs::write(&ready, b"ready").expect("frontend cache holder should signal readiness");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !std::path::Path::new(&stop).exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn external_environment() -> ProjectEnvironmentInput {
    ProjectEnvironmentInput {
        external_packages: vec![ProjectExternalPackageInput {
            id: ExternalPackageId(0),
            name: "dep".to_owned(),
            version: "1.0.0".to_owned(),
            edition: "2026".to_owned(),
            import_root: "dep".to_owned(),
        }],
        external_modules: vec![ProjectExternalModuleInput {
            package: Some(ExternalPackageId(0)),
            id: ExternalModuleId(1),
            path: ModulePath {
                segments: vec!["dep".to_owned(), "math".to_owned()],
            },
            exports: vec![
                ProjectExternalExportInput {
                    symbol: ExternalSymbolId(0),
                    name: "Number".to_owned(),
                    visibility: etas_hir::Visibility::Public,
                },
                ProjectExternalExportInput {
                    symbol: ExternalSymbolId(1),
                    name: "add".to_owned(),
                    visibility: etas_hir::Visibility::Public,
                },
            ],
        }],
        external_public_metadata: vec![ProjectExternalPublicMetadataInput {
            package: ExternalPackageId(0),
            types: vec![ProjectExternalNamedSignatureInput {
                path: vec!["dep".to_owned(), "math".to_owned(), "Number".to_owned()],
                visibility: "public".to_owned(),
                ty: Some(ProjectExternalTypeInput::Record {
                    fields: vec![ProjectExternalRecordFieldInput {
                        name: "value".to_owned(),
                        ty: ProjectExternalTypeInput::Primitive("i32".to_owned()),
                    }],
                }),
            }],
            values: Vec::new(),
            enums: Vec::new(),
            flows: vec![ProjectExternalFlowSignatureInput {
                path: vec!["dep".to_owned(), "math".to_owned(), "add".to_owned()],
                param_names: vec!["left".to_owned(), "right".to_owned()],
                params: vec![
                    ProjectExternalTypeInput::Primitive("i32".to_owned()),
                    ProjectExternalTypeInput::Primitive("i32".to_owned()),
                ],
                output: ProjectExternalTypeInput::Primitive("i32".to_owned()),
                effects: None,
                visibility: "public".to_owned(),
            }],
            agents: Vec::new(),
            tools: Vec::new(),
            tool_schemas: Vec::new(),
            effects: Vec::new(),
            actions: Vec::new(),
            trace_specs: Vec::new(),
            spec_signatures: Vec::new(),
            spec_impls: Vec::new(),
            type_spec_satisfactions: Vec::new(),
            callable_spec_satisfactions: Vec::new(),
            trace_spec_conformances: Vec::new(),
            effect_summaries: vec![ProjectExternalEffectSummaryInput {
                item: vec!["dep".to_owned(), "math".to_owned(), "add".to_owned()],
                public_effects: ProjectExternalEffectRowInput::default(),
                requested_actions: ProjectExternalEffectRowInput::default(),
                handled_requested_actions: ProjectExternalEffectRowInput::default(),
                latent_flows: Vec::new(),
            }],
            action_summaries: Vec::new(),
            trace_spec_summaries: Vec::new(),
            re_exports: Vec::new(),
        }],
        ..ProjectEnvironmentInput::default()
    }
}

fn external_network_effect_row() -> ProjectExternalEffectRowInput {
    ProjectExternalEffectRowInput {
        effects: vec![ProjectExternalEffectRefInput {
            path: vec!["Network".to_owned()],
            args: Vec::new(),
        }],
    }
}

fn external_error_effect_row(path: Vec<String>) -> ProjectExternalEffectRowInput {
    ProjectExternalEffectRowInput {
        effects: vec![ProjectExternalEffectRefInput {
            path: vec!["Error".to_owned()],
            args: vec![ProjectExternalEffectArgInput::Type(
                ProjectExternalTypeInput::Named(path),
            )],
        }],
    }
}

fn imported_type_id_for_path(
    checked: &etas_frontend::CheckedProject,
    types: &TypeOutput,
    expected_path: &[&str],
) -> Option<etas_types::TypeId> {
    checked.hir.symbols.iter().find_map(|symbol| {
        let etas_hir::SymbolDef::ImportAlias { path, .. } = &symbol.def else {
            return None;
        };
        if !path
            .iter()
            .map(String::as_str)
            .eq(expected_path.iter().copied())
        {
            return None;
        }
        match types.facts.symbol_types.get(&symbol.id) {
            Some(SymbolTypeFact::Type { constructor })
            | Some(SymbolTypeFact::NominalType { constructor, .. }) => {
                Some(etas_types::TypeId(constructor.0))
            }
            _ => None,
        }
    })
}

fn std_module_root() -> ModulePath {
    ModulePath {
        segments: vec!["std".to_owned()],
    }
}

fn src_source_root() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from("src"))
}

#[test]
fn frontend_check_builds_checked_output_for_local_flow() {
    let frontend = Frontend;
    let output = frontend.check(SourceInput::anonymous(
        r#"
flow main() -> unit {
  return;
}
"#,
    ));

    assert!(output.parse.is_some());
    assert!(output.hir.is_some());
    assert!(output.types.is_some());
    assert!(output.effects.is_some());
    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    let checked = output
        .checked
        .as_ref()
        .expect("checked program should exist");
    let entry = checked
        .entry
        .expect("main flow should be selected as entry");
    assert!(matches!(
        checked.hir.items.get(entry),
        Some(etas_hir::HirItem::Flow(_))
    ));
    assert!(
        output
            .types
            .as_ref()
            .is_some_and(|types| types.diagnostics.is_empty())
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn frontend_check_handles_long_logical_condition_without_stack_overflow() {
    let mut condition = String::new();
    for index in 0..160 {
        if index > 0 {
            condition.push_str(" && ");
        }
        condition.push_str("true");
    }
    let source = format!(
        r#"
flow main() -> i32 ![] {{
  if {condition} {{
    return 0;
  }}
  return 1;
}}
"#
    );

    let output = Frontend.check(SourceInput::anonymous(source));

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn frontend_loader_does_not_auto_load_imports_outside_allowed_roots() {
    let base = unique_temp_path("etas_frontend_import_boundary");
    let workspace = base.join("workspace");
    let app = workspace.join("src").join("app");
    let outside = base.join("leak");
    std::fs::create_dir_all(&app).expect("app source directory should be created");
    std::fs::create_dir_all(&outside).expect("outside module directory should be created");
    let main = app.join("main.es");
    std::fs::write(
        &main,
        r#"
module app.main;

import leak.support.{helper};

flow main() -> unit {
  helper();
  return;
}
"#,
    )
    .expect("main source should be written");
    std::fs::write(
        outside.join("support.es"),
        r#"
module leak.support;

public flow helper() -> unit {
  return;
}
"#,
    )
    .expect("outside support source should be written");

    let loaded = ProjectSourceLoader::default()
        .load(ProjectSourceLoadOptions {
            project_root: workspace.clone(),
            roots: vec![main],
            entry: ProjectEntry {
                module: None,
                flow: "main".to_owned(),
            },
            scope: ProjectSourceLoadScope::FullSourceTree,
            source_kind: None,
            import_search_roots: vec![workspace.clone(), workspace.join("src"), app],
            external_module_roots: vec![std_module_root()],
        })
        .expect("loader should not fail when an import remains unresolved");

    assert_eq!(
        loaded.sources.len(),
        1,
        "workspace-external import candidates must not be auto-loaded"
    );
    std::fs::remove_dir_all(&base).expect("import boundary temp tree should be removable");
}

#[test]
fn frontend_loader_import_closure_starts_from_entry_module() {
    let root = unique_temp_path("etas_frontend_entry_import_closure");
    let src = root.join("src");
    let app = src.join("app");
    std::fs::create_dir_all(&app).expect("app source directory should be created");
    std::fs::write(
        app.join("main.es"),
        r#"
module app.main;

import app.used.{helper};

flow main() -> unit {
  helper();
  return;
}
"#,
    )
    .expect("main source should be written");
    std::fs::write(
        app.join("used.es"),
        r#"
module app.used;

public flow helper() -> unit {
  return;
}
"#,
    )
    .expect("used source should be written");
    std::fs::write(
        app.join("unused.es"),
        r#"
module app.unused;

public flow helper() -> unit {
  return;
}
"#,
    )
    .expect("unused source should be written");

    let loaded = ProjectSourceLoader::default()
        .load(ProjectSourceLoadOptions {
            project_root: root.clone(),
            roots: vec![src.clone()],
            entry: ProjectEntry {
                module: Some(ModulePath {
                    segments: vec!["app".to_owned(), "main".to_owned()],
                }),
                flow: "main".to_owned(),
            },
            scope: ProjectSourceLoadScope::ImportClosureFromEntry,
            source_kind: None,
            import_search_roots: vec![root.clone(), src.clone()],
            external_module_roots: vec![std_module_root()],
        })
        .expect("entry import closure should load entry and imported support");

    let loaded_files = loaded
        .sources
        .iter()
        .filter_map(|(path, _)| path.file_name().and_then(|name| name.to_str()))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        loaded_files,
        ["main.es", "used.es"].into_iter().collect(),
        "entry import closure must not load unreachable project modules"
    );
    std::fs::remove_dir_all(&root).expect("entry-closure temp tree should be removable");
}

#[test]
fn frontend_loader_import_closure_fails_when_entry_module_is_missing() {
    let root = unique_temp_path("etas_frontend_missing_entry_import_closure");
    let src = root.join("src");
    std::fs::create_dir_all(&src).expect("source directory should be created");

    let result = ProjectSourceLoader::default().load(ProjectSourceLoadOptions {
        project_root: root.clone(),
        roots: vec![src.clone()],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
        scope: ProjectSourceLoadScope::ImportClosureFromEntry,
        source_kind: None,
        import_search_roots: vec![root.clone(), src],
        external_module_roots: vec![std_module_root()],
    });

    assert!(
        result.is_err(),
        "entry import closure must not broaden to the full source tree when the entry module is missing"
    );
    std::fs::remove_dir_all(&root).expect("missing-entry temp tree should be removable");
}

#[test]
fn frontend_source_loader_ignores_package_manifest_files() {
    let root = unique_temp_path("etas_frontend_source_loader_manifest_boundary");
    let app = root.join("src").join("app");
    std::fs::create_dir_all(&app).expect("app source directory should be created");
    std::fs::write(
        root.join("etas.toml"),
        "this is intentionally not a package manifest",
    )
    .expect("manifest sentinel should be written");
    std::fs::write(
        app.join("main.es"),
        r#"
module app.main;

flow main() -> unit {
  return;
}
"#,
    )
    .expect("main source should be written");

    let loaded = ProjectSourceLoader::default()
        .load(ProjectSourceLoadOptions {
            project_root: root.clone(),
            roots: vec![root.clone()],
            entry: ProjectEntry {
                module: None,
                flow: "main".to_owned(),
            },
            scope: ProjectSourceLoadScope::FullSourceTree,
            source_kind: None,
            import_search_roots: vec![root.clone()],
            external_module_roots: vec![std_module_root()],
        })
        .expect("source project loading must not parse etas.toml");

    assert_eq!(loaded.sources.len(), 1);
    assert!(loaded.input.environment.external_modules.is_empty());
    assert!(loaded.input.environment.tool_bindings.is_empty());
    std::fs::remove_dir_all(&root).expect("manifest boundary temp tree should be removable");
}

#[test]
fn frontend_source_loader_rejects_file_as_source_directory() {
    let root = unique_temp_path("etas_frontend_source_loader_file_root");
    std::fs::create_dir_all(&root).expect("loader temp directory should be created");
    let file_root = root.join("main.es");
    std::fs::write(&file_root, "flow main() -> unit { return; }")
        .expect("source-root sentinel file should be written");

    let result = ProjectSourceLoader::default().load(ProjectSourceLoadOptions {
        project_root: file_root,
        roots: Vec::new(),
        entry: ProjectEntry {
            module: None,
            flow: "main".to_owned(),
        },
        scope: ProjectSourceLoadScope::FullSourceTree,
        source_kind: Some(SourceKind::SourceProjectFile),
        import_search_roots: vec![root.clone()],
        external_module_roots: vec![std_module_root()],
    });

    assert!(
        result.is_err(),
        "explicit source-project roots must not fail open when the root is not a directory"
    );
    std::fs::remove_dir_all(&root).expect("file-root temp tree should be removable");
}

#[test]
fn frontend_session_check_produces_snapshot_and_artifact_manifest() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
flow main() -> unit {
  return;
}
"#,
    )));

    assert!(session.snapshot(project).is_none());

    let response = session
        .check(project, CheckRequest::default())
        .expect("session check should run");

    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    assert_eq!(response.revision, ProjectRevision(0));
    assert!(response.diagnostics.diagnostics.is_empty());
    let snapshot = response.snapshot.as_ref().expect("checked snapshot");
    assert_eq!(snapshot.revision, ProjectRevision(0));
    assert_eq!(snapshot.sources.files.len(), 1);
    assert!(snapshot.diagnostics.diagnostics.is_empty());
    assert!(!response.output.type_body_outputs.is_empty());
    assert_eq!(
        response.output.type_body_outputs.len(),
        response.output.effect_body_outputs.len()
    );
    assert!(
        response.output.effect_pipeline_artifacts.is_some(),
        "effect pipeline artifacts should be retained for frontend artifact/session tracking"
    );

    let manifest = session
        .artifact_manifest(project)
        .expect("artifact manifest should be stored in the session");
    assert!(
        manifest
            .get(&FrontendArtifactKey::project(
                FrontendArtifactKind::CheckedProject,
            ))
            .is_some()
    );
    assert!(
        manifest
            .get(&FrontendArtifactKey::source(
                FrontendArtifactKind::ParsedSource,
                etas_core::SourceId(0),
            ))
            .is_some()
    );
    assert!(
        manifest
            .get(&FrontendArtifactKey::project(
                FrontendArtifactKind::SignatureFacts,
            ))
            .is_some()
    );
    let effect_pipeline_key =
        FrontendArtifactKey::project(FrontendArtifactKind::EffectPipelineArtifacts);
    let effect_facts_key = FrontendArtifactKey::project(FrontendArtifactKind::EffectFacts);
    assert!(manifest.get(&effect_pipeline_key).is_some());
    assert!(
        response
            .cache
            .stored_artifacts
            .contains(&effect_pipeline_key.to_cache_key())
    );
    let effect_pipeline_fingerprint = manifest
        .get(&effect_pipeline_key)
        .expect("effect pipeline artifact should be recorded")
        .fingerprint;
    let effect_facts = manifest
        .get(&effect_facts_key)
        .expect("project effect facts should be recorded");
    assert!(
        effect_facts.dependencies.iter().any(|dependency| {
            dependency.key == effect_pipeline_key
                && dependency.fingerprint == Some(effect_pipeline_fingerprint)
        }),
        "project effect facts should depend on solved effect pipeline artifact fingerprints"
    );
    let body_units = response
        .output
        .units
        .as_ref()
        .expect("unit tree should exist")
        .nodes
        .iter()
        .filter_map(|(unit, node)| {
            (node.kind == UnitKind::Body)
                .then_some(etas_utils::UnitKey::new(BODY_UNIT_KIND, unit.0 as u64))
        })
        .collect::<Vec<_>>();
    assert!(!body_units.is_empty());
    for body in body_units {
        let type_key = FrontendArtifactKey::unit(FrontendArtifactKind::TypeFacts, body);
        assert!(manifest.get(&type_key).is_some());
        assert!(
            response
                .cache
                .stored_artifacts
                .contains(&type_key.to_cache_key())
        );
        let effect_key = FrontendArtifactKey::unit(FrontendArtifactKind::EffectFacts, body);
        assert!(manifest.get(&effect_key).is_some());
        assert!(
            response
                .cache
                .stored_artifacts
                .contains(&effect_key.to_cache_key())
        );
    }
}

#[test]
fn frontend_session_preserves_last_good_snapshot_after_bad_check() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
flow main() -> unit {
  return;
}
"#,
    )));

    let first = session
        .check(project, CheckRequest::default())
        .expect("initial session check should run");
    assert!(
        first.output.checked.is_some(),
        "{:?}",
        first.diagnostics.diagnostics
    );
    assert!(first.snapshot.is_some());
    assert!(session.snapshot(project).is_some());

    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source: etas_core::SourceId(0),
                    version: SourceVersion(1),
                    text: r#"
flow main() -> unit {
  let value: i32 = "bad";
  return;
}
"#
                    .to_owned(),
                }],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect("bad edit should apply");

    let bad = session
        .check(project, CheckRequest::incremental())
        .expect("diagnostic session check should run");
    assert!(bad.output.checked.is_none());
    assert!(bad.snapshot.is_none());
    assert!(!bad.diagnostics.diagnostics.is_empty());
    let last_good = session
        .snapshot(project)
        .expect("session should retain last-good snapshot");
    assert_eq!(last_good.revision, ProjectRevision(0));
    let delta = bad.delta.expect("diagnostic check should produce delta");
    assert!(
        delta
            .diagnostics
            .republish_sources
            .contains(&etas_core::SourceId(0)),
        "changed source diagnostics should be republished"
    );
}

#[test]
fn frontend_session_options_open_disk_backed_project_session() {
    let cache_root = unique_temp_path("etas_frontend_session_options_disk_cache");
    let mut session =
        FrontendSession::with_options(FrontendSessionOptions::disk_cache(&cache_root))
            .expect("disk-backed frontend session should open");
    let project = session
        .try_open_project(ProjectInput::single_source(SourceInput::anonymous(
            r#"
flow main() -> unit {
  return;
}
"#,
        )))
        .expect("disk-backed project should open");

    let response = session
        .check(project, CheckRequest::full_project())
        .expect("disk-backed session check should run");

    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    assert!(
        cache_root.join("v1").join("cache.sqlite").exists(),
        "disk-backed session should create the frontend disk cache index"
    );
    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_session_disk_read_only_does_not_persist_payload_artifacts() {
    let cache_root = unique_temp_path("etas_frontend_session_read_only_cache");
    let mut session = FrontendSession::with_options(
        FrontendSessionOptions::disk_cache(&cache_root)
            .with_disk_cache_access(DiskCacheAccess::read_only()),
    )
    .expect("read-only disk-backed frontend session should open");
    let project = session
        .try_open_project(ProjectInput::single_source(SourceInput::anonymous(
            r#"
flow main() -> unit {
  return;
}
"#,
        )))
        .expect("read-only disk-backed project should open");

    let response = session
        .check(project, CheckRequest::full_project())
        .expect("read-only disk-backed session check should run");

    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    let body_keys = response
        .output
        .type_body_outputs
        .keys()
        .map(|unit| {
            FrontendArtifactKey::unit(
                FrontendArtifactKind::TypeFacts,
                UnitKey::new(BODY_UNIT_KIND, unit.0 as u64),
            )
            .to_cache_key()
        })
        .collect::<Vec<_>>();
    assert!(!body_keys.is_empty());
    let manifest_key =
        FrontendArtifactKey::project(FrontendArtifactKind::ArtifactManifest).to_cache_key();
    let fresh_store = FrontendDiskArtifactStore::open(&cache_root)
        .expect("fresh frontend disk store should open");
    assert!(
        !fresh_store
            .contains(&manifest_key)
            .expect("manifest lookup should succeed"),
        "read-only disk access must not persist frontend payload artifacts"
    );
    let second = session
        .check(project, CheckRequest::incremental())
        .expect("read-only disk-backed session should reuse live memory artifacts");
    for key in body_keys {
        assert!(
            second.cache.reused_artifacts.contains(&key),
            "read-only disk access must still keep live memory artifact {key}"
        );
    }
    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_session_disk_cache_namespace_is_explicit() {
    let cache_root = unique_temp_path("etas_frontend_session_cache_namespace");
    let namespace = "frontend_custom_namespace";
    let mut session = FrontendSession::with_options(
        FrontendSessionOptions::disk_cache(&cache_root).with_cache_namespace(namespace),
    )
    .expect("custom-namespace disk-backed frontend session should open");
    let project = session
        .try_open_project(ProjectInput::single_source(SourceInput::anonymous(
            r#"
flow main() -> unit {
  return;
}
"#,
        )))
        .expect("custom-namespace disk-backed project should open");

    let response = session
        .check(project, CheckRequest::full_project())
        .expect("custom-namespace disk-backed session check should run");

    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    let manifest_key =
        FrontendArtifactKey::project(FrontendArtifactKind::ArtifactManifest).to_cache_key();
    let custom_store = FrontendDiskArtifactStore::open_with_policy_namespace_access(
        &cache_root,
        DiskArtifactStorePolicy::default(),
        CacheNamespace::new(namespace),
        DiskCacheAccess::read_write(),
    )
    .expect("custom namespace frontend disk store should open");
    assert!(
        custom_store
            .contains(&manifest_key)
            .expect("custom namespace manifest lookup should succeed"),
        "custom cache namespace should contain the persisted manifest"
    );
    let default_store = FrontendDiskArtifactStore::open(&cache_root)
        .expect("default namespace frontend disk store should open");
    assert!(
        !default_store
            .contains(&manifest_key)
            .expect("default namespace manifest lookup should succeed"),
        "custom cache namespace must not pollute the default frontend namespace"
    );
    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_session_disk_write_only_ignores_persisted_payload_artifacts() {
    let cache_root = unique_temp_path("etas_frontend_session_write_only_cache");
    let input = ProjectInput::single_source(SourceInput::anonymous(
        r#"
flow helper(value: i32) -> i32 {
  return value + 1;
}

flow main() -> i32 {
  return helper(1);
}
"#,
    ));
    let mut producer =
        FrontendSession::with_options(FrontendSessionOptions::disk_cache(&cache_root))
            .expect("producer disk-backed frontend session should open");
    let producer_project = producer
        .try_open_project(input.clone())
        .expect("producer project should open");
    let produced = producer
        .check(producer_project, CheckRequest::full_project())
        .expect("producer check should run");
    assert!(
        produced.output.checked.is_some(),
        "{:?}",
        produced.diagnostics.diagnostics
    );
    let body_keys = produced
        .output
        .type_body_outputs
        .keys()
        .map(|unit| {
            FrontendArtifactKey::unit(
                FrontendArtifactKind::TypeFacts,
                UnitKey::new(BODY_UNIT_KIND, unit.0 as u64),
            )
            .to_cache_key()
        })
        .collect::<Vec<_>>();
    assert!(!body_keys.is_empty());

    let mut write_only = FrontendSession::with_options(
        FrontendSessionOptions::disk_cache(&cache_root)
            .with_disk_cache_access(DiskCacheAccess::write_only()),
    )
    .expect("write-only disk-backed frontend session should open");
    let write_only_project = write_only
        .try_open_project(input)
        .expect("write-only project should open");
    let response = write_only
        .check(write_only_project, CheckRequest::incremental())
        .expect("write-only incremental check should run");

    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    for key in body_keys {
        assert!(
            !response.cache.reused_artifacts.contains(&key),
            "write-only disk access must not read persisted body artifact {key}"
        );
    }
    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_check_request_can_suppress_response_snapshot_without_clearing_session_snapshot() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
flow main() -> unit {
  return;
}
"#,
    )));

    let response = session
        .check(
            project,
            CheckRequest::full_project().with_snapshot_detail(SnapshotDetailLevel::None),
        )
        .expect("session check should run");

    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    assert!(response.snapshot.is_none());
    assert!(
        session.snapshot(project).is_some(),
        "session should retain its live snapshot for later incremental checks"
    );
}

#[test]
fn frontend_check_request_can_disable_memory_artifact_reuse() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
flow helper(value: i32) -> i32 {
  return value + 1;
}

flow main() -> i32 {
  return helper(1);
}
"#,
    )));

    let first = session
        .check(project, CheckRequest::full_project())
        .expect("initial check should run");
    assert!(
        first.output.checked.is_some(),
        "{:?}",
        first.diagnostics.diagnostics
    );
    let body_keys = first
        .output
        .type_body_outputs
        .keys()
        .map(|unit| {
            FrontendArtifactKey::unit(
                FrontendArtifactKind::TypeFacts,
                UnitKey::new(BODY_UNIT_KIND, unit.0 as u64),
            )
            .to_cache_key()
        })
        .collect::<Vec<_>>();
    assert!(!body_keys.is_empty());

    let second = session
        .check(
            project,
            CheckRequest::incremental().with_memory_artifact_reuse(MemoryArtifactReuse::disabled()),
        )
        .expect("incremental check without memory reuse should run");

    assert!(
        second.output.checked.is_some(),
        "{:?}",
        second.diagnostics.diagnostics
    );
    for key in body_keys {
        assert!(
            !second.cache.reused_artifacts.contains(&key),
            "memory artifact reuse disabled should recompute body artifact {key}"
        );
    }
}

#[test]
fn frontend_full_project_check_does_not_reuse_memory_artifacts() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
flow helper(value: i32) -> i32 {
  return value + 1;
}

flow main() -> i32 {
  return helper(1);
}
"#,
    )));

    session
        .check(project, CheckRequest::full_project())
        .expect("initial check should run");
    let second = session
        .check(project, CheckRequest::full_project())
        .expect("second full project check should run");

    assert!(
        second.output.checked.is_some(),
        "{:?}",
        second.diagnostics.diagnostics
    );
    assert!(
        second.cache.reused_artifacts.is_empty(),
        "full project checks should recompute rather than reporting incremental memory reuse: {:?}",
        second.cache.reused_artifacts
    );
}

#[test]
fn frontend_session_snapshot_exposes_semantic_definition_view() {
    let source = etas_core::SourceId(301);
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput {
        id: source,
        path: None,
        text: r#"
flow helper() -> i64 {
  return 1;
}

flow main() -> i64 {
  return helper();
}
"#
        .to_owned(),
        kind: SourceKind::SingleFileInput,
    }));

    let response = session
        .check(project, CheckRequest::default())
        .expect("session check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    let snapshot = response.snapshot.as_ref().expect("checked snapshot");
    let helper_symbol = snapshot
        .symbols
        .iter()
        .find(|symbol| {
            symbol.name == "helper" && matches!(&symbol.def, etas_hir::SymbolDef::Item { .. })
        })
        .expect("helper symbol should exist");
    let helper_target = DefinitionTarget::SourceSymbol {
        symbol: helper_symbol.id,
        item: helper_symbol.defining_item,
        span: helper_symbol.definition_span,
    };
    assert_eq!(
        snapshot.definition_target_for_symbol(helper_symbol.id),
        Some(&helper_target)
    );
    assert_eq!(helper_symbol.definition_span.source, source);

    let helper_expr = snapshot
        .resolved_paths
        .expr_paths
        .iter()
        .find_map(|resolution| match &resolution.result {
            HirPathResolution::DirectSymbol(symbol) if *symbol == helper_symbol.id => {
                Some(resolution.expr)
            }
            _ => None,
        })
        .expect("helper call path should resolve to the helper symbol");
    assert_eq!(
        snapshot.definition_target_for_expr(helper_expr),
        Some(&helper_target)
    );

    let origin = snapshot
        .hir_origin(etas_hir::HirNodeRef::Expr(helper_expr))
        .expect("helper expr should have source origin");
    let syntax = origin
        .primary_syntax()
        .expect("direct helper expr should have primary syntax");
    assert_eq!(snapshot.syntax_node(syntax.id), Some(&syntax));
    assert!(
        snapshot
            .hir_nodes_for_syntax(syntax.id)
            .contains(&etas_hir::HirNodeRef::Expr(helper_expr)),
        "snapshot should expose syntax-to-HIR reverse lookup"
    );
}

#[test]
fn frontend_session_body_artifact_fingerprints_include_body_text() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
flow helper() -> i64 {
  return 1;
}

flow main() -> i64 {
  return 2;
}
"#,
    )));

    let response = session
        .check(project, CheckRequest::default())
        .expect("session check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );

    let manifest = session
        .artifact_manifest(project)
        .expect("artifact manifest should be stored in the session");
    let body_type_fingerprints = manifest
        .records
        .iter()
        .filter(|record| {
            record.key.kind == FrontendArtifactKind::TypeFacts
                && record
                    .key
                    .to_cache_key()
                    .unit
                    .as_str()
                    .starts_with("unit:frontend:body:")
        })
        .map(|record| record.fingerprint)
        .collect::<Vec<_>>();

    assert_eq!(body_type_fingerprints.len(), 2);
    assert_ne!(body_type_fingerprints[0], body_type_fingerprints[1]);
}

#[test]
fn frontend_session_body_effect_artifacts_depend_on_matching_body_type_artifacts() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
flow first() -> i64 {
  return 1;
}

flow second() -> i64 {
  return first() + 1;
}
"#,
    )));

    session
        .check(project, CheckRequest::default())
        .expect("session check should succeed");

    let manifest = session
        .artifact_manifest(project)
        .expect("artifact manifest should be stored in the session");
    let body_effect_records = manifest
        .records
        .iter()
        .filter(|record| record.key.kind == FrontendArtifactKind::EffectFacts)
        .filter(|record| matches!(record.key.unit, FrontendUnitKey::Unit(unit) if unit.kind == BODY_UNIT_KIND))
        .collect::<Vec<_>>();
    let body_type_records = manifest
        .records
        .iter()
        .filter(|record| record.key.kind == FrontendArtifactKind::TypeFacts)
        .filter(|record| matches!(record.key.unit, FrontendUnitKey::Unit(unit) if unit.kind == BODY_UNIT_KIND))
        .collect::<Vec<_>>();

    assert_eq!(body_effect_records.len(), 2);
    assert_eq!(body_type_records.len(), 2);
    for record in &body_type_records {
        let FrontendUnitKey::Unit(unit) = &record.key.unit else {
            panic!("body type artifact should use a unit key");
        };
        let body_hir_key = FrontendArtifactKey::unit(FrontendArtifactKind::HirBody, *unit);
        let body_hir_fingerprint = manifest
            .get(&body_hir_key)
            .expect("body HIR artifact should be recorded")
            .fingerprint;
        assert!(
            record.dependencies.iter().any(|dependency| {
                dependency.key == body_hir_key
                    && dependency.fingerprint == Some(body_hir_fingerprint)
            }),
            "body type artifact {record:?} should depend on its matching body HIR artifact fingerprint"
        );
    }
    for record in body_effect_records {
        let FrontendUnitKey::Unit(unit) = &record.key.unit else {
            panic!("body effect artifact should use a unit key");
        };
        let body_type_key = FrontendArtifactKey::unit(FrontendArtifactKind::TypeFacts, *unit);
        let body_type_fingerprint = manifest
            .get(&body_type_key)
            .expect("body type artifact should be recorded")
            .fingerprint;
        assert!(
            record.dependencies.iter().any(|dependency| {
                dependency.key == body_type_key
                    && dependency.fingerprint == Some(body_type_fingerprint)
            }),
            "body effect artifact {record:?} should depend on its matching body type artifact fingerprint"
        );
    }
}

#[test]
fn frontend_session_incremental_check_reports_reused_body_analysis_artifacts() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
flow helper() -> i64 {
  return 1;
}

flow main() -> i64 {
  return helper();
}
"#,
    )));

    let first = session
        .check(project, CheckRequest::default())
        .expect("initial check should run");
    let body_type_keys = first
        .output
        .type_body_outputs
        .keys()
        .map(|unit| {
            FrontendArtifactKey::unit(
                FrontendArtifactKind::TypeFacts,
                UnitKey::new(BODY_UNIT_KIND, unit.0 as u64),
            )
            .to_cache_key()
        })
        .collect::<Vec<_>>();
    let body_effect_keys = first
        .output
        .effect_body_outputs
        .keys()
        .map(|unit| {
            FrontendArtifactKey::unit(
                FrontendArtifactKind::EffectFacts,
                UnitKey::new(BODY_UNIT_KIND, unit.0 as u64),
            )
            .to_cache_key()
        })
        .collect::<Vec<_>>();
    assert_eq!(body_type_keys.len(), 2);
    assert_eq!(body_effect_keys.len(), 2);

    let second = session
        .check(project, CheckRequest::incremental())
        .expect("incremental check should run");

    assert!(
        second.output.checked.is_some(),
        "{:?}",
        second.diagnostics.diagnostics
    );
    for key in body_type_keys {
        assert!(
            second.cache.reused_artifacts.contains(&key),
            "incremental check should report reused body type artifact {key}"
        );
    }
    for key in body_effect_keys {
        assert!(
            second.cache.reused_artifacts.contains(&key),
            "incremental check should report reused body effect artifact {key}"
        );
    }
}

#[test]
fn frontend_session_disk_write_only_check_can_reuse_live_memory_artifacts() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
flow helper() -> i64 {
  return 1;
}

flow main() -> i64 {
  return helper();
}
"#,
    )));

    let first = session
        .check(project, CheckRequest::default())
        .expect("initial check should run");
    let body_keys = first
        .output
        .type_body_outputs
        .keys()
        .map(|unit| {
            FrontendArtifactKey::unit(
                FrontendArtifactKind::TypeFacts,
                UnitKey::new(BODY_UNIT_KIND, unit.0 as u64),
            )
            .to_cache_key()
        })
        .chain(first.output.effect_body_outputs.keys().map(|unit| {
            FrontendArtifactKey::unit(
                FrontendArtifactKind::EffectFacts,
                UnitKey::new(BODY_UNIT_KIND, unit.0 as u64),
            )
            .to_cache_key()
        }))
        .collect::<Vec<_>>();
    assert_eq!(body_keys.len(), 4);

    let second = session
        .check(
            project,
            CheckRequest::incremental().with_disk_artifact_access(DiskCacheAccess::write_only()),
        )
        .expect("incremental write-only cache check should run");

    assert!(
        second.output.checked.is_some(),
        "{:?}",
        second.diagnostics.diagnostics
    );
    for key in &body_keys {
        assert!(
            second.cache.reused_artifacts.contains(key),
            "disk write-only check should still reuse live memory body artifact {key}"
        );
    }
    assert!(
        !second
            .cache
            .stored_artifacts
            .iter()
            .any(|key| body_keys.contains(key)),
        "live memory reuse should avoid recomputing body artifacts"
    );
}

#[test]
fn frontend_session_incremental_check_reuses_unchanged_body_after_same_source_edit() {
    let source = etas_core::SourceId(203);
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput {
        id: source,
        path: None,
        text: r#"
flow helper() -> i64 {
  return 1;
}

flow main() -> i64 {
  return 2;
}
"#
        .to_owned(),
        kind: SourceKind::SingleFileInput,
    }));

    let first = session
        .check(project, CheckRequest::default())
        .expect("initial check should run");
    let (helper_body, main_body) = helper_and_main_body_units(&first.output, source);
    let helper_type_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::TypeFacts, helper_body).to_cache_key();
    let helper_effect_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::EffectFacts, helper_body).to_cache_key();
    let main_type_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::TypeFacts, main_body).to_cache_key();
    let main_effect_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::EffectFacts, main_body).to_cache_key();

    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source,
                    version: SourceVersion(1),
                    text: r#"
flow helper() -> i64 {
  return 3;
}

flow main() -> i64 {
  return 2;
}
"#
                    .to_owned(),
                }],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect("source replacement should be accepted");
    assert!(
        session
            .artifact_manifest(project)
            .expect("session artifact manifest should be available")
            .get(&FrontendArtifactKey::unit(
                FrontendArtifactKind::TypeFacts,
                main_body,
            ))
            .is_some(),
        "fresh disk-backed session should restore the persisted artifact manifest before checking"
    );

    let second = session
        .check(project, CheckRequest::incremental())
        .expect("incremental check should run");

    assert!(
        second.output.checked.is_some(),
        "{:?}",
        second.diagnostics.diagnostics
    );
    let affected = second
        .output
        .affected_modules
        .as_ref()
        .expect("affected module set should exist");
    assert!(affected.bodies.contains(&helper_body));
    assert!(affected.bodies.contains(&main_body));
    assert!(
        second.cache.reused_artifacts.contains(&main_type_key),
        "unchanged same-source main body type artifact should be reused"
    );
    assert!(
        second.cache.reused_artifacts.contains(&main_effect_key),
        "unchanged same-source main body effect artifact should be reused"
    );
    assert!(
        !second.cache.reused_artifacts.contains(&helper_type_key),
        "changed helper body type artifact must not be reused"
    );
    assert!(
        !second.cache.reused_artifacts.contains(&helper_effect_key),
        "changed helper body effect artifact must not be reused"
    );
}

#[test]
fn frontend_session_incremental_check_rejects_body_reuse_when_called_signature_changes() {
    let source = etas_core::SourceId(204);
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput {
        id: source,
        path: None,
        text: r#"
flow helper() -> i64 {
  return 1;
}

flow main() -> unit {
  let value = helper();
  return;
}
"#
        .to_owned(),
        kind: SourceKind::SingleFileInput,
    }));

    let first = session
        .check(project, CheckRequest::default())
        .expect("initial check should run");
    assert!(
        first.output.checked.is_some(),
        "{:?}",
        first.diagnostics.diagnostics
    );
    let (_, main_body) = helper_and_main_body_units(&first.output, source);
    let main_type_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::TypeFacts, main_body).to_cache_key();
    let manifest = session
        .artifact_manifest(project)
        .expect("artifact manifest should be stored");
    let main_type_record = manifest
        .get(&FrontendArtifactKey::unit(
            FrontendArtifactKind::TypeFacts,
            main_body,
        ))
        .expect("main body type artifact should be recorded");
    assert!(
        main_type_record.dependencies.iter().any(|dependency| {
            dependency.key.kind == FrontendArtifactKind::SignatureFacts
                && matches!(dependency.key.unit, FrontendUnitKey::Item(_))
                && dependency.fingerprint.is_some()
        }),
        "main body TypeFacts should depend on the called helper item signature"
    );

    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source,
                    version: SourceVersion(1),
                    text: r#"
flow helper() -> string {
  return "one";
}

flow main() -> unit {
  let value = helper();
  return;
}
"#
                    .to_owned(),
                }],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect("source replacement should be accepted");

    let second = session
        .check(project, CheckRequest::incremental())
        .expect("incremental check should run");
    assert!(
        second.output.checked.is_some(),
        "{:?}",
        second.diagnostics.diagnostics
    );
    assert!(
        !second.cache.reused_artifacts.contains(&main_type_key),
        "unchanged caller body TypeFacts must not be reused when a called item signature changes"
    );
}

#[test]
fn frontend_session_incremental_check_rejects_body_reuse_when_type_annotation_signature_changes() {
    let source = etas_core::SourceId(205);
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput {
        id: source,
        path: None,
        text: r#"
alias Alias = i64;

flow main() -> unit {
  let value: Alias = 1;
  return;
}
"#
        .to_owned(),
        kind: SourceKind::SingleFileInput,
    }));

    let first = session
        .check(project, CheckRequest::default())
        .expect("initial check should run");
    assert!(
        first.output.checked.is_some(),
        "{:?}",
        first.diagnostics.diagnostics
    );
    let main_body = only_body_unit(&first.output, source);
    let main_type_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::TypeFacts, main_body).to_cache_key();
    let manifest = session
        .artifact_manifest(project)
        .expect("artifact manifest should be stored");
    let main_type_record = manifest
        .get(&FrontendArtifactKey::unit(
            FrontendArtifactKind::TypeFacts,
            main_body,
        ))
        .expect("main body type artifact should be recorded");
    assert!(
        main_type_record.dependencies.iter().any(|dependency| {
            dependency.key.kind == FrontendArtifactKind::SignatureFacts
                && matches!(dependency.key.unit, FrontendUnitKey::Item(_))
                && dependency.fingerprint.is_some()
        }),
        "main body TypeFacts should depend on the Alias item signature"
    );

    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source,
                    version: SourceVersion(1),
                    text: r#"
alias Alias = i32;

flow main() -> unit {
  let value: Alias = 1;
  return;
}
"#
                    .to_owned(),
                }],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect("source replacement should be accepted");

    let second = session
        .check(project, CheckRequest::incremental())
        .expect("incremental check should run");
    assert!(
        second.output.checked.is_some(),
        "{:?}",
        second.diagnostics.diagnostics
    );
    assert!(
        !second.cache.reused_artifacts.contains(&main_type_key),
        "body TypeFacts must not be reused when a referenced type item signature changes"
    );
}

#[test]
fn frontend_disk_backed_incremental_check_rebuilds_live_snapshot_from_cached_body_facts() {
    let source_a = etas_core::SourceId(261);
    let cache_root = unique_temp_path("etas_frontend_fresh_snapshot_from_cache");
    let project_input = ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: source_a,
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"
module app.main;

flow helper() -> i64 {
  return 1;
}

flow main() -> i64 {
  return helper();
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    };
    let store =
        FrontendDiskArtifactStore::open(&cache_root).expect("frontend disk store should open");
    let mut session =
        FrontendSession::<FrontendDiskArtifactStore>::with_store_factory(unused_disk_store_factory);
    let project = session.open_project_with_store(project_input.clone(), store);

    let first = session
        .check(project, CheckRequest::default())
        .expect("initial disk-backed check should run");
    assert!(
        first.output.checked.is_some(),
        "{:?}",
        first.diagnostics.diagnostics
    );
    let (helper_body, main_body) = helper_and_main_body_units(&first.output, source_a);
    let helper_type_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::TypeFacts, helper_body).to_cache_key();
    let helper_effect_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::EffectFacts, helper_body).to_cache_key();
    let main_type_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::TypeFacts, main_body).to_cache_key();
    let main_effect_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::EffectFacts, main_body).to_cache_key();
    drop(session);

    let store =
        FrontendDiskArtifactStore::open(&cache_root).expect("frontend disk store should reopen");
    let mut fresh_session =
        FrontendSession::<FrontendDiskArtifactStore>::with_store_factory(unused_disk_store_factory);
    let fresh_project = fresh_session.open_project_with_store(project_input, store);
    let second = fresh_session
        .check(fresh_project, CheckRequest::incremental())
        .expect("fresh disk-backed incremental check should run");
    assert!(
        second.output.checked.is_some(),
        "{:?}",
        second.diagnostics.diagnostics
    );
    let snapshot = second
        .snapshot
        .as_ref()
        .expect("live snapshot should exist");
    assert_eq!(snapshot.revision, ProjectRevision(0));
    assert_eq!(snapshot.sources.files.len(), 1);
    assert!(snapshot.hir.items.iter().next().is_some());
    assert!(
        snapshot
            .resolved_paths
            .expr_paths
            .iter()
            .any(|path| matches!(path.result, HirPathResolution::DirectSymbol(_))),
        "fresh check should rebuild resolved path facts in the live snapshot"
    );
    for key in [
        helper_type_key,
        helper_effect_key,
        main_type_key,
        main_effect_key,
    ] {
        assert!(
            second.cache.reused_artifacts.contains(&key),
            "fresh disk-backed incremental check should reuse body fact artifact {key}"
        );
    }

    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

fn helper_and_main_body_units(
    output: &ProjectOutput,
    source: etas_core::SourceId,
) -> (UnitKey, UnitKey) {
    let units = output.units.as_ref().expect("unit tree should exist");
    let mut bodies = units
        .nodes
        .iter()
        .filter_map(|(unit, node)| {
            let UnitTarget::AstBody(body) = &node.target else {
                return None;
            };
            (node.kind == UnitKind::Body && body.source == source)
                .then_some((body.item.index, UnitKey::new(BODY_UNIT_KIND, unit.0 as u64)))
        })
        .collect::<Vec<_>>();
    bodies.sort_by_key(|(index, _)| *index);
    assert_eq!(bodies.len(), 2);
    (bodies[0].1, bodies[1].1)
}

fn only_body_unit(output: &ProjectOutput, source: etas_core::SourceId) -> UnitKey {
    let units = output.units.as_ref().expect("unit tree should exist");
    let bodies = units
        .nodes
        .iter()
        .filter_map(|(unit, node)| {
            let UnitTarget::AstBody(body) = &node.target else {
                return None;
            };
            (node.kind == UnitKind::Body && body.source == source)
                .then_some(UnitKey::new(BODY_UNIT_KIND, unit.0 as u64))
        })
        .collect::<Vec<_>>();
    assert_eq!(bodies.len(), 1);
    bodies[0]
}

fn unused_disk_store_factory() -> FrontendDiskArtifactStore {
    panic!("test projects open with an explicit disk artifact store")
}

fn assert_fresh_disk_payload_absent<T: Clone + 'static>(
    cache_root: &std::path::Path,
    key: &ArtifactKey,
    artifact: &str,
) {
    let fresh_store =
        FrontendDiskArtifactStore::open(cache_root).expect("fresh frontend disk store should open");
    assert!(
        fresh_store
            .get::<T>(key)
            .expect("fresh store artifact lookup should not fail")
            .is_none(),
        "{artifact} must not be persisted as a default disk payload"
    );
}

fn assert_metadata_only<T: Clone + 'static>(store: &FrontendDiskArtifactStore, key: &ArtifactKey) {
    assert!(
        store
            .contains(key)
            .expect("metadata-only contains lookup should not fail"),
        "metadata-only artifact should have a disk metadata row: {key}"
    );
    let meta = store
        .meta(key)
        .expect("metadata-only meta lookup should not fail")
        .expect("metadata-only artifact should have metadata");
    assert_eq!(meta.payload_hash, None);
    assert_eq!(meta.payload_size, None);
    assert!(
        store
            .get::<T>(key)
            .expect("metadata-only payload lookup should not fail")
            .is_none(),
        "metadata-only artifact must not be readable as a disk payload: {key}"
    );
}

fn unique_temp_path(label: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{label}_{}_{}", std::process::id(), nanos))
}

struct FrontendCacheHolder {
    child: Child,
    stop: std::path::PathBuf,
}

fn spawn_frontend_cache_holder(cache_root: &std::path::Path) -> FrontendCacheHolder {
    let ready = unique_temp_path("etas_frontend_cache_holder_ready");
    let stop = unique_temp_path("etas_frontend_cache_holder_stop");
    let child = Command::new(std::env::current_exe().expect("test binary path"))
        .arg("--exact")
        .arg("frontend_disk_store_holder_helper")
        .arg("--ignored")
        .arg("--test-threads=1")
        .env(FRONTEND_CACHE_HOLDER_ROOT_ENV, cache_root)
        .env(FRONTEND_CACHE_HOLDER_READY_ENV, &ready)
        .env(FRONTEND_CACHE_HOLDER_STOP_ENV, &stop)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("frontend cache holder subprocess should start");
    wait_for_file(&ready, "frontend cache holder readiness");
    FrontendCacheHolder { child, stop }
}

fn stop_frontend_cache_holder(mut holder: FrontendCacheHolder) {
    std::fs::write(&holder.stop, b"stop").expect("frontend cache holder stop signal");
    wait_for_child_success(&mut holder.child, "frontend cache holder");
}

fn wait_for_file(path: &std::path::Path, label: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !path.exists() {
        assert!(std::time::Instant::now() < deadline, "{label} timed out");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn wait_for_child_success(child: &mut Child, label: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().expect("child wait should not fail") {
            assert!(status.success(), "{label} exited unsuccessfully: {status}");
            return;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{label} timed out");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn frontend_session_accepts_explicit_artifact_store_factory() {
    let mut session = FrontendSession::with_store_factory(MemoryArtifactStore::new);
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
flow main() -> unit {
  return;
}
"#,
    )));

    let response = session
        .check(project, CheckRequest::default())
        .expect("session check should run with an injected artifact store");

    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    assert!(
        session
            .artifact_manifest(project)
            .expect("artifact manifest should be stored")
            .get(&FrontendArtifactKey::project(
                FrontendArtifactKind::CheckedProject,
            ))
            .is_some()
    );
}

#[test]
fn frontend_session_incremental_check_reuses_body_artifacts_from_disk_store() {
    let source = etas_core::SourceId(206);
    let cache_root = unique_temp_path("etas_frontend_body_disk_cache");
    let initial_text = r#"
flow helper() -> i64 {
  return 1;
}

flow main() -> i64 {
  return 2;
}
"#;
    let changed_text = r#"
flow helper() -> i64 {
  return 3;
}

flow main() -> i64 {
  return 2;
}
"#;
    let store =
        FrontendDiskArtifactStore::open(&cache_root).expect("frontend disk store should open");
    let mut session =
        FrontendSession::<FrontendDiskArtifactStore>::with_store_factory(unused_disk_store_factory);
    let project = session.open_project_with_store(
        ProjectInput::single_source(SourceInput {
            id: source,
            path: None,
            text: initial_text.to_owned(),
            kind: SourceKind::SingleFileInput,
        }),
        store,
    );

    let first = session
        .check(project, CheckRequest::default())
        .expect("initial disk-backed check should run");
    assert!(
        first.output.checked.is_some(),
        "{:?}",
        first.diagnostics.diagnostics
    );
    assert!(
        cache_root.join("v1").join("cache.sqlite").exists(),
        "disk-backed frontend store should create a SQLite cache index"
    );
    let (helper_body, main_body) = helper_and_main_body_units(&first.output, source);
    let checked = first
        .output
        .checked
        .as_ref()
        .expect("checked project should exist");
    let helper_item = checked
        .hir
        .items
        .iter()
        .find_map(|(item, hir_item)| match hir_item {
            etas_hir::HirItem::Flow(flow)
                if checked
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "helper") =>
            {
                Some(item)
            }
            _ => None,
        })
        .expect("helper flow should lower");
    let signature_output = first
        .output
        .signature_types
        .as_ref()
        .expect("signature type output should exist");
    let original_helper_signature = signature_output
        .facts
        .item_signatures
        .get(&helper_item)
        .expect("helper item signature should exist")
        .clone();

    let helper_type_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::TypeFacts, helper_body).to_cache_key();
    let helper_effect_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::EffectFacts, helper_body).to_cache_key();
    let main_type_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::TypeFacts, main_body).to_cache_key();
    let main_effect_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::EffectFacts, main_body).to_cache_key();
    let manifest_key =
        FrontendArtifactKey::project(FrontendArtifactKind::ArtifactManifest).to_cache_key();
    let body_reuse_index_key =
        FrontendArtifactKey::project(FrontendArtifactKind::BodyArtifactReuseIndex).to_cache_key();
    let item_signature_key =
        FrontendArtifactKey::item(FrontendArtifactKind::SignatureFacts, helper_item).to_cache_key();
    let project_signature_key =
        FrontendArtifactKey::project(FrontendArtifactKind::SignatureFacts).to_cache_key();
    let effect_pipeline_key =
        FrontendArtifactKey::project(FrontendArtifactKind::EffectPipelineArtifacts).to_cache_key();
    let source_set_key =
        FrontendArtifactKey::project(FrontendArtifactKind::SourceSet).to_cache_key();
    let diagnostics_key =
        FrontendArtifactKey::source(FrontendArtifactKind::Diagnostics, source).to_cache_key();
    let checked_project_key =
        FrontendArtifactKey::project(FrontendArtifactKind::CheckedProject).to_cache_key();
    assert!(
        first.cache.stored_artifacts.contains(&body_reuse_index_key),
        "initial disk-backed check should store a body artifact reuse index"
    );
    assert!(
        first.cache.stored_artifacts.contains(&item_signature_key),
        "initial disk-backed check should store item-scoped signatures"
    );
    drop(session);

    let cache_holder = spawn_frontend_cache_holder(&cache_root);
    let allowlist_store = FrontendDiskArtifactStore::open(&cache_root)
        .expect("frontend disk store should reopen while another process has the cache open");
    let persisted_manifest = allowlist_store
        .get::<FrontendArtifactManifest>(&manifest_key)
        .expect("persisted frontend manifest should be readable")
        .expect("frontend manifest should be stored on disk");
    assert!(
        persisted_manifest
            .value
            .get(&FrontendArtifactKey::unit(
                FrontendArtifactKind::TypeFacts,
                main_body,
            ))
            .is_some(),
        "persisted frontend manifest should include the main body type artifact"
    );
    assert!(
        persisted_manifest
            .value
            .get(&FrontendArtifactKey::project(
                FrontendArtifactKind::EffectPipelineArtifacts,
            ))
            .is_some(),
        "persisted frontend manifest should include effect pipeline artifact metadata"
    );
    assert!(
        allowlist_store
            .contains(&body_reuse_index_key)
            .expect("body reuse index metadata lookup should not fail"),
        "body artifact reuse index should be visible from a fresh shared-root store"
    );
    let item_signature = allowlist_store
        .get::<ItemSignature>(&item_signature_key)
        .expect("fresh store item signature lookup should not fail")
        .expect("item signature artifact should be persisted to disk");
    assert_eq!(item_signature.value, original_helper_signature);
    let main_type = allowlist_store
        .get::<TypeOutput>(&main_type_key)
        .expect("fresh store body type lookup should not fail")
        .expect("body type output should be persisted to disk");
    assert!(main_type.value.diagnostics.is_empty());
    let main_effect = allowlist_store
        .get::<etas_effects::EffectOutput>(&main_effect_key)
        .expect("fresh store body effect lookup should not fail")
        .expect("body effect output should be persisted to disk");
    assert!(main_effect.value.diagnostics.is_empty());
    assert!(
        allowlist_store
            .get::<SourceSet>(&source_set_key)
            .expect("fresh store source set lookup should not fail")
            .is_none(),
        "SourceSet must remain memory-only even when the cache root is shared across processes"
    );
    assert!(
        allowlist_store
            .get::<DiagnosticSet>(&diagnostics_key)
            .expect("fresh store diagnostics lookup should not fail")
            .is_none(),
        "Diagnostics must remain memory-only even when the cache root is shared across processes"
    );
    assert!(
        allowlist_store
            .get::<etas_frontend::CheckedProject>(&checked_project_key)
            .expect("fresh store checked-project lookup should not fail")
            .is_none(),
        "CheckedProject must remain memory-only even when the cache root is shared across processes"
    );
    assert!(
        allowlist_store
            .get::<TypeOutput>(&project_signature_key)
            .expect("fresh store aggregate signature lookup should not fail")
            .is_none(),
        "aggregate project signature output must remain memory-only"
    );
    assert!(
        allowlist_store
            .get::<etas_effects::EffectPipelineArtifacts>(&effect_pipeline_key)
            .expect("fresh store effect pipeline artifact lookup should not fail")
            .is_none(),
        "effect pipeline artifacts are manifest-tracked metadata but remain memory-only payloads"
    );
    drop(allowlist_store);

    let store =
        FrontendDiskArtifactStore::open(&cache_root).expect("frontend disk store should reopen");
    let mut session =
        FrontendSession::<FrontendDiskArtifactStore>::with_store_factory(unused_disk_store_factory);
    let project = session.open_project_with_store(
        ProjectInput::single_source(SourceInput {
            id: source,
            path: None,
            text: initial_text.to_owned(),
            kind: SourceKind::SingleFileInput,
        }),
        store,
    );

    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source,
                    version: SourceVersion(1),
                    text: changed_text.to_owned(),
                }],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect("source replacement should be accepted");

    let second = session
        .check(project, CheckRequest::incremental())
        .expect("incremental disk-backed check should run");
    stop_frontend_cache_holder(cache_holder);

    assert!(
        second.output.checked.is_some(),
        "{:?}",
        second.diagnostics.diagnostics
    );
    assert!(
        second.cache.reused_artifacts.contains(&main_type_key),
        "unchanged same-source main body type artifact should be reused from the disk store"
    );
    assert!(
        second.cache.reused_artifacts.contains(&main_effect_key),
        "unchanged same-source main body effect artifact should be reused from the disk store"
    );
    assert!(
        !second.cache.reused_artifacts.contains(&helper_type_key),
        "changed helper body type artifact must not be reused from the disk store"
    );
    assert!(
        !second.cache.reused_artifacts.contains(&helper_effect_key),
        "changed helper body effect artifact must not be reused from the disk store"
    );

    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_disk_store_persists_compact_summaries_as_metadata_only() {
    let source = etas_core::SourceId(260);
    let cache_root = unique_temp_path("etas_frontend_metadata_summaries");
    let store =
        FrontendDiskArtifactStore::open(&cache_root).expect("frontend disk store should open");
    let mut session =
        FrontendSession::<FrontendDiskArtifactStore>::with_store_factory(unused_disk_store_factory);
    let project = session.open_project_with_store(
        ProjectInput::single_source(SourceInput {
            id: source,
            path: None,
            text: r#"
flow helper() -> i32 {
  return 1;
}

flow main() -> i32 {
  return helper();
}
"#
            .to_owned(),
            kind: SourceKind::SingleFileInput,
        }),
        store,
    );

    let response = session
        .check(project, CheckRequest::default())
        .expect("disk-backed check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );

    let source_summary_key =
        FrontendArtifactKey::project(FrontendArtifactKind::SourceFingerprintSummary).to_cache_key();
    let module_summary_key =
        FrontendArtifactKey::project(FrontendArtifactKind::ModuleImportExportSummary)
            .to_cache_key();
    let dependency_summary_key =
        FrontendArtifactKey::project(FrontendArtifactKind::ArtifactDependencySummary)
            .to_cache_key();
    let reuse_stats_key =
        FrontendArtifactKey::project(FrontendArtifactKind::ArtifactReuseStats).to_cache_key();
    for key in [
        &source_summary_key,
        &module_summary_key,
        &dependency_summary_key,
        &reuse_stats_key,
    ] {
        assert!(
            response.cache.stored_artifacts.contains(key),
            "metadata-only summary should be reported as stored: {key}"
        );
    }
    drop(session);

    let fresh_store = FrontendDiskArtifactStore::open(&cache_root)
        .expect("fresh frontend disk store should open");
    let manifest_key =
        FrontendArtifactKey::project(FrontendArtifactKind::ArtifactManifest).to_cache_key();
    let manifest = fresh_store
        .get::<FrontendArtifactManifest>(&manifest_key)
        .expect("manifest lookup should not fail")
        .expect("artifact manifest should be persisted as payload");
    for kind in [
        FrontendArtifactKind::SourceFingerprintSummary,
        FrontendArtifactKind::ModuleImportExportSummary,
        FrontendArtifactKind::ArtifactDependencySummary,
        FrontendArtifactKind::ArtifactReuseStats,
    ] {
        assert!(
            manifest
                .value
                .get(&FrontendArtifactKey::project(kind))
                .is_some(),
            "manifest should record metadata-only summary kind {kind:?}"
        );
    }

    assert_metadata_only::<SourceFingerprintSummary>(&fresh_store, &source_summary_key);
    assert_metadata_only::<ModuleImportExportSummary>(&fresh_store, &module_summary_key);
    assert_metadata_only::<ArtifactDependencySummary>(&fresh_store, &dependency_summary_key);
    assert_metadata_only::<ArtifactReuseStatsSummary>(&fresh_store, &reuse_stats_key);

    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_disk_store_reads_memory_artifact_after_disk_publication_unavailable() {
    let cache_root = unique_temp_path("etas_frontend_disk_unavailable_memory");
    let mut store =
        FrontendDiskArtifactStore::open(&cache_root).expect("frontend disk store should open");
    let manifest = FrontendArtifactManifest::new();
    let first_key =
        FrontendArtifactKey::project(FrontendArtifactKind::ArtifactManifest).to_cache_key();
    let memory_only_key = ArtifactKey::new(
        "frontend",
        "artifact_manifest",
        "same-payload-different-envelope",
    );

    store
        .put(CachedArtifact {
            key: first_key,
            meta: ArtifactMeta::new(
                ProjectRevision(0),
                manifest.fingerprint(),
                env!("CARGO_PKG_VERSION"),
                FRONTEND_ARTIFACT_SCHEMA_VERSION,
            ),
            value: manifest.clone(),
        })
        .expect("first frontend manifest should be persisted");
    store
        .put(CachedArtifact {
            key: memory_only_key.clone(),
            meta: ArtifactMeta::new(
                ProjectRevision(0),
                manifest.fingerprint(),
                env!("CARGO_PKG_VERSION"),
                FRONTEND_ARTIFACT_SCHEMA_VERSION,
            ),
            value: manifest.clone(),
        })
        .expect("disk-unavailable frontend manifest should stay in memory");

    let cached = store
        .get::<FrontendArtifactManifest>(&memory_only_key)
        .expect("memory fallback lookup should not fail")
        .expect("memory fallback artifact should be readable from the same store");
    assert_eq!(cached.value, manifest);
    assert!(
        store
            .meta(&memory_only_key)
            .expect("memory metadata")
            .is_some()
    );

    let fresh_store = FrontendDiskArtifactStore::open(&cache_root)
        .expect("fresh frontend disk store should open");
    assert!(
        fresh_store
            .get::<FrontendArtifactManifest>(&memory_only_key)
            .expect("fresh store lookup should not fail")
            .is_none(),
        "memory-only fallback artifacts must not appear as persisted disk artifacts"
    );

    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_disk_store_telemetry_records_codec_and_disk_events() {
    let cache_root = unique_temp_path("etas_frontend_disk_telemetry");
    let mut store =
        FrontendDiskArtifactStore::open_with_compression(&cache_root, CompressionKind::None)
            .expect("frontend disk store should open");
    let manifest = FrontendArtifactManifest::new();
    let key = FrontendArtifactKey::project(FrontendArtifactKind::ArtifactManifest).to_cache_key();

    store
        .put(CachedArtifact {
            key: key.clone(),
            meta: ArtifactMeta::new(
                ProjectRevision(0),
                manifest.fingerprint(),
                env!("CARGO_PKG_VERSION"),
                FRONTEND_ARTIFACT_SCHEMA_VERSION,
            ),
            value: manifest.clone(),
        })
        .expect("manifest should be persisted");
    let cached = store
        .get::<FrontendArtifactManifest>(&key)
        .expect("manifest lookup should not fail")
        .expect("manifest should be readable");
    assert_eq!(cached.value, manifest);

    let telemetry = store.telemetry();
    let entry = telemetry
        .artifact_kind(&key)
        .expect("manifest telemetry should exist");
    assert_eq!(entry.serialize_count, 1);
    assert_eq!(entry.deserialize_count, 1);
    assert_eq!(entry.hit_count, 1);
    assert!(entry.compressed_bytes > 0);

    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");

    let skipped_root = unique_temp_path("etas_frontend_disk_telemetry_skip");
    let policy = DiskArtifactStorePolicy {
        budget: DiskCacheBudgetPolicy::default().with_max_project_bytes(0),
        ..DiskArtifactStorePolicy::default()
    };
    let mut skipped_store = FrontendDiskArtifactStore::open_with_policy(&skipped_root, policy)
        .expect("frontend disk store with budget should open");
    let manifest = FrontendArtifactManifest::new();
    skipped_store
        .put(CachedArtifact {
            key: key.clone(),
            meta: ArtifactMeta::new(
                ProjectRevision(0),
                manifest.fingerprint(),
                env!("CARGO_PKG_VERSION"),
                FRONTEND_ARTIFACT_SCHEMA_VERSION,
            ),
            value: manifest.clone(),
        })
        .expect("oversized manifest should stay in memory");
    let cached = skipped_store
        .get::<FrontendArtifactManifest>(&key)
        .expect("memory fallback lookup should not fail")
        .expect("memory fallback manifest should be readable");
    assert_eq!(cached.value, manifest);

    let telemetry = skipped_store.telemetry();
    let entry = telemetry
        .artifact_kind(&key)
        .expect("skipped manifest telemetry should exist");
    assert_eq!(entry.serialize_count, 1);
    assert_eq!(entry.miss_count, 1);
    assert_eq!(entry.skipped_write_count, 1);

    std::fs::remove_dir_all(&skipped_root)
        .expect("skipped disk cache temp directory should be removable");
}

#[test]
fn frontend_disk_backed_session_continues_with_memory_artifacts_when_disk_write_is_locked() {
    let source = etas_core::SourceId(207);
    let cache_root = unique_temp_path("etas_frontend_disk_write_locked_session");
    let policy = DiskArtifactStorePolicy {
        busy_timeout: Duration::from_millis(1),
        stale_temp_file_age: Duration::from_secs(6 * 60 * 60),
        ..DiskArtifactStorePolicy::default()
    };
    let store = FrontendDiskArtifactStore::open_with_policy(&cache_root, policy)
        .expect("frontend disk store should open with a short lock timeout");
    let lock = rusqlite::Connection::open(cache_root.join("v1").join("cache.sqlite"))
        .expect("sqlite cache index should open for lock simulation");
    lock.execute_batch("BEGIN IMMEDIATE;")
        .expect("write transaction should hold the cache writer lock");

    let initial_text = r#"
flow helper() -> i64 {
  return 1;
}

flow main() -> i64 {
  return 2;
}
"#;
    let changed_text = r#"
flow helper() -> i64 {
  return 3;
}

flow main() -> i64 {
  return 2;
}
"#;
    let mut session =
        FrontendSession::<FrontendDiskArtifactStore>::with_store_factory(unused_disk_store_factory);
    let project = session.open_project_with_store(
        ProjectInput::single_source(SourceInput {
            id: source,
            path: None,
            text: initial_text.to_owned(),
            kind: SourceKind::SingleFileInput,
        }),
        store,
    );

    let first = session
        .check(project, CheckRequest::default())
        .expect("initial check should continue when disk writes are locked");
    assert!(
        first.output.checked.is_some(),
        "{:?}",
        first.diagnostics.diagnostics
    );
    let (helper_body, main_body) = helper_and_main_body_units(&first.output, source);
    let helper_type_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::TypeFacts, helper_body).to_cache_key();
    let helper_effect_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::EffectFacts, helper_body).to_cache_key();
    let main_type_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::TypeFacts, main_body).to_cache_key();
    let main_effect_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::EffectFacts, main_body).to_cache_key();

    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source,
                    version: SourceVersion(1),
                    text: changed_text.to_owned(),
                }],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect("source replacement should be accepted while disk writes are locked");

    let second = session
        .check(project, CheckRequest::incremental())
        .expect("incremental check should reuse memory artifacts when disk writes are locked");
    assert!(
        second.output.checked.is_some(),
        "{:?}",
        second.diagnostics.diagnostics
    );
    assert!(
        second.cache.reused_artifacts.contains(&main_type_key),
        "unchanged main body type artifact should be reused from memory after disk write fallback"
    );
    assert!(
        second.cache.reused_artifacts.contains(&main_effect_key),
        "unchanged main body effect artifact should be reused from memory after disk write fallback"
    );
    assert!(
        !second.cache.reused_artifacts.contains(&helper_type_key),
        "changed helper body type artifact must not be reused"
    );
    assert!(
        !second.cache.reused_artifacts.contains(&helper_effect_key),
        "changed helper body effect artifact must not be reused"
    );

    lock.execute_batch("ROLLBACK;")
        .expect("held write transaction should roll back");
    let fresh_store = FrontendDiskArtifactStore::open(&cache_root)
        .expect("fresh frontend disk store should open after lock release");
    let manifest_key =
        FrontendArtifactKey::project(FrontendArtifactKind::ArtifactManifest).to_cache_key();
    assert!(
        fresh_store
            .get::<FrontendArtifactManifest>(&manifest_key)
            .expect("fresh store manifest lookup should not fail")
            .is_none(),
        "artifacts written while the disk writer lock was held must remain memory-only"
    );

    drop(fresh_store);
    drop(session);
    drop(lock);
    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_disk_store_records_std_and_compiler_option_metadata() {
    let cache_root = unique_temp_path("etas_frontend_disk_metadata");
    let store =
        FrontendDiskArtifactStore::open(&cache_root).expect("frontend disk store should open");
    let mut session =
        FrontendSession::<FrontendDiskArtifactStore>::with_store_factory(unused_disk_store_factory);
    let project = session.open_project_with_store(
        ProjectInput::single_source(SourceInput {
            id: etas_core::SourceId(208),
            path: None,
            text: "flow main() -> unit { return; }".to_owned(),
            kind: SourceKind::SingleFileInput,
        }),
        store,
    );

    let response = session
        .check(project, CheckRequest::default())
        .expect("disk-backed check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    drop(session);

    let store = FrontendDiskArtifactStore::open(&cache_root)
        .expect("frontend disk store should reopen for metadata inspection");
    let manifest_key =
        FrontendArtifactKey::project(FrontendArtifactKind::ArtifactManifest).to_cache_key();
    let manifest_meta = store
        .meta(&manifest_key)
        .expect("artifact manifest metadata lookup should not fail")
        .expect("artifact manifest metadata should be persisted");

    assert_eq!(
        manifest_meta.std_version.as_deref(),
        Some("etas_std:phase1")
    );
    assert!(
        manifest_meta
            .options_hash
            .as_ref()
            .is_some_and(|hash| hash.len() == 64),
        "frontend artifact metadata should carry a concrete compiler option hash"
    );

    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_disk_store_keeps_diagnostics_artifacts_memory_only() {
    let source = etas_core::SourceId(210);
    let cache_root = unique_temp_path("etas_frontend_disk_diagnostics");
    let store =
        FrontendDiskArtifactStore::open(&cache_root).expect("frontend disk store should open");
    let mut session =
        FrontendSession::<FrontendDiskArtifactStore>::with_store_factory(unused_disk_store_factory);
    let project = session.open_project_with_store(
        ProjectInput::single_source(SourceInput {
            id: source,
            path: None,
            text: "flow main() -> unit { return; }".to_owned(),
            kind: SourceKind::SingleFileInput,
        }),
        store,
    );

    let response = session
        .check(project, CheckRequest::default())
        .expect("disk-backed check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    let diagnostics_key =
        FrontendArtifactKey::source(FrontendArtifactKind::Diagnostics, source).to_cache_key();
    assert!(
        response.cache.stored_artifacts.contains(&diagnostics_key),
        "diagnostics should be recorded as a session cache artifact"
    );
    drop(session);

    assert_fresh_disk_payload_absent::<DiagnosticSet>(&cache_root, &diagnostics_key, "diagnostics");

    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_disk_store_keeps_source_set_memory_only() {
    let source = etas_core::SourceId(213);
    let cache_root = unique_temp_path("etas_frontend_disk_source_set");
    let store =
        FrontendDiskArtifactStore::open(&cache_root).expect("frontend disk store should open");
    let mut session =
        FrontendSession::<FrontendDiskArtifactStore>::with_store_factory(unused_disk_store_factory);
    let source_text = "module app.main;\nflow main() -> unit {\n  return;\n}".to_owned();
    let project = session.open_project_with_store(
        ProjectInput {
            project_root: std::path::PathBuf::from("/workspace/demo"),
            source_root: None,
            options: Default::default(),
            environment: ProjectEnvironmentInput::default(),
            sources: vec![SourceInput {
                id: source,
                path: Some(std::path::PathBuf::from("src/app/main.es")),
                text: source_text.clone(),
                kind: SourceKind::SourceProjectFile,
            }],
            entry: ProjectEntry {
                module: Some(ModulePath {
                    segments: vec!["app".to_owned(), "main".to_owned()],
                }),
                flow: "main".to_owned(),
            },
        },
        store,
    );

    let response = session
        .check(project, CheckRequest::default())
        .expect("disk-backed check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    let source_set_key =
        FrontendArtifactKey::project(FrontendArtifactKind::SourceSet).to_cache_key();
    assert!(
        response.cache.stored_artifacts.contains(&source_set_key),
        "source set should be recorded as a session cache artifact"
    );
    drop(session);

    assert_fresh_disk_payload_absent::<SourceSet>(&cache_root, &source_set_key, "source set");

    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_disk_store_keeps_ast_hir_and_checked_project_memory_only() {
    let source = etas_core::SourceId(222);
    let cache_root = unique_temp_path("etas_frontend_disk_ast_hir_checked_memory_only");
    let store =
        FrontendDiskArtifactStore::open(&cache_root).expect("frontend disk store should open");
    let mut session =
        FrontendSession::<FrontendDiskArtifactStore>::with_store_factory(unused_disk_store_factory);
    let project = session.open_project_with_store(
        ProjectInput::single_source(SourceInput {
            id: source,
            path: None,
            text: "flow main() -> unit { return; }".to_owned(),
            kind: SourceKind::SingleFileInput,
        }),
        store,
    );

    let response = session
        .check(project, CheckRequest::default())
        .expect("disk-backed check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    let parsed_source_key =
        FrontendArtifactKey::source(FrontendArtifactKind::ParsedSource, source).to_cache_key();
    let parsed_source_set_key =
        FrontendArtifactKey::project(FrontendArtifactKind::ParsedSourceSet).to_cache_key();
    let hir_key = FrontendArtifactKey::project(FrontendArtifactKind::HirProgram).to_cache_key();
    let checked_key =
        FrontendArtifactKey::project(FrontendArtifactKind::CheckedProject).to_cache_key();
    for key in [
        &parsed_source_key,
        &parsed_source_set_key,
        &hir_key,
        &checked_key,
    ] {
        assert!(
            response.cache.stored_artifacts.contains(key),
            "artifact {key} should be recorded in the live session cache"
        );
    }
    drop(session);

    assert_fresh_disk_payload_absent::<ParsedSource>(
        &cache_root,
        &parsed_source_key,
        "parsed source",
    );
    assert_fresh_disk_payload_absent::<Vec<ParsedSource>>(
        &cache_root,
        &parsed_source_set_key,
        "parsed source set",
    );
    assert_fresh_disk_payload_absent::<HirOutput>(&cache_root, &hir_key, "HIR program");
    assert_fresh_disk_payload_absent::<etas_frontend::CheckedProject>(
        &cache_root,
        &checked_key,
        "checked project",
    );

    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_disk_store_keeps_project_graph_artifacts_memory_only() {
    let source_a = etas_core::SourceId(214);
    let source_b = etas_core::SourceId(215);
    let cache_root = unique_temp_path("etas_frontend_disk_project_graph");
    let store =
        FrontendDiskArtifactStore::open(&cache_root).expect("frontend disk store should open");
    let mut session =
        FrontendSession::<FrontendDiskArtifactStore>::with_store_factory(unused_disk_store_factory);
    let project = session.open_project_with_store(
        ProjectInput {
            project_root: std::path::PathBuf::from("/workspace/demo"),
            source_root: None,
            options: Default::default(),
            environment: ProjectEnvironmentInput::default(),
            sources: vec![
                SourceInput {
                    id: source_a,
                    path: Some(std::path::PathBuf::from("src/app/a.es")),
                    text: "module app.a;\nimport app.b;\nflow main() -> unit { return; }"
                        .to_owned(),
                    kind: SourceKind::SourceProjectFile,
                },
                SourceInput {
                    id: source_b,
                    path: Some(std::path::PathBuf::from("src/app/b.es")),
                    text: "module app.b;\nflow helper() -> unit { return; }".to_owned(),
                    kind: SourceKind::SourceProjectFile,
                },
            ],
            entry: ProjectEntry {
                module: Some(ModulePath {
                    segments: vec!["app".to_owned(), "a".to_owned()],
                }),
                flow: "main".to_owned(),
            },
        },
        store,
    );

    let response = session
        .check(project, CheckRequest::default())
        .expect("disk-backed check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    let module_index_key =
        FrontendArtifactKey::project(FrontendArtifactKind::ModuleIndex).to_cache_key();
    let import_graph_key =
        FrontendArtifactKey::project(FrontendArtifactKind::ImportGraph).to_cache_key();
    let topo_key =
        FrontendArtifactKey::project(FrontendArtifactKind::ModuleTopoOrder).to_cache_key();
    for key in [&module_index_key, &import_graph_key, &topo_key] {
        assert!(
            response.cache.stored_artifacts.contains(key),
            "project graph artifact {key} should be recorded as stored"
        );
    }
    drop(session);

    assert_fresh_disk_payload_absent::<ModuleIndex>(&cache_root, &module_index_key, "module index");
    assert_fresh_disk_payload_absent::<ImportGraph>(&cache_root, &import_graph_key, "import graph");
    assert_fresh_disk_payload_absent::<ModuleTopoOrder>(
        &cache_root,
        &topo_key,
        "module topo order",
    );

    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_disk_store_keeps_unit_tree_and_affected_modules_memory_only() {
    let source_a = etas_core::SourceId(216);
    let source_b = etas_core::SourceId(217);
    let cache_root = unique_temp_path("etas_frontend_disk_unit_graph");
    let store =
        FrontendDiskArtifactStore::open(&cache_root).expect("frontend disk store should open");
    let mut session =
        FrontendSession::<FrontendDiskArtifactStore>::with_store_factory(unused_disk_store_factory);
    let path_a = ModulePath {
        segments: vec!["app".to_owned(), "a".to_owned()],
    };
    let project = session.open_project_with_store(
        ProjectInput {
            project_root: std::path::PathBuf::from("/workspace/demo"),
            source_root: None,
            options: Default::default(),
            environment: ProjectEnvironmentInput::default(),
            sources: vec![
                SourceInput {
                    id: source_a,
                    path: Some(std::path::PathBuf::from("src/app/a.es")),
                    text: "module app.a;\nimport app.b;\nflow main() -> unit { return; }"
                        .to_owned(),
                    kind: SourceKind::SourceProjectFile,
                },
                SourceInput {
                    id: source_b,
                    path: Some(std::path::PathBuf::from("src/app/b.es")),
                    text: "module app.b;\nflow helper() -> unit { return; }".to_owned(),
                    kind: SourceKind::SourceProjectFile,
                },
            ],
            entry: ProjectEntry {
                module: Some(path_a.clone()),
                flow: "main".to_owned(),
            },
        },
        store,
    );

    let response = session
        .check(project, CheckRequest::default())
        .expect("disk-backed check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    let unit_tree_key = FrontendArtifactKey::project(FrontendArtifactKind::UnitTree).to_cache_key();
    let affected_key =
        FrontendArtifactKey::project(FrontendArtifactKind::AffectedModuleSet).to_cache_key();
    assert!(response.cache.stored_artifacts.contains(&unit_tree_key));
    assert!(response.cache.stored_artifacts.contains(&affected_key));
    drop(session);

    assert_fresh_disk_payload_absent::<UnitTree>(&cache_root, &unit_tree_key, "unit tree");
    assert_fresh_disk_payload_absent::<AffectedModuleSet>(
        &cache_root,
        &affected_key,
        "affected module set",
    );

    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_disk_store_keeps_resolution_and_entry_facts_memory_only() {
    let source_a = etas_core::SourceId(218);
    let source_b = etas_core::SourceId(219);
    let cache_root = unique_temp_path("etas_frontend_disk_resolution_entry");
    let store =
        FrontendDiskArtifactStore::open(&cache_root).expect("frontend disk store should open");
    let mut session =
        FrontendSession::<FrontendDiskArtifactStore>::with_store_factory(unused_disk_store_factory);
    let entry_module = ModulePath {
        segments: vec!["app".to_owned(), "a".to_owned()],
    };
    let project = session.open_project_with_store(
        ProjectInput {
            project_root: std::path::PathBuf::from("/workspace/demo"),
            source_root: None,
            options: Default::default(),
            environment: ProjectEnvironmentInput::default(),
            sources: vec![
                SourceInput {
                    id: source_a,
                    path: Some(std::path::PathBuf::from("src/app/a.es")),
                    text: "module app.a;\nimport app.b.{helper};\nflow main() -> unit { helper(); return; }"
                        .to_owned(),
                    kind: SourceKind::SourceProjectFile,
                },
                SourceInput {
                    id: source_b,
                    path: Some(std::path::PathBuf::from("src/app/b.es")),
                    text: "module app.b;\npublic flow helper() -> unit { return; }".to_owned(),
                    kind: SourceKind::SourceProjectFile,
                },
            ],
            entry: ProjectEntry {
                module: Some(entry_module.clone()),
                flow: "main".to_owned(),
            },
        },
        store,
    );

    let response = session
        .check(project, CheckRequest::default())
        .expect("disk-backed check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    assert!(
        response
            .output
            .entry
            .as_ref()
            .expect("entry fact should exist")
            .resolved
            .is_some(),
        "entry should resolve during the live check"
    );

    let imports_key =
        FrontendArtifactKey::project(FrontendArtifactKind::ResolvedImports).to_cache_key();
    let paths_key =
        FrontendArtifactKey::project(FrontendArtifactKind::ResolvedPaths).to_cache_key();
    let entry_key = FrontendArtifactKey::project(FrontendArtifactKind::ProjectEntry).to_cache_key();
    assert!(response.cache.stored_artifacts.contains(&imports_key));
    assert!(response.cache.stored_artifacts.contains(&paths_key));
    assert!(response.cache.stored_artifacts.contains(&entry_key));
    drop(session);

    assert_fresh_disk_payload_absent::<ResolvedImports>(
        &cache_root,
        &imports_key,
        "resolved imports",
    );
    assert_fresh_disk_payload_absent::<ResolvedPaths>(&cache_root, &paths_key, "resolved paths");
    assert_fresh_disk_payload_absent::<ProjectEntryFact>(
        &cache_root,
        &entry_key,
        "project entry fact",
    );

    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_disk_store_keeps_top_level_let_facts_memory_only() {
    let source = etas_core::SourceId(220);
    let cache_root = unique_temp_path("etas_frontend_disk_top_level_lets");
    let store =
        FrontendDiskArtifactStore::open(&cache_root).expect("frontend disk store should open");
    let mut session =
        FrontendSession::<FrontendDiskArtifactStore>::with_store_factory(unused_disk_store_factory);
    let project = session.open_project_with_store(
        ProjectInput {
            project_root: std::path::PathBuf::from("/workspace/demo"),
            source_root: None,
            options: Default::default(),
            environment: ProjectEnvironmentInput::default(),
            sources: vec![SourceInput {
                id: source,
                path: Some(std::path::PathBuf::from("src/app/main.es")),
                text:
                    "module app.main;\nlet Answer: i64 = 41;\nflow main() -> i64 { return Answer + 1; }"
                        .to_owned(),
                kind: SourceKind::SourceProjectFile,
            }],
            entry: ProjectEntry {
                module: Some(ModulePath {
                    segments: vec!["app".to_owned(), "main".to_owned()],
                }),
                flow: "main".to_owned(),
            },
        },
        store,
    );

    let response = session
        .check(project, CheckRequest::default())
        .expect("disk-backed check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    assert!(
        response
            .output
            .top_level_lets
            .as_ref()
            .is_some_and(|facts| !facts.items.is_empty()),
        "top-level let facts should exist in the live output"
    );

    let top_level_lets_key =
        FrontendArtifactKey::project(FrontendArtifactKind::TopLevelLetFacts).to_cache_key();
    assert!(
        response
            .cache
            .stored_artifacts
            .contains(&top_level_lets_key)
    );
    drop(session);

    assert_fresh_disk_payload_absent::<TopLevelLetFacts>(
        &cache_root,
        &top_level_lets_key,
        "top-level let facts",
    );

    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_disk_store_persists_item_signature_facts_but_not_project_signature_output() {
    let source = etas_core::SourceId(221);
    let cache_root = unique_temp_path("etas_frontend_disk_signature_facts");
    let store =
        FrontendDiskArtifactStore::open(&cache_root).expect("frontend disk store should open");
    let mut session =
        FrontendSession::<FrontendDiskArtifactStore>::with_store_factory(unused_disk_store_factory);
    let project = session.open_project_with_store(
        ProjectInput {
            project_root: std::path::PathBuf::from("/workspace/demo"),
            source_root: None,
            options: Default::default(),
            environment: ProjectEnvironmentInput::default(),
            sources: vec![SourceInput {
                id: source,
                path: Some(std::path::PathBuf::from("src/app/main.es")),
                text: "module app.main;\nlet Answer: i64 = 41;\nflow helper(input: i64) -> i64 { return input + Answer; }\nflow main() -> i64 { return helper(1); }"
                    .to_owned(),
                kind: SourceKind::SourceProjectFile,
            }],
            entry: ProjectEntry {
                module: Some(ModulePath {
                    segments: vec!["app".to_owned(), "main".to_owned()],
                }),
                flow: "main".to_owned(),
            },
        },
        store,
    );

    let response = session
        .check(project, CheckRequest::default())
        .expect("disk-backed check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    let checked = response
        .output
        .checked
        .as_ref()
        .expect("checked project should exist");
    let helper_item = checked
        .hir
        .items
        .iter()
        .find_map(|(item, hir_item)| match hir_item {
            etas_hir::HirItem::Flow(flow)
                if checked
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "helper") =>
            {
                Some(item)
            }
            _ => None,
        })
        .expect("helper flow should lower");
    let signature_output = response
        .output
        .signature_types
        .as_ref()
        .expect("signature type output should exist");
    let original_signature = signature_output
        .facts
        .item_signatures
        .get(&helper_item)
        .expect("helper item signature should exist")
        .clone();

    let project_signature_key =
        FrontendArtifactKey::project(FrontendArtifactKind::SignatureFacts).to_cache_key();
    let item_signature_key =
        FrontendArtifactKey::item(FrontendArtifactKind::SignatureFacts, helper_item).to_cache_key();
    assert!(
        response
            .cache
            .stored_artifacts
            .contains(&project_signature_key)
    );
    assert!(
        response
            .cache
            .stored_artifacts
            .contains(&item_signature_key)
    );
    drop(session);

    let fresh_store = FrontendDiskArtifactStore::open(&cache_root)
        .expect("fresh frontend disk store should open");
    assert!(
        fresh_store
            .get::<TypeOutput>(&project_signature_key)
            .expect("fresh store project signature lookup should not fail")
            .is_none(),
        "aggregate project signature output must stay memory-only"
    );
    let item_signature = fresh_store
        .get::<ItemSignature>(&item_signature_key)
        .expect("fresh store item signature lookup should not fail")
        .expect("item signature artifact should be persisted to disk");

    assert_eq!(item_signature.value, original_signature);
    assert!(matches!(item_signature.value, ItemSignature::Flow(_)));
    assert_eq!(
        item_signature.meta.std_version.as_deref(),
        Some("etas_std:phase1")
    );

    std::fs::remove_dir_all(&cache_root).expect("disk cache temp directory should be removable");
}

#[test]
fn frontend_session_dependency_change_does_not_reuse_hot_body_artifacts() {
    let source = etas_core::SourceId(209);
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput {
        id: source,
        path: None,
        text: r#"
flow helper() -> i64 {
  return 1;
}

flow main() -> i64 {
  return 2;
}
"#
        .to_owned(),
        kind: SourceKind::SingleFileInput,
    }));

    let first = session
        .check(project, CheckRequest::default())
        .expect("initial check should run");
    assert!(
        first.output.checked.is_some(),
        "{:?}",
        first.diagnostics.diagnostics
    );
    let (_helper_body, main_body) = helper_and_main_body_units(&first.output, source);
    let main_type_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::TypeFacts, main_body).to_cache_key();
    let main_effect_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::EffectFacts, main_body).to_cache_key();

    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: Vec::new(),
                dependency_overlay_changes: Vec::new(),
                environment_change: Some(EnvironmentChange::Replace(
                    ProjectEnvironmentInput::default(),
                )),
                option_changes: Vec::new(),
            },
        )
        .expect("dependency change should be accepted");

    let second = session
        .check(project, CheckRequest::incremental())
        .expect("incremental check after dependency change should run");
    assert!(
        second.output.checked.is_some(),
        "{:?}",
        second.diagnostics.diagnostics
    );
    assert!(
        !second.cache.reused_artifacts.contains(&main_type_key),
        "dependency changes must not reuse stale hot type body artifacts"
    );
    assert!(
        !second.cache.reused_artifacts.contains(&main_effect_key),
        "dependency changes must not reuse stale hot effect body artifacts"
    );
    assert!(
        second.cache.stored_artifacts.contains(&main_type_key),
        "dependency changes should recompute and store body type artifacts"
    );
    assert!(
        second.cache.stored_artifacts.contains(&main_effect_key),
        "dependency changes should recompute and store body effect artifacts"
    );
}

#[test]
fn frontend_session_external_effect_metadata_change_invalidates_effect_artifacts() {
    let source = etas_core::SourceId(210);
    let mut environment = external_environment();
    environment.external_effect_metadata = DependencyEffectMetadata {
        tags: vec![DependencyEffectTag {
            path: vec!["dep".into(), "payments".into(), "Payment".into()],
            runtime_requirement: Some(RuntimeRequirementReason::Network),
        }],
        actions: Vec::new(),
        extensions: vec![DependencyEffectExtension {
            package: None,
            child: vec!["dep".into(), "payments".into(), "Payment".into()],
            parent: vec!["Network".into()],
        }],
    };
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: environment.clone(),
        sources: vec![SourceInput {
            id: source,
            path: None,
            text: r#"
flow main(args: Array<string>) -> i32 {
  return 0;
}
"#
            .to_owned(),
            kind: SourceKind::SingleFileInput,
        }],
        entry: ProjectEntry {
            module: None,
            flow: "main".to_owned(),
        },
    });

    let first = session
        .check(project, CheckRequest::default())
        .expect("initial check should run");
    assert!(
        first.output.checked.is_some(),
        "{:?}",
        first.diagnostics.diagnostics
    );
    let main_body = only_body_unit(&first.output, source);
    let main_effect_key =
        FrontendArtifactKey::unit(FrontendArtifactKind::EffectFacts, main_body).to_cache_key();
    let effect_pipeline_key =
        FrontendArtifactKey::project(FrontendArtifactKind::EffectPipelineArtifacts);
    let first_effect_pipeline_fingerprint = session
        .artifact_manifest(project)
        .expect("manifest should be stored after first check")
        .get(&effect_pipeline_key)
        .expect("effect pipeline artifact record should exist")
        .fingerprint;

    environment.external_effect_metadata = DependencyEffectMetadata {
        tags: vec![DependencyEffectTag {
            path: vec!["dep".into(), "audit".into(), "AuditTrail".into()],
            runtime_requirement: Some(RuntimeRequirementReason::FileIO),
        }],
        actions: Vec::new(),
        extensions: vec![DependencyEffectExtension {
            package: None,
            child: vec!["dep".into(), "audit".into(), "AuditTrail".into()],
            parent: vec!["FileIO".into()],
        }],
    };
    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: Vec::new(),
                dependency_overlay_changes: Vec::new(),
                environment_change: Some(EnvironmentChange::Replace(environment.clone())),
                option_changes: Vec::new(),
            },
        )
        .expect("environment metadata replacement should be accepted");

    let second = session
        .check(project, CheckRequest::incremental())
        .expect("incremental check after external metadata change should run");
    assert!(
        second.output.checked.is_some(),
        "{:?}",
        second.diagnostics.diagnostics
    );
    let second_effect_pipeline_fingerprint = session
        .artifact_manifest(project)
        .expect("manifest should be stored after second check")
        .get(&effect_pipeline_key)
        .expect("effect pipeline artifact record should exist")
        .fingerprint;

    assert_ne!(
        first_effect_pipeline_fingerprint, second_effect_pipeline_fingerprint,
        "external effect metadata must contribute to effect pipeline artifact fingerprints"
    );
    assert!(
        !second.cache.reused_artifacts.contains(&main_effect_key),
        "external effect metadata changes must not reuse stale body effect artifacts"
    );
    assert_eq!(
        second
            .output
            .effect_pipeline_artifacts
            .as_ref()
            .expect("effect pipeline artifacts")
            .dependency_metadata,
        environment.external_effect_metadata
    );
}

#[test]
fn frontend_session_incremental_check_reuses_valid_unchanged_source_artifacts() {
    let changed = etas_core::SourceId(201);
    let unchanged = etas_core::SourceId(202);
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: changed,
                path: Some(std::path::PathBuf::from("src/app/a.es")),
                text: "module app.a;\nflow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: unchanged,
                path: Some(std::path::PathBuf::from("src/app/b.es")),
                text: "module app.b;\nflow main() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "b".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    let first = session
        .check(project, CheckRequest::default())
        .expect("initial full check should run");
    assert!(first.cache.reused_artifacts.is_empty());
    assert!(first.cache.stored_artifacts.contains(
        &FrontendArtifactKey::source(FrontendArtifactKind::ParsedSource, unchanged).to_cache_key()
    ));
    let unchanged_body_units = first
        .output
        .units
        .as_ref()
        .expect("unit tree should exist")
        .nodes
        .iter()
        .filter_map(|(unit, node)| {
            (node.kind == UnitKind::Body && node.source == Some(unchanged))
                .then_some(UnitKey::new(BODY_UNIT_KIND, unit.0 as u64))
        })
        .collect::<Vec<_>>();
    assert_eq!(unchanged_body_units.len(), 1);
    let unchanged_body_type_keys = unchanged_body_units
        .iter()
        .map(|unit| {
            FrontendArtifactKey::unit(FrontendArtifactKind::TypeFacts, *unit).to_cache_key()
        })
        .collect::<Vec<_>>();
    let unchanged_body_effect_keys = unchanged_body_units
        .iter()
        .map(|unit| {
            FrontendArtifactKey::unit(FrontendArtifactKind::EffectFacts, *unit).to_cache_key()
        })
        .collect::<Vec<_>>();

    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source: changed,
                    version: SourceVersion(1),
                    text: "module app.a;\nflow helper() -> unit {\n  return;\n}".to_owned(),
                }],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect("source replacement should be accepted");

    let second = session
        .check(project, CheckRequest::incremental())
        .expect("incremental check should run");
    assert!(
        second.output.checked.is_some(),
        "{:?}",
        second.diagnostics.diagnostics
    );
    assert!(second.cache.reused_artifacts.contains(
        &FrontendArtifactKey::source(FrontendArtifactKind::ParsedSource, unchanged).to_cache_key()
    ));
    assert!(second.cache.stored_artifacts.contains(
        &FrontendArtifactKey::source(FrontendArtifactKind::ParsedSource, changed).to_cache_key()
    ));
    for key in unchanged_body_type_keys {
        assert!(
            second.cache.reused_artifacts.contains(&key),
            "unchanged source body type artifact should be reused after sibling source edit: {key}"
        );
    }
    for key in unchanged_body_effect_keys {
        assert!(
            second.cache.reused_artifacts.contains(&key),
            "unchanged source body effect artifact should be reused after sibling source edit: {key}"
        );
    }
}

#[test]
fn frontend_session_incremental_delta_preserves_reused_diagnostics() {
    let source = etas_core::SourceId(206);
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput {
        id: source,
        path: None,
        text: "flow main() -> unit { return; }".to_owned(),
        kind: SourceKind::SingleFileInput,
    }));

    session
        .check(project, CheckRequest::default())
        .expect("initial check should run");

    let second = session
        .check(project, CheckRequest::incremental())
        .expect("incremental check should run");
    let diagnostics_key =
        FrontendArtifactKey::source(FrontendArtifactKind::Diagnostics, source).to_cache_key();

    assert!(
        second.cache.reused_artifacts.contains(&diagnostics_key),
        "unchanged diagnostics artifact should be reused"
    );
    assert!(
        !second.cache.stored_artifacts.contains(&diagnostics_key),
        "unchanged diagnostics artifact should not be rewritten"
    );
    let delta = second.delta.expect("incremental delta");
    assert!(
        delta.diagnostics.republish_sources.is_empty(),
        "reused diagnostics must not be reported as replaced"
    );
}

#[test]
fn frontend_session_incremental_check_computes_reverse_import_affected_modules() {
    let source_a = etas_core::SourceId(211);
    let source_b = etas_core::SourceId(212);

    let changed_importer = incremental_import_project(source_a, source_b);
    let changed_importer = run_incremental_replace_and_collect_affected_units(
        changed_importer,
        source_a,
        SourceVersion(1),
        "module app.a;\nimport app.b;\nflow main() -> unit {\n  return;\n}".to_owned(),
    );
    assert_eq!(
        changed_importer.module_paths,
        vec![vec!["app".to_owned(), "a".to_owned()]]
    );
    assert_eq!(changed_importer.item_count, 1);
    assert_eq!(changed_importer.body_count, 1);
    assert!(changed_importer.item_keys_are_stable);
    assert!(changed_importer.body_keys_are_stable);

    let changed_imported = incremental_import_project(source_a, source_b);
    let changed_imported = run_incremental_replace_and_collect_affected_units(
        changed_imported,
        source_b,
        SourceVersion(1),
        "module app.b;\nflow helper() -> unit {\n  return;\n}".to_owned(),
    );
    assert_eq!(
        changed_imported.module_paths,
        vec![
            vec!["app".to_owned(), "b".to_owned()],
            vec!["app".to_owned(), "a".to_owned()],
        ]
    );
    assert_eq!(changed_imported.item_count, 2);
    assert_eq!(changed_imported.body_count, 2);
    assert!(changed_imported.item_keys_are_stable);
    assert!(changed_imported.body_keys_are_stable);
}

#[test]
fn frontend_session_apply_changes_mutates_sources_without_running_check() {
    let source = etas_core::SourceId(91);
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput {
        id: source,
        path: None,
        text: "flow main() -> unit { return; }".to_owned(),
        kind: SourceKind::SingleFileInput,
    }));

    let first = session
        .check(project, CheckRequest::default())
        .expect("initial check should run");
    assert_eq!(
        first.snapshot.expect("initial snapshot").revision,
        ProjectRevision(0)
    );

    let summary = session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source,
                    version: SourceVersion(1),
                    text: "flow main() -> unit {\n  let x = 1;\n  return;\n}".to_owned(),
                }],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect("source replacement should be accepted");

    assert_eq!(summary.changed_sources, vec![source]);
    assert!(summary.invalidated_artifacts.contains(
        &FrontendArtifactKey::source(FrontendArtifactKind::ParsedSource, source).to_cache_key()
    ));
    assert!(
        summary.invalidated_artifacts.contains(
            &FrontendArtifactKey::project(FrontendArtifactKind::ModuleIndex).to_cache_key()
        )
    );
    assert!(summary.invalidated_artifacts.contains(
        &FrontendArtifactKey::project(FrontendArtifactKind::CheckedProject).to_cache_key()
    ));
    assert!(summary.invalidated_artifacts.contains(
        &FrontendArtifactKey::source(FrontendArtifactKind::Diagnostics, source).to_cache_key()
    ));
    let source_set = session
        .source_set(project)
        .expect("session should expose current source state");
    let updated_source = source_set
        .files
        .iter()
        .find(|file| file.id == source)
        .expect("updated source file should remain in session source state");
    assert_eq!(
        updated_source.text.as_ref(),
        "flow main() -> unit {\n  let x = 1;\n  return;\n}"
    );
    assert_eq!(updated_source.line_index.line_count(), 4);
    assert_eq!(
        session
            .snapshot(project)
            .expect("previous snapshot")
            .revision,
        ProjectRevision(0)
    );

    let second = session
        .check(project, CheckRequest::default())
        .expect("second check should run");
    assert_eq!(
        second.snapshot.expect("updated snapshot").revision,
        ProjectRevision(1)
    );
    let delta = second.delta.expect("incremental delta");
    assert_eq!(delta.changed_sources, vec![source]);
    assert_eq!(delta.diagnostics.republish_sources, vec![source]);
}

#[test]
fn frontend_session_project_wide_changes_invalidate_graph_and_affect_all_modules() {
    let source_a = etas_core::SourceId(93);
    let source_b = etas_core::SourceId(94);
    let cases = vec![
        ProjectChangeSet {
            revision: ProjectRevision(1),
            source_changes: Vec::new(),
            dependency_overlay_changes: Vec::new(),
            environment_change: Some(EnvironmentChange::Replace(external_environment())),
            option_changes: Vec::new(),
        },
        ProjectChangeSet {
            revision: ProjectRevision(1),
            source_changes: Vec::new(),
            dependency_overlay_changes: Vec::new(),
            environment_change: Some(EnvironmentChange::Replace(
                ProjectEnvironmentInput::default(),
            )),
            option_changes: Vec::new(),
        },
        ProjectChangeSet {
            revision: ProjectRevision(1),
            source_changes: Vec::new(),
            dependency_overlay_changes: Vec::new(),
            environment_change: None,
            option_changes: vec![CompilerOptionChange {
                name: "std-registry".to_owned(),
            }],
        },
    ];

    for changes in cases {
        let mut session = FrontendSession::new();
        let project = session.open_project(incremental_import_project(source_a, source_b));
        let initial = session
            .check(project, CheckRequest::default())
            .expect("initial full check should run");
        assert!(
            initial.output.checked.is_some(),
            "{:?}",
            initial.diagnostics.diagnostics
        );

        let summary = session
            .apply_changes(project, changes)
            .expect("project-wide change should be accepted");

        assert!(summary.changed_sources.is_empty());
        assert!(summary.invalidated_artifacts.contains(
            &FrontendArtifactKey::project(FrontendArtifactKind::SourceSet).to_cache_key()
        ));
        assert!(summary.invalidated_artifacts.contains(
            &FrontendArtifactKey::project(FrontendArtifactKind::ModuleIndex).to_cache_key()
        ));
        assert!(summary.invalidated_artifacts.contains(
            &FrontendArtifactKey::project(FrontendArtifactKind::ImportGraph).to_cache_key()
        ));
        assert!(summary.invalidated_artifacts.contains(
            &FrontendArtifactKey::project(FrontendArtifactKind::ResolvedImports).to_cache_key()
        ));
        assert!(summary.invalidated_artifacts.contains(
            &FrontendArtifactKey::project(FrontendArtifactKind::ResolvedPaths).to_cache_key()
        ));
        assert!(summary.invalidated_artifacts.contains(
            &FrontendArtifactKey::project(FrontendArtifactKind::CheckedProject).to_cache_key()
        ));
        assert!(
            !summary.invalidated_artifacts.contains(
                &FrontendArtifactKey::source(FrontendArtifactKind::ParsedSource, source_a)
                    .to_cache_key()
            )
        );
        assert!(
            !summary.invalidated_artifacts.contains(
                &FrontendArtifactKey::source(FrontendArtifactKind::ParsedSource, source_b)
                    .to_cache_key()
            )
        );

        let response = session
            .check(project, CheckRequest::incremental())
            .expect("incremental project-wide check should run");
        assert!(
            response.output.checked.is_some(),
            "{:?}",
            response.diagnostics.diagnostics
        );
        assert_eq!(
            affected_module_paths(&response.output),
            vec![
                vec!["app".to_owned(), "b".to_owned()],
                vec!["app".to_owned(), "a".to_owned()],
            ]
        );
    }
}

#[test]
fn frontend_session_source_membership_changes_are_project_wide() {
    let source_a = etas_core::SourceId(95);
    let source_b = etas_core::SourceId(96);

    let mut add_session = FrontendSession::new();
    let add_project = add_session.open_project(independent_module_project(source_a, None));
    let initial_add = add_session
        .check(add_project, CheckRequest::default())
        .expect("initial add-source project should check");
    assert!(
        initial_add.output.checked.is_some(),
        "{:?}",
        initial_add.diagnostics.diagnostics
    );
    let add_summary = add_session
        .apply_changes(
            add_project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Add(independent_module_source(
                    source_b, "app.b", "helper",
                ))],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect("source add should be accepted");
    assert_eq!(add_summary.added_sources, vec![source_b]);
    assert_project_graph_invalidated(&add_summary.invalidated_artifacts);
    let add_response = add_session
        .check(add_project, CheckRequest::incremental())
        .expect("incremental add-source check should run");
    assert!(
        add_response.output.checked.is_some(),
        "{:?}",
        add_response.diagnostics.diagnostics
    );
    let mut add_affected = affected_module_paths(&add_response.output);
    add_affected.sort();
    assert_eq!(
        add_affected,
        vec![
            vec!["app".to_owned(), "a".to_owned()],
            vec!["app".to_owned(), "b".to_owned()],
        ]
    );

    let mut remove_session = FrontendSession::new();
    let remove_project =
        remove_session.open_project(independent_module_project(source_a, Some(source_b)));
    let initial_remove = remove_session
        .check(remove_project, CheckRequest::default())
        .expect("initial remove-source project should check");
    assert!(
        initial_remove.output.checked.is_some(),
        "{:?}",
        initial_remove.diagnostics.diagnostics
    );
    let remove_summary = remove_session
        .apply_changes(
            remove_project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Remove(source_b)],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect("source remove should be accepted");
    assert_eq!(remove_summary.removed_sources, vec![source_b]);
    assert_project_graph_invalidated(&remove_summary.invalidated_artifacts);
    let remove_response = remove_session
        .check(remove_project, CheckRequest::incremental())
        .expect("incremental remove-source check should run");
    assert!(
        remove_response.output.checked.is_some(),
        "{:?}",
        remove_response.diagnostics.diagnostics
    );
    assert_eq!(
        affected_module_paths(&remove_response.output),
        vec![vec!["app".to_owned(), "a".to_owned()]]
    );
}

#[test]
fn frontend_session_dependency_overlay_change_is_not_project_wide() {
    let source_a = etas_core::SourceId(97);
    let overlay_source = etas_core::SourceId(98);
    let package = ExternalPackageId(7);
    let import_root = "dep";
    let mut session = FrontendSession::new();
    let project = session.open_project(independent_module_project(source_a, None));
    let initial = session
        .check(project, CheckRequest::default())
        .expect("initial project should check");
    assert!(
        initial.output.checked.is_some(),
        "{:?}",
        initial.diagnostics.diagnostics
    );

    let summary = session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: Vec::new(),
                dependency_overlay_changes: vec![DependencyOverlayChange {
                    package,
                    import_root: import_root.to_owned(),
                    added_sources: vec![dependency_overlay_source(
                        overlay_source,
                        package,
                        import_root,
                        "api",
                    )],
                }],
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect("dependency overlay add should be accepted");

    assert_eq!(summary.added_sources, vec![overlay_source]);
    assert_eq!(summary.changed_sources, vec![overlay_source]);
    assert!(summary.removed_sources.is_empty());
    assert!(
        summary.invalidated_artifacts.contains(
            &FrontendArtifactKey::project(FrontendArtifactKind::SourceSet).to_cache_key()
        )
    );
    assert!(
        summary.invalidated_artifacts.contains(
            &FrontendArtifactKey::source(FrontendArtifactKind::ParsedSource, overlay_source)
                .to_cache_key()
        )
    );
    assert!(!summary.invalidated_artifacts.contains(
        &FrontendArtifactKey::source(FrontendArtifactKind::ParsedSource, source_a).to_cache_key()
    ));

    let source_set = session
        .source_set(project)
        .expect("session should expose current source state");
    let overlay = source_set
        .files
        .iter()
        .find(|file| file.id == overlay_source)
        .expect("overlay source should be added to source state");
    assert_eq!(
        overlay.kind,
        SourceKind::DependencySourceOverlay {
            package,
            import_root: import_root.to_owned(),
        }
    );

    let incremental = session
        .check(project, CheckRequest::incremental())
        .expect("dependency overlay incremental check should run");
    assert!(
        incremental.output.checked.is_some(),
        "{:?}",
        incremental.diagnostics.diagnostics
    );
    assert_eq!(
        incremental.output.parsed_sources.len(),
        2,
        "incremental overlay check should include project and overlay parsed sources"
    );
    assert_eq!(
        incremental.output.reused_parsed_sources, 1,
        "unchanged project source should reuse its parsed artifact after adding dependency overlay"
    );
}

#[test]
fn frontend_session_dependency_overlay_change_rejects_invalid_sources() {
    let source_a = etas_core::SourceId(99);
    let package = ExternalPackageId(7);
    let mut session = FrontendSession::new();
    let project = session.open_project(independent_module_project(source_a, None));

    let empty_error = session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: Vec::new(),
                dependency_overlay_changes: vec![DependencyOverlayChange {
                    package,
                    import_root: "dep".to_owned(),
                    added_sources: Vec::new(),
                }],
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect_err("empty overlay should be rejected");
    assert_eq!(
        empty_error,
        FrontendSessionError::EmptyDependencyOverlay {
            package,
            import_root: "dep".to_owned(),
        }
    );

    let invalid_kind_error = session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: Vec::new(),
                dependency_overlay_changes: vec![DependencyOverlayChange {
                    package,
                    import_root: "dep".to_owned(),
                    added_sources: vec![SourceInput {
                        id: etas_core::SourceId(100),
                        path: Some(std::path::PathBuf::from("src/dep/not_overlay.es")),
                        text: "module dep.not_overlay;\nflow helper() -> unit { return; }"
                            .to_owned(),
                        kind: SourceKind::SourceProjectFile,
                    }],
                }],
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect_err("non-overlay source should be rejected");
    assert_eq!(
        invalid_kind_error,
        FrontendSessionError::InvalidDependencyOverlaySource {
            source: etas_core::SourceId(100),
        }
    );

    let mismatch_error = session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: Vec::new(),
                dependency_overlay_changes: vec![DependencyOverlayChange {
                    package,
                    import_root: "dep".to_owned(),
                    added_sources: vec![dependency_overlay_source(
                        etas_core::SourceId(101),
                        ExternalPackageId(8),
                        "dep",
                        "mismatch",
                    )],
                }],
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect_err("mismatched overlay package should be rejected");
    assert_eq!(
        mismatch_error,
        FrontendSessionError::MismatchedDependencyOverlaySource {
            source: etas_core::SourceId(101),
            expected_package: package,
            expected_import_root: "dep".to_owned(),
        }
    );
}

#[test]
fn frontend_session_dependency_overlay_change_rejects_path_conflicts() {
    let source_a = etas_core::SourceId(102);
    let package = ExternalPackageId(7);
    let mut session = FrontendSession::new();
    let project = session.open_project(independent_module_project(source_a, None));

    let error = session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: Vec::new(),
                dependency_overlay_changes: vec![DependencyOverlayChange {
                    package,
                    import_root: "dep".to_owned(),
                    added_sources: vec![SourceInput {
                        id: etas_core::SourceId(103),
                        path: Some(std::path::PathBuf::from("src/app/a.es")),
                        text: "module dep.conflict;\nflow helper() -> unit { return; }".to_owned(),
                        kind: SourceKind::DependencySourceOverlay {
                            package,
                            import_root: "dep".to_owned(),
                        },
                    }],
                }],
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect_err("overlay source path conflict should be rejected");
    assert_eq!(
        error,
        FrontendSessionError::DuplicateSourcePath(std::path::PathBuf::from("src/app/a.es"))
    );
}

fn independent_module_project(
    source_a: etas_core::SourceId,
    source_b: Option<etas_core::SourceId>,
) -> ProjectInput {
    let mut sources = vec![independent_module_source(source_a, "app.a", "main")];
    if let Some(source_b) = source_b {
        sources.push(independent_module_source(source_b, "app.b", "helper"));
    }
    ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources,
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    }
}

fn independent_module_source(source: etas_core::SourceId, module: &str, flow: &str) -> SourceInput {
    SourceInput {
        id: source,
        path: Some(std::path::PathBuf::from(format!(
            "src/{}.es",
            module.replace('.', "/")
        ))),
        text: format!("module {module};\nflow {flow}() -> unit {{ return; }}"),
        kind: SourceKind::SourceProjectFile,
    }
}

fn dependency_overlay_source(
    source: etas_core::SourceId,
    package: ExternalPackageId,
    import_root: &str,
    module_suffix: &str,
) -> SourceInput {
    SourceInput {
        id: source,
        path: Some(std::path::PathBuf::from(format!(
            "src/{import_root}/{module_suffix}.es"
        ))),
        text: format!("module {import_root}.{module_suffix};\nflow helper() -> unit {{ return; }}"),
        kind: SourceKind::DependencySourceOverlay {
            package,
            import_root: import_root.to_owned(),
        },
    }
}

fn assert_project_graph_invalidated(invalidated: &[ArtifactKey]) {
    assert!(
        invalidated.contains(
            &FrontendArtifactKey::project(FrontendArtifactKind::SourceSet).to_cache_key()
        )
    );
    assert!(
        invalidated.contains(
            &FrontendArtifactKey::project(FrontendArtifactKind::ModuleIndex).to_cache_key()
        )
    );
    assert!(
        invalidated.contains(
            &FrontendArtifactKey::project(FrontendArtifactKind::ImportGraph).to_cache_key()
        )
    );
    assert!(invalidated.contains(
        &FrontendArtifactKey::project(FrontendArtifactKind::ResolvedImports).to_cache_key()
    ));
    assert!(invalidated.contains(
        &FrontendArtifactKey::project(FrontendArtifactKind::ResolvedPaths).to_cache_key()
    ));
    assert!(invalidated.contains(
        &FrontendArtifactKey::project(FrontendArtifactKind::CheckedProject).to_cache_key()
    ));
}

fn incremental_import_project(
    source_a: etas_core::SourceId,
    source_b: etas_core::SourceId,
) -> ProjectInput {
    ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: source_a,
                path: Some(std::path::PathBuf::from("src/app/a.es")),
                text: "module app.a;\nimport app.b;\nflow main() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: source_b,
                path: Some(std::path::PathBuf::from("src/app/b.es")),
                text: "module app.b;\nflow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    }
}

struct AffectedUnitSummary {
    module_paths: Vec<Vec<String>>,
    item_count: usize,
    body_count: usize,
    item_keys_are_stable: bool,
    body_keys_are_stable: bool,
}

fn run_incremental_replace_and_collect_affected_units(
    input: ProjectInput,
    source: etas_core::SourceId,
    version: SourceVersion,
    text: String,
) -> AffectedUnitSummary {
    let mut session = FrontendSession::new();
    let project = session.open_project(input);
    let initial = session
        .check(project, CheckRequest::default())
        .expect("initial full check should run");
    assert!(
        initial.output.checked.is_some(),
        "{:?}",
        initial.diagnostics.diagnostics
    );

    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source,
                    version,
                    text,
                }],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect("source replacement should be accepted");

    let response = session
        .check(project, CheckRequest::incremental())
        .expect("incremental check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    let delta = response
        .delta
        .expect("incremental delta should be produced");
    AffectedUnitSummary {
        module_paths: affected_module_paths(&response.output),
        item_count: delta.affected_items.len(),
        body_count: delta.affected_bodies.len(),
        item_keys_are_stable: delta
            .affected_items
            .iter()
            .all(|unit| unit.kind == ITEM_UNIT_KIND),
        body_keys_are_stable: delta
            .affected_bodies
            .iter()
            .all(|unit| unit.kind == BODY_UNIT_KIND),
    }
}

fn affected_module_paths(output: &ProjectOutput) -> Vec<Vec<String>> {
    let units = output.units.as_ref().expect("unit tree should exist");
    let modules = output.modules.as_ref().expect("module index should exist");
    output
        .affected_modules
        .as_ref()
        .expect("affected modules should exist")
        .modules
        .iter()
        .map(|unit| {
            let node = units
                .nodes
                .get(UnitId(unit.id as u32))
                .expect("affected module unit should exist");
            let UnitTarget::Module(module) = &node.target else {
                panic!("affected module unit should target a module");
            };
            modules
                .modules
                .get(*module)
                .expect("affected module should exist")
                .path
                .segments
                .clone()
        })
        .collect()
}

#[test]
fn frontend_session_rejects_non_increasing_source_versions() {
    let source = etas_core::SourceId(92);
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput {
        id: source,
        path: None,
        text: "flow main() -> unit { return; }".to_owned(),
        kind: SourceKind::SingleFileInput,
    }));

    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source,
                    version: SourceVersion(1),
                    text: "flow main() -> unit { let x = 1; return; }".to_owned(),
                }],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect("first source version should be accepted");

    let error = session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(2),
                source_changes: vec![SourceChange::Replace {
                    source,
                    version: SourceVersion(1),
                    text: "flow main() -> unit { let x = 2; return; }".to_owned(),
                }],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .expect_err("same source version must be rejected");

    assert!(matches!(
        error,
        FrontendSessionError::NonIncreasingSourceVersion {
            source: rejected,
            current: SourceVersion(1),
            requested: SourceVersion(1),
        } if rejected == source
    ));
}

#[test]
fn import_graph_does_not_rewrite_external_child_module_to_source_prefix() {
    let frontend = Frontend;
    let environment = ProjectEnvironmentInput {
        external_packages: vec![ProjectExternalPackageInput {
            id: ExternalPackageId(0),
            name: "edk".to_owned(),
            version: "1.0.0".to_owned(),
            edition: "2026".to_owned(),
            import_root: "edk".to_owned(),
        }],
        external_modules: vec![ProjectExternalModuleInput {
            package: Some(ExternalPackageId(0)),
            id: ExternalModuleId(7),
            path: ModulePath {
                segments: vec!["edk".into(), "http".into(), "types".into()],
            },
            exports: vec![ProjectExternalExportInput {
                symbol: ExternalSymbolId(0),
                name: "HttpResponse".to_owned(),
                visibility: etas_hir::Visibility::Public,
            }],
        }],
        ..ProjectEnvironmentInput::default()
    };

    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(10_001),
                path: Some(std::path::PathBuf::from("src/app/main.es")),
                text: r#"
module app.main;
import edk.http.types.HttpResponse as DirectHttpResponse;
import edk.http.HttpResponse as FacadeHttpResponse;

flow main(args: Array<string>) -> i32 {
  return 0;
}
"#
                .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(10_002),
                path: Some(std::path::PathBuf::from("src/edk/http.es")),
                text: r#"
module edk.http;
public import edk.http.types.HttpResponse;

flow request() -> unit {
  return;
}
"#
                .to_owned(),
                kind: SourceKind::DependencySourceOverlay {
                    package: ExternalPackageId(0),
                    import_root: "edk".to_owned(),
                },
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    let modules = output.modules.as_ref().expect("module index should exist");
    let app_main = modules
        .by_path
        .get(&ModulePath {
            segments: vec!["app".to_owned(), "main".to_owned()],
        })
        .copied()
        .expect("app.main module should exist");
    let edk_http = modules
        .by_path
        .get(&ModulePath {
            segments: vec!["edk".to_owned(), "http".to_owned()],
        })
        .copied()
        .expect("edk.http source module should exist");
    assert!(matches!(
        modules.modules.get(edk_http).map(|module| &module.origin),
        Some(etas_frontend::ModuleOrigin::DependencySourceOverlay {
            package: ExternalPackageId(0),
            import_root,
        }) if import_root == "edk"
    ));
    let graph = output
        .import_graph
        .as_ref()
        .expect("import graph should exist");
    let resolved_imports = output
        .resolved_imports
        .as_ref()
        .expect("resolved imports should exist");

    assert!(
        resolved_imports.imports.iter().any(|import| {
            import.local_name == "DirectHttpResponse"
                && matches!(
                    &import.target,
                    ImportTarget::ExternalItem {
                        package: Some(ExternalPackageId(0)),
                        module: ExternalModuleId(7),
                        module_path,
                        name,
                        symbol: ExternalSymbolId(0),
                        ..
                    } if module_path.segments == ["edk", "http", "types"]
                        && name == "HttpResponse"
                )
        }),
        "direct child import should resolve to external metadata item, got {:#?}",
        resolved_imports.imports
    );

    assert!(
        resolved_imports.imports.iter().any(|import| {
            import.local_name == "FacadeHttpResponse"
                && matches!(
                    &import.target,
                    ImportTarget::ExternalItem {
                        package: Some(ExternalPackageId(0)),
                        module: ExternalModuleId(7),
                        module_path,
                        name,
                        symbol: ExternalSymbolId(0),
                        ..
                    } if module_path.segments == ["edk", "http", "types"]
                        && name == "HttpResponse"
                )
        }),
        "source facade re-export should resolve to external metadata item, got {:#?}",
        resolved_imports.imports
    );

    assert!(
        graph
            .edges
            .iter()
            .all(|edge| !(edge.from == app_main && edge.resolved == Some(edk_http))),
        "external metadata child import must not become a source prefix edge: {:#?}",
        graph.edges
    );
}

#[test]
fn frontend_computes_runtime_source_requirements_from_reachable_external_call() {
    let frontend = Frontend;
    let environment = ProjectEnvironmentInput {
        external_packages: vec![ProjectExternalPackageInput {
            id: ExternalPackageId(0),
            name: "dep".to_owned(),
            version: "1.0.0".to_owned(),
            edition: "2026".to_owned(),
            import_root: "dep".to_owned(),
        }],
        external_modules: vec![ProjectExternalModuleInput {
            package: Some(ExternalPackageId(0)),
            id: ExternalModuleId(2),
            path: ModulePath {
                segments: vec!["dep".to_owned(), "api".to_owned()],
            },
            exports: vec![ProjectExternalExportInput {
                symbol: ExternalSymbolId(3),
                name: "call".to_owned(),
                visibility: etas_hir::Visibility::Public,
            }],
        }],
        external_public_metadata: vec![ProjectExternalPublicMetadataInput {
            package: ExternalPackageId(0),
            types: Vec::new(),
            values: Vec::new(),
            enums: Vec::new(),
            flows: vec![ProjectExternalFlowSignatureInput {
                path: vec!["dep".to_owned(), "api".to_owned(), "call".to_owned()],
                param_names: Vec::new(),
                params: Vec::new(),
                output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
                effects: None,
                visibility: "public".to_owned(),
            }],
            agents: Vec::new(),
            tools: Vec::new(),
            tool_schemas: Vec::new(),
            effects: Vec::new(),
            actions: Vec::new(),
            trace_specs: Vec::new(),
            spec_signatures: Vec::new(),
            spec_impls: Vec::new(),
            type_spec_satisfactions: Vec::new(),
            callable_spec_satisfactions: Vec::new(),
            trace_spec_conformances: Vec::new(),
            effect_summaries: vec![ProjectExternalEffectSummaryInput {
                item: vec!["dep".to_owned(), "api".to_owned(), "call".to_owned()],
                public_effects: ProjectExternalEffectRowInput::default(),
                requested_actions: ProjectExternalEffectRowInput::default(),
                handled_requested_actions: ProjectExternalEffectRowInput::default(),
                latent_flows: Vec::new(),
            }],
            action_summaries: Vec::new(),
            trace_spec_summaries: Vec::new(),
            re_exports: Vec::new(),
        }],
        ..ProjectEnvironmentInput::default()
    };

    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput {
            id: etas_core::SourceId(11_001),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"
module app.main;
import dep.api.call;

flow main() -> unit {
  call();
  return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    let requirements = output
        .runtime_source_requirements
        .as_ref()
        .expect("runtime source requirements should be computed");
    let requirement = requirements
        .dependencies
        .get(&ExternalPackageId(0))
        .expect("dep package should require runtime source");
    assert_eq!(requirement.import_root, "dep");
    assert!(requirement.seed_modules.contains(&ModulePath {
        segments: vec!["dep".to_owned(), "api".to_owned()],
    }));
}

#[test]
fn frontend_runtime_source_requirements_follow_external_re_export() {
    let environment = ProjectEnvironmentInput {
        external_packages: vec![runtime_dep_package()],
        external_modules: vec![
            ProjectExternalModuleInput {
                package: Some(ExternalPackageId(0)),
                id: ExternalModuleId(2),
                path: ModulePath {
                    segments: vec!["dep".to_owned()],
                },
                exports: vec![ProjectExternalExportInput {
                    symbol: ExternalSymbolId(4),
                    name: "get".to_owned(),
                    visibility: etas_hir::Visibility::Public,
                }],
            },
            ProjectExternalModuleInput {
                package: Some(ExternalPackageId(0)),
                id: ExternalModuleId(3),
                path: ModulePath {
                    segments: vec!["dep".to_owned(), "api".to_owned()],
                },
                exports: vec![ProjectExternalExportInput {
                    symbol: ExternalSymbolId(5),
                    name: "call".to_owned(),
                    visibility: etas_hir::Visibility::Public,
                }],
            },
        ],
        external_public_metadata: vec![ProjectExternalPublicMetadataInput {
            package: ExternalPackageId(0),
            flows: vec![
                external_unit_flow(&["dep", "get"]),
                external_unit_flow(&["dep", "api", "call"]),
            ],
            effect_summaries: vec![
                empty_external_summary(&["dep", "get"]),
                empty_external_summary(&["dep", "api", "call"]),
            ],
            re_exports: vec![ProjectExternalReExportInput {
                from: vec!["dep".to_owned(), "api".to_owned(), "call".to_owned()],
                exported: vec!["dep".to_owned(), "get".to_owned()],
            }],
            ..empty_external_public_metadata(ExternalPackageId(0))
        }],
        ..ProjectEnvironmentInput::default()
    };

    let output = runtime_requirements_project(
        environment,
        r#"
module app.main;
import dep.get;

flow main() -> unit {
  get();
  return;
}
"#,
    );

    assert_runtime_requirement_seed_modules(&output, &[&["dep", "api"]]);
}

#[test]
fn frontend_runtime_source_requirements_follow_external_wildcard_import() {
    let environment = ProjectEnvironmentInput {
        external_packages: vec![runtime_dep_package()],
        external_modules: vec![ProjectExternalModuleInput {
            package: Some(ExternalPackageId(0)),
            id: ExternalModuleId(2),
            path: ModulePath {
                segments: vec!["dep".to_owned(), "plugins".to_owned()],
            },
            exports: vec![ProjectExternalExportInput {
                symbol: ExternalSymbolId(4),
                name: "register".to_owned(),
                visibility: etas_hir::Visibility::Public,
            }],
        }],
        external_public_metadata: vec![ProjectExternalPublicMetadataInput {
            package: ExternalPackageId(0),
            flows: vec![external_unit_flow(&["dep", "plugins", "register"])],
            effect_summaries: vec![empty_external_summary(&["dep", "plugins", "register"])],
            ..empty_external_public_metadata(ExternalPackageId(0))
        }],
        ..ProjectEnvironmentInput::default()
    };

    let output = runtime_requirements_project(
        environment,
        r#"
module app.main;
import dep.plugins.*;

flow main() -> unit {
  register();
  return;
}
"#,
    );

    assert_runtime_requirement_seed_modules(&output, &[&["dep", "plugins"]]);
}

#[test]
fn frontend_runtime_source_requirements_ignore_provider_bound_external_tool() {
    let environment = ProjectEnvironmentInput {
        external_packages: vec![runtime_dep_package()],
        external_modules: vec![ProjectExternalModuleInput {
            package: Some(ExternalPackageId(0)),
            id: ExternalModuleId(2),
            path: ModulePath {
                segments: vec!["dep".to_owned(), "host".to_owned()],
            },
            exports: vec![ProjectExternalExportInput {
                symbol: ExternalSymbolId(4),
                name: "tool".to_owned(),
                visibility: etas_hir::Visibility::Public,
            }],
        }],
        external_public_metadata: vec![ProjectExternalPublicMetadataInput {
            package: ExternalPackageId(0),
            tools: vec![ProjectExternalToolSignatureInput {
                path: vec!["dep".to_owned(), "host".to_owned(), "tool".to_owned()],
                param_names: Vec::new(),
                input: Vec::new(),
                output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
                effects: None,
                visibility: "public".to_owned(),
            }],
            effect_summaries: vec![empty_external_summary(&["dep", "host", "tool"])],
            ..empty_external_public_metadata(ExternalPackageId(0))
        }],
        tool_bindings: vec![ProjectToolBindingInput {
            tool: "dep.host.tool".to_owned(),
            provider: "test.provider".to_owned(),
            effect_row: Vec::new(),
            action_row: Vec::new(),
        }],
        ..ProjectEnvironmentInput::default()
    };

    let output = runtime_requirements_project(
        environment,
        r#"
module app.main;
import dep.host.tool;

flow main() -> unit {
  tool();
  return;
}
"#,
    );

    assert_runtime_requirements_empty(&output);
}

#[test]
fn frontend_runtime_source_requirements_ignore_unreachable_external_import() {
    let environment = ProjectEnvironmentInput {
        external_packages: vec![runtime_dep_package()],
        external_modules: vec![ProjectExternalModuleInput {
            package: Some(ExternalPackageId(0)),
            id: ExternalModuleId(2),
            path: ModulePath {
                segments: vec!["dep".to_owned(), "api".to_owned()],
            },
            exports: vec![ProjectExternalExportInput {
                symbol: ExternalSymbolId(4),
                name: "call".to_owned(),
                visibility: etas_hir::Visibility::Public,
            }],
        }],
        external_public_metadata: vec![ProjectExternalPublicMetadataInput {
            package: ExternalPackageId(0),
            flows: vec![external_unit_flow(&["dep", "api", "call"])],
            effect_summaries: vec![empty_external_summary(&["dep", "api", "call"])],
            ..empty_external_public_metadata(ExternalPackageId(0))
        }],
        ..ProjectEnvironmentInput::default()
    };

    let output = runtime_requirements_project(
        environment,
        r#"
module app.main;
import dep.api.call;

flow main() -> unit {
  return;
}
"#,
    );

    assert_runtime_requirements_empty(&output);
}

#[test]
fn frontend_entry_reachable_scope_checks_only_reachable_bodies() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(12_101),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"
module app.main;

flow helper() -> unit {
  return;
}

flow unused() -> unit {
  return;
}

flow main() -> unit {
  helper();
  return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    let response = session
        .check(
            project,
            CheckRequest::full_project().with_scope(CheckScope::EntryReachable),
        )
        .expect("entry-reachable check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    let reachability = response
        .output
        .reachability
        .as_ref()
        .expect("entry-reachable check should produce reachability facts");
    let all_bodies = response
        .output
        .units
        .as_ref()
        .expect("unit tree should be built")
        .nodes
        .iter()
        .filter(|(_, node)| node.kind == UnitKind::Body)
        .count();
    assert!(
        reachability.reachable_bodies.len() < all_bodies,
        "unused body should not be entry-reachable"
    );
    assert_eq!(
        response.output.type_body_outputs.len(),
        reachability.reachable_bodies.len(),
        "entry-reachable type checking should only emit reachable body facts"
    );
    assert_eq!(
        response.output.effect_body_outputs.len(),
        reachability.reachable_bodies.len(),
        "entry-reachable effect checking should only emit reachable body facts"
    );
}

#[test]
fn frontend_entry_reachable_scope_follows_source_import_call_targets() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(12_111),
                path: Some(std::path::PathBuf::from("src/app/main.es")),
                text: r#"
module app.main;
import app.api.{wrapper as run_wrapper};

flow main() -> i32 {
  return run_wrapper();
}
"#
                .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(12_112),
                path: Some(std::path::PathBuf::from("src/app/api.es")),
                text: r#"
module app.api;
import app.impl.{helper as inner_helper};

public flow wrapper() -> i32 {
  return inner_helper();
}

public flow unused_api() -> i32 {
  return 0;
}
"#
                .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(12_113),
                path: Some(std::path::PathBuf::from("src/app/impl.es")),
                text: r#"
module app.impl;

public flow helper() -> i32 {
  return 1;
}

public flow unused_impl() -> i32 {
  return 0;
}
"#
                .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    let response = session
        .check(
            project,
            CheckRequest::full_project().with_scope(CheckScope::EntryReachable),
        )
        .expect("entry-reachable check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    let names = reachable_item_names(&response.output);
    assert!(names.contains(&"main".to_owned()), "{names:?}");
    assert!(names.contains(&"wrapper".to_owned()), "{names:?}");
    assert!(names.contains(&"helper".to_owned()), "{names:?}");
    assert!(!names.contains(&"unused_api".to_owned()), "{names:?}");
    assert!(!names.contains(&"unused_impl".to_owned()), "{names:?}");
}

#[test]
fn frontend_entry_reachable_scope_includes_referenced_handler_values() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(12_121),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"
module app.main;

effect Gate {
  action request() -> i32;
}

let GateHandler: ![Gate => []] = handler {
  Gate.request() => {
    resume 7;
  }
};

flow main() -> i32 {
  let selected = GateHandler;
  return perform Gate.request() with selected;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    let response = session
        .check(
            project,
            CheckRequest::full_project().with_scope(CheckScope::EntryReachable),
        )
        .expect("entry-reachable check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    let names = reachable_item_names(&response.output);
    assert!(names.contains(&"main".to_owned()), "{names:?}");
    assert!(names.contains(&"GateHandler".to_owned()), "{names:?}");
}

#[test]
fn frontend_entry_reachable_scope_does_not_execute_plain_flow_value_reference() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(12_122),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"
module app.main;

public flow referenced() -> i32 {
  return 1;
}

flow main() -> i32 {
  let callback = referenced;
  return 0;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    let response = session
        .check(
            project,
            CheckRequest::full_project().with_scope(CheckScope::EntryReachable),
        )
        .expect("entry-reachable check should run");
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics.diagnostics
    );
    let names = reachable_item_names(&response.output);
    assert!(names.contains(&"main".to_owned()), "{names:?}");
    assert!(
        !names.contains(&"referenced".to_owned()),
        "plain flow value reference must not make body executable-reachable: {names:?}"
    );
}

fn reachable_item_names(output: &ProjectOutput) -> Vec<String> {
    let hir = &output.hir.as_ref().expect("HIR output should exist").hir;
    let mut names = output
        .reachability
        .as_ref()
        .expect("reachability should exist")
        .reachable_items
        .iter()
        .filter_map(|item| match hir.items.get(*item)? {
            etas_hir::HirItem::Flow(flow) => hir.symbols.get(flow.symbol),
            etas_hir::HirItem::Agent(agent) => hir.symbols.get(agent.symbol),
            etas_hir::HirItem::Tool(tool) => hir.symbols.get(tool.symbol),
            etas_hir::HirItem::TopLevelLet(value) => hir.symbols.get(value.symbol),
            _ => None,
        })
        .map(|symbol| symbol.name.clone())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn runtime_dep_package() -> ProjectExternalPackageInput {
    ProjectExternalPackageInput {
        id: ExternalPackageId(0),
        name: "dep".to_owned(),
        version: "1.0.0".to_owned(),
        edition: "2026".to_owned(),
        import_root: "dep".to_owned(),
    }
}

fn path_eq(path: &[String], expected: &[&str]) -> bool {
    path.iter().map(String::as_str).eq(expected.iter().copied())
}

fn empty_external_public_metadata(
    package: ExternalPackageId,
) -> ProjectExternalPublicMetadataInput {
    ProjectExternalPublicMetadataInput {
        package,
        types: Vec::new(),
        values: Vec::new(),
        enums: Vec::new(),
        flows: Vec::new(),
        agents: Vec::new(),
        tools: Vec::new(),
        tool_schemas: Vec::new(),
        effects: Vec::new(),
        actions: Vec::new(),
        trace_specs: Vec::new(),
        spec_signatures: Vec::new(),
        spec_impls: Vec::new(),
        type_spec_satisfactions: Vec::new(),
        callable_spec_satisfactions: Vec::new(),
        trace_spec_conformances: Vec::new(),
        effect_summaries: Vec::new(),
        action_summaries: Vec::new(),
        trace_spec_summaries: Vec::new(),
        re_exports: Vec::new(),
    }
}

fn external_unit_flow(path: &[&str]) -> ProjectExternalFlowSignatureInput {
    ProjectExternalFlowSignatureInput {
        path: path.iter().map(|segment| (*segment).to_owned()).collect(),
        param_names: Vec::new(),
        params: Vec::new(),
        output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
        effects: None,
        visibility: "public".to_owned(),
    }
}

fn empty_external_summary(path: &[&str]) -> ProjectExternalEffectSummaryInput {
    ProjectExternalEffectSummaryInput {
        item: path.iter().map(|segment| (*segment).to_owned()).collect(),
        public_effects: ProjectExternalEffectRowInput::default(),
        requested_actions: ProjectExternalEffectRowInput::default(),
        handled_requested_actions: ProjectExternalEffectRowInput::default(),
        latent_flows: Vec::new(),
    }
}

fn external_effect(
    path: &[&str],
    args: Vec<ProjectExternalEffectArgInput>,
) -> ProjectExternalEffectRefInput {
    ProjectExternalEffectRefInput {
        path: path.iter().map(|segment| (*segment).to_owned()).collect(),
        args,
    }
}

fn external_effect_row(
    effects: Vec<ProjectExternalEffectRefInput>,
) -> ProjectExternalEffectRowInput {
    ProjectExternalEffectRowInput { effects }
}

fn external_effect_path_arg(path: &[&str]) -> ProjectExternalEffectArgInput {
    ProjectExternalEffectArgInput::Path(path.iter().map(|segment| (*segment).to_owned()).collect())
}

fn runtime_requirements_project(environment: ProjectEnvironmentInput, text: &str) -> ProjectOutput {
    let output = Frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput {
            id: etas_core::SourceId(12_001),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: text.to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });
    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    output
}

fn assert_runtime_requirement_seed_modules(output: &ProjectOutput, expected: &[&[&str]]) {
    let requirements = output
        .runtime_source_requirements
        .as_ref()
        .expect("runtime source requirements should be computed");
    let requirement = requirements
        .dependencies
        .get(&ExternalPackageId(0))
        .expect("dep package should require runtime source");
    let actual = requirement
        .seed_modules
        .iter()
        .map(|module| module.segments.clone())
        .collect::<Vec<_>>();
    let expected = expected
        .iter()
        .map(|module| {
            module
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

fn assert_runtime_requirements_empty(output: &ProjectOutput) {
    assert!(
        output
            .runtime_source_requirements
            .as_ref()
            .is_some_and(|requirements| requirements.is_empty()),
        "runtime source requirements should be empty: {:?}",
        output.runtime_source_requirements
    );
}

#[test]
fn frontend_effect_pipeline_consumes_external_effect_metadata() {
    let mut environment = external_environment();
    environment.external_effect_metadata = DependencyEffectMetadata {
        tags: vec![DependencyEffectTag {
            path: vec!["dep".into(), "payments".into(), "Payment".into()],
            runtime_requirement: Some(RuntimeRequirementReason::Network),
        }],
        actions: Vec::new(),
        extensions: vec![DependencyEffectExtension {
            package: None,
            child: vec!["dep".into(), "payments".into(), "Payment".into()],
            parent: vec!["Network".into()],
        }],
    };
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::env::current_dir().expect("current dir should exist"),
        source_root: None,
        options: Default::default(),
        environment: environment.clone(),
        sources: vec![SourceInput::anonymous(
            r#"
flow main() -> unit {
  return;
}
"#,
        )],
        entry: ProjectEntry {
            module: None,
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    let artifacts = output
        .effect_pipeline_artifacts
        .as_ref()
        .expect("effect pipeline artifacts should be retained");
    assert_eq!(
        artifacts.dependency_metadata,
        environment.external_effect_metadata
    );
    let payment = artifacts
        .registry
        .tag_by_name("dep.payments.Payment")
        .expect("external effect metadata should enter registry");
    assert!(
        artifacts
            .registry
            .tag_extends(payment, etas_effects::NETWORK_TAG)
    );
}

#[test]
fn frontend_check_rejects_single_file_without_entry_flow() {
    let frontend = Frontend;
    let output = frontend.check(SourceInput::anonymous(
        r#"
flow helper() -> unit {
  return;
}
"#,
    ));

    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("entry flow `main` was not found")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn frontend_check_records_runtime_required_effects_as_non_blocking_readiness() {
    let frontend = Frontend;
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

flow main(args: Array<string>) -> i32 ![Network]

{
  perform Payment.ping();
  return 0;
}
"#,
    ));
    input.options.entry_policy = EntryPolicy::Runnable;
    let output = frontend.check_project(input);

    assert!(output.effects.is_some());
    assert!(output.checked.is_some());
    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::RuntimeRequiredInPhase1)
            && diagnostic.severity == etas_core::Severity::Warning
    }));
}

#[test]
fn frontend_check_records_agent_pipeline_stage_readiness() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: Some(std::path::PathBuf::from("src")),
        options: ProjectCompileOptions {
            entry_policy: EntryPolicy::Runnable,
        },
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(61),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"
module app.main;
import std.agent.prompt.Prompt;

agent Reviewer(input: Prompt) -> string ![] {
  return input;
}

flow run_stage(input: Prompt) -> string {
  return input ~> Reviewer;
}

flow main(args: Array<string>) -> i32 {
  let prompt = Prompt.new().user(Public("hello"));
  let result = run_stage(prompt);
  if result == "" {
    return 1;
  }
  return 0;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.effects.is_some());
    assert!(output.checked.is_some());
    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::RuntimeRequiredInPhase1)
            && diagnostic.severity == etas_core::Severity::Warning
    }));
}

#[test]
fn frontend_check_records_agent_run_method_call_readiness() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: Some(std::path::PathBuf::from("src")),
        options: ProjectCompileOptions {
            entry_policy: EntryPolicy::Runnable,
        },
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(62),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"
module app.main;
import std.agent.prompt.Prompt;

agent Reviewer(input: Prompt) -> string ![] {
  return input;
}

flow run_agent(input: Prompt) -> string {
  return Reviewer.run(input);
}

flow main(args: Array<string>) -> i32 {
  let prompt = Prompt.new().user(Public("hello"));
  let result = run_agent(prompt);
  if result == "" {
    return 1;
  }
  return 0;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.effects.is_some());
    assert!(output.checked.is_some());
    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::RuntimeRequiredInPhase1)
            && diagnostic.severity == etas_core::Severity::Warning
    }));
}

#[test]
fn frontend_check_classifies_top_level_memory_region_handle() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: src_source_root(),
        options: ProjectCompileOptions {
            entry_policy: EntryPolicy::Runnable,
        },
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(70),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r##"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>,
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main(args: Array<string>) -> i32 {
  let existing = ProjectMemory.Papers.get("paper-1");
  ProjectMemory.Papers.put("paper-1", "draft");
  return 0;
}
"##
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    let types = output.types.as_ref().expect("type output should exist");
    let checked = output
        .checked
        .as_ref()
        .expect("checked project should exist");
    let top_level_let = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::TopLevelLet(_) => Some(id),
            _ => None,
        })
        .expect("top-level let should lower");
    let fact = checked
        .top_level_lets
        .items
        .get(&top_level_let)
        .expect("top-level let facts should exist");
    assert_eq!(
        fact.classification,
        etas_hir::TopLevelLetClassification::ResourceHandle(etas_hir::ResourceKind::MemoryRegion,)
    );
    let get_call_ty = checked
        .hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            etas_hir::HirExpr::MethodCall { method, .. } if method == "get" => {
                types.facts.expr_types.get(&id).copied()
            }
            _ => None,
        })
        .expect("store get call should have a type fact");
    assert!(matches!(
        types.store.get(get_call_ty),
        Some(etas_types::Type::Option(inner))
            if matches!(
                types.store.get(*inner),
                Some(etas_types::Type::Primitive(etas_types::PrimitiveType::String))
            )
    ));
    let put_call_ty = checked
        .hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            etas_hir::HirExpr::MethodCall { method, .. } if method == "put" => {
                types.facts.expr_types.get(&id).copied()
            }
            _ => None,
        })
        .expect("store put call should have a type fact");
    assert!(matches!(
        types.store.get(put_call_ty),
        Some(etas_types::Type::Primitive(etas_types::PrimitiveType::Unit))
    ));
    let effects = output.effects.as_ref().expect("effect output should exist");
    let main_id = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if checked
                    .hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some(id)
            }
            _ => None,
        })
        .expect("main flow should lower");
    let summary = effects
        .facts
        .item_effects
        .get(&main_id)
        .expect("main effect summary should exist");
    assert!(
        summary.requested_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                etas_effects::Effect::AppliedAction(action)
                    if action.action.action == etas_effects::MEMORY_READ_ACTION
            )
        }),
        "{:?}",
        summary.requested_actions.effects.iter().collect::<Vec<_>>()
    );
    assert!(
        summary.requested_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                etas_effects::Effect::AppliedAction(action)
                    if action.action.action == etas_effects::MEMORY_WRITE_ACTION
            )
        }),
        "{:?}",
        summary.requested_actions.effects.iter().collect::<Vec<_>>()
    );
    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::RuntimeRequiredInPhase1)
            && diagnostic.severity == etas_core::Severity::Warning
    }));
}

#[test]
fn frontend_check_rejects_invalid_top_level_let_initializer() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: src_source_root(),
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(71),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"
module app.main;

flow helper() -> string {
  return "value";
}

let Bad = helper();

flow main() -> unit {
  return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Syntax(etas_core::SyntaxDiagnosticCode::InvalidItem)
            && diagnostic
                .message
                .contains("top-level `let` initializer must be a compile-time constant")
    }));
}

#[test]
fn frontend_check_accepts_parent_memory_region_effect_upper_bound() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: src_source_root(),
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(72),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r##"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>,
  Drafts: Store<string, string>,
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> unit ![Memory.read<ProjectMemory>]
{
  let existing = ProjectMemory.Papers.get("paper-1");
  return;
}
"##
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(!output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::EscapedEffect)
    }));
}

#[test]
fn frontend_check_rejects_narrow_memory_region_effect_upper_bound() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: src_source_root(),
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(73),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r##"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>,
  Drafts: Store<string, string>,
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> unit ![Memory.read<ProjectMemory.Drafts>]
{
  let existing = ProjectMemory.Papers.get("paper-1");
  return;
}
"##
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::EscapedEffect)
    }));
}

#[test]
fn frontend_check_lowers_command_sandbox_effect_args_as_static_resource_paths() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(74),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"
module app.main;
import std.host.command.{Command, CommandResult, run};

flow main(cmd: Command) -> CommandResult ![Command.run<DefaultCommandSandbox>]
{
  return run(cmd, DefaultCommandSandbox);
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(!output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::EscapedEffect)
    }));
    let checked = output
        .checked
        .as_ref()
        .expect("checked output should exist");
    let effects = output.effects.as_ref().expect("effect output should exist");
    let main_id = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if checked
                    .hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some(id)
            }
            _ => None,
        })
        .expect("main flow should lower");
    let summary = effects
        .facts
        .item_effects
        .get(&main_id)
        .expect("main effect summary should exist");
    assert!(
        summary.requested_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                etas_effects::Effect::AppliedAction(action)
                        if action.action.action == etas_effects::COMMAND_RUN_ACTION
                            && action.args == vec![
                                etas_types::EffectArgRef::Path(vec![
                                    "std".to_owned(),
                                    "host".to_owned(),
                                    "sandbox".to_owned(),
                                    "DefaultCommandSandbox".to_owned()
                                ])
                            ]
            )
        }),
        "{:?}",
        summary.requested_actions.effects.iter().collect::<Vec<_>>()
    );
}

#[test]
fn frontend_check_accepts_memory_selection_limit_chain() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(75),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r##"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>,
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> unit ![Memory.read<ProjectMemory>]
{
  let papers = ProjectMemory.Papers.select("topic").limit(Tokens(20_000));
  return;
}
"##
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(!output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::EscapedEffect)
    }));
    assert!(!output.diagnostics.iter().any(|diagnostic| {
        matches!(
            diagnostic.code,
            DiagnosticCode::Type(TypeDiagnosticCode::UnknownType)
                | DiagnosticCode::Type(TypeDiagnosticCode::NonCallableCallee)
        )
    }));
}

#[test]
fn frontend_check_accepts_prompt_builder_methods_before_readiness_classification() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: ProjectCompileOptions {
            entry_policy: EntryPolicy::Runnable,
        },
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(63),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"
module app.main;
import std.agent.prompt.Prompt;

agent Reviewer(input: Prompt) -> string {
  return input;
}

flow run_prompt(input: Prompt) -> string {
  return Reviewer.run(input);
}

flow main(args: Array<string>) -> i32 {
  let prompt = Prompt.new().user(Public("hello"));
  let result = run_prompt(prompt);
  if result == "" {
    return 1;
  }
  return 0;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.effects.is_some());
    assert!(output.checked.is_some());
    assert!(
        !output.diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic.code,
                DiagnosticCode::Syntax(_)
                    | DiagnosticCode::Name(_)
                    | DiagnosticCode::Type(TypeDiagnosticCode::UnknownType)
                    | DiagnosticCode::Type(TypeDiagnosticCode::NonCallableCallee)
            )
        }),
        "{:?}",
        output.diagnostics
    );
    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::RuntimeRequiredInPhase1)
            && diagnostic.severity == etas_core::Severity::Warning
    }));
}

#[test]
fn frontend_check_rejects_limit_budget_dimension_type_mismatch() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(72),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"
module app.main;

flow main(flag: bool) -> unit {
  while flag limit Cost(3), WallTime(3) {
    break;
  }
  return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    let mismatches = output
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
        })
        .count();
    assert_eq!(mismatches, 2, "{:?}", output.diagnostics);
}

#[test]
fn frontend_check_project_accepts_std_prelude_prompt_without_explicit_import() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: ProjectCompileOptions {
            entry_policy: EntryPolicy::Runnable,
        },
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(71),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"
module app.main;

flow BuildPrompt(input: string) -> Prompt {
  return Prompt.new().system(Trusted("review")).user(Public(input));
}

agent Reviewer(input: string) -> string ![] {
  return BuildPrompt(input);
}

flow run_prompt() -> string {
  return Reviewer.run("hello");
}

flow main(args: Array<string>) -> i32 {
  let result = run_prompt();
  if result == "" {
    return 1;
  }
  return 0;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(
        output
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("Prompt")),
        "{:?}",
        output.diagnostics
    );
    assert!(output.checked.is_some());
    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::RuntimeRequiredInPhase1)
            && diagnostic.severity == etas_core::Severity::Warning
    }));
}

#[test]
fn frontend_check_rejects_non_prompt_agent_body() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(60),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"
module app.main;

agent Writer(input: string) -> string {
  return 1;
}

flow main() -> unit {
  return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
    }));
    let hir = &output.hir.as_ref().expect("HIR output").hir;
    let agent = hir.items.iter().find_map(|(_, item)| match item {
        etas_hir::HirItem::Agent(agent) => Some(agent),
        _ => None,
    });
    let agent = agent.expect("agent item");
    let etas_hir::HirAgentBody::Source { block } = agent.body else {
        panic!("expected source-bodied agent");
    };
    assert!(hir.blocks.get(block).is_some());
}

#[test]
fn frontend_check_project_builds_project_level_artifacts() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(7),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"
module app.main;

flow main() -> i64 {
  return 1;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.checked.is_some());
    assert_eq!(output.parsed_sources.len(), 1);
    assert_eq!(output.sources.as_ref().expect("source set").files.len(), 1);
    let modules = output.modules.as_ref().expect("module index");
    assert_eq!(modules.modules.len(), 1);
    assert_eq!(modules.parts.len(), 1);
    assert_eq!(
        modules.modules.iter().next().unwrap().1.path.segments,
        ["app", "main"]
    );

    let units = output.units.as_ref().expect("unit tree");
    assert!(
        units
            .nodes
            .iter()
            .any(|(_, node)| node.kind == UnitKind::SourceFile)
    );
    assert!(
        units
            .nodes
            .iter()
            .any(|(_, node)| node.kind == UnitKind::Project)
    );
    assert!(
        units
            .nodes
            .iter()
            .any(|(_, node)| node.kind == UnitKind::ModulePart)
    );
    assert!(
        units
            .nodes
            .iter()
            .any(|(_, node)| node.kind == UnitKind::Body)
    );
    assert!(
        units
            .nodes
            .iter()
            .any(|(_, node)| node.kind == UnitKind::Block)
    );
    assert!(
        units
            .nodes
            .iter()
            .any(|(_, node)| node.kind == UnitKind::Expression)
    );
    let expression_parent = units
        .nodes
        .iter()
        .find_map(|(_, node)| (node.kind == UnitKind::Expression).then_some(node.parent))
        .flatten()
        .and_then(|parent| units.nodes.get(parent))
        .expect("expression parent");
    assert!(matches!(
        expression_parent.kind,
        UnitKind::Block | UnitKind::Body
    ));
    assert!(
        output
            .entry
            .as_ref()
            .is_some_and(|entry| entry.resolved.is_some())
    );
    let bindings = output
        .hir_item_bindings
        .as_ref()
        .expect("HIR item bindings");
    let ast_item = &modules.parts.iter().next().expect("module part").1.items[0];
    let hir_item = bindings
        .ast_to_hir
        .get(ast_item)
        .copied()
        .expect("AST item should map to HIR item");
    assert_eq!(bindings.hir_to_ast.get(&hir_item), Some(ast_item));
    let body_bindings = output
        .hir_body_bindings
        .as_ref()
        .expect("HIR body bindings");
    let ast_body = units
        .nodes
        .iter()
        .find_map(|(_, node)| match &node.target {
            etas_frontend::UnitTarget::AstBody(body) => Some(body.clone()),
            _ => None,
        })
        .expect("AST body unit");
    let root_block = body_bindings
        .ast_to_root_block
        .get(&ast_body)
        .copied()
        .expect("AST body should map to root HIR block");
    assert_eq!(
        body_bindings.root_block_to_ast.get(&root_block),
        Some(&ast_body)
    );
    let body_blocks = body_bindings
        .body_blocks
        .get(&ast_body)
        .expect("AST body block set");
    assert!(!body_blocks.is_empty());
    for block in body_blocks {
        assert_eq!(body_bindings.block_to_ast.get(block), Some(&ast_body));
    }
    let body_exprs = body_bindings
        .body_exprs
        .get(&ast_body)
        .expect("AST body expr set");
    assert!(!body_exprs.is_empty());
    for expr in body_exprs {
        assert_eq!(body_bindings.expr_to_ast.get(expr), Some(&ast_body));
    }
    let checked = output.checked.as_ref().expect("checked project");
    assert_eq!(checked.module_index.modules.len(), 1);
    assert_eq!(
        checked
            .entry_fact
            .requested
            .module
            .as_ref()
            .map(|path| &path.segments),
        Some(&vec!["app".to_owned(), "main".to_owned()])
    );
    assert!(checked.entry_fact.resolved.is_some());
}

#[test]
fn frontend_check_project_rejects_package_file_without_module_declaration() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(9),
            path: Some(std::path::PathBuf::from("src/main.es")),
            text: "flow main() -> unit { return; }".to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: None,
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("missing a `module` declaration")
    }));
}

#[test]
fn frontend_check_project_allows_module_and_entry_flow_to_share_name() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(216),
            path: Some(std::path::PathBuf::from("src/main.es")),
            text: "module main;\nflow main() -> unit { return; }".to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        output.diagnostics.iter().all(|diagnostic| {
            !diagnostic.message.contains("duplicate symbol `main`")
                && !diagnostic
                    .message
                    .contains("duplicate top-level declaration `main`")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn frontend_check_project_allows_support_module_and_flow_to_share_name() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(217),
            path: Some(std::path::PathBuf::from("src/support.es")),
            text: "module support;\npublic flow support() -> unit { return; }".to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["support".to_owned()],
            }),
            flow: "support".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
}

#[test]
fn frontend_check_project_still_rejects_duplicate_flows_with_entry_name() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(218),
            path: Some(std::path::PathBuf::from("src/main.es")),
            text: "module main;\nflow main() -> unit { return; }\nflow main() -> unit { return; }"
                .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("duplicate") && diagnostic.message.contains("main")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn frontend_check_project_resolves_configured_entry_module() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(39),
                path: Some(std::path::PathBuf::from("src/app/a.es")),
                text: "module app.a;\nflow main() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(40),
                path: Some(std::path::PathBuf::from("src/app/b.es")),
                text: "module app.b;\nflow main() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "b".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    let entry = output.entry.as_ref().expect("entry fact");
    let resolved = entry.resolved.as_ref().expect("resolved entry");
    let checked = output.checked.as_ref().expect("checked project");
    assert_eq!(checked.entry, Some(resolved.item));
    let hir = output.hir.as_ref().expect("HIR output");
    let module = hir
        .hir
        .modules_arena
        .get(resolved.module)
        .expect("entry module");
    let path = module.name.as_ref().expect("entry module path");
    let segments = path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(segments, ["app", "b"]);
}

#[test]
fn frontend_check_project_rejects_missing_configured_entry() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(41),
            path: Some(std::path::PathBuf::from("src/app/a.es")),
            text: "module app.a;\nflow helper() -> unit { return; }".to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(
        output
            .entry
            .as_ref()
            .is_some_and(|entry| entry.resolved.is_none())
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("entry flow `main` was not found")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn frontend_check_project_rejects_ambiguous_unqualified_entry() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(42),
                path: Some(std::path::PathBuf::from("src/app/a.es")),
                text: "module app.a;\nflow main() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(43),
                path: Some(std::path::PathBuf::from("src/app/b.es")),
                text: "module app.b;\nflow main() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: None,
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("entry flow `main` is ambiguous")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn frontend_check_project_lowers_each_module_part_into_project_hir() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(10),
                path: Some(std::path::PathBuf::from("src/app/main.es")),
                text: r#"
module app.main;

flow main() -> unit {
  return;
}
"#
                .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(11),
                path: Some(std::path::PathBuf::from("src/app/main/extra.es")),
                text: r#"
module app.main;

flow helper() -> unit {
  return;
}
"#
                .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let modules = output.modules.as_ref().expect("module index");
    assert_eq!(modules.modules.len(), 1);
    assert_eq!(modules.parts.len(), 2);
    let hir = output.hir.as_ref().expect("project HIR");
    let module = hir
        .hir
        .modules_arena
        .iter()
        .next()
        .map(|(_, module)| module)
        .expect("lowered HIR module");
    assert_eq!(module.items.len(), 2);
    let flow_names = hir
        .hir
        .items
        .iter()
        .filter_map(|(_, item)| match item {
            etas_hir::HirItem::Flow(flow) => hir
                .hir
                .symbols
                .get(flow.symbol)
                .map(|symbol| symbol.name.as_str()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert!(flow_names.contains("main"));
    assert!(flow_names.contains("helper"));
}

#[test]
fn frontend_check_project_rejects_module_path_mismatch() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: src_source_root(),
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(14),
            path: Some(std::path::PathBuf::from("src/app/wrong.es")),
            text: "module app.main;\nflow main() -> unit { return; }".to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("module declaration does not match source path")
    }));
}

#[test]
fn frontend_check_project_rejects_ambiguous_canonical_module_files() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: src_source_root(),
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(15),
                path: Some(std::path::PathBuf::from("src/app/main.es")),
                text: "module app.main;\nflow main() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(16),
                path: Some(std::path::PathBuf::from("src/app/main/mod.es")),
                text: "module app.main;\nflow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("ambiguous module files for `app.main`")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn frontend_check_project_reports_duplicate_top_level_names_across_module_parts() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: src_source_root(),
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(12),
                path: Some(std::path::PathBuf::from("src/app/main/a.es")),
                text: "module app.main;\nflow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(13),
                path: Some(std::path::PathBuf::from("src/app/main/b.es")),
                text: "module app.main;\nflow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "helper".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("duplicate top-level declaration `helper`")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn frontend_check_project_keeps_imports_local_to_each_module_part() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: src_source_root(),
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(72),
                path: Some(std::path::PathBuf::from("src/app/main/a.es")),
                text: "module app.main;\nimport app.util.{helper};\nflow use_helper() -> unit { helper(); return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(73),
                path: Some(std::path::PathBuf::from("src/app/main/b.es")),
                text: "module app.main;\nflow main() -> unit { helper(); return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(74),
                path: Some(std::path::PathBuf::from("src/app/util.es")),
                text: "module app.util;\npublic flow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("helper")
                && matches!(diagnostic.code, DiagnosticCode::Name(_))
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn frontend_check_project_builds_import_graph_and_topo_order() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: src_source_root(),
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(17),
                path: Some(std::path::PathBuf::from("src/app/a.es")),
                text: "module app.a;\nimport app.b;\nflow main() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(18),
                path: Some(std::path::PathBuf::from("src/app/b.es")),
                text: "module app.b;\nflow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    let modules = output.modules.as_ref().expect("module index");
    let graph = output.import_graph.as_ref().expect("import graph");
    assert_eq!(graph.edges.len(), 1);
    let app_b = modules
        .by_path
        .get(&ModulePath {
            segments: vec!["app".to_owned(), "b".to_owned()],
        })
        .copied()
        .expect("app.b module");
    let app_a = modules
        .by_path
        .get(&ModulePath {
            segments: vec!["app".to_owned(), "a".to_owned()],
        })
        .copied()
        .expect("app.a module");
    assert_eq!(graph.edges[0].resolved, Some(app_b));
    let topo = output
        .module_topo_order
        .as_ref()
        .expect("module topo order");
    let app_b_order = topo
        .modules
        .iter()
        .position(|module| *module == app_b)
        .expect("app.b in topo order");
    let app_a_order = topo
        .modules
        .iter()
        .position(|module| *module == app_a)
        .expect("app.a in topo order");
    assert!(app_b_order < app_a_order);

    let affected = output.affected_modules.as_ref().expect("affected modules");
    assert_eq!(affected.modules.len(), 2);
    assert_eq!(affected.module_parts.len(), 2);
    assert_eq!(affected.items.len(), 2);
    assert_eq!(affected.bodies.len(), 2);
}

#[test]
fn frontend_check_project_reports_import_cycles() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: src_source_root(),
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(19),
                path: Some(std::path::PathBuf::from("src/app/a.es")),
                text: "module app.a;\nimport app.b;\nflow main() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(20),
                path: Some(std::path::PathBuf::from("src/app/b.es")),
                text: "module app.b;\nimport app.a;\nflow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.message.contains("import cycle detected") })
    );
    assert!(
        output
            .module_topo_order
            .as_ref()
            .is_some_and(|order| !order.cyclic_modules.is_empty())
    );
}

#[test]
fn frontend_check_project_resolves_public_grouped_source_import() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: src_source_root(),
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(21),
                path: Some(std::path::PathBuf::from("src/app/a.es")),
                text: "module app.a;\nimport app.b.{helper};\nflow main() -> unit { return helper(); }"
                    .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(22),
                path: Some(std::path::PathBuf::from("src/app/b.es")),
                text: "module app.b;\npublic flow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    let resolved = output.resolved_imports.as_ref().expect("resolved imports");
    assert_eq!(resolved.imports.len(), 1);
    assert_eq!(resolved.imports[0].local_name, "helper");
    let resolved_paths = output.resolved_paths.as_ref().expect("resolved paths");
    assert!(resolved_paths.expr_paths.iter().any(|fact| matches!(
        &fact.result,
        HirPathResolution::ExplicitImport {
            target: ImportTarget::SourceItem { name, .. },
        } if name == "helper"
    )));
}

#[test]
fn frontend_check_project_accepts_imported_tool_in_agent_tools_config() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(214),
                path: Some(std::path::PathBuf::from("src/app/main.es")),
                text: r#"
module app.main;
import std.agent.prompt.Prompt;
import app.tools.{Search};

@tools([Search])
agent Reviewer(input: Prompt) -> string {
  return input;
}

flow main(input: Prompt) -> string {
  return Reviewer.run(input);
}
"#
                .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(215),
                path: Some(std::path::PathBuf::from("src/app/tools.es")),
                text: r#"
module app.tools;

public tool Search(input: string) -> string {
  return input;
}
"#
                .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        !output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
                && diagnostic.message.contains("tool declarations")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn frontend_check_project_propagates_source_import_flow_requirements() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(212),
                path: Some(std::path::PathBuf::from("src/app/a.es")),
                text: r#"
module app.a;
import app.b.{helper};

flow main(q: string) -> string ![Network] {
  return helper(q);
}
"#
                .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(213),
                path: Some(std::path::PathBuf::from("src/app/b.es")),
                text: r#"
module app.b;

		tool search(q: string) -> string ![Network] {
		  return q;
		}

public flow helper(q: string) -> string ![Network] {
  return search(q);
}
"#
                .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(!output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::EscapedEffect)
    }));

    let hir = &output.hir.as_ref().expect("hir output").hir;
    let main_id = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some(id)
            }
            _ => None,
        })
        .expect("main flow should lower");
    assert!(
        output
            .effects
            .as_ref()
            .expect("effect output")
            .facts
            .requirements
            .items
            .get(&main_id)
            .is_none_or(|requirements| requirements.iter().next().is_none()),
        "{:?}",
        output
            .effects
            .as_ref()
            .expect("effect output")
            .facts
            .requirements
    );
}

#[test]
fn frontend_check_project_records_partial_import_member_path_fact() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(210),
                path: Some(std::path::PathBuf::from("src/app/a.es")),
                text: "module app.a;\nimport app.b as b;\nflow main() -> unit { let value = b.helper; return; }"
                    .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(211),
                path: Some(std::path::PathBuf::from("src/app/b.es")),
                text: "module app.b;\npublic flow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    let resolved_paths = output.resolved_paths.as_ref().expect("resolved paths");
    assert!(resolved_paths.expr_paths.iter().any(|fact| matches!(
        &fact.result,
        HirPathResolution::PartialImport {
            target: ImportTarget::Module(target),
            resolved_segments,
            remaining,
            reason: etas_hir::PartialResolutionReason::ModuleMemberMissing,
        } if matches!(target, etas_frontend::ResolvedModuleTarget::Source { .. })
            && *resolved_segments == 1
            && remaining == &vec!["helper".to_owned()]
    )));
}

#[test]
fn frontend_check_project_rejects_private_source_import() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(23),
                path: Some(std::path::PathBuf::from("src/app/a.es")),
                text: "module app.a;\nimport app.b.{helper};\nflow main() -> unit { return; }"
                    .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(24),
                path: Some(std::path::PathBuf::from("src/app/b.es")),
                text: "module app.b;\nflow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("is private")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn frontend_check_project_rejects_missing_source_import_targets() {
    let frontend = Frontend;
    let missing_module = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(25),
            path: Some(std::path::PathBuf::from("src/app/a.es")),
            text: "module app.a;\nimport app.missing.{helper};\nflow main() -> unit { return; }"
                .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });
    assert!(missing_module.checked.is_none());
    assert!(missing_module.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("missing imported module `app.missing`")
    }));

    let missing_item = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(26),
                path: Some(std::path::PathBuf::from("src/app/a.es")),
                text: "module app.a;\nimport app.b.{missing};\nflow main() -> unit { return; }"
                    .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(27),
                path: Some(std::path::PathBuf::from("src/app/b.es")),
                text: "module app.b;\npublic flow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });
    assert!(missing_item.checked.is_none());
    assert!(missing_item.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("missing exported item `missing`")
    }));
}

#[test]
fn frontend_check_project_records_public_wildcard_re_exports() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(28),
                path: Some(std::path::PathBuf::from("src/app/a.es")),
                text: "module app.a;\npublic import app.b.*;\nflow main() -> unit { return; }"
                    .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(29),
                path: Some(std::path::PathBuf::from("src/app/b.es")),
                text: "module app.b;\npublic flow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    let resolved = output.resolved_imports.as_ref().expect("resolved imports");
    assert_eq!(resolved.wildcard_imports.len(), 1);
    assert_eq!(resolved.re_exports.len(), 1);
    assert!(
        resolved.wildcard_imports[0]
            .exported_names
            .iter()
            .any(|name| name == "helper")
    );
}

#[test]
fn frontend_check_project_resolves_source_wildcard_import_at_use_site() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(230),
                path: Some(std::path::PathBuf::from("src/app/a.es")),
                text: r#"
module app.a;
import app.b.*;

flow main() -> unit {
  return helper();
}
"#
                .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(231),
                path: Some(std::path::PathBuf::from("src/app/b.es")),
                text: "module app.b;\npublic flow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    let resolved = output.resolved_imports.as_ref().expect("resolved imports");
    assert_eq!(resolved.wildcard_imports.len(), 1);
    assert!(
        resolved.wildcard_imports[0]
            .exported_names
            .iter()
            .any(|name| name == "helper")
    );
    let resolved_paths = output.resolved_paths.as_ref().expect("resolved paths");
    assert!(resolved_paths.expr_paths.iter().any(|fact| matches!(
        &fact.result,
        HirPathResolution::WildcardImport { target }
            if matches!(target, etas_frontend::ResolvedModuleTarget::Source { .. })
    )));
}

#[test]
fn frontend_check_project_resolves_standard_library_grouped_imports() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(36),
            path: Some(std::path::PathBuf::from("src/app/a.es")),
            text: "module app.a;\nimport std.collections.{List, len};\nflow main() -> unit { return; }"
                .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    let resolved = output.resolved_imports.as_ref().expect("resolved imports");
    assert_eq!(resolved.imports.len(), 2);
    assert!(resolved.imports.iter().any(|import| {
        import.local_name == "List"
            && matches!(
                import.target,
                ImportTarget::StdItem {
                    ref module_path,
                    ref name,
                    ..
                } if module_path.segments == ["std", "collections"] && name == "List"
            )
    }));
    assert!(resolved.imports.iter().any(|import| {
        import.local_name == "len"
            && matches!(
                import.target,
                ImportTarget::StdItem {
                    ref module_path,
                    ref name,
                    ..
                } if module_path.segments == ["std", "collections"] && name == "len"
            )
    }));
}

#[test]
fn frontend_check_project_resolves_standard_io_and_effect_imports() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(48),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: "module app.main;\nimport std.io.{read_all, println};\nflow main() -> unit ![Error<IOError>] { println(read_all()); }"
                .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(
        output.diagnostics.iter().all(|diagnostic| !diagnostic
            .message
            .contains("missing exported item")
            && !diagnostic.message.contains("missing imported module")),
        "{:?}",
        output.diagnostics
    );
    let imports = output.resolved_imports.as_ref().expect("resolved imports");
    assert!(imports.imports.iter().any(|import| {
        matches!(
            &import.target,
            ImportTarget::StdItem { module_path, name, .. }
                if module_path.segments == ["std", "io"] && name == "read_all"
        )
    }));
    assert!(imports.imports.iter().any(|import| {
        matches!(
            &import.target,
            ImportTarget::StdItem { module_path, name, .. }
                if module_path.segments == ["std", "io"] && name == "println"
        )
    }));
}

#[test]
fn frontend_check_project_rejects_missing_standard_library_item() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(37),
            path: Some(std::path::PathBuf::from("src/app/a.es")),
            text:
                "module app.a;\nimport std.collections.{missing};\nflow main() -> unit { return; }"
                    .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("missing exported item `missing` in module `std.collections`")
    }));
}

#[test]
fn frontend_check_project_records_standard_wildcard_re_exports() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(38),
            path: Some(std::path::PathBuf::from("src/app/a.es")),
            text:
                "module app.a;\npublic import std.collections.*;\nflow main() -> unit { return; }"
                    .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    let resolved = output.resolved_imports.as_ref().expect("resolved imports");
    assert_eq!(resolved.wildcard_imports.len(), 1);
    assert_eq!(resolved.re_exports.len(), 1);
    assert_eq!(
        resolved.re_exports[0].target.segments,
        ["std", "collections"]
    );
    assert!(
        resolved.wildcard_imports[0]
            .exported_names
            .iter()
            .any(|name| name == "List")
    );
    assert!(
        resolved.wildcard_imports[0]
            .exported_names
            .iter()
            .any(|name| name == "len")
    );
}

#[test]
fn frontend_check_project_resolves_external_grouped_imports() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: external_environment(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(41),
            path: Some(std::path::PathBuf::from("src/app/a.es")),
            text: "module app.a;\nimport dep.math.{Number, add};\nflow main() -> unit { return; }"
                .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    let resolved = output.resolved_imports.as_ref().expect("resolved imports");
    assert_eq!(resolved.imports.len(), 2);
    assert!(resolved.imports.iter().any(|import| {
        import.local_name == "Number"
            && matches!(
                import.target,
                ImportTarget::ExternalItem {
                    module: ExternalModuleId(1),
                    ref module_path,
                    ref name,
                    symbol: ExternalSymbolId(0),
                    ..
                } if module_path.segments == ["dep", "math"] && name == "Number"
            )
    }));
    assert!(resolved.imports.iter().any(|import| {
        import.local_name == "add"
            && matches!(
                import.target,
                ImportTarget::ExternalItem {
                    module: ExternalModuleId(1),
                    ref module_path,
                    ref name,
                    symbol: ExternalSymbolId(1),
                    ..
                } if module_path.segments == ["dep", "math"] && name == "add"
            )
    }));
}

#[test]
fn frontend_check_project_uses_external_public_flow_metadata_for_type_checking() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: external_environment(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(43),
            path: Some(std::path::PathBuf::from("src/app/a.es")),
            text: r#"module app.a;

import dep.math.{add};

flow main() -> i32 {
    return add(1, 2);
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        output.diagnostics
    );
    let types = output.types.as_ref().expect("type output should exist");
    assert!(
        types.facts.symbol_types.values().any(|fact| matches!(
            fact,
            SymbolTypeFact::Flow { signature }
                if signature.params.len() == 2
                    && signature
                        .params
                        .iter()
                        .all(|ty| matches!(
                            types.store.get(*ty),
                            Some(Type::Primitive(PrimitiveType::I32))
                        ))
                    && matches!(
                        types.store.get(signature.output),
                        Some(Type::Primitive(PrimitiveType::I32))
                    )
        )),
        "external add import should be bridged as a typed flow signature"
    );
}

#[test]
fn frontend_check_project_uses_external_trace_spec_metadata_for_conformance() {
    let mut environment = ProjectEnvironmentInput {
        external_packages: vec![runtime_dep_package()],
        external_modules: vec![ProjectExternalModuleInput {
            package: Some(ExternalPackageId(0)),
            id: ExternalModuleId(1),
            path: ModulePath {
                segments: vec!["dep".to_owned(), "guard".to_owned()],
            },
            exports: vec![ProjectExternalExportInput {
                symbol: ExternalSymbolId(0),
                name: "Safe".to_owned(),
                visibility: etas_hir::Visibility::Public,
            }],
        }],
        external_public_metadata: vec![empty_external_public_metadata(ExternalPackageId(0))],
        ..ProjectEnvironmentInput::default()
    };
    environment.external_public_metadata[0].trace_specs =
        vec![ProjectExternalNamedSignatureInput {
            path: vec!["dep".to_owned(), "guard".to_owned(), "Safe".to_owned()],
            visibility: "public".to_owned(),
            ty: None,
        }];
    environment.external_public_metadata[0].spec_signatures =
        vec![ProjectExternalSpecSignatureInput {
            path: vec!["dep".to_owned(), "guard".to_owned(), "Safe".to_owned()],
            visibility: "public".to_owned(),
            kind: ProjectExternalSpecKindInput::Trace,
            param_names: Vec::new(),
            callable: None,
            methods: Vec::new(),
            super_specs: Vec::new(),
        }];
    environment.external_public_metadata[0].trace_spec_summaries =
        vec![ProjectExternalTraceSpecSummaryInput {
            trace_spec: vec!["dep".to_owned(), "guard".to_owned(), "Safe".to_owned()],
            clauses: vec![ProjectExternalTraceSpecClauseInput {
                kind: ProjectExternalTraceSpecClauseKindInput::Allow,
                pattern: Some(ProjectExternalEffectRowInput {
                    effects: vec![ProjectExternalEffectRefInput {
                        path: vec!["Console".to_owned(), "stdout_write".to_owned()],
                        args: Vec::new(),
                    }],
                }),
                guard: None,
                target: None,
                obligation: None,
            }],
        }];

    let output = Frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/external-trace-spec"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput::anonymous(
            r#"
module app.main;
import dep.guard.Safe;

flow main() -> unit ~ Safe {
    return;
}
"#,
        )],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:#?}",
        output.diagnostics
    );
    let checked = output.checked.expect("project should check");
    assert_eq!(
        checked.types.trace_spec_conformances.len(),
        1,
        "external trace spec conformance should be recorded as a type fact"
    );
}

#[test]
fn frontend_check_project_applies_external_trace_spec_metadata_monitor() {
    let mut environment = ProjectEnvironmentInput {
        external_packages: vec![runtime_dep_package()],
        external_modules: vec![ProjectExternalModuleInput {
            package: Some(ExternalPackageId(0)),
            id: ExternalModuleId(1),
            path: ModulePath {
                segments: vec!["dep".to_owned(), "guard".to_owned()],
            },
            exports: vec![ProjectExternalExportInput {
                symbol: ExternalSymbolId(0),
                name: "Safe".to_owned(),
                visibility: etas_hir::Visibility::Public,
            }],
        }],
        external_public_metadata: vec![empty_external_public_metadata(ExternalPackageId(0))],
        ..ProjectEnvironmentInput::default()
    };
    environment.external_public_metadata[0].trace_specs =
        vec![ProjectExternalNamedSignatureInput {
            path: vec!["dep".to_owned(), "guard".to_owned(), "Safe".to_owned()],
            visibility: "public".to_owned(),
            ty: None,
        }];
    environment.external_public_metadata[0].spec_signatures =
        vec![ProjectExternalSpecSignatureInput {
            path: vec!["dep".to_owned(), "guard".to_owned(), "Safe".to_owned()],
            visibility: "public".to_owned(),
            kind: ProjectExternalSpecKindInput::Trace,
            param_names: Vec::new(),
            callable: None,
            methods: Vec::new(),
            super_specs: Vec::new(),
        }];
    environment.external_public_metadata[0].trace_spec_summaries =
        vec![ProjectExternalTraceSpecSummaryInput {
            trace_spec: vec!["dep".to_owned(), "guard".to_owned(), "Safe".to_owned()],
            clauses: vec![ProjectExternalTraceSpecClauseInput {
                kind: ProjectExternalTraceSpecClauseKindInput::Deny,
                pattern: Some(ProjectExternalEffectRowInput {
                    effects: vec![ProjectExternalEffectRefInput {
                        path: vec!["Console".to_owned(), "stdout_write".to_owned()],
                        args: Vec::new(),
                    }],
                }),
                guard: None,
                target: None,
                obligation: None,
            }],
        }];

    let output = Frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/external-trace-spec-deny"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput::anonymous(
            r#"
module app.main;
import std.io.println;
import dep.guard.Safe;

flow main() -> unit ![Error<IOError>] ~ Safe {
    println("x");
    return;
}
"#,
        )],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecDenied)
        }),
        "external trace spec deny clause should reject println requested action: {:#?}",
        output.diagnostics
    );
}

#[test]
fn frontend_check_project_replays_external_spec_conformance_metadata() {
    let mut environment = ProjectEnvironmentInput {
        external_packages: vec![runtime_dep_package()],
        external_modules: vec![ProjectExternalModuleInput {
            package: Some(ExternalPackageId(0)),
            id: ExternalModuleId(1),
            path: ModulePath {
                segments: vec!["dep".to_owned(), "api".to_owned()],
            },
            exports: vec![
                ProjectExternalExportInput {
                    symbol: ExternalSymbolId(0),
                    name: "call".to_owned(),
                    visibility: etas_hir::Visibility::Public,
                },
                ProjectExternalExportInput {
                    symbol: ExternalSymbolId(1),
                    name: "Pure".to_owned(),
                    visibility: etas_hir::Visibility::Public,
                },
                ProjectExternalExportInput {
                    symbol: ExternalSymbolId(2),
                    name: "Safe".to_owned(),
                    visibility: etas_hir::Visibility::Public,
                },
            ],
        }],
        external_public_metadata: vec![empty_external_public_metadata(ExternalPackageId(0))],
        ..ProjectEnvironmentInput::default()
    };
    environment.external_public_metadata[0].flows = vec![ProjectExternalFlowSignatureInput {
        path: vec!["dep".to_owned(), "api".to_owned(), "call".to_owned()],
        param_names: Vec::new(),
        params: Vec::new(),
        output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
        effects: None,
        visibility: "public".to_owned(),
    }];
    environment.external_public_metadata[0].spec_signatures = vec![
        ProjectExternalSpecSignatureInput {
            path: vec!["dep".to_owned(), "api".to_owned(), "Pure".to_owned()],
            visibility: "public".to_owned(),
            kind: ProjectExternalSpecKindInput::Callable,
            param_names: Vec::new(),
            callable: Some(ProjectExternalFlowSignatureInput {
                path: vec!["dep".to_owned(), "api".to_owned(), "Pure".to_owned()],
                param_names: Vec::new(),
                params: Vec::new(),
                output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
                effects: None,
                visibility: "public".to_owned(),
            }),
            methods: Vec::new(),
            super_specs: Vec::new(),
        },
        ProjectExternalSpecSignatureInput {
            path: vec!["dep".to_owned(), "api".to_owned(), "Safe".to_owned()],
            visibility: "public".to_owned(),
            kind: ProjectExternalSpecKindInput::Trace,
            param_names: Vec::new(),
            callable: None,
            methods: Vec::new(),
            super_specs: Vec::new(),
        },
    ];
    environment.external_public_metadata[0].callable_spec_satisfactions =
        vec![ProjectExternalCallableSpecSatisfactionInput {
            item: vec!["dep".to_owned(), "api".to_owned(), "call".to_owned()],
            spec: vec!["dep".to_owned(), "api".to_owned(), "Pure".to_owned()],
            args: Vec::new(),
        }];
    environment.external_public_metadata[0].trace_spec_conformances =
        vec![ProjectExternalTraceSpecConformanceInput {
            item: vec!["dep".to_owned(), "api".to_owned(), "call".to_owned()],
            target: ProjectExternalTraceSpecConformanceTargetInput::Named {
                spec: vec!["dep".to_owned(), "api".to_owned(), "Safe".to_owned()],
                args: Vec::new(),
            },
        }];
    environment.external_public_metadata[0].trace_spec_summaries =
        vec![ProjectExternalTraceSpecSummaryInput {
            trace_spec: vec!["dep".to_owned(), "api".to_owned(), "Safe".to_owned()],
            clauses: vec![ProjectExternalTraceSpecClauseInput {
                kind: ProjectExternalTraceSpecClauseKindInput::Allow,
                pattern: Some(ProjectExternalEffectRowInput {
                    effects: vec![ProjectExternalEffectRefInput {
                        path: vec!["Console".to_owned(), "stdout_write".to_owned()],
                        args: Vec::new(),
                    }],
                }),
                guard: None,
                target: None,
                obligation: None,
            }],
        }];
    environment.external_public_metadata[0].effect_summaries =
        vec![ProjectExternalEffectSummaryInput {
            item: vec!["dep".to_owned(), "api".to_owned(), "call".to_owned()],
            public_effects: ProjectExternalEffectRowInput::default(),
            requested_actions: ProjectExternalEffectRowInput::default(),
            handled_requested_actions: ProjectExternalEffectRowInput::default(),
            latent_flows: Vec::new(),
        }];

    let output = Frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/external-spec-conformance"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput::anonymous(
            r#"
module app.main;
import dep.api.{call, Pure, Safe};

flow main() -> unit {
    return;
}
"#,
        )],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:#?}",
        output.diagnostics
    );
    let checked = output.checked.expect("project should check");
    assert!(
        checked
            .types
            .external_callable_spec_satisfactions
            .iter()
            .any(|fact| path_eq(&fact.item, &["dep", "api", "call"])
                && path_eq(&fact.spec, &["dep", "api", "Pure"])),
        "external callable conformance metadata should be replayed as a path fact: {:#?}",
        checked.types.external_callable_spec_satisfactions
    );
    assert!(
        checked
            .types
            .external_trace_spec_conformances
            .iter()
            .any(|fact| match &fact.target {
                etas_types::ExternalTraceSpecConformanceTarget::Named { spec, .. } => {
                    path_eq(&fact.item, &["dep", "api", "call"])
                        && path_eq(spec, &["dep", "api", "Safe"])
                }
                etas_types::ExternalTraceSpecConformanceTarget::Inline => false,
            }),
        "external trace conformance metadata should be replayed as a path fact: {:#?}",
        checked.types.external_trace_spec_conformances
    );
}

#[test]
fn frontend_check_project_validates_external_trace_spec_conformance_metadata() {
    let mut environment = ProjectEnvironmentInput {
        external_packages: vec![runtime_dep_package()],
        external_modules: vec![ProjectExternalModuleInput {
            package: Some(ExternalPackageId(0)),
            id: ExternalModuleId(1),
            path: ModulePath {
                segments: vec!["dep".to_owned(), "api".to_owned()],
            },
            exports: vec![
                ProjectExternalExportInput {
                    symbol: ExternalSymbolId(0),
                    name: "call".to_owned(),
                    visibility: etas_hir::Visibility::Public,
                },
                ProjectExternalExportInput {
                    symbol: ExternalSymbolId(1),
                    name: "Safe".to_owned(),
                    visibility: etas_hir::Visibility::Public,
                },
            ],
        }],
        external_public_metadata: vec![empty_external_public_metadata(ExternalPackageId(0))],
        ..ProjectEnvironmentInput::default()
    };
    environment.external_public_metadata[0].flows = vec![ProjectExternalFlowSignatureInput {
        path: vec!["dep".to_owned(), "api".to_owned(), "call".to_owned()],
        param_names: Vec::new(),
        params: Vec::new(),
        output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
        effects: None,
        visibility: "public".to_owned(),
    }];
    environment.external_public_metadata[0].spec_signatures =
        vec![ProjectExternalSpecSignatureInput {
            path: vec!["dep".to_owned(), "api".to_owned(), "Safe".to_owned()],
            visibility: "public".to_owned(),
            kind: ProjectExternalSpecKindInput::Trace,
            param_names: Vec::new(),
            callable: None,
            methods: Vec::new(),
            super_specs: Vec::new(),
        }];
    environment.external_public_metadata[0].trace_spec_summaries =
        vec![ProjectExternalTraceSpecSummaryInput {
            trace_spec: vec!["dep".to_owned(), "api".to_owned(), "Safe".to_owned()],
            clauses: vec![ProjectExternalTraceSpecClauseInput {
                kind: ProjectExternalTraceSpecClauseKindInput::Deny,
                pattern: Some(ProjectExternalEffectRowInput {
                    effects: vec![ProjectExternalEffectRefInput {
                        path: vec!["Console".to_owned(), "stdout_write".to_owned()],
                        args: Vec::new(),
                    }],
                }),
                guard: None,
                target: None,
                obligation: None,
            }],
        }];
    environment.external_public_metadata[0].trace_spec_conformances =
        vec![ProjectExternalTraceSpecConformanceInput {
            item: vec!["dep".to_owned(), "api".to_owned(), "call".to_owned()],
            target: ProjectExternalTraceSpecConformanceTargetInput::Named {
                spec: vec!["dep".to_owned(), "api".to_owned(), "Safe".to_owned()],
                args: Vec::new(),
            },
        }];
    environment.external_public_metadata[0].effect_summaries =
        vec![ProjectExternalEffectSummaryInput {
            item: vec!["dep".to_owned(), "api".to_owned(), "call".to_owned()],
            public_effects: ProjectExternalEffectRowInput::default(),
            requested_actions: external_effect_row(vec![external_effect(
                &["Console", "stdout_write"],
                Vec::new(),
            )]),
            handled_requested_actions: ProjectExternalEffectRowInput::default(),
            latent_flows: Vec::new(),
        }];

    let output = Frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/external-trace-conformance-deny"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput::anonymous(
            r#"
module app.main;
import dep.api.call;

flow main() -> unit {
    call();
    return;
}
"#,
        )],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecDenied)),
        "external trace conformance should validate external solved summary metadata: {:#?}",
        output.diagnostics
    );
}

#[test]
fn frontend_check_project_rejects_effectful_external_flow_without_summary_metadata() {
    let frontend = Frontend;
    let mut environment = external_environment();
    environment.external_modules[0]
        .exports
        .push(ProjectExternalExportInput {
            symbol: ExternalSymbolId(2),
            name: "fetch".to_owned(),
            visibility: etas_hir::Visibility::Public,
        });
    environment.external_public_metadata[0]
        .flows
        .push(ProjectExternalFlowSignatureInput {
            path: vec!["dep".to_owned(), "math".to_owned(), "fetch".to_owned()],
            param_names: Vec::new(),
            params: Vec::new(),
            output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
            effects: Some(external_network_effect_row()),
            visibility: "public".to_owned(),
        });

    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput {
            id: etas_core::SourceId(58),
            path: Some(std::path::PathBuf::from("src/app/a.es")),
            text: r#"module app.a;

import dep.math.{fetch};

flow main() -> unit ![Network] {
    fetch();
    return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic
                    .message
                    .contains("does not provide a solved effect summary")
        }),
        "effectful external callable without solved summary must fail closed: {:?}",
        output.diagnostics
    );
}

#[test]
fn frontend_check_project_specializes_external_summary_parameter_actions() {
    let frontend = Frontend;
    let mut environment = external_environment();
    environment.external_modules[0]
        .exports
        .push(ProjectExternalExportInput {
            symbol: ExternalSymbolId(2),
            name: "request".to_owned(),
            visibility: etas_hir::Visibility::Public,
        });
    environment.external_public_metadata[0]
        .effects
        .push(ProjectExternalNamedSignatureInput {
            path: vec!["dep".to_owned(), "math".to_owned(), "Transport".to_owned()],
            visibility: "public".to_owned(),
            ty: None,
        });
    environment.external_public_metadata[0]
        .actions
        .push(ProjectExternalActionSignatureInput {
            path: vec![
                "dep".to_owned(),
                "math".to_owned(),
                "Transport".to_owned(),
                "request".to_owned(),
            ],
            params: vec![
                ProjectExternalTypeInput::Primitive("string".to_owned()),
                ProjectExternalTypeInput::Primitive("string".to_owned()),
            ],
            effect_args: vec![
                ProjectExternalActionArgKindInput::StringPattern,
                ProjectExternalActionArgKindInput::StringPattern,
            ],
            selector_param_names: vec!["method".to_owned(), "host".to_owned()],
            selector_defaults: vec![None, None],
            output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
            returns_never: false,
            visibility: "public".to_owned(),
        });
    environment.external_public_metadata[0]
        .flows
        .push(ProjectExternalFlowSignatureInput {
            path: vec!["dep".to_owned(), "math".to_owned(), "request".to_owned()],
            param_names: vec!["method".to_owned(), "host".to_owned()],
            params: vec![
                ProjectExternalTypeInput::Primitive("string".to_owned()),
                ProjectExternalTypeInput::Primitive("string".to_owned()),
            ],
            output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
            effects: None,
            visibility: "public".to_owned(),
        });
    environment.external_public_metadata[0]
        .effect_summaries
        .push(ProjectExternalEffectSummaryInput {
            item: vec!["dep".to_owned(), "math".to_owned(), "request".to_owned()],
            public_effects: ProjectExternalEffectRowInput::default(),
            requested_actions: external_effect_row(vec![external_effect(
                &["dep", "math", "Transport", "request"],
                vec![
                    external_effect_path_arg(&["method"]),
                    external_effect_path_arg(&["host"]),
                ],
            )]),
            handled_requested_actions: ProjectExternalEffectRowInput::default(),
            latent_flows: Vec::new(),
        });

    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput {
            id: etas_core::SourceId(59),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"module app.main;

import dep.math.request;

flow main() -> unit {
    request("GET", "example.test");
    return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        output.diagnostics
    );
    let checked = output.checked.as_ref().expect("checked output");
    let effects = output.effects.as_ref().expect("effect output");
    let main = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow) => checked
                .hir
                .symbols
                .get(flow.symbol)
                .is_some_and(|symbol| symbol.name == "main")
                .then_some(id),
            _ => None,
        })
        .expect("main flow should exist");
    let summary = effects
        .facts
        .item_effects
        .get(&main)
        .expect("main effect summary should exist");
    assert!(
        summary
            .requested_actions
            .effects
            .iter()
            .any(|effect| matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.args
                        == vec![
                            etas_types::EffectArgRef::String("GET".to_owned()),
                            etas_types::EffectArgRef::String("example.test".to_owned()),
                        ]
            )),
        "{summary:?}"
    );
}

#[test]
fn frontend_check_project_replays_external_action_selector_defaults_from_metadata() {
    let frontend = Frontend;
    let mut environment = external_environment();
    environment.external_modules[0]
        .exports
        .push(ProjectExternalExportInput {
            symbol: ExternalSymbolId(2),
            name: "default_request".to_owned(),
            visibility: etas_hir::Visibility::Public,
        });
    environment.external_public_metadata[0]
        .effects
        .push(ProjectExternalNamedSignatureInput {
            path: vec!["dep".to_owned(), "math".to_owned(), "Transport".to_owned()],
            visibility: "public".to_owned(),
            ty: None,
        });
    environment.external_public_metadata[0]
        .actions
        .push(ProjectExternalActionSignatureInput {
            path: vec![
                "dep".to_owned(),
                "math".to_owned(),
                "Transport".to_owned(),
                "request".to_owned(),
            ],
            params: Vec::new(),
            effect_args: vec![
                ProjectExternalActionArgKindInput::StringPattern,
                ProjectExternalActionArgKindInput::Type,
            ],
            selector_param_names: vec!["method".to_owned(), "resource".to_owned()],
            selector_defaults: vec![
                Some(ProjectExternalEffectArgInput::String("GET".to_owned())),
                Some(ProjectExternalEffectArgInput::Type(
                    ProjectExternalTypeInput::Array(Box::new(ProjectExternalTypeInput::Primitive(
                        "i32".to_owned(),
                    ))),
                )),
            ],
            output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
            returns_never: false,
            visibility: "public".to_owned(),
        });
    environment.external_public_metadata[0]
        .flows
        .push(ProjectExternalFlowSignatureInput {
            path: vec![
                "dep".to_owned(),
                "math".to_owned(),
                "default_request".to_owned(),
            ],
            param_names: Vec::new(),
            params: Vec::new(),
            output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
            effects: None,
            visibility: "public".to_owned(),
        });
    environment.external_public_metadata[0]
        .effect_summaries
        .push(ProjectExternalEffectSummaryInput {
            item: vec![
                "dep".to_owned(),
                "math".to_owned(),
                "default_request".to_owned(),
            ],
            public_effects: ProjectExternalEffectRowInput::default(),
            requested_actions: external_effect_row(vec![external_effect(
                &["dep", "math", "Transport", "request"],
                Vec::new(),
            )]),
            handled_requested_actions: ProjectExternalEffectRowInput::default(),
            latent_flows: Vec::new(),
        });

    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput {
            id: etas_core::SourceId(60),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"module app.main;

import dep.math.default_request;

flow main() -> unit {
    default_request();
    return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        output.diagnostics
    );
    let checked = output.checked.as_ref().expect("checked output");
    let effects = output.effects.as_ref().expect("effect output");
    let main = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow) => checked
                .hir
                .symbols
                .get(flow.symbol)
                .is_some_and(|symbol| symbol.name == "main")
                .then_some(id),
            _ => None,
        })
        .expect("main flow should exist");
    let summary = effects
        .facts
        .item_effects
        .get(&main)
        .expect("main effect summary should exist");
    let action = summary
        .requested_actions
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::AppliedAction(action) => Some(action),
            _ => None,
        })
        .expect("external requested action");
    let [
        etas_types::EffectArgRef::String(method),
        etas_types::EffectArgRef::Type(resource),
    ] = action.args.as_slice()
    else {
        panic!("typed selector defaults must not be widened: {action:?}");
    };
    assert_eq!(method, "GET");
    let types = output.types.as_ref().expect("type output");
    let Some(Type::Array(element)) = types.store.get(*resource) else {
        panic!("type selector must preserve Array<i32>: {resource:?}");
    };
    assert_eq!(
        types.store.get(*element),
        Some(&Type::Primitive(PrimitiveType::I32))
    );
}

#[test]
fn frontend_check_project_propagates_external_function_parameter_effects() {
    let frontend = Frontend;
    let mut environment = external_environment();
    environment.external_modules[0]
        .exports
        .push(ProjectExternalExportInput {
            symbol: ExternalSymbolId(2),
            name: "run".to_owned(),
            visibility: etas_hir::Visibility::Public,
        });
    environment.external_public_metadata[0]
        .flows
        .push(ProjectExternalFlowSignatureInput {
            path: vec!["dep".to_owned(), "math".to_owned(), "run".to_owned()],
            param_names: vec!["callback".to_owned()],
            params: vec![ProjectExternalTypeInput::Function {
                input: Vec::new(),
                output: Box::new(ProjectExternalTypeInput::Primitive("unit".to_owned())),
                effects: Some(external_network_effect_row()),
            }],
            output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
            effects: None,
            visibility: "public".to_owned(),
        });
    environment.external_public_metadata[0]
        .effect_summaries
        .push(ProjectExternalEffectSummaryInput {
            item: vec!["dep".to_owned(), "math".to_owned(), "run".to_owned()],
            public_effects: ProjectExternalEffectRowInput::default(),
            requested_actions: ProjectExternalEffectRowInput::default(),
            handled_requested_actions: ProjectExternalEffectRowInput::default(),
            latent_flows: Vec::new(),
        });

    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput {
            id: etas_core::SourceId(51),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"module app.main;

import dep.math.run;

flow main() -> unit ![Network] {
    run(() => {
        return;
    });
    return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        output.diagnostics
    );
    let checked = output.checked.as_ref().expect("checked output");
    let effects = output.effects.as_ref().expect("effect output");
    let main_id = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if checked
                    .hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some(id)
            }
            _ => None,
        })
        .expect("main flow should lower");
    let summary = effects
        .facts
        .item_effects
        .get(&main_id)
        .expect("main effect summary");
    assert!(
        summary
            .escaping_effects
            .effects
            .contains(&etas_effects::Effect::Tag(etas_effects::NETWORK_TAG)),
        "external function parameter row must propagate into call summary: {summary:?}"
    );
}

#[test]
fn frontend_check_project_realizes_external_callback_latent_effects() {
    let frontend = Frontend;
    let mut environment = external_environment();
    environment.external_modules[0]
        .exports
        .push(ProjectExternalExportInput {
            symbol: ExternalSymbolId(3),
            name: "run_callback".to_owned(),
            visibility: etas_hir::Visibility::Public,
        });
    environment.external_public_metadata[0]
        .flows
        .push(ProjectExternalFlowSignatureInput {
            path: vec![
                "dep".to_owned(),
                "math".to_owned(),
                "run_callback".to_owned(),
            ],
            param_names: vec!["callback".to_owned()],
            params: vec![ProjectExternalTypeInput::Function {
                input: Vec::new(),
                output: Box::new(ProjectExternalTypeInput::Primitive("unit".to_owned())),
                effects: None,
            }],
            output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
            effects: None,
            visibility: "public".to_owned(),
        });
    environment.external_public_metadata[0]
        .effect_summaries
        .push(ProjectExternalEffectSummaryInput {
            item: vec![
                "dep".to_owned(),
                "math".to_owned(),
                "run_callback".to_owned(),
            ],
            public_effects: ProjectExternalEffectRowInput::default(),
            requested_actions: ProjectExternalEffectRowInput::default(),
            handled_requested_actions: ProjectExternalEffectRowInput::default(),
            latent_flows: Vec::new(),
        });

    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput {
            id: etas_core::SourceId(53),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"module app.main;

import dep.math.run_callback;
import std.io.println;

flow main() -> unit ![Error<IOError>] {
    run_callback(() => {
        println("callback");
        return;
    });
    return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        output.diagnostics
    );
    let checked = output.checked.as_ref().expect("checked output");
    let effects = output.effects.as_ref().expect("effect output");
    let main_id = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if checked
                    .hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some(id)
            }
            _ => None,
        })
        .expect("main flow should lower");
    let summary = effects
        .facts
        .item_effects
        .get(&main_id)
        .expect("main effect summary");
    assert!(
        summary
            .escaping_effects
            .effects
            .iter()
            .any(|effect| matches!(effect, etas_effects::Effect::Error(_))),
        "external higher-order call must realize the callback latent effect: {summary:?}"
    );
}

#[test]
fn frontend_check_project_does_not_realize_unbound_external_latent_flow_metadata() {
    let frontend = Frontend;
    let mut environment = external_environment();
    environment.external_modules[0]
        .exports
        .push(ProjectExternalExportInput {
            symbol: ExternalSymbolId(3),
            name: "run_latent".to_owned(),
            visibility: etas_hir::Visibility::Public,
        });
    environment.external_public_metadata[0]
        .flows
        .push(ProjectExternalFlowSignatureInput {
            path: vec!["dep".to_owned(), "math".to_owned(), "run_latent".to_owned()],
            param_names: Vec::new(),
            params: Vec::new(),
            output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
            effects: None,
            visibility: "public".to_owned(),
        });
    environment.external_public_metadata[0]
        .effect_summaries
        .push(ProjectExternalEffectSummaryInput {
            item: vec!["dep".to_owned(), "math".to_owned(), "run_latent".to_owned()],
            public_effects: ProjectExternalEffectRowInput::default(),
            requested_actions: ProjectExternalEffectRowInput::default(),
            handled_requested_actions: ProjectExternalEffectRowInput::default(),
            latent_flows: vec![ProjectExternalLatentFlowSummaryInput {
                declared_bound: ProjectExternalEffectRowInput::default(),
                inferred_effects: external_effect_row(vec![external_effect(
                    &["Network"],
                    Vec::new(),
                )]),
            }],
        });

    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput {
            id: etas_core::SourceId(52),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"module app.main;

import dep.math.run_latent;

flow main() -> unit ![] {
    run_latent();
    return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        output.diagnostics
    );
    let checked = output.checked.as_ref().expect("checked output");
    let effects = output.effects.as_ref().expect("effect output");
    let main_id = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if checked
                    .hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some(id)
            }
            _ => None,
        })
        .expect("main flow should lower");
    let summary = effects
        .facts
        .item_effects
        .get(&main_id)
        .expect("main effect summary");
    assert!(
        !summary
            .escaping_effects
            .effects
            .contains(&etas_effects::Effect::Tag(etas_effects::NETWORK_TAG)),
        "unbound external latent metadata must not be realized on every external call; function-typed parameters carry callback effects: {summary:?}"
    );
}

#[test]
fn frontend_check_project_propagates_external_typed_error_metadata_by_full_path() {
    let frontend = Frontend;
    let mut environment = external_environment();
    environment.external_modules[0].path = ModulePath {
        segments: vec!["dep".to_owned(), "http".to_owned()],
    };
    environment.external_modules[0].exports = vec![
        ProjectExternalExportInput {
            symbol: ExternalSymbolId(0),
            name: "HttpError".to_owned(),
            visibility: etas_hir::Visibility::Public,
        },
        ProjectExternalExportInput {
            symbol: ExternalSymbolId(1),
            name: "fail".to_owned(),
            visibility: etas_hir::Visibility::Public,
        },
    ];
    environment.external_public_metadata[0].types = vec![ProjectExternalNamedSignatureInput {
        path: vec!["dep".to_owned(), "http".to_owned(), "HttpError".to_owned()],
        visibility: "public".to_owned(),
        ty: Some(ProjectExternalTypeInput::Record {
            fields: vec![ProjectExternalRecordFieldInput {
                name: "code".to_owned(),
                ty: ProjectExternalTypeInput::Primitive("i32".to_owned()),
            }],
        }),
    }];
    environment.external_public_metadata[0].flows = vec![ProjectExternalFlowSignatureInput {
        path: vec!["dep".to_owned(), "http".to_owned(), "fail".to_owned()],
        param_names: Vec::new(),
        params: Vec::new(),
        output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
        effects: Some(external_error_effect_row(vec![
            "dep".to_owned(),
            "http".to_owned(),
            "HttpError".to_owned(),
        ])),
        visibility: "public".to_owned(),
    }];
    environment.external_public_metadata[0].effect_summaries =
        vec![ProjectExternalEffectSummaryInput {
            item: vec!["dep".to_owned(), "http".to_owned(), "fail".to_owned()],
            public_effects: external_effect_row(vec![external_effect(
                &["Error"],
                vec![external_effect_path_arg(&["dep", "http", "HttpError"])],
            )]),
            requested_actions: ProjectExternalEffectRowInput::default(),
            handled_requested_actions: ProjectExternalEffectRowInput::default(),
            latent_flows: Vec::new(),
        }];

    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput {
            id: etas_core::SourceId(53),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"module app.main;

import dep.http.{fail, HttpError};

flow main() -> unit ![Error<HttpError>] {
    fail();
    return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        output.diagnostics
    );
    let checked = output.checked.as_ref().expect("checked output");
    let types = output.types.as_ref().expect("type output");
    let http_error_ty = imported_type_id_for_path(checked, types, &["dep", "http", "HttpError"])
        .expect("HttpError import should have a canonical type fact");
    let effects = output.effects.as_ref().expect("effect output");
    let main_id = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if checked
                    .hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some(id)
            }
            _ => None,
        })
        .expect("main flow should lower");
    let summary = effects
        .facts
        .item_effects
        .get(&main_id)
        .expect("main effect summary");
    assert!(
        summary.escaping_effects.effects.iter().any(|effect| {
            matches!(
                effect,
                etas_effects::Effect::Error(error) if *error == http_error_ty
            )
        }),
        "external typed error should use the canonical external type id: {summary:?}"
    );
}

#[test]
fn frontend_check_project_handles_external_typed_error_metadata_by_full_path() {
    let frontend = Frontend;
    let mut environment = external_environment();
    environment.external_modules[0].path = ModulePath {
        segments: vec!["dep".to_owned(), "http".to_owned()],
    };
    environment.external_modules[0].exports = vec![
        ProjectExternalExportInput {
            symbol: ExternalSymbolId(0),
            name: "HttpError".to_owned(),
            visibility: etas_hir::Visibility::Public,
        },
        ProjectExternalExportInput {
            symbol: ExternalSymbolId(1),
            name: "fail".to_owned(),
            visibility: etas_hir::Visibility::Public,
        },
    ];
    environment.external_public_metadata[0].types = vec![ProjectExternalNamedSignatureInput {
        path: vec!["dep".to_owned(), "http".to_owned(), "HttpError".to_owned()],
        visibility: "public".to_owned(),
        ty: Some(ProjectExternalTypeInput::Record {
            fields: vec![ProjectExternalRecordFieldInput {
                name: "code".to_owned(),
                ty: ProjectExternalTypeInput::Primitive("i32".to_owned()),
            }],
        }),
    }];
    environment.external_public_metadata[0].flows = vec![ProjectExternalFlowSignatureInput {
        path: vec!["dep".to_owned(), "http".to_owned(), "fail".to_owned()],
        param_names: Vec::new(),
        params: Vec::new(),
        output: ProjectExternalTypeInput::Primitive("string".to_owned()),
        effects: Some(external_error_effect_row(vec![
            "dep".to_owned(),
            "http".to_owned(),
            "HttpError".to_owned(),
        ])),
        visibility: "public".to_owned(),
    }];
    environment.external_public_metadata[0].effect_summaries =
        vec![ProjectExternalEffectSummaryInput {
            item: vec!["dep".to_owned(), "http".to_owned(), "fail".to_owned()],
            public_effects: external_effect_row(vec![external_effect(
                &["Error"],
                vec![external_effect_path_arg(&["dep", "http", "HttpError"])],
            )]),
            requested_actions: ProjectExternalEffectRowInput::default(),
            handled_requested_actions: ProjectExternalEffectRowInput::default(),
            latent_flows: Vec::new(),
        }];

    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput {
            id: etas_core::SourceId(54),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"module app.main;

import dep.http.{fail, HttpError};

flow main() -> string ![] {
    return fail() with {
        Error<HttpError>.raise(err) => {
            finish "recovered";
        }
    };
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        output.diagnostics
    );
    let checked = output.checked.as_ref().expect("checked output");
    let types = output.types.as_ref().expect("type output");
    let http_error_ty = imported_type_id_for_path(checked, types, &["dep", "http", "HttpError"])
        .expect("HttpError import should have a canonical type fact");
    let effects = output.effects.as_ref().expect("effect output");
    let main_id = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if checked
                    .hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some(id)
            }
            _ => None,
        })
        .expect("main flow should lower");
    let summary = effects
        .facts
        .item_effects
        .get(&main_id)
        .expect("main effect summary");
    assert!(
        !summary.escaping_effects.effects.iter().any(|effect| {
            matches!(
                effect,
                etas_effects::Effect::Error(error) if *error == http_error_ty
            )
        }),
        "handler should eliminate external package-defined typed Error<HttpError>: {summary:?}"
    );
}

#[test]
fn frontend_check_project_rejects_short_external_typed_error_metadata() {
    let frontend = Frontend;
    let mut environment = external_environment();
    environment.external_modules[0].path = ModulePath {
        segments: vec!["dep".to_owned(), "http".to_owned()],
    };
    environment.external_modules[0].exports = vec![
        ProjectExternalExportInput {
            symbol: ExternalSymbolId(0),
            name: "HttpError".to_owned(),
            visibility: etas_hir::Visibility::Public,
        },
        ProjectExternalExportInput {
            symbol: ExternalSymbolId(1),
            name: "fail".to_owned(),
            visibility: etas_hir::Visibility::Public,
        },
    ];
    environment.external_public_metadata[0].types = vec![ProjectExternalNamedSignatureInput {
        path: vec!["dep".to_owned(), "http".to_owned(), "HttpError".to_owned()],
        visibility: "public".to_owned(),
        ty: None,
    }];
    environment.external_public_metadata[0].flows = vec![ProjectExternalFlowSignatureInput {
        path: vec!["dep".to_owned(), "http".to_owned(), "fail".to_owned()],
        param_names: Vec::new(),
        params: Vec::new(),
        output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
        effects: Some(external_error_effect_row(vec![
            "dep".to_owned(),
            "http".to_owned(),
            "HttpError".to_owned(),
        ])),
        visibility: "public".to_owned(),
    }];
    environment.external_public_metadata[0].effect_summaries =
        vec![ProjectExternalEffectSummaryInput {
            item: vec!["dep".to_owned(), "http".to_owned(), "fail".to_owned()],
            public_effects: external_effect_row(vec![external_effect(
                &["Error"],
                vec![external_effect_path_arg(&["HttpError"])],
            )]),
            requested_actions: ProjectExternalEffectRowInput::default(),
            handled_requested_actions: ProjectExternalEffectRowInput::default(),
            latent_flows: Vec::new(),
        }];

    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput {
            id: etas_core::SourceId(54),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"module app.main;

import dep.http.{fail, HttpError};

flow main() -> unit ![Error<HttpError>] {
    fail();
    return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic.code,
                DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
            ) && diagnostic
                .message
                .contains("source import target requires checked effect facts")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn frontend_package_metadata_preserves_scoped_custom_requested_action_args() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/edk-http"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(55),
            path: Some(std::path::PathBuf::from("src/edk/http/client.es")),
            text: r#"module edk.http.client;

public type HttpRequest = {
    id: i32,
};

public type HttpResponse = {
    status: i32,
};

public effect EdkHttp extends Network {
    action request<Scope>(method: string, host: string, request: HttpRequest) -> HttpResponse;
}

public flow request(method: string, host: string, request: HttpRequest) -> HttpResponse ![EdkHttp.request<_>] {
    return perform EdkHttp.request(method, host, request);
}

flow main() -> unit {
    return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["edk".to_owned(), "http".to_owned(), "client".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        output.diagnostics
    );
    let checked = output.checked.as_ref().expect("checked project");
    let artifact = build_package_metadata_artifact_from_checked(
        PackageMetadataBuildInput {
            package_id: "edk-http".to_owned(),
            package_version: "0.1.0".to_owned(),
            package_edition: "2026".to_owned(),
            compiler_version: "test".to_owned(),
            source_payload_hash: "blake3:test-source".to_owned(),
            manifest_hash: "blake3:test-manifest".to_owned(),
            dependency_lock_hash: "blake3:test-lock".to_owned(),
            bins: Vec::new(),
            dependencies: Vec::new(),
            tool_bindings: Vec::new(),
        },
        checked,
    )
    .expect("package metadata should emit");
    let (_, metadata) =
        package_metadata_from_artifact(std::path::Path::new("package.etasmeta"), &artifact.bytes)
            .expect("package metadata should decode");
    let summary = metadata
        .public_metadata
        .effect_summaries
        .iter()
        .find(|summary| {
            summary.item
                == vec![
                    "edk".to_owned(),
                    "http".to_owned(),
                    "client".to_owned(),
                    "request".to_owned(),
                ]
        })
        .expect("request effect summary should be published");
    let flow_signature = metadata
        .public_metadata
        .flows
        .iter()
        .find(|signature| signature.path == summary.item)
        .expect("request flow signature should be published");
    assert_eq!(
        flow_signature.param_names,
        vec!["method".to_owned(), "host".to_owned(), "request".to_owned()]
    );
    assert!(
        summary
            .requested_actions
            .effects
            .iter()
            .any(
                |action| action.path == ["edk", "http", "client", "EdkHttp", "request"]
                    && action.args.iter().any(|arg| matches!(
                        arg.kind,
                        etas_package_metadata::EffectArgKind::Wildcard
                    ))
            ),
        "package metadata must preserve explicit static action selector scope: {summary:?}"
    );
    assert!(
        !summary
            .requested_actions
            .effects
            .iter()
            .any(
                |action| action.path == ["edk", "http", "client", "EdkHttp", "request"]
                    && action
                        .args
                        .iter()
                        .any(|arg| arg.path == ["method"] || arg.path == ["host"])
            ),
        "package metadata must not derive static action selectors from runtime payload parameter names: {summary:?}"
    );
    assert!(
        metadata.effect_metadata.extensions.iter().any(|extension| {
            extension.child
                == vec![
                    "edk".to_owned(),
                    "http".to_owned(),
                    "client".to_owned(),
                    "EdkHttp".to_owned(),
                ]
                && extension
                    .parent
                    .last()
                    .is_some_and(|segment| segment == "Network")
        }),
        "package metadata must publish source effect extensions: {:?}",
        metadata.effect_metadata.extensions
    );
}

#[test]
fn frontend_package_metadata_marks_scoped_handler_requested_actions_as_handled() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/edk-http"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(57),
            path: Some(std::path::PathBuf::from("src/edk/http/client.es")),
            text: r#"module edk.http.client;

public effect LowTransport {
    action tcp() -> i32;
}

public effect EdkHttp {
    action request() -> i32;
}

public let EdkHttpDefault: ![EdkHttp => LowTransport for i32] = handler {
    EdkHttp.request() => {
        let value = perform LowTransport.tcp();
        resume value;
    }
};

flow raw_request() -> i32 ![EdkHttp.request] {
    return perform EdkHttp.request();
}

public flow request() -> i32 ![LowTransport] {
    return raw_request() with EdkHttpDefault;
}

flow main() -> unit {
    return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["edk".to_owned(), "http".to_owned(), "client".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        output.diagnostics
    );
    let checked = output.checked.as_ref().expect("checked project");
    let artifact = build_package_metadata_artifact_from_checked(
        PackageMetadataBuildInput {
            package_id: "edk-http".to_owned(),
            package_version: "0.1.0".to_owned(),
            package_edition: "2026".to_owned(),
            compiler_version: "test".to_owned(),
            source_payload_hash: "blake3:test-source".to_owned(),
            manifest_hash: "blake3:test-manifest".to_owned(),
            dependency_lock_hash: "blake3:test-lock".to_owned(),
            bins: Vec::new(),
            dependencies: Vec::new(),
            tool_bindings: Vec::new(),
        },
        checked,
    )
    .expect("package metadata should emit");
    let (_, metadata) =
        package_metadata_from_artifact(std::path::Path::new("package.etasmeta"), &artifact.bytes)
            .expect("package metadata should decode");
    let summary = metadata
        .public_metadata
        .effect_summaries
        .iter()
        .find(|summary| {
            summary.item
                == vec![
                    "edk".to_owned(),
                    "http".to_owned(),
                    "client".to_owned(),
                    "request".to_owned(),
                ]
        })
        .expect("request effect summary should be published");

    assert!(
        summary
            .requested_actions
            .effects
            .iter()
            .any(|action| action.path == ["edk", "http", "client", "EdkHttp", "request"]),
        "public wrapper metadata must retain the handled high-level request action: {summary:?}"
    );
    assert!(
        summary
            .requested_actions
            .effects
            .iter()
            .any(|action| action.path == ["edk", "http", "client", "LowTransport", "tcp"]),
        "public wrapper metadata must retain handler body transport actions: {summary:?}"
    );
    assert!(
        summary
            .handled_requested_actions
            .effects
            .iter()
            .any(|action| action.path == ["edk", "http", "client", "EdkHttp", "request"]),
        "public wrapper metadata must mark the high-level request action as handled: {summary:?}"
    );
    assert!(
        !summary
            .handled_requested_actions
            .effects
            .iter()
            .any(|action| action.path == ["edk", "http", "client", "LowTransport", "tcp"]),
        "handler body transport action should remain residual, not handled: {summary:?}"
    );
}

#[test]
fn frontend_package_metadata_publishes_item_annotations() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/annotated-agent"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(58),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"module app.main;

@derive([Schema, PromptEncode])
@doc("review output")
public type Review = {
    summary: string,
};

public tool Search(q: string) -> string {
    return q;
}

@model(adapter = "omlx-openai", model = "local")
@tools([Search])
@limits([Tokens(128)])
@trace(VirtualStages([Logical, Search]))
public agent Reviewer(input: string) -> string {
    return abort("not implemented");
}

flow main() -> unit {
    return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        output.diagnostics
    );
    let artifact = build_package_metadata_artifact_from_checked(
        PackageMetadataBuildInput {
            package_id: "annotated-agent".to_owned(),
            package_version: "0.1.0".to_owned(),
            package_edition: "2026".to_owned(),
            compiler_version: "test".to_owned(),
            source_payload_hash: "blake3:test-source".to_owned(),
            manifest_hash: "blake3:test-manifest".to_owned(),
            dependency_lock_hash: "blake3:test-lock".to_owned(),
            bins: Vec::new(),
            dependencies: Vec::new(),
            tool_bindings: Vec::new(),
        },
        output.checked.as_ref().expect("checked project"),
    )
    .expect("package metadata should emit");
    let (_, metadata) =
        package_metadata_from_artifact(std::path::Path::new("package.etasmeta"), &artifact.bytes)
            .expect("package metadata should decode");

    let review_item = vec!["app".to_owned(), "main".to_owned(), "Review".to_owned()];
    let reviewer_item = vec!["app".to_owned(), "main".to_owned(), "Reviewer".to_owned()];
    assert!(
        metadata
            .public_metadata
            .annotations
            .iter()
            .any(|annotation| {
                annotation.item == review_item
                    && annotation.path == vec!["derive".to_owned()]
                    && annotation
                        .args
                        .first()
                        .is_some_and(|arg| arg.value.elements.len() == 2)
            }),
        "derive annotation should be published: {:?}",
        metadata.public_metadata.annotations
    );
    assert!(
        metadata
            .public_metadata
            .annotations
            .iter()
            .any(|annotation| {
                annotation.item == review_item
                    && annotation.path == vec!["doc".to_owned()]
                    && annotation
                        .args
                        .first()
                        .is_some_and(|arg| arg.value.value == "review output")
            }),
        "user-defined annotation should be published as metadata: {:?}",
        metadata.public_metadata.annotations
    );
    assert!(
        metadata
            .public_metadata
            .annotations
            .iter()
            .any(|annotation| {
                annotation.item == reviewer_item
                    && annotation.path == vec!["model".to_owned()]
                    && annotation
                        .args
                        .iter()
                        .any(|arg| arg.name == "adapter" && arg.value.value == "omlx-openai")
            }),
        "model annotation should be published: {:?}",
        metadata.public_metadata.annotations
    );
    assert!(
        metadata
            .public_metadata
            .annotations
            .iter()
            .any(|annotation| {
                annotation.item == reviewer_item
                    && annotation.path == vec!["tools".to_owned()]
                    && annotation.args.first().is_some_and(|arg| {
                        arg.value.elements.first().is_some_and(|value| {
                            value.path
                                == vec!["app".to_owned(), "main".to_owned(), "Search".to_owned()]
                        })
                    })
            }),
        "tools annotation should preserve canonical tool path: {:?}",
        metadata.public_metadata.annotations
    );
    assert!(
        metadata
            .public_metadata
            .annotations
            .iter()
            .any(|annotation| {
                annotation.item == reviewer_item
                    && annotation.path == vec!["limits".to_owned()]
                    && annotation.args.first().is_some_and(|arg| {
                        arg.value
                            .elements
                            .first()
                            .is_some_and(|value| value.value == "Tokens")
                    })
            }),
        "limits annotation should preserve static limit constructor: {:?}",
        metadata.public_metadata.annotations
    );
    assert!(
        metadata
            .public_metadata
            .annotations
            .iter()
            .any(|annotation| {
                annotation.item == reviewer_item
                    && annotation.path == vec!["trace".to_owned()]
                    && annotation.args.first().is_some_and(|arg| {
                        arg.value.kind == AnnotationValueKind::Constructor
                            && arg.value.value == "VirtualStages"
                            && arg.value.path
                                == vec![
                                    "std".to_owned(),
                                    "runtime".to_owned(),
                                    "trace".to_owned(),
                                    "VirtualStages".to_owned(),
                                ]
                    })
            }),
        "trace annotation should preserve static trace constructor: {:?}",
        metadata.public_metadata.annotations
    );
}

#[test]
fn frontend_package_metadata_publishes_public_trace_spec() {
    let output = Frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/trace-spec-package"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(59),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"module app.main;

public spec Safe: trace = +Console.stdout_write;

public flow main() -> unit ~ Safe {
    return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        output.diagnostics
    );

    let artifact = build_package_metadata_artifact_from_checked(
        PackageMetadataBuildInput {
            package_id: "trace-spec-package".to_owned(),
            package_version: "0.1.0".to_owned(),
            package_edition: "2026".to_owned(),
            compiler_version: "test".to_owned(),
            source_payload_hash: "blake3:test-source".to_owned(),
            manifest_hash: "blake3:test-manifest".to_owned(),
            dependency_lock_hash: "blake3:test-lock".to_owned(),
            bins: Vec::new(),
            dependencies: Vec::new(),
            tool_bindings: Vec::new(),
        },
        output.checked.as_ref().expect("checked project"),
    )
    .expect("package metadata should emit");
    let (_, metadata) =
        package_metadata_from_artifact(std::path::Path::new("package.etasmeta"), &artifact.bytes)
            .expect("package metadata should decode");

    assert!(
        metadata
            .public_metadata
            .trace_specs
            .iter()
            .any(|signature| signature.path == ["app", "main", "Safe"]),
        "public trace spec should be exported in package metadata: {:?}",
        metadata.public_metadata.trace_specs
    );
    assert!(
        metadata
            .public_metadata
            .spec_signatures
            .iter()
            .any(|signature| {
                signature.path == ["app", "main", "Safe"]
                    && signature.kind == etas_package_metadata::SpecKind::Trace
            }),
        "public trace spec should export a structured spec signature: {:?}",
        metadata.public_metadata.spec_signatures
    );
    let summary = metadata
        .public_metadata
        .trace_spec_summaries
        .iter()
        .find(|summary| summary.trace_spec == ["app", "main", "Safe"])
        .expect("public trace spec should export structured trace clauses");
    assert_eq!(summary.clauses.len(), 1, "{summary:?}");
    assert!(summary.clauses[0].pattern.is_some(), "{summary:?}");
    assert!(
        metadata
            .public_metadata
            .trace_spec_conformances
            .iter()
            .any(|fact| {
                fact.item == ["app", "main", "main"]
                    && matches!(
                        &fact.target,
                        etas_package_metadata::TraceSpecConformanceTarget::Named { spec, .. }
                            if spec == &vec![
                                "app".to_owned(),
                                "main".to_owned(),
                                "Safe".to_owned()
                            ]
                    )
            }),
        "public flow trace conformance should be exported as metadata: {:?}",
        metadata.public_metadata.trace_spec_conformances
    );
}

#[test]
fn frontend_package_metadata_publishes_public_callable_spec_conformance() {
    let output = Frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/callable-spec-package"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(60),
            path: Some(std::path::PathBuf::from("src/app/main.es")),
            text: r#"module app.main;

public spec Pure<I, O> I => O ![];

public flow main() -> unit ~ Pure<unit, unit> {
    return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        output.diagnostics
    );

    let artifact = build_package_metadata_artifact_from_checked(
        PackageMetadataBuildInput {
            package_id: "callable-spec-package".to_owned(),
            package_version: "0.1.0".to_owned(),
            package_edition: "2026".to_owned(),
            compiler_version: "test".to_owned(),
            source_payload_hash: "blake3:test-source".to_owned(),
            manifest_hash: "blake3:test-manifest".to_owned(),
            dependency_lock_hash: "blake3:test-lock".to_owned(),
            bins: Vec::new(),
            dependencies: Vec::new(),
            tool_bindings: Vec::new(),
        },
        output.checked.as_ref().expect("checked project"),
    )
    .expect("package metadata should emit");
    let (_, metadata) =
        package_metadata_from_artifact(std::path::Path::new("package.etasmeta"), &artifact.bytes)
            .expect("package metadata should decode");

    assert!(
        metadata
            .public_metadata
            .spec_signatures
            .iter()
            .any(|signature| {
                signature.path == ["app", "main", "Pure"]
                    && signature.kind == etas_package_metadata::SpecKind::Callable
            }),
        "public callable spec should export a structured spec signature: {:?}",
        metadata.public_metadata.spec_signatures
    );
    assert!(
        metadata
            .public_metadata
            .callable_spec_satisfactions
            .iter()
            .any(|fact| {
                fact.item == ["app", "main", "main"]
                    && fact.spec == ["app", "main", "Pure"]
                    && fact.args.len() == 2
            }),
        "public flow callable conformance should be exported as metadata: {:?}",
        metadata.public_metadata.callable_spec_satisfactions
    );
}

#[test]
fn frontend_package_metadata_publishes_public_tool_schema() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/edk-search"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(56),
            path: Some(std::path::PathBuf::from("src/edk/search/tools.es")),
            text: r#"module edk.search.tools;

public type SearchQuery = {
    query: string,
    limit: i32,
};

public tool Search(input: SearchQuery) -> string {
    return input.query;
}

flow main() -> unit {
    return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["edk".to_owned(), "search".to_owned(), "tools".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        output.diagnostics
    );
    let artifact = build_package_metadata_artifact_from_checked(
        PackageMetadataBuildInput {
            package_id: "edk-search".to_owned(),
            package_version: "0.1.0".to_owned(),
            package_edition: "2026".to_owned(),
            compiler_version: "test".to_owned(),
            source_payload_hash: "blake3:test-source".to_owned(),
            manifest_hash: "blake3:test-manifest".to_owned(),
            dependency_lock_hash: "blake3:test-lock".to_owned(),
            bins: Vec::new(),
            dependencies: Vec::new(),
            tool_bindings: Vec::new(),
        },
        output.checked.as_ref().expect("checked project"),
    )
    .expect("package metadata should emit");
    let (_, metadata) =
        package_metadata_from_artifact(std::path::Path::new("package.etasmeta"), &artifact.bytes)
            .expect("package metadata should decode");
    let schema = metadata
        .public_metadata
        .tool_schemas
        .iter()
        .find(|schema| {
            schema.tool
                == vec![
                    "edk".to_owned(),
                    "search".to_owned(),
                    "tools".to_owned(),
                    "Search".to_owned(),
                ]
        })
        .expect("public tool schema should be published");
    let value: serde_json::Value =
        serde_json::from_str(&schema.schema_json).expect("schema should be valid JSON");
    assert_eq!(value["type"], "object");
    assert_eq!(value["properties"]["input"]["type"], "object");
    assert_eq!(
        value["properties"]["input"]["properties"]["query"]["type"],
        "string"
    );
    assert_eq!(
        value["properties"]["input"]["properties"]["limit"]["type"],
        "integer"
    );
}

#[test]
fn frontend_package_metadata_preserves_alias_and_nominal_type_kinds() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/identity-types"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(58),
            path: Some(std::path::PathBuf::from("src/identity/types.es")),
            text: r#"module identity.types;

public alias PublicPath = string;
public type UserId = string;
public type OpaqueHandle;
public type ReportsRoot;
public type WorkspacePath<R> = string;
public alias DefaultReportPath = WorkspacePath<ReportsRoot>;

flow main() -> unit {
    return;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["identity".to_owned(), "types".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        output.diagnostics
    );
    let artifact = build_package_metadata_artifact_from_checked(
        PackageMetadataBuildInput {
            package_id: "identity-types".to_owned(),
            package_version: "0.1.0".to_owned(),
            package_edition: "2026".to_owned(),
            compiler_version: "test".to_owned(),
            source_payload_hash: "blake3:test-source".to_owned(),
            manifest_hash: "blake3:test-manifest".to_owned(),
            dependency_lock_hash: "blake3:test-lock".to_owned(),
            bins: Vec::new(),
            dependencies: Vec::new(),
            tool_bindings: Vec::new(),
        },
        output.checked.as_ref().expect("checked project"),
    )
    .expect("package metadata should emit");
    let (_, metadata) =
        package_metadata_from_artifact(std::path::Path::new("package.etasmeta"), &artifact.bytes)
            .expect("package metadata should decode");
    let public_path = metadata
        .public_metadata
        .types
        .iter()
        .find(|signature| {
            signature.path
                == vec![
                    "identity".to_owned(),
                    "types".to_owned(),
                    "PublicPath".to_owned(),
                ]
        })
        .and_then(|signature| signature.ty.as_ref())
        .expect("public alias type should be published");
    assert_eq!(public_path.kind, etas_package_metadata::TypeKind::Alias);
    assert_eq!(
        public_path.path,
        vec![
            "identity".to_owned(),
            "types".to_owned(),
            "PublicPath".to_owned()
        ]
    );
    assert_eq!(public_path.children.len(), 1);
    assert_eq!(
        public_path.children[0].kind,
        etas_package_metadata::TypeKind::Primitive
    );
    assert_eq!(public_path.children[0].name, "string");

    let user_id = metadata
        .public_metadata
        .types
        .iter()
        .find(|signature| {
            signature.path
                == vec![
                    "identity".to_owned(),
                    "types".to_owned(),
                    "UserId".to_owned(),
                ]
        })
        .and_then(|signature| signature.ty.as_ref())
        .expect("public nominal type should be published");
    assert_eq!(user_id.kind, etas_package_metadata::TypeKind::Nominal);
    assert_eq!(
        user_id.path,
        vec![
            "identity".to_owned(),
            "types".to_owned(),
            "UserId".to_owned()
        ]
    );
    assert_eq!(user_id.children.len(), 1);
    assert_eq!(
        user_id.children[0].kind,
        etas_package_metadata::TypeKind::Primitive
    );
    assert_eq!(user_id.children[0].name, "string");

    let opaque = metadata
        .public_metadata
        .types
        .iter()
        .find(|signature| {
            signature.path
                == vec![
                    "identity".to_owned(),
                    "types".to_owned(),
                    "OpaqueHandle".to_owned(),
                ]
        })
        .and_then(|signature| signature.ty.as_ref())
        .expect("bodyless nominal type should be published");
    assert_eq!(opaque.kind, etas_package_metadata::TypeKind::Nominal);
    assert!(opaque.children.is_empty());

    let report_path = metadata
        .public_metadata
        .types
        .iter()
        .find(|signature| {
            signature.path
                == vec![
                    "identity".to_owned(),
                    "types".to_owned(),
                    "DefaultReportPath".to_owned(),
                ]
        })
        .and_then(|signature| signature.ty.as_ref())
        .expect("applied alias should be published");
    assert_eq!(report_path.kind, etas_package_metadata::TypeKind::Alias);
    assert_eq!(report_path.children.len(), 1);
    let applied = &report_path.children[0];
    assert_eq!(applied.kind, etas_package_metadata::TypeKind::Applied);
    assert_eq!(
        applied.path,
        vec![
            "identity".to_owned(),
            "types".to_owned(),
            "WorkspacePath".to_owned()
        ]
    );
    assert_eq!(applied.children.len(), 1);
    assert_eq!(
        applied.children[0].kind,
        etas_package_metadata::TypeKind::Nominal
    );
    assert_eq!(
        applied.children[0].path,
        vec![
            "identity".to_owned(),
            "types".to_owned(),
            "ReportsRoot".to_owned()
        ]
    );
}

#[test]
fn frontend_check_project_uses_external_public_type_contract_for_record_construction() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: external_environment(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(45),
            path: Some(std::path::PathBuf::from("src/app/a.es")),
            text: r#"module app.a;

import dep.math.{Number};

flow main() -> i32 {
    let n = Number { value = 7 };
    return n.value;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn frontend_check_project_uses_external_public_type_contract_inside_local_alias() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: external_environment(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(46),
            path: Some(std::path::PathBuf::from("src/app/a.es")),
            text: r#"module app.a;

import dep.math.Number;

type Wrapped = {
    n: Number,
};

flow main() -> i32 {
    let w = Wrapped { n = Number { value = 7 } };
    return w.n.value;
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn frontend_check_project_preserves_external_type_contract_through_source_imported_alias() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: external_environment(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(47),
                path: Some(std::path::PathBuf::from("src/app/types.es")),
                text: r#"module app.types;

import dep.math.Number;

public type Wrapped = {
    n: Number,
};
"#
                .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(48),
                path: Some(std::path::PathBuf::from("src/app/main.es")),
                text: r#"module app.main;

import app.types.{Wrapped};
import dep.math.{Number};

flow main() -> i32 {
    let w = Wrapped { n = Number { value = 7 } };
    return w.n.value;
}
"#
                .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn frontend_check_project_uses_multi_segment_external_import_root_type_contract() {
    let frontend = Frontend;
    let mut environment = external_environment();
    environment.external_packages[0].import_root = "dep.math".to_owned();
    environment.external_modules[0].path = ModulePath {
        segments: vec!["dep".to_owned(), "math".to_owned(), "types".to_owned()],
    };
    environment.external_public_metadata[0].types[0].path = vec![
        "dep".to_owned(),
        "math".to_owned(),
        "types".to_owned(),
        "Number".to_owned(),
    ];
    environment.external_public_metadata[0].flows.clear();
    environment.external_public_metadata[0]
        .effect_summaries
        .clear();

    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(49),
                path: Some(std::path::PathBuf::from("src/app/types.es")),
                text: r#"module app.types;

import dep.math.types.Number;

public type Wrapped = {
    n: Number,
};
"#
                .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(50),
                path: Some(std::path::PathBuf::from("src/app/main.es")),
                text: r#"module app.main;

import app.types.Wrapped;
import dep.math.types.Number;

flow main() -> i32 {
    let w = Wrapped { n = Number { value = 7 } };
    return w.n.value;
}
"#
                .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn frontend_check_project_reports_invalid_external_public_metadata() {
    let frontend = Frontend;
    let mut environment = external_environment();
    environment.external_public_metadata[0].flows[0].params[0] =
        ProjectExternalTypeInput::Primitive("not_a_primitive".to_owned());
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput {
            id: etas_core::SourceId(44),
            path: Some(std::path::PathBuf::from("src/app/a.es")),
            text: r#"module app.a;

import dep.math.{add};

flow main() -> i32 {
    return add(1, 2);
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(output.diagnostics.iter().any(|diagnostic| {
        matches!(
            diagnostic.code,
            DiagnosticCode::Type(TypeDiagnosticCode::IncompleteTypeFacts)
        ) && diagnostic
            .message
            .contains("invalid external package metadata")
    }));
}

#[test]
fn frontend_check_project_rejects_malformed_external_action_selector_metadata() {
    let frontend = Frontend;
    let mut environment = external_environment();
    environment.external_public_metadata[0]
        .actions
        .push(ProjectExternalActionSignatureInput {
            path: vec![
                "dep".to_owned(),
                "math".to_owned(),
                "Transport".to_owned(),
                "request".to_owned(),
            ],
            params: vec![ProjectExternalTypeInput::Primitive("string".to_owned())],
            effect_args: vec![ProjectExternalActionArgKindInput::StringPattern],
            selector_param_names: vec!["host".to_owned()],
            selector_defaults: Vec::new(),
            output: ProjectExternalTypeInput::Primitive("unit".to_owned()),
            returns_never: false,
            visibility: "public".to_owned(),
        });
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment,
        sources: vec![SourceInput {
            id: etas_core::SourceId(45),
            path: Some(std::path::PathBuf::from("src/app/a.es")),
            text: r#"module app.a;

import dep.math.{add};

flow main() -> i32 {
    return add(1, 2);
}
"#
            .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(output.diagnostics.iter().any(|diagnostic| {
        matches!(
            diagnostic.code,
            DiagnosticCode::Type(TypeDiagnosticCode::IncompleteTypeFacts)
        ) && diagnostic.message.contains("selector_defaults length")
    }));
}

#[test]
fn frontend_check_project_records_external_wildcard_re_exports() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: external_environment(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(42),
            path: Some(std::path::PathBuf::from("src/app/a.es")),
            text: "module app.a;\npublic import dep.math.*;\nflow main() -> unit { return; }"
                .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    let resolved = output.resolved_imports.as_ref().expect("resolved imports");
    assert_eq!(resolved.wildcard_imports.len(), 1);
    assert_eq!(resolved.re_exports.len(), 1);
    assert_eq!(resolved.re_exports[0].target.segments, ["dep", "math"]);
    assert!(
        resolved.wildcard_imports[0]
            .exported_names
            .iter()
            .any(|name| name == "Number")
    );
    assert!(
        resolved.wildcard_imports[0]
            .exported_names
            .iter()
            .any(|name| name == "add")
    );
}

#[test]
fn frontend_check_project_rejects_duplicate_explicit_import_names() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(30),
                path: Some(std::path::PathBuf::from("src/app/a.es")),
                text: "module app.a;\nimport app.b.{helper};\nimport app.c.{helper};\nflow main() -> unit { return; }"
                    .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(31),
                path: Some(std::path::PathBuf::from("src/app/b.es")),
                text: "module app.b;\npublic flow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(32),
                path: Some(std::path::PathBuf::from("src/app/c.es")),
                text: "module app.c;\npublic flow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("duplicate explicit import `helper`")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn frontend_check_project_rejects_wildcard_ambiguity_at_use_site() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![
            SourceInput {
                id: etas_core::SourceId(33),
                path: Some(std::path::PathBuf::from("src/app/a.es")),
                text: r#"
module app.a;
import app.b.*;
import app.c.*;

flow main() -> unit {
  return helper();
}
"#
                .to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(34),
                path: Some(std::path::PathBuf::from("src/app/b.es")),
                text: "module app.b;\npublic flow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
            SourceInput {
                id: etas_core::SourceId(35),
                path: Some(std::path::PathBuf::from("src/app/c.es")),
                text: "module app.c;\npublic flow helper() -> unit { return; }".to_owned(),
                kind: SourceKind::SourceProjectFile,
            },
        ],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("wildcard import ambiguity for `helper`")),
        "{:?}",
        output.diagnostics
    );
    let resolved_paths = output.resolved_paths.as_ref().expect("resolved paths");
    assert!(resolved_paths.expr_paths.iter().any(|fact| matches!(
        &fact.result,
        HirPathResolution::AmbiguousWildcard { targets } if targets.len() == 2
    )));
}

#[test]
fn frontend_dump_apis_preserve_intermediate_outputs() {
    let frontend = Frontend;
    let parsed = frontend.parse(SourceInput::anonymous("flow main() -> unit { return; }"));
    let checked = frontend.check(SourceInput::anonymous("flow main() -> unit { return; }"));

    let ast_dump = dump_ast(&parsed, DumpOptions::default());
    let hir_dump = dump_hir(
        &checked,
        HirDumpOptions {
            include_spans: false,
            include_diagnostics: true,
            include_symbols: true,
            include_scopes: true,
            include_source_map: false,
        },
    );

    assert!(ast_dump.contains("Program"));
    assert!(hir_dump.contains("HirProgram"));
}

#[test]
fn frontend_check_output_preserves_all_project_sources() {
    let frontend = Frontend;
    let output = frontend
        .check_project(ProjectInput {
            project_root: std::path::PathBuf::from("/workspace/demo"),
            source_root: None,
            options: Default::default(),
            environment: ProjectEnvironmentInput::default(),
            sources: vec![
                SourceInput {
                    id: etas_core::SourceId(51),
                    path: Some(std::path::PathBuf::from("src/app/main.es")),
                    text: r#"
module app.main;

flow main() -> unit {
  return;
}
"#
                    .to_owned(),
                    kind: SourceKind::SourceProjectFile,
                },
                SourceInput {
                    id: etas_core::SourceId(52),
                    path: Some(std::path::PathBuf::from("src/app/util.es")),
                    text: "module app.util;\npublic flow helper() -> unit { return; }".to_owned(),
                    kind: SourceKind::SourceProjectFile,
                },
            ],
            entry: ProjectEntry {
                module: Some(ModulePath {
                    segments: vec!["app".to_owned(), "main".to_owned()],
                }),
                flow: "main".to_owned(),
            },
        })
        .into_check_output();

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(
        output
            .sources
            .as_ref()
            .expect("project source bundle")
            .sources
            .len(),
        2
    );
    assert_eq!(output.parsed_sources.len(), 2);
    assert_eq!(
        output.parse.as_ref().expect("single-source view").source.id,
        etas_core::SourceId(51)
    );
    assert!(output.checked.is_some());
}

#[test]
fn frontend_dump_diagnostics_preserves_structured_fields() {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: std::path::PathBuf::from("/workspace/demo"),
        source_root: None,
        options: Default::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: etas_core::SourceId(43),
            path: Some(std::path::PathBuf::from("src/app/a.es")),
            text: "module app.a;\nimport app.missing.{helper};\nflow main() -> unit { return; }"
                .to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "a".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });

    let document = dump_diagnostics(&output.diagnostics, DiagnosticDumpOptions::default());
    let missing_import = document
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("missing imported module"))
        .expect("missing import diagnostic");
    assert_eq!(missing_import.primary.source, etas_core::SourceId(43));
    assert!(missing_import.primary.end > missing_import.primary.start);
    assert_eq!(missing_import.severity, "Error");
    assert_eq!(missing_import.phase, "Parse");
    assert!(!missing_import.labels.is_empty());

    let compact = dump_diagnostics(
        &output.diagnostics,
        DiagnosticDumpOptions {
            include_labels: false,
            include_notes: false,
            include_suggestions: false,
        },
    );
    assert!(compact.diagnostics.iter().all(|diagnostic| {
        diagnostic.labels.is_empty()
            && diagnostic.notes.is_empty()
            && diagnostic.help.is_none()
            && diagnostic.suggestions.is_empty()
    }));
}
