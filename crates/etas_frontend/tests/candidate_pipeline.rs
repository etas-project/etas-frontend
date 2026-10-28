use etas_code::{
    CodeFingerprint, PackageKey, descriptor::DescriptorLimits, syntax::CodeSyntaxOperations,
};
use etas_core::{DiagnosticCode, SourceId, TypeDiagnosticCode};
use etas_frontend::{
    CheckedProgram, EntryPolicy, Frontend, FrontendSession, ProjectInput, SourceInput, SourceKind,
    code::{
        definition::{DefinitionCatalog, FrozenDefinitionEnvironment, ReflectionOptions},
        draft::DefaultBodyOverlay,
        identity::{DefinitionOrigins, SourceModuleOrigin},
        service::{CandidateBodyAnalysis, NativeCodeSyntaxOperations},
    },
};
use etas_hir::{HirExpr, HirExprId};
use std::{collections::BTreeMap, sync::Arc};

fn catalog(checked: &CheckedProgram) -> DefinitionCatalog {
    let key = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.candidate.package",
        b"app",
    ));
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(checked, key).unwrap(),
        standard_package: key,
        external_definitions: BTreeMap::new(),
    };
    DefinitionCatalog::build(checked.clone(), &origins, DescriptorLimits::default()).unwrap()
}

fn reflection(checked: &CheckedProgram) -> HirExprId {
    checked
        .hir
        .exprs
        .iter()
        .find_map(|(id, expr)| matches!(expr, HirExpr::Reflect(_)).then_some(id))
        .unwrap()
}

fn freeze(source: &str) -> (CheckedProgram, Arc<FrozenDefinitionEnvironment>) {
    freeze_sources(&[source], "app.Worker")
}

fn freeze_sources(
    sources: &[&str],
    target: &str,
) -> (CheckedProgram, Arc<FrozenDefinitionEnvironment>) {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
    input.sources = sources
        .iter()
        .enumerate()
        .map(|(index, text)| SourceInput {
            id: SourceId(index as u32),
            path: None,
            text: (*text).into(),
            kind: SourceKind::SourceProjectFile,
        })
        .collect();
    input.options.entry_policy = EntryPolicy::Optional;
    input.options.reflection = ReflectionOptions {
        retain_bodies: vec![target.into()],
        export_bodies: Vec::new(),
    };
    let result = Frontend.check_project(input);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let checked = result.checked.unwrap();
    let frozen = catalog(&checked)
        .freeze_agent(reflection(&checked))
        .unwrap();
    (checked, frozen)
}

#[test]
fn candidate_dependency_completion_cannot_replace_project_sources_or_add_unlocked_packages() {
    let (_, frozen) = freeze(
        "module app; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let definition = ^Worker; return; }",
    );
    let syntax = NativeCodeSyntaxOperations::default();
    for (kind, expected) in [
        (SourceKind::SourceProjectFile, "only dependency overlays"),
        (
            SourceKind::DependencySourceOverlay {
                package: etas_frontend::ExternalPackageId(9),
                import_root: "unlocked".into(),
            },
            "outside its frozen package environment",
        ),
    ] {
        let overlay = DefaultBodyOverlay::prepare(
            frozen.clone(),
            syntax.parse_body("return 1;").unwrap(),
            &syntax,
        )
        .unwrap();
        let result = FrontendSession::new().analyze_body_overlay_with_dependencies(
            overlay,
            vec![SourceInput {
                id: SourceId(u32::MAX),
                path: Some(std::path::PathBuf::from("/unlocked/source.es")),
                text: "module unlocked;".into(),
                kind,
            }],
            Default::default(),
            None,
        );
        assert!(
            matches!(result, Err(etas_frontend::code::service::CandidateAnalysisError::Pipeline { reason, .. }) if reason.contains(expected))
        );
    }
}

