use std::collections::HashSet;

use etas_effects::EffectUnit;
use etas_frontend::{
    CheckRequest, FrontendSession, ProjectChangeSet, ProjectInput, ProjectOutput, ProjectRevision,
    SourceChange, SourceInput, SourceVersion,
};

fn check(source: String) -> ProjectOutput {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    let response = session
        .check(project, CheckRequest::full_project())
        .unwrap();
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics
    );
    response.output
}

fn assert_partition(output: &ProjectOutput) -> usize {
    let hir = output.hir.as_ref().unwrap();
    let global = &output.effects.as_ref().unwrap().facts;
    let mut expressions = HashSet::new();
    let mut statements = HashSet::new();
    let mut units = HashSet::new();
    let mut contracts = HashSet::new();
    let mut environments = HashSet::new();
    let mut captures = HashSet::new();
    let owners = output
        .effect_body_outputs
        .values()
        .map(|body| body.owner)
        .collect::<HashSet<_>>();
    let mut encoded_bytes = 0;
    for body in output.effect_body_outputs.values() {
        let facts = &body.facts;
        for (id, snapshot) in &facts.callable_environments {
            assert_eq!(snapshot.owner, body.owner);
            assert!(
                environments.insert(*id),
                "captured environment duplicated across bodies"
            );
            assert_eq!(snapshot, &global.callable_environments[id]);
        }
        for (id, analysis) in &facts.captured_callables {
            assert_eq!(
                global.callable_environments[&id.environment].owner,
                body.owner
            );
            assert!(
                captures.insert(*id),
                "capture descriptor duplicated across bodies"
            );
            assert_eq!(analysis, &global.captured_callables[id]);
        }
        assert!(facts.interpreter_support.entry.is_none());
        assert!(facts.item_effects.keys().all(|item| *item == body.owner));
        for expr in facts.expr_effects.keys() {
            assert_eq!(hir.hir.index().expr_item[expr], body.owner);
            assert!(
                expressions.insert(*expr),
                "expression duplicated across bodies"
            );
            assert_eq!(facts.expr_effects[expr], global.expr_effects[expr]);
        }
        for stmt in facts.stmt_effects.keys() {
            assert_eq!(hir.hir.index().stmt_item[stmt], body.owner);
            assert!(
                statements.insert(*stmt),
                "statement duplicated across bodies"
            );
        }
        for unit in facts.unit_effects.keys() {
            let owner = match unit {
                EffectUnit::Item(item) => *item,
                EffectUnit::AnonymousFlow { owner, .. } | EffectUnit::HandlerArm { owner, .. } => {
                    *owner
                }
            };
            assert_eq!(owner, body.owner);
            assert!(
                units.insert(*unit),
                "analysis unit duplicated across bodies"
            );
        }
        for contract in &facts.public_contracts {
            assert_eq!(contract.item, body.owner);
            assert!(contracts.insert(contract.item));
        }
        for ids in [
            facts.handler_values.keys().copied().collect::<Vec<_>>(),
            facts.handle_applications.keys().copied().collect(),
            facts.performed_actions.keys().copied().collect(),
            facts.try_captures.keys().copied().collect(),
            facts.latent_effects.keys().copied().collect(),
        ] {
            assert!(
                ids.iter()
                    .all(|id| hir.hir.index().expr_item[id] == body.owner)
            );
        }
        assert!(facts.expr_effects.capacity() <= 4 * facts.expr_effects.len());
        assert!(facts.stmt_effects.capacity() <= 4 * facts.stmt_effects.len());
        assert!(facts.unit_effects.capacity() <= 4 * facts.unit_effects.len());
        encoded_bytes += bincode::serde::encode_to_vec(body, bincode::config::standard())
            .unwrap()
            .len();
    }
    // Top-level initializers have project facts but no frontend Body unit. They
    // remain on the project output, never duplicated into unrelated callables.
    assert_eq!(
        expressions,
        global
            .expr_effects
            .keys()
            .copied()
            .filter(|expr| owners.contains(&hir.hir.index().expr_item[expr]))
            .collect()
    );
    assert_eq!(
        statements,
        global
            .stmt_effects
            .keys()
            .copied()
            .filter(|stmt| owners.contains(&hir.hir.index().stmt_item[stmt]))
            .collect()
    );
    assert_eq!(
        units,
        global
            .unit_effects
            .keys()
            .copied()
            .filter(|unit| {
                let owner = match unit {
                    EffectUnit::Item(item) => item,
                    EffectUnit::AnonymousFlow { owner, .. }
                    | EffectUnit::HandlerArm { owner, .. } => owner,
                };
                owners.contains(owner)
            })
            .collect()
    );
    assert_eq!(
        contracts,
        global
            .public_contracts
            .iter()
            .map(|contract| contract.item)
            .filter(|item| owners.contains(item))
            .collect()
    );
    assert_eq!(
        environments,
        global
            .callable_environments
            .iter()
            .filter(|(_, snapshot)| owners.contains(&snapshot.owner))
            .map(|(id, _)| *id)
            .collect()
    );
    assert_eq!(
        captures,
        global
            .captured_callables
            .keys()
            .filter(|id| owners.contains(&global.callable_environments[&id.environment].owner))
            .copied()
            .collect()
    );
    encoded_bytes
}

