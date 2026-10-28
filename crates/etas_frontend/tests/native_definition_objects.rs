use std::{collections::BTreeMap, sync::Arc};

use etas_code::{
    CodeFingerprint, DefinitionKey, PackageKey, RevisionInfo,
    definition::{
        AgentDefinitionLease, AgentDefinitionRef, AnalysisUnavailableReason,
        CodeDefinitionOperations, DefinitionOperationError, DefinitionResource,
    },
    report::CheckStatus,
    syntax::{CodeSyntaxOperations, SyntaxOperationError},
};
use etas_core::{SourceId, memory::MemoryFootprint};
use etas_frontend::{
    EntryPolicy, Frontend, FrontendSession, ProjectInput, SourceInput, SourceKind,
    code::{
        definition::{
            DefinitionCatalog, NativeDefinitionError, NativeDefinitionLimits,
            NativeDefinitionStore, ReflectionOptions,
        },
        identity::{DefinitionOrigins, SourceModuleOrigin},
        service::{NativeCodeSyntaxOperations, NativeSyntaxLimits},
    },
};

fn catalog(
    sources: &[&str],
    retain: &[&str],
) -> (Arc<DefinitionCatalog>, Vec<etas_hir::HirExprId>) {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
    input.options.entry_policy = EntryPolicy::Optional;
    input.options.reflection = ReflectionOptions {
        retain_bodies: retain.iter().map(|item| (*item).into()).collect(),
        export_bodies: vec![],
    };
    input.sources = sources
        .iter()
        .enumerate()
        .map(|(index, text)| SourceInput {
            id: SourceId(index as u32 + 1),
            path: None,
            text: (*text).into(),
            kind: SourceKind::SourceProjectFile,
        })
        .collect();
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let key = |name: &[u8]| {
        PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
            "etas.test.native-definitions.v1",
            name,
        ))
    };
    let origins = DefinitionOrigins {
        source_modules: checked
            .hir
            .modules
            .iter()
            .map(|module| {
                (
                    *module,
                    SourceModuleOrigin::declared(
                        &checked,
                        *module,
                        key(
                            if checked.modules_arena[*module]
                                .name
                                .as_ref()
                                .unwrap()
                                .segments[0]
                                .name
                                == "foreign"
                            {
                                b"foreign"
                            } else {
                                b"app"
                            },
                        ),
                    )
                    .unwrap(),
                )
            })
            .collect(),
        standard_package: key(b"std"),
        external_definitions: BTreeMap::new(),
    };
    let expressions = checked
        .hir
        .exprs
        .iter()
        .filter_map(|(id, expr)| matches!(expr, etas_hir::HirExpr::Reflect(_)).then_some(id))
        .collect();
    (
        Arc::new(DefinitionCatalog::build(checked, &origins, Default::default()).unwrap()),
        expressions,
    )
}

const SOURCE: &str = "module app; import std.io.println; agent Worker() -> i32 { return 1; } impl agent Worker { public flow help() -> i32 { return 9; } } flow inspect() -> unit { let d = ^Worker; return; }";

#[test]
fn real_native_definitions_share_the_frozen_base_and_invalidate_edited_analysis() {
    let (catalog, expressions) = catalog(&[SOURCE], &["app.Worker"]);
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let store = NativeDefinitionStore::new(catalog, syntax.clone(), Default::default());
    let original = store.reflect_agent(expressions[0]).unwrap();
    let original_view = store.resolve(&original).unwrap();
    assert!(original_view.signature().is_some());
    assert!(original_view.current_analysis().is_ok());
    let method = original_view
        .environment()
        .unwrap()
        .contract()
        .header()
        .methods[0]
        .clone();
    assert!(
        original_view
            .current_method_analysis(&method)
            .unwrap()
            .is_some()
    );
    let body = syntax.parse_body("return 2;").unwrap();
    let edited = store.with_body(&original, body.clone()).unwrap();
    let edited_view = store.resolve(&edited).unwrap();
    assert!(Arc::ptr_eq(
        original_view.environment().unwrap(),
        edited_view.environment().unwrap()
    ));
    assert_eq!(original_view.signature(), edited_view.signature());
    assert_eq!(edited.identity(), original.identity());
    assert_eq!(
        edited.revision().inputs().parent,
        Some(original.revision().key())
    );
    assert_eq!(
        edited_view.current_analysis(),
        Err(AnalysisUnavailableReason::EditedBody)
    );
    assert_eq!(
        edited_view.current_method_analysis(&method),
        Err(AnalysisUnavailableReason::EditedBody)
    );
    assert!(original_view.current_analysis().is_ok());
    assert!(
        syntax
            .format(original_view.body().unwrap())
            .unwrap()
            .contains("return 1;")
    );
    assert_eq!(
        syntax.format(edited_view.body().unwrap()).unwrap(),
        "return 2;"
    );
    assert_eq!(
        edited.revision().inputs().content,
        body.identity().content()
    );
}