#[test]
fn candidate_resolves_import_aliases_in_the_frozen_module_part() {
    let (_, frozen) = freeze_sources(
        &[
            "module app.types; public enum Choice { Yes(i32), No, }",
            "module app.worker; import app.types.Choice as C; agent Worker(value: i32) -> i32 { return value; } flow inspect() -> unit { let d = ^Worker; return; }",
        ],
        "app.worker.Worker",
    );
    let syntax = NativeCodeSyntaxOperations::default();
    let analysis = analyze(
        frozen,
        "let selected = C.Yes(value); return match selected { C.Yes(number) => number, C.No => 0, };",
        &syntax,
    );
    assert!(
        analysis.output().diagnostics.is_empty(),
        "{:?}",
        analysis.output().diagnostics
    );
    let checked = analysis.output().checked.as_ref().unwrap();
    assert_eq!(checked.source_map.lexical_source(SourceId(2)), SourceId(1));
    assert_eq!(
        checked
            .entry_fact
            .requested
            .module
            .as_ref()
            .unwrap()
            .segments,
        ["app", "worker"]
    );
}

#[test]
fn candidate_default_impl_uses_private_helpers_from_the_implementation_module() {
    let (_, frozen) = freeze_sources(
        &[
            "module app.component; public agent Worker;",
            "module app.implementation; import app.component.Worker as Target; flow adjust(value: i32) -> i32 { return value + 2; } impl agent Target { public flow run(value: i32) -> i32 { return value; } }",
            "module app.client; import app.component.Worker; flow inspect() -> unit { let d = ^Worker; return; }",
        ],
        "app.component.Worker",
    );
    let syntax = NativeCodeSyntaxOperations::default();
    let analysis = analyze(frozen, "return adjust(value);", &syntax);
    assert!(
        analysis.output().diagnostics.is_empty(),
        "{:?}",
        analysis.output().diagnostics
    );
    let checked = analysis.output().checked.as_ref().unwrap();
    assert_eq!(checked.source_map.lexical_source(SourceId(3)), SourceId(1));
    assert_eq!(
        checked
            .entry_fact
            .requested
            .module
            .as_ref()
            .unwrap()
            .segments,
        ["app", "implementation"]
    );
}

fn actions(checked: &CheckedProgram, row: &etas_effects::EffectRow) -> Vec<String> {
    row.effects
        .iter()
        .map(|effect| {
            let action = match effect {
                etas_effects::Effect::Action(action) => action,
                etas_effects::Effect::AppliedAction(instance) => &instance.action,
                _ => panic!("requested action expected"),
            };
            format!(
                "{}.{}",
                checked.effect_registry.tag_name(action.tag).unwrap(),
                checked
                    .effect_registry
                    .action_name(action.tag, action.action)
                    .unwrap()
            )
        })
        .collect()
}

#[test]
fn candidate_effects_include_the_actual_transitive_body_not_unrelated_main() {
    let (_, frozen) = freeze(
        "module app; import std.io.eprintln; import std.io.println; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; } flow main() -> unit { eprintln(\"unrelated\")?; return; }",
    );
    let syntax = NativeCodeSyntaxOperations::default();
    let analysis = analyze(frozen, "println(\"changed\")?; return 2;", &syntax);
    assert!(
        !analysis
            .output()
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
        "{:?}",
        analysis.output().diagnostics
    );
    let checked = analysis.output().checked.as_ref().unwrap();
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert!(summary.escaping_effects.is_empty());
    assert_eq!(
        actions(checked, &summary.requested_actions),
        ["Console.stdout_write"]
    );
    assert_eq!(
        actions(checked, &summary.default_actions),
        ["Console.stdout_write"]
    );
}

#[test]
fn candidate_body_passes_through_existing_trace_spec_validation() {
    for declaration in [
        "agent Worker() -> i32 ~ Quiet { return 1; }",
        "agent Worker; impl agent Worker ~ Quiet { flow run() -> i32 { return 1; } }",
        "agent Worker() -> i32 ~ Quiet + Permitted { return 1; }",
        "agent Worker; impl agent Worker ~ Quiet + Permitted { flow run() -> i32 { return 1; } }",
    ] {
        let (_, frozen) = freeze(&format!(
            "module app; import std.io.println; spec Quiet: trace = -Console.stdout_write; spec Permitted: trace = +Console.stdout_write; {declaration} flow inspect() -> unit {{ let d = ^Worker; return; }}"
        ));
        let syntax = NativeCodeSyntaxOperations::default();
        let analysis = analyze(frozen, "println(\"forbidden\")?; return 2;", &syntax);
        assert!(analysis.output().checked.is_none());
        assert!(
            analysis
                .output()
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code
                    == DiagnosticCode::Effect(etas_core::EffectDiagnosticCode::TraceSpecDenied)),
            "{:?}",
            analysis.output().diagnostics
        );
    }
}