#[test]
fn captured_callable_environments_are_owned_and_survive_body_codec_roundtrip() {
    for (source, generic) in [
        (
            r#"module main;
flow make(f: () -> unit ![]) -> (() -> unit ![]) ![] { return () => { f(); return; }; }
flow first() -> unit ![] { return; }
flow second() -> unit ![] { return; }
flow main() -> unit ![] { let a = make(first); let b = make(second); a(); b(); return; }
"#,
            false,
        ),
        (
            r#"module main;
flow make<T, effect E>(f: (T) -> T ![E]) -> ((T) -> T ![E]) ![] { return (value: T) => { return f(value); }; }
flow first(value: i32) -> i32 ![] { return value; }
flow second(value: string) -> string ![] { return value; }
flow main() -> unit ![] {
    let a = make<i32, ![]>(first); let b = make<string, ![]>(second);
    a(1); b("second"); return;
}
"#,
            true,
        ),
    ] {
        let output = check(source.into());
        assert_partition(&output);
        let facts = &output.effects.as_ref().unwrap().facts;
        assert!(!facts.callable_environments.is_empty());
        assert!(!facts.captured_callables.is_empty());
        assert_eq!(
            facts
                .callable_environments
                .values()
                .any(|snapshot| !snapshot.generics.is_empty()),
            generic
        );
        for body in output.effect_body_outputs.values() {
            let bytes = bincode::serde::encode_to_vec(body, bincode::config::standard()).unwrap();
            let (decoded, consumed): (etas_effects::BodyEffectOutput, _) =
                bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
            assert_eq!(consumed, bytes.len());
            assert_eq!(
                decoded.facts.callable_environments,
                body.facts.callable_environments
            );
            assert_eq!(
                decoded.facts.captured_callables,
                body.facts.captured_callables
            );
        }
    }
}

#[test]
fn body_effect_payload_and_capacity_scale_with_local_facts() {
    let mut sizes = Vec::new();
    for count in [16, 32, 64, 128] {
        let mut source = "module main;\n".to_owned();
        for i in 0..count {
            source.push_str(&format!(
                "public flow f_{i}(x: i32) -> i32 {{ let y = x + 0; return y; }}\n"
            ));
        }
        source.push_str("flow main() -> i32 { return f_0(1); }");
        let builds_before = etas_hir::HirTreeIndex::thread_build_count();
        let output = check(source);
        // Lowering and import application each finalize a HIR revision. Analysis
        // of any number of bodies must reuse the latter revision's index.
        assert_eq!(
            etas_hir::HirTreeIndex::thread_build_count() - builds_before,
            2
        );
        assert_eq!(output.effect_body_outputs.len(), count + 1);
        sizes.push(assert_partition(&output));
    }
    for pair in sizes.windows(2) {
        assert!(
            pair[1] < pair[0] * 3,
            "body payload must grow linearly: {sizes:?}"
        );
    }
}