#[test]
fn native_definition_lineage_reaches_the_actual_candidate_emit_pass() {
    let (catalog, expressions) = catalog(&[SOURCE], &["app.Worker"]);
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let store = NativeDefinitionStore::new(catalog, syntax.clone(), Default::default());
    let original = store.reflect_agent(expressions[0]).unwrap();
    let body = syntax.parse_body("return 2;").unwrap();
    let first = store.with_body(&original, body.clone()).unwrap();
    let direct = etas_frontend::code::draft::DefaultBodyOverlay::prepare(
        store
            .resolve(&original)
            .unwrap()
            .environment()
            .unwrap()
            .clone(),
        body.clone(),
        &syntax,
    )
    .unwrap();
    assert_eq!(direct.revision(), first.revision());
    assert_eq!(
        direct.revision().inputs().parent,
        Some(original.revision().key())
    );
    let second = store.with_body(&first, body).unwrap();
    assert_eq!(
        second.revision().inputs().parent,
        Some(first.revision().key())
    );
    let first_overlay = store.body_overlay(&first).unwrap();
    let second_overlay = store.body_overlay(&second).unwrap();
    assert_ne!(first_overlay.fingerprint(), second_overlay.fingerprint());
    let mut session = FrontendSession::new();
    for (definition, overlay) in [(&first, first_overlay), (&second, second_overlay)] {
        let candidate = session.compile_body_overlay(overlay).unwrap();
        assert_eq!(candidate.revision(), definition.revision());
        assert_eq!(candidate.identity(), original.identity());
        assert!(Arc::ptr_eq(
            candidate.frozen(),
            store.resolve(&original).unwrap().environment().unwrap()
        ));
    }
    let bad = store
        .with_body(
            &second,
            syntax
                .parse_body("println(\"new authority\")?; return 3;")
                .unwrap(),
        )
        .unwrap();
    let analysis = session
        .analyze_body_overlay(store.body_overlay(&bad).unwrap())
        .unwrap();
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(report.status, CheckStatus::Rejected);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "code::AdditionalRequestedActions")
    );
    assert_eq!(
        bad.revision().inputs().contract,
        original.revision().inputs().contract
    );
}

#[derive(Debug, etas_core::RetainedSize)]
struct ClaimedDefinition {
    identity: DefinitionKey,
    revision: RevisionInfo,
}
impl AgentDefinitionLease for ClaimedDefinition {
    fn identity(&self) -> &DefinitionKey {
        &self.identity
    }
    fn revision(&self) -> &RevisionInfo {
        &self.revision
    }
}