#[test]
fn generic_trace_spec_instantiates_its_checked_type_arguments() {
    let (_, frozen) = freeze(
        "module app; effect Probe { action read<T>(value: T) -> unit; } spec NoProbe<R>: trace = -Probe.read<R>; agent Worker; impl agent Worker ~ NoProbe<i32> { flow run() -> i32 { return 1; } } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let syntax = NativeCodeSyntaxOperations::default();
    for (argument, rejected) in [("i32>(42", true), ("string>(\"allowed\"", false)] {
        let body = format!(
            "handle {{ perform Probe.read<{argument}); }} with {{ Probe.read<_>(_) => resume; }}; return 2;"
        );
        let analysis = analyze(frozen.clone(), &body, &syntax);
        let denied = analysis.output().diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Effect(etas_core::EffectDiagnosticCode::TraceSpecDenied)
        });
        assert_eq!(denied, rejected, "{:?}", analysis.output().diagnostics);
        assert_eq!(
            analysis.output().checked.is_none(),
            rejected,
            "{:?}",
            analysis.output().diagnostics
        );
    }
}

fn analyze(
    frozen: Arc<FrozenDefinitionEnvironment>,
    body: &str,
    syntax: &NativeCodeSyntaxOperations,
) -> CandidateBodyAnalysis {
    let overlay =
        DefaultBodyOverlay::prepare(frozen, syntax.parse_body(body).unwrap(), syntax).unwrap();
    FrontendSession::new()
        .analyze_body_overlay(overlay)
        .unwrap()
}

#[test]
fn candidate_reuses_frozen_ast_and_registry_without_main_or_program_reparse() {
    let (base, frozen) = freeze(
        "module app; flow helper(x: i32) -> i32 { return x + 1; } agent Worker(x: i32) -> i32 { return x; } flow inspect() -> unit { let d = ^Worker; return; } flow main() -> string { return \"not a CLI entry\"; }",
    );
    let syntax = NativeCodeSyntaxOperations::default();
    let analysis = analyze(frozen, "return helper(x);", &syntax);
    let output = analysis.output();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.as_ref().unwrap();
    assert!(Arc::ptr_eq(&base.std_registry, &checked.std_registry));
    let old_source = base.definition_sources.source(SourceId(0)).unwrap();
    let new_source = checked.definition_sources.source(SourceId(0)).unwrap();
    assert!(Arc::ptr_eq(&old_source.program, &new_source.program));
    assert!(Arc::ptr_eq(&old_source.file, &new_source.file));
    assert_eq!(output.reused_parsed_sources, 1);
    let names = analysis
        .pipeline_records()
        .iter()
        .map(|record| record.pass)
        .collect::<Vec<_>>();
    for required in [
        "FreezeDefinitionEnvironmentPass",
        "ApplyCandidateBodyOverlaysPass",
        "ResolveCandidateTargetPass",
        "BuildSignatureFactsPass",
        "TypeCheckBodyPass",
        "RunEffectPipelinePass",
        "BuildDefinitionDescriptorsPass",
        "ValidateCandidateContractPass",
    ] {
        assert!(names.contains(&required), "missing {required}: {names:?}");
    }
    assert!(!names.contains(&"ParseSourceFilePass"));
    assert!(!names.contains(&"ResolveEntryItemPass"));
    assert!(!names.contains(&"ValidateEntryContractPass"));
    assert_eq!(checked.entry_fact.requested.flow, "Worker");
    assert!(matches!(
        checked.hir.items[checked.entry.unwrap()],
        etas_hir::HirItem::Agent(_)
    ));
    assert_eq!(checked.sources.sources.len(), 2);
}