const NESTED: &str = r#"
module main;
effect Gate { action request() -> i32; }
public let H: ![Gate => []] = handler { Gate.request() => { resume 7; } };
public flow nested() -> i32 {
    let later = () => {
        let local: ![Gate => []] = handler { Gate.request() => { resume 8; } };
        return perform Gate.request() with local;
    };
    return later();
}
public flow untouched() -> unit { return; }
flow main() -> i32 { return nested(); }
"#;

#[test]
fn nested_lambda_and_handler_facts_have_one_body_owner() {
    let output = check(NESTED.into());
    assert_partition(&output);
    let facts = &output.effects.as_ref().unwrap().facts;
    assert!(!facts.handler_values.is_empty());
    assert!(!facts.latent_effects.is_empty());
    assert!(!facts.performed_actions.is_empty());
    assert!(
        facts
            .unit_effects
            .keys()
            .any(|unit| matches!(unit, EffectUnit::HandlerArm { .. }))
    );
    assert!(
        facts
            .unit_effects
            .keys()
            .any(|unit| matches!(unit, EffectUnit::AnonymousFlow { .. }))
    );
}

#[test]
fn all_effect_stages_share_the_finalized_hir_index() {
    let output = check(NESTED.into());
    let hir = output.hir.as_ref().unwrap();
    let view = hir.view();
    let context = etas_hir_analysis::HirAnalysisContext::new(&hir.hir);
    let other = context.clone();
    assert!(std::ptr::eq(view.index(), other.view().index()));
    let before = etas_hir::HirTreeIndex::thread_build_count();
    let result = etas_effects::RunEffectPipeline::run(etas_effects::EffectPipelineInput {
        external_implementations: &[],
        frozen_calls: None,
        hir: &hir.hir,
        types: output.types.as_ref().unwrap(),
        std_registry: &etas_std::standard_registry(),
        dependency_metadata: None,
        tool_bindings: &[],
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .unwrap();
    assert_eq!(etas_hir::HirTreeIndex::thread_build_count(), before);
    let expected = &output.effects.as_ref().unwrap().facts;
    assert_eq!(result.effects.facts.item_effects, expected.item_effects);
    assert_eq!(result.effects.facts.unit_effects, expected.unit_effects);
    assert_eq!(result.effects.facts.expr_effects, expected.expr_effects);
    assert_eq!(result.effects.facts.stmt_effects, expected.stmt_effects);
    assert!(result.effects.diagnostics.is_empty());
    for stage in [
        "effects.analyze_memory_provenance",
        "effects.solve_summaries",
        "effects.trace_spec.analyze_monitors",
    ] {
        assert!(
            result
                .artifacts
                .pass_timings
                .iter()
                .any(|timing| timing.pass == stage)
        );
    }
}

#[test]
fn edited_body_does_not_retain_sibling_effect_facts() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(NESTED)));
    let first = session
        .check(project, CheckRequest::full_project())
        .unwrap();
    assert_partition(&first.output);
    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source: etas_core::SourceId(0),
                    version: SourceVersion(1),
                    text: NESTED.replace("return later();", "return 1;"),
                }],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .unwrap();
    let next = session.check(project, CheckRequest::incremental()).unwrap();
    assert!(next.output.checked.is_some(), "{:?}", next.diagnostics);
    assert_partition(&next.output);
    let cold = check(NESTED.replace("return later();", "return 1;"));
    assert_eq!(
        next.output.effects.as_ref().unwrap().facts.item_effects,
        cold.effects.as_ref().unwrap().facts.item_effects,
    );
}