#[test]
fn definition_and_syntax_handles_require_the_exact_originating_backend_lease() {
    let (catalog, expressions) = catalog(&[SOURCE], &[]);
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let store = NativeDefinitionStore::new(catalog.clone(), syntax.clone(), Default::default());
    let other = NativeDefinitionStore::new(catalog, syntax.clone(), Default::default());
    let original = store.reflect_agent(expressions[0]).unwrap();
    let foreign = other.reflect_agent(expressions[0]).unwrap();
    assert_eq!(original.revision(), foreign.revision());
    assert!(matches!(
        store.resolve(&foreign),
        Err(DefinitionOperationError::ForeignDefinition)
    ));
    let claimed = AgentDefinitionRef::from_backend(Arc::new(ClaimedDefinition {
        identity: original.identity().clone(),
        revision: original.revision().clone(),
    }));
    assert!(matches!(
        store.resolve(&claimed),
        Err(DefinitionOperationError::ForeignDefinition)
    ));
    let foreign_syntax = NativeCodeSyntaxOperations::default()
        .parse_body("return 2;")
        .unwrap();
    assert!(matches!(
        store.with_body(&original, foreign_syntax),
        Err(DefinitionOperationError::Syntax(
            SyntaxOperationError::ForeignSyntaxObject
        ))
    ));
}

#[test]
fn unretained_body_is_absent_but_its_real_content_identity_is_preserved() {
    let (catalog, expressions) = catalog(&[SOURCE], &[]);
    let syntax = Arc::new(NativeCodeSyntaxOperations::new(NativeSyntaxLimits {
        max_retained_objects: 0,
        ..Default::default()
    }));
    let store = NativeDefinitionStore::new(catalog, syntax, Default::default());
    let original = store.reflect_agent(expressions[0]).unwrap();
    let view = store.resolve(&original).unwrap();
    assert!(view.signature().is_some());
    assert!(view.body().is_none());
    assert!(view.current_analysis().is_ok());
    assert!(matches!(
        store.body_overlay(&original),
        Err(NativeDefinitionError::Overlay(
            etas_frontend::code::draft::OverlayError::Syntax(
                SyntaxOperationError::ResourceLimit { .. }
            )
        ))
    ));
}

#[test]
fn bare_agent_drafts_do_not_invent_a_default_signature() {
    let (catalog, expressions) = catalog(
        &[
            "module app; agent Bare; impl agent Bare { public flow help() -> i32 { return 9; } } flow inspect() -> unit { let d = ^Bare; return; }",
        ],
        &[],
    );
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let store = NativeDefinitionStore::new(catalog, syntax.clone(), Default::default());
    let original = store.reflect_agent(expressions[0]).unwrap();
    let edited = store
        .with_body(&original, syntax.parse_body("return 2;").unwrap())
        .unwrap();
    let view = store.resolve(&edited).unwrap();
    assert!(view.signature().is_none());
    assert!(view.body().is_some());
    assert_eq!(
        view.current_analysis(),
        Err(AnalysisUnavailableReason::MissingDefaultEntry)
    );
    assert!(matches!(
        store.body_overlay(&edited),
        Err(NativeDefinitionError::Overlay(
            etas_frontend::code::draft::OverlayError::MissingDefaultEntry
        ))
    ));
}

#[test]
fn object_quota_and_shared_memory_accounting_follow_live_leases() {
    let (catalog, expressions) = catalog(&[SOURCE], &[]);
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let store = NativeDefinitionStore::new(
        catalog,
        syntax.clone(),
        NativeDefinitionLimits {
            max_retained_objects: 1,
            ..Default::default()
        },
    );
    let original = store.reflect_agent(expressions[0]).unwrap();
    let copies = vec![original.clone(); 100];
    let retained = MemoryFootprint::of(&copies).estimated_bytes();
    assert!(
        retained
            <= MemoryFootprint::of(&original).estimated_bytes()
                + (copies.capacity() * size_of::<AgentDefinitionRef>()) as u64
                + 1024
    );
    assert!(matches!(
        store.with_body(&original, syntax.parse_body("return 2;").unwrap()),
        Err(DefinitionOperationError::ResourceLimit {
            resource: DefinitionResource::RetainedObjects,
            limit: 1
        })
    ));
    drop(original);
    drop(copies);
    assert_eq!(store.retained_bytes().unwrap(), 0);
    store.reflect_agent(expressions[0]).unwrap();
}

