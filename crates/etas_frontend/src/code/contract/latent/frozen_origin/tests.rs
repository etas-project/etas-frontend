use super::*;
use crate::{
    EntryPolicy, Frontend, FrontendSession, ProjectInput, SourceInput,
    code::{
        definition::DefinitionCatalog, draft::DefaultBodyOverlay, identity::DefinitionOrigins,
        service::NativeCodeSyntaxOperations,
    },
};
use etas_code::{CodeFingerprint, PackageKey, syntax::CodeSyntaxOperations};
use std::{collections::BTreeMap, sync::Arc};

fn prepared() -> (Arc<FrozenDefinitionEnvironment>, CheckedProgram) {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        "module app; agent Worker(value: i32) -> (() -> i32 ![]) { return () => value; } flow inspect() -> unit { let definition = ^Worker; return; }",
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.frozen-origin",
        b"app",
    ));
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
        standard_package: package,
        external_definitions: BTreeMap::new(),
    };
    let reflection = *checked.types.reflections.keys().next().unwrap();
    let frozen = DefinitionCatalog::build(checked, &origins, Default::default())
        .unwrap()
        .freeze_agent(reflection)
        .unwrap();
    let syntax = NativeCodeSyntaxOperations::default();
    let overlay = DefaultBodyOverlay::prepare(
        frozen.clone(),
        syntax.parse_body("return Worker(value);").unwrap(),
        &syntax,
    )
    .unwrap();
    let candidate = FrontendSession::new()
        .compile_body_overlay(overlay)
        .unwrap();
    (frozen, candidate.program().clone())
}

fn proven(frozen: &FrozenDefinitionEnvironment, candidate: &CheckedProgram) -> bool {
    let id = frozen.contract().header().default_entry.as_ref().unwrap();
    let signature = frozen.contract().callable(id).unwrap().signature;
    preserves(
        frozen,
        id,
        frozen.contract().descriptors(),
        signature,
        candidate,
        candidate.entry.unwrap(),
    )
    .unwrap()
}

#[test]
fn a_checked_frozen_origin_proof_does_not_require_an_old_lambda_in_candidate_hir() {
    let (frozen, candidate) = prepared();
    assert!(proven(&frozen, &candidate));
    assert!(
        !candidate
            .hir
            .exprs
            .iter()
            .any(|(_, expr)| matches!(expr, etas_hir::HirExpr::Lambda { .. }))
    );
}

#[test]
fn draft_overlays_share_the_frozen_callable_graph_without_copying_it() {
    let (frozen, candidate) = prepared();
    let syntax = NativeCodeSyntaxOperations::default();
    let body = syntax.parse_body("return Worker(value);").unwrap();
    let id = frozen.contract().header().default_entry.as_ref().unwrap();
    let graph = frozen.contract().latent_graph(id).unwrap();
    let weak_graph = Arc::downgrade(graph);
    let shared = etas_core::memory::MemoryFootprint::of(&frozen);
    let shared_allocations = shared
        .allocations
        .iter()
        .map(|allocation| allocation.id)
        .collect::<std::collections::BTreeSet<_>>();
    let mut private_per_draft = None;
    for count in [1, 10, 100] {
        let drafts = (0..count)
            .map(|_| DefaultBodyOverlay::prepare(frozen.clone(), body.clone(), &syntax).unwrap())
            .collect::<Vec<_>>();
        for draft in &drafts {
            assert!(Arc::ptr_eq(
                graph,
                draft.frozen().contract().latent_graph(id).unwrap()
            ));
        }
        let retained = etas_core::memory::MemoryFootprint::of(&drafts);
        let private_bytes: u64 = retained
            .allocations
            .iter()
            .filter(|allocation| !shared_allocations.contains(&allocation.id))
            .map(|allocation| allocation.estimated_bytes)
            .sum();
        assert!(private_bytes <= *private_per_draft.get_or_insert(private_bytes) * count as u64);
        eprintln!(
            "frozen callable drafts={count} shared_bytes={} private_bytes={private_bytes} total_bytes={}",
            shared.estimated_bytes(),
            retained.estimated_bytes()
        );
    }
    drop(candidate);
    drop(frozen);
    assert!(weak_graph.upgrade().is_none());
}

#[test]
fn an_equal_graph_digest_without_compiler_origin_evidence_is_not_a_proof() {
    let (frozen, mut candidate) = prepared();
    assert!(proven(&frozen, &candidate));
    Arc::make_mut(&mut Arc::make_mut(&mut candidate).effects)
        .frozen_callable_graphs
        .clear();
    assert!(!candidate.effects.metadata_callable_graphs.is_empty());
    assert!(!proven(&frozen, &candidate));
}

#[test]
fn frozen_origin_evidence_is_bound_to_environment_identity_and_checked_graph() {
    let (frozen, candidate) = prepared();
    for change in 0..3 {
        let mut tampered = candidate.clone();
        let facts = Arc::make_mut(&mut Arc::make_mut(&mut tampered).effects);
        match change {
            0 => facts.frozen_callable_graphs.values_mut().for_each(|proof| {
                proof.environment =
                    CodeFingerprint::of_bytes("etas.test.wrong-environment", b"wrong")
            }),
            1 => facts.frozen_callable_graphs.values_mut().for_each(|proof| {
                proof.graph = CodeFingerprint::of_bytes("etas.test.wrong-graph", b"wrong")
            }),
            _ => facts.metadata_callable_graphs.clear(),
        }
        assert!(!proven(&frozen, &tampered), "change {change}");
    }
}