#[test]
fn candidate_errors_have_generated_source_spans_and_cannot_capture_other_locals() {
    let (_, frozen) = freeze(
        "module app; agent Worker(x: i32) -> i32 { let previous = 1; return x; } flow inspect() -> unit { let caller_local = 2; let d = ^Worker; return; }",
    );
    let syntax = NativeCodeSyntaxOperations::default();
    for name in ["previous", "caller_local"] {
        let body = format!("\n\nreturn {name};\n");
        let analysis = analyze(frozen.clone(), &body, &syntax);
        assert!(analysis.output().checked.is_none());
        let diagnostic = analysis
            .output()
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.primary.span.source == SourceId(1))
            .unwrap();
        let sources = analysis.output().sources.as_ref().unwrap();
        let source = sources
            .files
            .iter()
            .find(|source| source.id == SourceId(1))
            .unwrap();
        assert_eq!(source.kind, SourceKind::VirtualGenerated);
        assert_eq!(source.text.as_ref(), body);
        assert_eq!(
            &source.text[diagnostic.primary.span.range.start.to_usize()
                ..diagnostic.primary.span.range.end.to_usize()],
            name
        );
    }
    let analysis = analyze(frozen, "return \"wrong output\";", &syntax);
    assert!(analysis.output().checked.is_none());
    assert!(
        analysis
            .output()
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code
                == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
                && diagnostic.primary.span.source == SourceId(1)),
        "{:?}",
        analysis.output().diagnostics
    );
}