#[test]
fn same_revision_does_not_merge_visible_and_hidden_body_views() {
    let (catalog, expressions) = catalog(
        &[
            "module app; public agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
            "module foreign; import app.Worker; flow inspect() -> unit { let d = ^Worker; return; }",
        ],
        &["app.Worker"],
    );
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let store = NativeDefinitionStore::new(catalog, syntax, Default::default());
    let visible = store.reflect_agent(expressions[0]).unwrap();
    let hidden = store.reflect_agent(expressions[1]).unwrap();
    assert_eq!(visible.revision(), hidden.revision());
    assert!(store.resolve(&visible).unwrap().body().is_some());
    assert!(store.resolve(&hidden).unwrap().body().is_none());
}

#[test]
fn repeated_native_reflection_shares_one_definition_and_releases_every_body_lease() {
    let (catalog, expressions) = catalog(&[SOURCE], &["app.Worker"]);
    let syntax = Arc::new(NativeCodeSyntaxOperations::new(NativeSyntaxLimits {
        max_retained_objects: 1,
        ..Default::default()
    }));
    let store = NativeDefinitionStore::new(
        catalog,
        syntax.clone(),
        NativeDefinitionLimits {
            max_retained_objects: 1,
            ..Default::default()
        },
    );
    let mut baseline = None;
    for count in [1, 10, 100] {
        let definitions = (0..count)
            .map(|_| store.reflect_agent(expressions[0]).unwrap())
            .collect::<Vec<_>>();
        let body = store
            .resolve(&definitions[0])
            .unwrap()
            .body()
            .unwrap()
            .clone();
        let definition_bytes = store.retained_bytes().unwrap();
        let syntax_bytes = MemoryFootprint::of(&body).estimated_bytes();
        let current = (definition_bytes, syntax_bytes);
        assert!(definition_bytes > 0 && syntax_bytes > 0);
        if let Some(baseline) = baseline {
            assert_eq!(current, baseline, "{count} actual reflection requests");
        } else {
            baseline = Some(current);
        }
        println!(
            "reflect_count={count} retained_definition_bytes={definition_bytes} retained_syntax_bytes={syntax_bytes}"
        );
        drop(body);
        drop(definitions);
        assert_eq!(store.retained_bytes().unwrap(), 0);
        // A different syntax object can use the one-object quota only after
        // every retained source-body lease has actually been released.
        drop(syntax.parse_body("return 0;").unwrap());
    }
}

