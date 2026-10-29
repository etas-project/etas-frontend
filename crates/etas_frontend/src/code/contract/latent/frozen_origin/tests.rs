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

fn prepared() -> (DefaultBodyOverlay, CheckedProgram) {
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
        .compile_body_overlay(overlay.clone())
        .unwrap();
    (overlay, candidate.program().clone())
}

fn proven(overlay: &DefaultBodyOverlay, candidate: &CheckedProgram) -> bool {
    let frozen = overlay.frozen();
    let preservation =
        crate::code::contract::preservation::BodyPreservation::verify(overlay, candidate).unwrap();
    let id = frozen.contract().entry().unwrap();
    let signature = frozen.contract().callable(id).unwrap().signature;
    preserves(
        &preservation,
        id,
        frozen.contract().descriptors(),
        signature,
        candidate.entry.unwrap(),
        Default::default(),
    )
    .unwrap()
}

#[test]
fn returning_the_same_pinned_flow_preserves_its_executable_origin() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        "module app; flow helper(value: bool) -> bool ![] { return value; } flow other(value: bool) -> bool ![] { return value; } flow factory() -> ((bool) -> bool ![]) ![] { return helper; } flow inspect() -> unit { let definition = ^factory; return; }",
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.frozen-flow-origin",
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
        .freeze_flow(reflection)
        .unwrap();
    let syntax = NativeCodeSyntaxOperations::default();
    let overlay = DefaultBodyOverlay::prepare(
        frozen.clone(),
        syntax
            .parse_body("let selected = helper; return selected;")
            .unwrap(),
        &syntax,
    )
    .unwrap();
    let analysis = FrontendSession::new()
        .analyze_body_overlay(overlay.clone())
        .unwrap();
    let candidate = analysis.output().checked.as_ref().unwrap();
    assert!(
        proven(&overlay, candidate),
        "{:?}",
        candidate.effects.item_effects[&candidate.entry.unwrap()].exits,
    );
    assert_eq!(
        analysis.check_report(Default::default()).unwrap().status,
        etas_code::report::CheckStatus::Accepted,
    );
    let unrelated =
        DefaultBodyOverlay::prepare(frozen, syntax.parse_body("return other;").unwrap(), &syntax)
            .unwrap();
    let analysis = FrontendSession::new()
        .analyze_body_overlay(unrelated.clone())
        .unwrap();
    assert!(!proven(
        &unrelated,
        analysis.output().checked.as_ref().unwrap()
    ));
    assert_eq!(
        analysis.check_report(Default::default()).unwrap().status,
        etas_code::report::CheckStatus::Rejected,
        "equal signatures and source contents cannot identify an executable origin",
    );
}

#[test]
fn returned_generic_reference_preserves_type_and_effect_arguments() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        "module app; flow helper<T, effect E>(value: T, f: () -> unit ![E]) -> T ![E] { f(); return value; } flow factory<T, effect E>() -> ((T, () -> unit ![E]) -> T ![E]) ![] { let selected: (T, () -> unit ![E]) -> T ![E] = helper; return selected; } flow inspect() -> unit { let definition = ^factory; return; }",
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.generic-flow-origin",
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
        .freeze_flow(reflection)
        .unwrap();
    let syntax = NativeCodeSyntaxOperations::default();
    let overlay = DefaultBodyOverlay::prepare(
        frozen,
        syntax
            .parse_body("let renamed: (T, () -> unit ![E]) -> T ![E] = helper; return renamed;")
            .unwrap(),
        &syntax,
    )
    .unwrap();
    let analysis = FrontendSession::new()
        .analyze_body_overlay(overlay.clone())
        .unwrap();
    let candidate = analysis.output().checked.as_ref().unwrap();
    assert!(proven(&overlay, candidate));
    assert_eq!(
        analysis.check_report(Default::default()).unwrap().status,
        etas_code::report::CheckStatus::Accepted
    );
    let owner = candidate.entry.unwrap();
    let source = candidate.effects.item_effects[&owner]
        .exits
        .as_ref()
        .unwrap()
        .callable_return_sources()
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let Source::Reference(reference) = source else {
        panic!("expected an instantiated reference")
    };
    for change in 0..3 {
        let mut tampered = candidate.clone();
        let boolean = tampered
            .type_store
            .iter()
            .find_map(|(id, ty)| {
                matches!(
                    ty,
                    etas_types::Type::Primitive(etas_types::PrimitiveType::Bool)
                )
                .then_some(id)
            })
            .unwrap();
        let facts = Arc::make_mut(&mut Arc::make_mut(&mut tampered).effects);
        let solved = facts.callable_references.remove(&reference).unwrap();
        if change != 2 {
            let mut altered = reference.clone();
            if change == 0 {
                altered.type_bindings[0].1 = boolean;
            } else {
                altered.effect_row_bindings[0].1.tail = None;
            }
            facts.callable_references.insert(altered.clone(), solved);
            for exit in &mut facts
                .item_effects
                .get_mut(&owner)
                .unwrap()
                .exits
                .as_mut()
                .unwrap()
                .exits
            {
                if let etas_effects::CallableOutcome::Returned {
                    observation: etas_effects::ReturnObservation::CallableSources(sources),
                    ..
                } = &mut exit.outcome
                {
                    assert!(sources.remove(&Source::Reference(reference.clone())));
                    sources.insert(Source::Reference(altered.clone()));
                }
            }
        }
        assert!(!proven(&overlay, &tampered), "change {change}");
    }
}

#[test]
fn a_checked_frozen_origin_proof_does_not_require_an_old_lambda_in_candidate_hir() {
    let (overlay, candidate) = prepared();
    assert!(proven(&overlay, &candidate));
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
    let (overlay, candidate) = prepared();
    let frozen = overlay.frozen().clone();
    drop(overlay);
    let syntax = NativeCodeSyntaxOperations::default();
    let body = syntax.parse_body("return Worker(value);").unwrap();
    let id = frozen.contract().entry().unwrap();
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
    let (overlay, mut candidate) = prepared();
    assert!(proven(&overlay, &candidate));
    Arc::make_mut(&mut Arc::make_mut(&mut candidate).effects)
        .frozen_callable_graphs
        .clear();
    assert!(!candidate.effects.metadata_callable_graphs.is_empty());
    assert!(!proven(&overlay, &candidate));
}

#[test]
fn frozen_origin_evidence_is_bound_to_environment_identity_and_checked_graph() {
    let (overlay, candidate) = prepared();
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
        assert!(!proven(&overlay, &tampered), "change {change}");
    }
}