#[test]
fn candidate_retention_returns_the_replacement_and_successive_edits_share_the_original_ast() {
    let (base, frozen) = freeze(
        "module app; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let syntax = NativeCodeSyntaxOperations::default();
    let first = analyze(frozen, "return 2;", &syntax);
    let checked = first.output().checked.as_ref().unwrap();
    let body = checked
        .definition_sources
        .body_revision_syntax(checked.entry.unwrap())
        .unwrap();
    assert_eq!(syntax.format(body).unwrap(), "return 2;");
    let second = analyze(first.frozen().clone(), "return 3;", &syntax);
    let next = second.output().checked.as_ref().unwrap();
    let body = next
        .definition_sources
        .body_revision_syntax(next.entry.unwrap())
        .unwrap();
    assert_eq!(syntax.format(body).unwrap(), "return 3;");
    assert!(Arc::ptr_eq(
        &base.definition_sources.source(SourceId(0)).unwrap().program,
        &next.definition_sources.source(SourceId(0)).unwrap().program
    ));
    assert_eq!(
        next.sources.sources.len(),
        2,
        "replaced virtual sources must not accumulate"
    );
    let old_body = catalog(&base)
        .retained_body(reflection(&base), &syntax)
        .unwrap()
        .unwrap();
    assert_eq!(syntax.format(&old_body).unwrap(), "return 1;");
}

#[test]
fn generated_revisions_bind_body_content_to_the_original_canonical_callable() {
    use etas_code::definition::CodeDefinitionOperations;
    use etas_frontend::code::definition::NativeDefinitionStore;
    let (base, _) = freeze_sources(
        &[
            "module app; agent First() -> i32 { return 1; } agent Second() -> i32 { return 2; } flow inspect() -> unit { let first = ^First; let second = ^Second; return; }",
        ],
        "app.First",
    );
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let reflect_agent = |checked: &CheckedProgram, name: &str| {
        checked
            .types
            .reflections
            .iter()
            .find_map(|(expression, fact)| match fact.target {
                etas_types::CheckedReflectionTarget::Agent { declaration, .. }
                    if checked.symbols.get(declaration).unwrap().name == name =>
                {
                    Some(*expression)
                }
                _ => None,
            })
            .unwrap()
    };
    let store =
        NativeDefinitionStore::new(Arc::new(catalog(&base)), syntax.clone(), Default::default());
    let first = store.reflect_agent(reflect_agent(&base, "First")).unwrap();
    let second = store.reflect_agent(reflect_agent(&base, "Second")).unwrap();
    let body = syntax.parse_body("return 10;").unwrap();
    let edited_first = store.with_body(&first, body.clone()).unwrap();
    let edited_second = store.with_body(&second, body).unwrap();
    assert_eq!(
        edited_first.revision().inputs().content,
        edited_second.revision().inputs().content
    );
    assert_ne!(
        edited_first.revision().key(),
        edited_second.revision().key()
    );
    assert_eq!(edited_first.identity(), first.identity());
    assert_eq!(edited_second.identity(), second.identity());
    let mut session = FrontendSession::new();
    for definition in [&edited_first, &edited_second] {
        let candidate = store
            .compile_definition(definition, &mut session, Default::default())
            .unwrap();
        assert_eq!(candidate.revision(), definition.revision());
        assert_eq!(candidate.identity(), definition.identity());
    }
}

#[test]
fn candidate_catalog_keeps_frozen_package_origins_and_current_analysis() {
    let (base, _) = freeze(
        "module app; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let key = |name: &[u8]| {
        PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
            "etas.test.frozen-origin",
            name,
        ))
    };
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&base, key(b"owner")).unwrap(),
        standard_package: key(b"standard"),
        external_definitions: BTreeMap::new(),
    };
    let catalog =
        DefinitionCatalog::build(base.clone(), &origins, DescriptorLimits::default()).unwrap();
    let frozen = catalog.freeze_agent(reflection(&base)).unwrap();
    let syntax = NativeCodeSyntaxOperations::default();
    let analysis = analyze(frozen.clone(), "let d = ^Worker; return 2;", &syntax);
    let checked = analysis.output().checked.as_ref().unwrap();
    let next = analysis
        .definition_catalog(DescriptorLimits::default())
        .unwrap()
        .unwrap();
    assert!(matches!(
        next.freeze_agent(reflection(checked)),
        Err(etas_frontend::code::descriptor::ProjectionError::RuntimeRevisionRequired)
    ));
    let etas_frontend::code::definition::StaticReflectionView::Agent(agent) =
        next.static_reflection(reflection(checked)).unwrap()
    else {
        panic!("agent")
    };
    assert_eq!(&agent.identity, frozen.identity());
    assert_eq!(agent.identity.package(), key(b"owner"));
    let method = next
        .callable(agent.default_entry.as_ref().unwrap())
        .unwrap();
    assert!(method.analysis.effects.escaping.is_some());
    assert!(method.analysis.trace.is_some());
    let rejected = analyze(frozen, "return \"wrong\";", &syntax);
    assert!(
        rejected
            .definition_catalog(DescriptorLimits::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn frozen_fingerprint_includes_dependency_module_package_identity() {
    let (base, _) = freeze_sources(
        &[
            "module dependency; public flow helper() -> i32 { return 1; }",
            "module app; import dependency.helper; agent Worker() -> i32 { return helper(); } flow inspect() -> unit { let d = ^Worker; return; }",
        ],
        "app.Worker",
    );
    let key = |name: &[u8]| {
        PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
            "etas.test.origin-binding",
            name,
        ))
    };
    let build = |dependency| {
        let origins = DefinitionOrigins {
            source_modules: base
                .hir
                .modules
                .iter()
                .map(|module| {
                    let name = &base.modules_arena[*module].name.as_ref().unwrap().segments[0].name;
                    (
                        *module,
                        SourceModuleOrigin::declared(
                            &base,
                            *module,
                            key(if name == "dependency" {
                                dependency
                            } else {
                                b"owner"
                            }),
                        )
                        .unwrap(),
                    )
                })
                .collect(),
            standard_package: key(b"standard"),
            external_definitions: BTreeMap::new(),
        };
        DefinitionCatalog::build(base.clone(), &origins, DescriptorLimits::default())
            .unwrap()
            .freeze_agent(reflection(&base))
            .unwrap()
    };
    let original = build(b"dependency-a");
    let changed = build(b"dependency-b");
    assert_eq!(original.identity(), changed.identity());
    assert_ne!(original.fingerprint(), changed.fingerprint());
}