#[test]
fn cached_source_views_still_require_the_checked_agent_binding_and_descriptor_kind() {
    let (catalog, _) = catalog(
        &[r#"
module app;
agent Worker() -> i32 { return 1; }
agent Other() -> i32 { return 2; }
flow main() -> unit {
    let original = ^Worker;
    let other = ^Other;
    let current = Worker;
    let value = ^current;
    let method = ^current.run;
    return;
}
"#],
        &["app.Worker", "app.Other"],
    );
    let checked = catalog.checked_program();
    let declaration = |name: &str| {
        checked
            .checked_reflections()
            .iter()
            .find_map(|(expr, fact)| {
                let etas_types::CheckedReflectionTarget::Agent { declaration, .. } = &fact.target
                else {
                    return None;
                };
                (checked.symbols.get(*declaration).unwrap().name == name)
                    .then_some((*expr, *declaration))
            })
            .unwrap()
    };
    let item = |symbol| match &checked.symbols.get(symbol).unwrap().def {
        etas_hir::SymbolDef::Item { item } => *item,
        _ => panic!("source agent must have a checked item"),
    };
    let (worker_expr, worker_symbol) = declaration("Worker");
    let (other_expr, other_symbol) = declaration("Other");
    let worker_item = item(worker_symbol);
    let other_item = item(other_symbol);
    let value_expression = |has_method| {
        checked
            .checked_reflections()
            .iter()
            .find_map(|(expr, fact)| match fact.target {
                etas_types::CheckedReflectionTarget::AgentValue { method, .. }
                    if method.is_some() == has_method =>
                {
                    Some(*expr)
                }
                _ => None,
            })
            .unwrap()
    };
    let value_expr = value_expression(false);
    let method_expr = value_expression(true);
    let store = NativeDefinitionStore::new(
        catalog,
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    );
    let original = store.reflect_agent(worker_expr).unwrap();
    let _other = store.reflect_agent(other_expr).unwrap();
    assert!(
        store
            .reflect_agent_value(value_expr, worker_item)
            .unwrap()
            .is_same_object(&original)
    );
    assert!(store.reflect_agent_value(value_expr, other_item).is_err());
    assert!(store.reflect_agent_value(method_expr, worker_item).is_err());
}

#[test]
fn successive_drafts_keep_parent_ids_without_retaining_an_ancestor_object_chain() {
    let (catalog, expressions) = catalog(&[SOURCE], &[]);
    let syntax = Arc::new(NativeCodeSyntaxOperations::new(NativeSyntaxLimits {
        max_retained_objects: 2,
        ..Default::default()
    }));
    let store = NativeDefinitionStore::new(
        catalog,
        syntax.clone(),
        NativeDefinitionLimits {
            max_retained_objects: 2,
            ..Default::default()
        },
    );
    let mut current = store.reflect_agent(expressions[0]).unwrap();
    let baseline_bytes = store.retained_bytes().unwrap();
    for index in 0..40 {
        let parent = current.revision().key();
        let body = syntax.parse_body(&format!("return {index};")).unwrap();
        current = store.with_body(&current, body).unwrap();
        assert_eq!(current.revision().inputs().parent, Some(parent));
        assert!(store.retained_bytes().unwrap() < baseline_bytes * 2 + 64 * 1024);
    }
    let candidate = FrontendSession::new()
        .compile_body_overlay(store.body_overlay(&current).unwrap())
        .unwrap();
    assert_eq!(candidate.revision(), current.revision());
}

#[test]
fn byte_admission_failure_does_not_publish_a_partially_registered_definition() {
    let (catalog, expressions) = catalog(&[SOURCE], &[]);
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let store = NativeDefinitionStore::new(
        catalog,
        syntax,
        NativeDefinitionLimits {
            max_retained_bytes: 1,
            ..Default::default()
        },
    );
    assert!(matches!(
        store.reflect_agent(expressions[0]),
        Err(NativeDefinitionError::Operation(
            DefinitionOperationError::ResourceLimit {
                resource: DefinitionResource::RetainedBytes,
                limit: 1
            }
        ))
    ));
    assert_eq!(store.retained_bytes().unwrap(), 0);
}

#[test]
fn auxiliary_analysis_roots_do_not_change_the_primary_invocation_summary() {
    let (catalog, expressions) = catalog(
        &[
            "module app; import std.io.println; agent Worker() -> i32 { return 1; } impl agent Worker { public flow report() -> unit { println(\"auxiliary only\")?; return; } } flow inspect() -> unit { let d = ^Worker; return; }",
        ],
        &[],
    );
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let store = NativeDefinitionStore::new(catalog, syntax.clone(), Default::default());
    let original = store.reflect_agent(expressions[0]).unwrap();
    let edited = store
        .with_body(&original, syntax.parse_body("return 2;").unwrap())
        .unwrap();
    let analysis = FrontendSession::new()
        .analyze_body_overlay(store.body_overlay(&edited).unwrap())
        .unwrap();
    let checked = analysis.output().checked.as_ref().unwrap();
    let default = checked.entry.unwrap();
    let primary = checked.effects.item_effects.get(&default).unwrap();
    assert!(primary.requested_actions.is_empty());
    assert!(primary.escaping_effects.is_empty());
    assert!(checked.types.agent_methods.values().any(|method| {
        method.body != default
            && checked
                .effects
                .item_effects
                .get(&method.body)
                .is_some_and(|summary| !summary.requested_actions.is_empty())
    }));
    let roots = analysis
        .pipeline_records()
        .iter()
        .find(|record| record.pass == "ResolveCandidateTargetPass")
        .unwrap();
    assert!(
        roots
            .produced
            .iter_refs()
            .any(|artifact| artifact.key.name == "project_analysis_roots")
    );
}

#[test]
fn reflecting_an_emitted_revision_restores_current_analysis_without_refreezing_its_contract() {
    let (catalog, expressions) = catalog(&[SOURCE], &["app.Worker"]);
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let store = NativeDefinitionStore::new(catalog, syntax.clone(), Default::default());
    let original = store.reflect_agent(expressions[0]).unwrap();
    let draft = store
        .with_body(&original, syntax.parse_body("return 2;").unwrap())
        .unwrap();
    let mut session = FrontendSession::new();
    let analysis = session
        .analyze_body_overlay(store.body_overlay(&draft).unwrap())
        .unwrap();
    assert_eq!(
        analysis.check_report(Default::default()).unwrap().status,
        CheckStatus::Accepted
    );
    assert_eq!(
        store.resolve(&draft).unwrap().current_analysis(),
        Err(AnalysisUnavailableReason::EditedBody)
    );
    let candidate = analysis.into_checked_candidate(Default::default()).unwrap();
    assert!(matches!(
        store.reflect_checked(&original, &candidate, Default::default()),
        Err(NativeDefinitionError::CandidateInputMismatch)
    ));
    let reflected = store
        .reflect_checked(&draft, &candidate, Default::default())
        .unwrap();
    let view = store.resolve(&reflected).unwrap();
    assert!(view.current_analysis().is_ok());
    assert_eq!(
        store.resolve(&draft).unwrap().current_analysis(),
        Err(AnalysisUnavailableReason::EditedBody)
    );
    assert!(Arc::ptr_eq(
        view.environment().unwrap(),
        store.resolve(&original).unwrap().environment().unwrap()
    ));
    let implementation = view.checked_implementation().unwrap();
    assert!(Arc::ptr_eq(implementation.program(), candidate.program()));
    let weak = Arc::downgrade(candidate.program());
    drop(candidate);
    assert!(weak.upgrade().is_some());
    assert_eq!(syntax.format(view.body().unwrap()).unwrap(), "return 2;");
    let next = store
        .with_body(&reflected, syntax.parse_body("return 3;").unwrap())
        .unwrap();
    assert_eq!(
        next.revision().inputs().parent,
        Some(reflected.revision().key())
    );
    assert_eq!(
        next.revision().inputs().contract,
        original.revision().inputs().contract
    );
    assert!(
        store
            .resolve(&next)
            .unwrap()
            .checked_implementation()
            .is_none()
    );
    assert!(
        view.current_analysis_descriptors()
            .unwrap()
            .effect_row(view.current_analysis().unwrap().effects.escaping.unwrap())
            .is_some()
    );
}

#[test]
fn compiled_body_visibility_inherits_the_original_disclosure_policy() {
    let (catalog, expressions) = catalog(
        &[
            "module app; public agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
            "module foreign; import app.Worker; flow inspect() -> unit { let d = ^Worker; return; }",
        ],
        &["app.Worker"],
    );
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let store = NativeDefinitionStore::new(catalog, syntax.clone(), Default::default());
    let body = syntax.parse_body("return 2;").unwrap();
    let visible = store.reflect_agent(expressions[0]).unwrap();
    let hidden = store.reflect_agent(expressions[1]).unwrap();
    let visible = store.with_body(&visible, body.clone()).unwrap();
    let hidden = store.with_body(&hidden, body).unwrap();
    assert_eq!(visible.revision(), hidden.revision());
    let mut session = FrontendSession::new();
    for (definition, may_read_body) in [(visible, true), (hidden, false)] {
        assert!(store.resolve(&definition).unwrap().body().is_some());
        let candidate = session
            .compile_body_overlay(store.body_overlay(&definition).unwrap())
            .unwrap();
        let reflected = store
            .reflect_checked(&definition, &candidate, Default::default())
            .unwrap();
        let view = store.resolve(&reflected).unwrap();
        assert_eq!(view.body().is_some(), may_read_body);
        assert!(view.current_analysis().is_ok());
        let again = session
            .compile_body_overlay(store.body_overlay(&reflected).unwrap())
            .unwrap();
        assert_eq!(again.revision(), reflected.revision());
        assert_eq!(
            store.resolve(&reflected).unwrap().body().is_some(),
            may_read_body
        );
    }
}

#[test]
fn a_candidate_catalog_cannot_establish_a_new_broader_replacement_baseline() {
    let (catalog, expressions) = catalog(&[SOURCE], &[]);
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let store = NativeDefinitionStore::new(catalog, syntax.clone(), Default::default());
    let original = store.reflect_agent(expressions[0]).unwrap();
    let draft = store
        .with_body(&original, syntax.parse_body("return 2;").unwrap())
        .unwrap();
    let analysis = FrontendSession::new()
        .analyze_body_overlay(store.body_overlay(&draft).unwrap())
        .unwrap();
    let candidate_catalog = analysis
        .definition_catalog(Default::default())
        .unwrap()
        .unwrap();
    assert!(matches!(
        candidate_catalog.freeze_agent(etas_hir::HirExprId(0)),
        Err(etas_frontend::code::descriptor::ProjectionError::RuntimeRevisionRequired)
    ));
    let foreign = NativeDefinitionStore::new(candidate_catalog, syntax, Default::default());
    assert!(matches!(
        foreign.reflect_agent(etas_hir::HirExprId(0)),
        Err(NativeDefinitionError::MissingRevisionContext)
    ));
    let candidate = analysis.into_checked_candidate(Default::default()).unwrap();
    store
        .reflect_checked(&draft, &candidate, Default::default())
        .unwrap();
}

#[test]
fn native_check_and_compile_consume_the_actual_definition_not_a_prior_report() {
    use etas_frontend::code::service::NativeCompilerError;
    let (catalog, expressions) = catalog(&[SOURCE], &[]);
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let store = NativeDefinitionStore::new(catalog, syntax.clone(), Default::default());
    let original = store.reflect_agent(expressions[0]).unwrap();
    let good = store
        .with_body(&original, syntax.parse_body("return 2;").unwrap())
        .unwrap();
    let mut session = FrontendSession::new();
    let report = store
        .check_definition(&good, &mut session, Default::default())
        .unwrap();
    assert_eq!(report.status, CheckStatus::Accepted);
    assert_eq!(&report.revision, good.revision());
    assert_eq!(
        store.resolve(&good).unwrap().current_analysis(),
        Err(AnalysisUnavailableReason::EditedBody)
    );
    let fresh = store
        .with_body(&original, syntax.parse_body("return 3;").unwrap())
        .unwrap();
    let candidate = store
        .compile_definition(&fresh, &mut session, Default::default())
        .unwrap();
    assert_eq!(candidate.revision(), fresh.revision());
    let bad = store
        .with_body(&original, syntax.parse_body("return \"wrong\";").unwrap())
        .unwrap();
    let rejected = store
        .check_definition(&bad, &mut session, Default::default())
        .unwrap();
    assert_eq!(rejected.status, CheckStatus::Rejected);
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "type::TypeMismatch")
    );
    match store.compile_definition(&bad, &mut session, Default::default()) {
        Err(NativeCompilerError::Rejected(report)) => assert_eq!(&report.revision, bad.revision()),
        _ => panic!("invalid source must be a normal rejection, not native compiler evidence"),
    }
}

#[test]
fn native_bare_agent_check_returns_a_rejected_report_with_real_source_coordinates() {
    use etas_frontend::code::service::NativeCompilerError;
    let (catalog, expressions) = catalog(
        &["module app; agent Bare; flow inspect() -> unit { let d = ^Bare; return; }"],
        &[],
    );
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let store = NativeDefinitionStore::new(catalog, syntax.clone(), Default::default());
    let original = store.reflect_agent(expressions[0]).unwrap();
    let body = syntax.parse_body("return 2;").unwrap();
    let edited = store.with_body(&original, body.clone()).unwrap();
    let mut session = FrontendSession::new();
    for definition in [&original, &edited] {
        let report = store
            .check_definition(definition, &mut session, Default::default())
            .unwrap();
        assert_eq!(report.status, CheckStatus::Rejected);
        assert!(report.effects.escaping.is_none());
        assert!(report.effects.requested.is_none());
        assert!(report.determinism.is_none());
        assert!(report.trace.is_none());
        let diagnostic = &report.diagnostics[0];
        assert_eq!(diagnostic.code, "code::MissingDefaultEntry");
        let location = diagnostic.location.as_ref().unwrap();
        assert!(location.byte_end > location.byte_start);
        if definition.revision() == edited.revision() {
            assert_eq!(location.source, body.identity().source());
            assert_eq!(location.revision, body.identity().content());
            assert_eq!(
                (location.byte_start, location.byte_end),
                (0, "return 2;".len())
            );
        }
        assert!(matches!(
            store.compile_definition(definition, &mut session, Default::default()),
            Err(NativeCompilerError::Rejected(_))
        ));
    }
}

#[test]
fn native_compilation_of_unretained_source_does_not_expose_its_body() {
    let (catalog, expressions) = catalog(&[SOURCE], &[]);
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let store = NativeDefinitionStore::new(catalog, syntax, Default::default());
    let original = store.reflect_agent(expressions[0]).unwrap();
    let mut session = FrontendSession::new();
    let candidate = store
        .compile_definition(&original, &mut session, Default::default())
        .unwrap();
    assert_eq!(candidate.revision(), original.revision());
    assert!(store.resolve(&original).unwrap().body().is_none());
    let reflected = store
        .reflect_checked(&original, &candidate, Default::default())
        .unwrap();
    assert!(store.resolve(&reflected).unwrap().body().is_none());
    assert!(
        store
            .resolve(&reflected)
            .unwrap()
            .current_analysis()
            .is_ok()
    );
}

#[test]
fn generic_default_methods_keep_original_binders_through_native_definition_edits() {
    let (catalog, expressions) = catalog(
        &[
            "module app; agent Worker; impl agent Worker { public flow run<T>(value: T) -> i32 { return 1; } } flow inspect() -> unit { let d = ^Worker; return; }",
        ],
        &["app.Worker"],
    );
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let store = NativeDefinitionStore::new(catalog, syntax.clone(), Default::default());
    let original = store.reflect_agent(expressions[0]).unwrap();
    let edited = store
        .with_body(
            &original,
            syntax.parse_body("let result = value; return 2;").unwrap(),
        )
        .unwrap();
    let mut session = FrontendSession::new();
    let candidate = store
        .compile_definition(&edited, &mut session, Default::default())
        .unwrap();
    let current = store
        .reflect_checked(&edited, &candidate, Default::default())
        .unwrap();
    let original = store.resolve(&original).unwrap();
    let current = store.resolve(&current).unwrap();
    assert_eq!(original.signature(), current.signature());
    assert!(current.signature().is_some());
    assert!(current.current_analysis().is_ok());
    assert!(Arc::ptr_eq(
        original.environment().unwrap(),
        current.environment().unwrap()
    ));
}

#[test]
fn generic_callable_outputs_require_latent_behavior_proof_instead_of_empty_summaries() {
    let (catalog, expressions) = catalog(
        &[
            "module app; agent Worker; impl agent Worker { public flow run<T>(value: T) -> T { return value; } } flow inspect() -> unit { let d = ^Worker; return; }",
        ],
        &["app.Worker"],
    );
    let syntax = Arc::new(NativeCodeSyntaxOperations::default());
    let store = NativeDefinitionStore::new(catalog, syntax.clone(), Default::default());
    let original = store.reflect_agent(expressions[0]).unwrap();
    let edited = store
        .with_body(
            &original,
            syntax
                .parse_body("let result = value; return result;")
                .unwrap(),
        )
        .unwrap();
    let mut session = FrontendSession::new();
    let report = store
        .check_definition(&edited, &mut session, Default::default())
        .unwrap();
    assert_eq!(report.status, CheckStatus::Rejected);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "code::MissingLatentBehaviorSummary")
    );
    assert!(matches!(
        store.compile_definition(&edited, &mut session, Default::default()),
        Err(etas_frontend::code::service::NativeCompilerError::Rejected(
            _
        ))
    ));
}
