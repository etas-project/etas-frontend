use etas_core::{DiagnosticCode, EffectDiagnosticCode, Span, SyntaxDiagnosticCode, TextSize};
use etas_effects::{
    AGENTIC_INFER_ACTION, AGENTIC_TAG, APPROVAL_REQUEST_ACTION, APPROVAL_TAG, ActionEvent,
    ActionEventSource, ActionRef, ActionTraceDomain, COMMAND_RUN_ACTION,
    CONSOLE_STDOUT_WRITE_ACTION, CONSOLE_TAG, CoreEffect, Determinism, Effect, EffectActionArgKind,
    EffectFacts, EffectRegistry, EffectRow, EffectSet, EffectSummary, EffectTagId, EffectUnit,
    ExtensionGraph, FILE_IO_TAG, FrontendRejectionReason, HUMAN_TAG, HandlerValueRef,
    HostRequirementKind, HostRequirementSet, InterpreterSupport, LimitBudgetKind, LimitKind,
    LimitRequirement, LimitValue, MEMORY_READ_ACTION, MEMORY_TAG, MEMORY_WRITE_ACTION, NETWORK_TAG,
    RequirementFact, RequirementSet, ResumeSummary, RunEffectPipeline, RuntimeRequirementReason,
    TIME_TAG,
};
use etas_hir::{
    HirExpr, HirItem, ImportAliasOrigin, ResolveResult, SymbolData, SymbolDef, SymbolId,
    SymbolKind, Visibility, lower_program,
};
use etas_utils::JoinSemiLattice;

fn test_std_registry() -> &'static etas_std::StdRegistry {
    static REGISTRY: std::sync::OnceLock<etas_std::StdRegistry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(etas_std::standard_registry)
}

fn check_program(
    hir: &etas_hir::HirProgram,
    types: &etas_types::TypeOutput,
) -> etas_effects::EffectOutput {
    RunEffectPipeline::run(etas_effects::EffectPipelineInput {
        hir,
        types,
        std_registry: test_std_registry(),
        dependency_metadata: None,
        tool_bindings: &[],
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .expect("effect pipeline should run")
    .effects
}

fn local_only() -> InterpreterSupport {
    InterpreterSupport::LocalOnly
}

fn is_approval_request_effect(effect: &Effect) -> bool {
    matches!(
        effect,
        Effect::Tag(APPROVAL_TAG)
            | Effect::Action(ActionRef {
                tag: APPROVAL_TAG,
                action: APPROVAL_REQUEST_ACTION
            })
            | Effect::AppliedAction(etas_effects::ActionInstanceRef {
                action: ActionRef {
                    tag: APPROVAL_TAG,
                    action: APPROVAL_REQUEST_ACTION
                },
                ..
            })
    )
}

fn is_action_effect(effect: &Effect) -> bool {
    matches!(effect, Effect::Action(_) | Effect::AppliedAction(_))
}

fn is_agentic_infer_effect(effect: &Effect) -> bool {
    matches!(
        effect,
        Effect::Action(ActionRef {
            tag: AGENTIC_TAG,
            action: AGENTIC_INFER_ACTION
        }) | Effect::AppliedAction(etas_effects::ActionInstanceRef {
            action: ActionRef {
                tag: AGENTIC_TAG,
                action: AGENTIC_INFER_ACTION
            },
            ..
        })
    )
}

fn has_agentic_infer(row: &EffectRow) -> bool {
    row.effects.iter().any(is_agentic_infer_effect)
}

fn has_agentic_escape(row: &EffectRow) -> bool {
    row.effects.iter().any(|effect| {
        matches!(effect, Effect::Tag(tag) if *tag == AGENTIC_TAG) || is_agentic_infer_effect(effect)
    })
}

fn trace_has_agentic_infer(trace: &ActionTraceDomain) -> bool {
    match trace {
        ActionTraceDomain::Empty => false,
        ActionTraceDomain::Event(event) => is_agentic_infer_effect(&event.action),
        ActionTraceDomain::Seq(parts) | ActionTraceDomain::Choice(parts) => {
            parts.iter().any(trace_has_agentic_infer)
        }
        ActionTraceDomain::Repeat(inner) => trace_has_agentic_infer(inner),
        ActionTraceDomain::UnknownOrder(actions) => actions.iter().any(is_agentic_infer_effect),
    }
}

fn trace_has_agentic_infer_source(trace: &ActionTraceDomain, source: ActionEventSource) -> bool {
    match trace {
        ActionTraceDomain::Empty => false,
        ActionTraceDomain::Event(event) => {
            is_agentic_infer_effect(&event.action) && event.source == source
        }
        ActionTraceDomain::Seq(parts) | ActionTraceDomain::Choice(parts) => parts
            .iter()
            .any(|part| trace_has_agentic_infer_source(part, source.clone())),
        ActionTraceDomain::Repeat(inner) => trace_has_agentic_infer_source(inner, source),
        ActionTraceDomain::UnknownOrder(_) => false,
    }
}

fn requires_host(kind: HostRequirementKind) -> InterpreterSupport {
    InterpreterSupport::RequiresHost(HostRequirementSet::one(kind))
}

fn support_requires_host(support: Option<&InterpreterSupport>, kind: HostRequirementKind) -> bool {
    match support {
        Some(InterpreterSupport::RequiresHost(requirements)) => requirements.kinds.contains(&kind),
        Some(InterpreterSupport::RequiresInterpreterOrchestration(requirements)) => {
            requirements.host.kinds.contains(&kind)
        }
        _ => false,
    }
}

#[test]
fn effect_registry_reads_substrate_actions_from_std_registry() {
    let registry = EffectRegistry::with_standard_effects();

    let network = registry
        .tag_by_core(CoreEffect::Network)
        .expect("Network core tag should exist");
    let file_io = registry
        .tag_by_core(CoreEffect::FileIO)
        .expect("FileIO core tag should exist");
    let net = registry
        .tag_by_name("Net")
        .expect("Net substrate tag should be registered");
    let fs = registry
        .tag_by_name("Fs")
        .expect("Fs substrate tag should be registered");
    let browser = registry
        .tag_by_name("Browser")
        .expect("Browser substrate tag should be registered");
    assert!(registry.tag_extends(net, network));
    assert!(registry.tag_extends(fs, file_io));
    assert!(registry.tag_extends(browser, network));

    for (name, expected_requirement) in [
        ("Net.tcp_connect", RuntimeRequirementReason::Tcp),
        ("Stream.read", RuntimeRequirementReason::Stream),
        ("Stream.write", RuntimeRequirementReason::Stream),
        ("Stream.flush", RuntimeRequirementReason::Stream),
        ("Stream.close", RuntimeRequirementReason::Stream),
        ("Tls.handshake", RuntimeRequirementReason::Tls),
        ("Fs.read", RuntimeRequirementReason::FileIO),
        ("Fs.write", RuntimeRequirementReason::FileIO),
        ("Fs.list", RuntimeRequirementReason::FileIO),
        ("Fs.stat", RuntimeRequirementReason::FileIO),
        ("Fs.atomic_replace", RuntimeRequirementReason::FileIO),
        ("Secret.read", RuntimeRequirementReason::SecretAccess),
        ("Browser.attach", RuntimeRequirementReason::Browser),
        ("Browser.send", RuntimeRequirementReason::Browser),
        ("Browser.recv", RuntimeRequirementReason::Browser),
        ("Browser.close", RuntimeRequirementReason::Browser),
    ] {
        let action = registry
            .action_by_name(name)
            .unwrap_or_else(|| panic!("{name} should resolve as a standard action"));
        let signature = registry
            .action_signature(&action)
            .unwrap_or_else(|| panic!("{name} should have a signature"));
        assert_eq!(
            signature.runtime_requirement,
            Some(expected_requirement.clone()),
            "{name} should carry runtime metadata from etas_std"
        );
    }

    let fs_write = registry
        .action_by_name("Fs.write")
        .expect("Fs.write should resolve");
    assert!(
        registry.action_requires_high_impact_ack(&fs_write),
        "workspace writes should be marked high impact"
    );

    let stream_read = registry
        .action_signature(
            &registry
                .action_by_name("Stream.read")
                .expect("Stream.read should resolve"),
        )
        .expect("Stream.read should have a signature");
    assert_eq!(
        stream_read.effect_args,
        vec![EffectActionArgKind::StaticResourcePath {
            ty: "ByteStream".to_owned()
        }]
    );
}

#[test]
fn standard_flow_effect_metadata_resolves_without_string_fallback() {
    let registry = EffectRegistry::with_standard_effects();
    let std = etas_std::standard_registry();

    for symbol in std.symbols() {
        let etas_std::StdDecl::Flow(flow) = &symbol.decl else {
            continue;
        };
        for effect in &flow.public_effects {
            let name = effect.path.join(".");
            assert!(
                registry.tag_by_name(&name).is_some(),
                "std flow `{}` has an unresolvable public effect `{:?}`",
                symbol.qualified_path.join("."),
                effect,
            );
        }
        for action in &flow.requested_actions {
            let name = action.path.join(".");
            assert!(
                registry.action_by_name(&name).is_some(),
                "std flow `{}` has an unresolvable requested action `{:?}`",
                symbol.qualified_path.join("."),
                action,
            );
        }
    }
}

fn flow_item(hir: &etas_hir::HirProgram, name: &str) -> etas_hir::HirItemId {
    hir.items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == name) =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("flow `{name}` should lower"))
}

fn type_id_by_symbol_name(
    hir: &etas_hir::HirProgram,
    types: &etas_types::TypeOutput,
    name: &str,
) -> etas_types::TypeId {
    hir.symbols
        .iter()
        .find_map(|symbol| {
            if symbol.name != name {
                return None;
            }
            match types.facts.symbol_types.get(&symbol.id)? {
                etas_types::SymbolTypeFact::Type { constructor }
                | etas_types::SymbolTypeFact::NominalType { constructor, .. } => {
                    Some(etas_types::TypeId(constructor.0))
                }
                etas_types::SymbolTypeFact::TypeAlias { target, .. } => Some(*target),
                _ => None,
            }
        })
        .unwrap_or_else(|| panic!("type `{name}` should lower"))
}

fn tool_item(hir: &etas_hir::HirProgram, name: &str) -> etas_hir::HirItemId {
    hir.items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Tool(tool)
                if hir
                    .symbols
                    .get(tool.symbol)
                    .is_some_and(|symbol| symbol.name == name) =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("tool `{name}` should lower"))
}

fn agent_item(hir: &etas_hir::HirProgram, name: &str) -> etas_hir::HirItemId {
    hir.items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Agent(agent)
                if hir
                    .symbols
                    .get(agent.symbol)
                    .is_some_and(|symbol| symbol.name == name) =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("agent `{name}` should lower"))
}

fn add_std_symbol_type_facts(hir: &etas_hir::HirProgram, output: &mut etas_types::TypeOutput) {
    let _ = (hir, output);
}

#[test]
fn extension_graph_computes_transitive_effect_extensions() {
    let file_io = EffectTagId(0);
    let read_pdf = EffectTagId(1);
    let read_invoice = EffectTagId(2);

    let mut graph = ExtensionGraph::default();
    graph.add_extension(read_pdf, file_io);
    graph.add_extension(read_invoice, read_pdf);

    assert!(graph.directly_extends(read_pdf, file_io));
    assert!(graph.extends(read_invoice, file_io));
    assert!(graph.extends(read_invoice, read_invoice));
    assert!(!graph.extends(file_io, read_invoice));
}

#[test]
fn effect_coverage_does_not_let_action_cover_sibling_action() {
    let registry = EffectRegistry::with_standard_effects();
    let type_store = etas_types::TypeStore::new();
    let coverage = etas_effects::EffectCoverage {
        registry: &registry,
        types: &type_store,
    };
    let console = Effect::Tag(CONSOLE_TAG);
    let stdout = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: CONSOLE_STDOUT_WRITE_ACTION,
    });
    let stdin = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: etas_effects::CONSOLE_STDIN_READ_LINE_ACTION,
    });

    assert!(coverage.covers(&console, &stdout));
    assert!(coverage.covers(&stdout, &stdout));
    assert!(
        !coverage.covers(&stdout, &stdin),
        "a concrete action pattern must not cover sibling actions through tag coverage"
    );
}

#[test]
fn effect_coverage_allows_parent_memory_place_to_cover_child_place() {
    let mut interner = etas_types::TypeInterner::new();
    let project_memory =
        interner.intern(etas_types::Type::MemoryPlace(etas_types::MemoryPlaceType {
            segments: vec!["ProjectMemory".to_owned()],
        }));
    let papers_memory =
        interner.intern(etas_types::Type::MemoryPlace(etas_types::MemoryPlaceType {
            segments: vec!["ProjectMemory".to_owned(), "Papers".to_owned()],
        }));
    let registry = EffectRegistry::with_standard_effects_and_memory_places(interner.store());
    let coverage = etas_effects::EffectCoverage {
        registry: &registry,
        types: interner.store(),
    };
    let memory_read = registry
        .action_by_name("Memory.read")
        .expect("Memory.read action should be registered");
    let parent_read = Effect::AppliedAction(etas_effects::ActionInstanceRef {
        action: memory_read.clone(),
        args: vec![etas_types::EffectArgRef::Type(project_memory)],
    });
    let child_read = Effect::AppliedAction(etas_effects::ActionInstanceRef {
        action: memory_read.clone(),
        args: vec![etas_types::EffectArgRef::Type(papers_memory)],
    });
    let wildcard_read = Effect::AppliedAction(etas_effects::ActionInstanceRef {
        action: memory_read,
        args: vec![etas_types::EffectArgRef::Wildcard],
    });
    let parent_path_read = Effect::AppliedAction(etas_effects::ActionInstanceRef {
        action: registry
            .action_by_name("Memory.read")
            .expect("Memory.read action should be registered"),
        args: vec![etas_types::EffectArgRef::Path(vec![
            "ProjectMemory".to_owned(),
        ])],
    });
    let child_path_read = Effect::AppliedAction(etas_effects::ActionInstanceRef {
        action: registry
            .action_by_name("Memory.read")
            .expect("Memory.read action should be registered"),
        args: vec![etas_types::EffectArgRef::Path(vec![
            "ProjectMemory".to_owned(),
            "Papers".to_owned(),
        ])],
    });

    assert!(coverage.covers(&parent_read, &child_read));
    assert!(coverage.covers(&wildcard_read, &child_read));
    assert!(coverage.covers(&parent_path_read, &child_path_read));
    assert!(
        !coverage.covers(&child_read, &parent_read),
        "child memory place must not cover the parent region"
    );
    assert_eq!(
        registry
            .memory_place(project_memory)
            .expect("parent memory place should enter registry")
            .segments,
        ["ProjectMemory"]
    );
}

#[test]
fn check_program_uses_effect_coverage_for_user_extensions() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action charge() -> unit;
}

flow main() -> unit ![Network, Error<IndexError>] {
  perform Payment.charge();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().all(|diagnostic| diagnostic.code
            != DiagnosticCode::Effect(EffectDiagnosticCode::EscapedEffect)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_checked_index_error_outside_declared_row() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

flow main(args: Array<string>) -> string ![Error<IOError>] {
  return args[0];
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::EffectOutsideDeclaredRow)),
        "{:?}",
        output.diagnostics
    );
    let main = flow_item(&hir, "main");
    assert!(
        output
            .facts
            .item_effects
            .get(&main)
            .is_some_and(|summary| matches!(
                summary.support,
                InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect)
            )),
        "unchecked public row mismatch should reject the entry summary"
    );
}

#[test]
fn check_program_records_precise_agentic_infer_action_without_escaping() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

@limits([Tokens(128)])
agent Writer(input: string) -> string {}

flow main(input: string) -> string ![] {
  return Writer.run(input);
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let main = flow_item(&hir, "main");

    assert!(
        output.diagnostics.iter().all(|diagnostic| diagnostic.code
            != DiagnosticCode::Effect(EffectDiagnosticCode::EffectOutsideDeclaredRow)
            && diagnostic.code != DiagnosticCode::Effect(EffectDiagnosticCode::EscapedEffect)),
        "{:?}",
        output.diagnostics
    );

    let summary = output
        .facts
        .item_effects
        .get(&main)
        .expect("main summary should be materialized");
    assert!(
        summary
            .requested_actions
            .effects
            .contains(&Effect::AppliedAction(etas_effects::ActionInstanceRef {
                action: ActionRef {
                    tag: AGENTIC_TAG,
                    action: AGENTIC_INFER_ACTION,
                },
                args: vec![etas_types::EffectArgRef::Path(vec![
                    "app".to_owned(),
                    "main".to_owned(),
                    "Writer".to_owned(),
                ])],
            })),
        "{summary:?}"
    );
    assert!(
        summary
            .default_actions
            .effects
            .contains(&Effect::AppliedAction(etas_effects::ActionInstanceRef {
                action: ActionRef {
                    tag: AGENTIC_TAG,
                    action: AGENTIC_INFER_ACTION,
                },
                args: vec![etas_types::EffectArgRef::Path(vec![
                    "app".to_owned(),
                    "main".to_owned(),
                    "Writer".to_owned(),
                ])],
            })),
        "{summary:?}"
    );
    assert!(
        trace_has_agentic_infer(&summary.action_trace),
        "{summary:?}"
    );
    assert!(
        trace_has_agentic_infer_source(&summary.action_trace, ActionEventSource::AgentCall),
        "agent invocation trace should be compiler-generated call metadata, not a declared row: {summary:?}"
    );
    assert!(
        summary
            .requirements
            .contains(&RequirementFact::Limit(LimitRequirement {
                kind: LimitKind::Tokens,
                budget: LimitBudgetKind::Count,
                value: Some(LimitValue::Count(128)),
            })),
        "agent invocation summary should include @limits runtime requirements: {summary:?}"
    );
    assert!(
        !summary
            .escaping_effects
            .effects
            .iter()
            .any(|effect| matches!(
                effect,
                Effect::Tag(tag) if *tag == AGENTIC_TAG
            ) || matches!(
                effect,
                Effect::Action(action)
                    if action.tag == AGENTIC_TAG && action.action == AGENTIC_INFER_ACTION
            ) || matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.action.tag == AGENTIC_TAG
                        && action.action.action == AGENTIC_INFER_ACTION
            )),
        "Agentic.infer should not escape a normal agent call: {summary:?}"
    );
    assert!(
        !summary
            .requested_actions
            .effects
            .contains(&Effect::AppliedAction(etas_effects::ActionInstanceRef {
                action: ActionRef {
                    tag: AGENTIC_TAG,
                    action: AGENTIC_INFER_ACTION,
                },
                args: vec![etas_types::EffectArgRef::Path(vec!["Writer".to_owned()])],
            })),
        "Agentic.infer action args must be canonicalized, not duplicated as short paths: {summary:?}"
    );
}

#[test]
fn check_program_propagates_agent_body_default_actions_to_call_summary() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

import std.io.{read_line};

agent Writer() -> string ![Error<IOError>] {
  return read_line();
}

flow main() -> string ![Error<IOError>] {
  return Writer.run();
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    let output = check_program(&hir, &types);
    assert!(
        output.diagnostics.iter().all(|diagnostic| diagnostic.code
            != DiagnosticCode::Effect(EffectDiagnosticCode::EffectOutsideDeclaredRow)
            && diagnostic.code != DiagnosticCode::Effect(EffectDiagnosticCode::EscapedEffect)),
        "{:?}",
        output.diagnostics
    );

    let summary = output
        .facts
        .item_effects
        .get(&flow_item(&hir, "main"))
        .expect("main summary should be materialized");
    let stdin = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: etas_effects::CONSOLE_STDIN_READ_LINE_ACTION,
    });
    assert!(
        summary.requested_actions.effects.contains(&stdin),
        "agent call summary must include checked requested actions from the agent prompt body: {summary:?}"
    );
    assert!(
        summary.default_actions.effects.contains(&stdin),
        "agent call summary must include checked default action actions from the agent prompt body: {summary:?}"
    );
    assert!(
        has_agentic_infer(&summary.requested_actions),
        "agent invocation should still record internal Agentic.infer requested action: {summary:?}"
    );
    assert!(
        has_agentic_infer(&summary.default_actions),
        "agent invocation should still record default Agentic.infer action: {summary:?}"
    );
    assert!(
        !has_agentic_escape(&summary.escaping_effects),
        "Agentic.infer must not escape through agent body action propagation: {summary:?}"
    );
}

#[test]
fn check_program_records_latent_effect_without_immediate_lambda_effect() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

flow main() -> unit ![] {
  let later = () => {
    perform Payment.ping();
    return;
  };
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let lambda = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| matches!(expr, etas_hir::HirExpr::Lambda { .. }).then_some(id))
        .expect("lambda should lower");
    let latent = output
        .facts
        .latent_effects
        .get(&lambda)
        .expect("lambda should record a latent effect fact");
    assert!(
        !latent.inferred.effects.is_empty(),
        "lambda body effect should be latent, not erased"
    );
    assert!(
        latent.realized_at.is_empty(),
        "creating a flow value must not realize its latent effect"
    );
    assert!(
        output
            .facts
            .expr_effects
            .get(&lambda)
            .is_some_and(|summary| summary.escaping_effects.effects.is_empty()),
        "lambda creation should have no immediate computation effect"
    );
}

#[test]
fn check_program_realizes_local_latent_effect_on_call() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

flow main() -> unit ![Network, Error<IndexError>] {
  let later = () => {
    perform Payment.ping();
    return;
  };
  later();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let latent = output
        .facts
        .latent_effects
        .values()
        .next()
        .expect("lambda should record a latent effect fact");
    assert_eq!(
        latent.realized_at.len(),
        1,
        "calling a local flow value should realize its latent row once"
    );
    let main = flow_item(&hir, "main");
    assert!(
        output
            .facts
            .item_effects
            .get(&main)
            .is_some_and(|summary| !summary.escaping_effects.effects.is_empty()),
        "main summary should include the realized latent effect"
    );
}

#[test]
fn check_program_realizes_array_element_latent_effect_on_call() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

flow main() -> unit ![Network, Error<IndexError>] {
  let callbacks = [
    () => {
      perform Payment.ping();
      return;
    }
  ];
  let selected = callbacks[0];
  selected();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let latent = output
        .facts
        .latent_effects
        .values()
        .next()
        .expect("array element lambda should record a latent effect fact");
    assert_eq!(
        latent.realized_at.len(),
        1,
        "calling a flow value through an array element should realize its latent row"
    );
    let main = flow_item(&hir, "main");
    assert!(
        output
            .facts
            .item_effects
            .get(&main)
            .is_some_and(|summary| !summary.escaping_effects.effects.is_empty()),
        "main summary should include the array element latent effect"
    );
}

#[test]
fn check_program_rejects_projected_flow_call_without_latent_source_facts() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
alias Callback = unit -> unit;

flow main(callbacks: Array<Callback>) -> unit {
  (callbacks[0])();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic
                    .message
                    .contains("latent flow realization requires checked latent source facts")
        }),
        "{:?}",
        output.diagnostics
    );
    let main = flow_item(&hir, "main");
    assert!(matches!(
        output.facts.interpreter_support.items.get(&main),
        Some(InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        ))
    ));
}

#[test]
fn check_program_rejects_local_flow_container_without_latent_source_facts() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

flow main() -> unit ![Network, Error<IndexError>] {
  let later: unit -> unit ![Network] = () => {
    perform Payment.ping();
    return;
  };
  let callbacks: Array<unit -> unit ![Network]> = [later];
  let selected = callbacks[0];
  selected();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let later_ref = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            HirExpr::Array { elems, .. } => elems.first().copied().map(|_| id),
            _ => None,
        })
        .and_then(|array| match hir.exprs.get(array) {
            Some(HirExpr::Array { elems, .. }) => elems.first().copied(),
            _ => None,
        })
        .expect("callbacks initializer should contain a later path");
    let HirExpr::Path(path) = hir
        .exprs
        .get_mut(later_ref)
        .expect("later path should be mutable")
    else {
        unreachable!("callbacks initializer element should lower as a path");
    };
    path.resolution = ResolveResult::Unresolved;

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic
                    .message
                    .contains("local flow-value container requires checked latent source facts")
        }),
        "{:?}",
        output.diagnostics
    );
    let main = flow_item(&hir, "main");
    assert_eq!(
        output.facts.interpreter_support.items.get(&main),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        )),
        "local flow-value containers with missing latent source facts must not look pure"
    );
}

#[test]
fn check_program_realizes_record_field_latent_effect_on_call() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

alias Callbacks = {
  run: unit -> unit,
};

flow main() -> unit ![Network] {
  let callbacks = Callbacks {
    run = () => {
      perform Payment.ping();
      return;
    }
  };
  let selected = callbacks.run;
  selected();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let latent = output
        .facts
        .latent_effects
        .values()
        .next()
        .expect("record field lambda should record a latent effect fact");
    assert_eq!(
        latent.realized_at.len(),
        1,
        "calling a flow value through a record field should realize its latent row"
    );
    let main = flow_item(&hir, "main");
    assert!(
        output
            .facts
            .item_effects
            .get(&main)
            .is_some_and(|summary| !summary.escaping_effects.effects.is_empty()),
        "main summary should include the record field latent effect"
    );
}

#[test]
fn check_program_realizes_map_value_latent_effect_on_call() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

flow main() -> unit ![Network] {
  let callbacks = {
    "run" => () => {
      perform Payment.ping();
      return;
    }
  };
  let selected = callbacks["run"];
  selected();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let latent = output
        .facts
        .latent_effects
        .values()
        .next()
        .expect("map value lambda should record a latent effect fact");
    assert_eq!(
        latent.realized_at.len(),
        1,
        "calling a flow value through a map value should realize its latent row"
    );
    let main = flow_item(&hir, "main");
    assert!(
        output
            .facts
            .item_effects
            .get(&main)
            .is_some_and(|summary| !summary.escaping_effects.effects.is_empty()),
        "main summary should include the map value latent effect"
    );
}

#[test]
fn check_program_realizes_option_wrapped_latent_effect_on_call() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.option.unwrap;

effect Payment extends Network {
  action ping() -> unit;
}

flow main() -> unit ![Network] {
  let later = () => {
    perform Payment.ping();
    return;
  };
  let wrapped: Option<unit -> unit> = Some(later);
  let selected = unwrap(wrapped);
  selected();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    add_std_symbol_type_facts(&hir, &mut types);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let latent = output
        .facts
        .latent_effects
        .values()
        .next()
        .expect("option-wrapped lambda should record a latent effect fact");
    assert_eq!(
        latent.realized_at.len(),
        1,
        "calling a flow value through Some/unwrap should realize its latent row"
    );
    let main = flow_item(&hir, "main");
    assert!(
        output
            .facts
            .item_effects
            .get(&main)
            .is_some_and(|summary| !summary.escaping_effects.effects.is_empty()),
        "main summary should include the option-wrapped latent effect"
    );
}

#[test]
fn check_program_realizes_result_wrapped_latent_effect_on_call() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.result.unwrap;

effect Payment extends Network {
  action ping() -> unit;
}

flow main() -> unit ![Network] {
  let later = () => {
    perform Payment.ping();
    return;
  };
  let wrapped: Result<unit -> unit, string> = Ok(later);
  let selected = unwrap(wrapped);
  selected();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    add_std_symbol_type_facts(&hir, &mut types);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let latent = output
        .facts
        .latent_effects
        .values()
        .next()
        .expect("result-wrapped lambda should record a latent effect fact");
    assert_eq!(
        latent.realized_at.len(),
        1,
        "calling a flow value through Ok/unwrap should realize its latent row"
    );
    let main = flow_item(&hir, "main");
    assert!(
        output
            .facts
            .item_effects
            .get(&main)
            .is_some_and(|summary| !summary.escaping_effects.effects.is_empty()),
        "main summary should include the result-wrapped latent effect"
    );
}

#[test]
fn check_program_rejects_latent_effect_escaping_explicit_pure_function_bound() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

flow main() -> unit ![] {
  let later: unit -> unit ![] = () => {
    perform Payment.ping();
    return;
  };
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::EscapedEffect)
                && diagnostic.message.contains("latent flow effect escapes")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_applies_explicit_effect_row_on_first_class_flow_parameter_call() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow run(callback: unit -> unit ![Network]) -> unit ![Network] {
  callback();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let run = flow_item(&hir, "run");
    let summary = output
        .facts
        .item_effects
        .get(&run)
        .expect("run should have effect summary");
    assert!(
        summary
            .escaping_effects
            .effects
            .contains(&Effect::Tag(EffectTagId(1))),
        "calling a function parameter with explicit effect row must realize that row: {summary:?}"
    );
}

#[test]
fn check_program_applies_explicit_effect_row_on_first_class_pipeline_stage() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow run(callback: string -> string ![Network], input: string) -> string ![] {
  return input ~> callback;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::EffectOutsideDeclaredRow)
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_first_class_flow_parameter_call_without_effect_facts() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow run(callback: unit -> unit) -> unit {
  callback();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic
                    .message
                    .contains("first-class flow call requires a checked latent effect fact")
        }),
        "{:?}",
        output.diagnostics
    );
    let run = flow_item(&hir, "run");
    assert!(matches!(
        output.facts.interpreter_support.items.get(&run),
        Some(InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        ))
    ));
}

#[test]
fn check_program_specializes_first_class_flow_parameter_call_site() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

flow apply(callback: unit -> unit) -> unit {
  callback();
  return;
}

flow main() -> unit ![Network] {
  let later = () => {
    perform Payment.ping();
    return;
  };
  apply(later);
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let apply = flow_item(&hir, "apply");
    assert!(
        !matches!(
            output.facts.interpreter_support.items.get(&apply),
            Some(InterpreterSupport::Rejected(
                FrontendRejectionReason::EscapedEffect
            ))
        ),
        "a high-order flow with a solved call site must not be rejected as a pure fallback"
    );
    let main = flow_item(&hir, "main");
    assert!(
        output
            .facts
            .item_effects
            .get(&main)
            .is_some_and(|summary| summary
                .escaping_effects
                .effects
                .iter()
                .any(is_action_effect)),
        "main should include the action realized through the specialized callback call"
    );
}

#[test]
fn check_program_rejects_specialized_flow_call_without_param_type_fact() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

flow apply(callback: unit -> unit) -> unit {
  callback();
  return;
}

flow main() -> unit ![Network] {
  let later = () => {
    perform Payment.ping();
    return;
  };
  apply(later);
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let apply = flow_item(&hir, "apply");
    let callback_param = match hir.items.get(apply) {
        Some(HirItem::Flow(flow)) => flow.params[0],
        _ => unreachable!("apply should lower as a flow"),
    };
    types.facts.symbol_types.remove(&callback_param);

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic.message.contains(
                    "first-class flow call specialization requires checked parameter type facts",
                )
        }),
        "{:?}",
        output.diagnostics
    );
    assert_eq!(
        output.facts.interpreter_support.items.get(&apply),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        )),
        "missing higher-order parameter type facts must not be treated as non-flow parameters"
    );
}

#[test]
fn check_program_does_not_let_one_specialized_call_hide_unsolved_call_site() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

flow apply(callback: unit -> unit) -> unit {
  callback();
  return;
}

flow solved() -> unit ![Network] {
  let later = () => {
    perform Payment.ping();
    return;
  };
  apply(later);
  return;
}

flow unsolved(callback: unit -> unit) -> unit {
  apply(callback);
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic.message.contains(
                    "first-class flow call requires a checked latent effect fact or an explicit function effect row",
                )
        }),
        "{:?}",
        output.diagnostics
    );
    let apply = flow_item(&hir, "apply");
    assert!(matches!(
        output.facts.interpreter_support.items.get(&apply),
        Some(InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        ))
    ));
}

#[test]
fn check_program_rejects_first_class_call_when_callee_type_fact_is_missing() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow apply(callback: unit -> unit) -> unit {
  callback();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let callback_callee = hir
        .exprs
        .iter()
        .find_map(|(_, expr)| {
            let HirExpr::Call { callee, .. } = expr else {
                return None;
            };
            let HirExpr::Path(path) = &hir.exprs[*callee] else {
                return None;
            };
            let ResolveResult::Resolved(symbol) = path.resolution else {
                return None;
            };
            hir.symbols
                .get(symbol)
                .is_some_and(|symbol| symbol.name == "callback")
                .then_some(*callee)
        })
        .expect("callback call callee should lower");
    types.facts.expr_types.remove(&callback_callee);

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
        }),
        "{:?}",
        output.diagnostics
    );
    let apply = flow_item(&hir, "apply");
    assert_eq!(
        output.facts.interpreter_support.items.get(&apply),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        )),
        "missing callee type facts must not be treated as a pure first-class call"
    );
}

#[test]
fn check_program_rejects_missing_item_signature_instead_of_pure_fallback() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> unit ![Network] {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let main = flow_item(&hir, "main");
    types.facts.item_signatures.remove(&main);

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
        }),
        "{:?}",
        output.diagnostics
    );
    assert_eq!(
        output.facts.interpreter_support.items.get(&main),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        )),
        "missing item signature facts must block effect solving instead of becoming pure"
    );
}

#[test]
fn check_program_rejects_missing_callable_symbol_type_instead_of_empty_call_summary() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

flow helper() -> unit ![Network] {
  perform Payment.ping();
  return;
}

flow main() -> unit ![Network] {
  helper();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let helper_symbol = hir
        .items
        .get(flow_item(&hir, "helper"))
        .and_then(|item| match item {
            HirItem::Flow(flow) => Some(flow.symbol),
            _ => None,
        })
        .expect("helper flow should lower");
    types.facts.symbol_types.remove(&helper_symbol);

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
        }),
        "{:?}",
        output.diagnostics
    );
    let main = flow_item(&hir, "main");
    assert_eq!(
        output.facts.interpreter_support.items.get(&main),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        )),
        "missing callable symbol type facts must not be treated as an empty call summary"
    );
}

#[test]
fn check_program_rejects_missing_source_import_callable_target() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

flow helper() -> unit ![Payment] {
  perform Payment.ping();
  return;
}

flow main() -> unit ![Payment] {
  helper();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let helper_symbol = hir
        .items
        .get(flow_item(&hir, "helper"))
        .and_then(|item| match item {
            HirItem::Flow(flow) => Some(flow.symbol),
            _ => None,
        })
        .expect("helper flow should lower");
    hir.symbols
        .get_mut(helper_symbol)
        .expect("helper symbol should be mutable")
        .def = SymbolDef::ImportAlias {
        path: vec![
            "missing".to_owned(),
            "module".to_owned(),
            "helper".to_owned(),
        ],
        origin: ImportAliasOrigin::SourceImport,
    };

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic.message.contains("source import target")
        }),
        "{:?}",
        output.diagnostics
    );
    let main = flow_item(&hir, "main");
    assert_eq!(
        output.facts.interpreter_support.items.get(&main),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        )),
        "missing source-import callable targets must not fall back to alias type facts"
    );
}

#[test]
fn check_program_propagates_returned_first_class_flow_latent_fact() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

flow wrap(callback: unit -> unit) -> unit -> unit {
  return () => {
    callback();
    return;
  };
}

flow main() -> unit ![Network] {
  let later = () => {
    perform Payment.ping();
    return;
  };
  let wrapped = wrap(later);
  wrapped();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = flow_item(&hir, "main");
    assert!(
        output
            .facts
            .item_effects
            .get(&main)
            .is_some_and(|summary| summary
                .escaping_effects
                .effects
                .iter()
                .any(is_action_effect)),
        "calling a returned flow value should realize its propagated latent effect"
    );
}

#[test]
fn check_program_rejects_returned_unsolved_first_class_flow_source() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow choose(callback: unit -> unit) -> unit -> unit {
  return callback;
}

flow main(callback: unit -> unit) -> unit {
  let wrapped = choose(callback);
  wrapped();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic.message.contains(
                    "returned first-class flow propagation requires a checked latent return source",
                )
        }),
        "{:?}",
        output.diagnostics
    );
    let main = flow_item(&hir, "main");
    assert!(matches!(
        output.facts.interpreter_support.items.get(&main),
        Some(InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        ))
    ));
}

#[test]
fn check_program_realizes_stage_composed_flow_latent_effect_on_call() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

flow ping(value: i32) -> i32 {
  perform Payment.ping();
  return value;
}

flow main() -> unit ![Network] {
  let pipeline = ping | ping;
  pipeline(1);
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = flow_item(&hir, "main");
    assert!(
        output
            .facts
            .item_effects
            .get(&main)
            .is_some_and(|summary| summary
                .escaping_effects
                .effects
                .iter()
                .any(is_action_effect)),
        "calling a composed local flow value should realize the stage effects"
    );
}

#[test]
fn check_program_realizes_top_level_latent_effect_on_call() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

let Later: unit -> unit ![Network] = () => {
  perform Payment.ping();
  return;
};

flow main() -> unit ![Network] {
  Later();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let latent = output
        .facts
        .latent_effects
        .values()
        .next()
        .expect("top-level lambda should record a latent effect fact");
    assert_eq!(
        latent.realized_at.len(),
        1,
        "calling a top-level flow value should realize its latent row once"
    );
    let main = flow_item(&hir, "main");
    assert!(
        output
            .facts
            .item_effects
            .get(&main)
            .is_some_and(|summary| !summary.escaping_effects.effects.is_empty()),
        "main summary should include the realized top-level latent effect"
    );
}

#[test]
fn check_program_rejects_missing_top_level_value_type_fact_instead_of_local_summary() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

let Later: unit -> unit ![Network] = () => {
  perform Payment.ping();
  return;
};
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let (later_id, later_symbol) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::TopLevelLet(item)
                if hir
                    .symbols
                    .get(item.symbol)
                    .is_some_and(|symbol| symbol.name == "Later") =>
            {
                Some((id, item.symbol))
            }
            _ => None,
        })
        .expect("Later top-level value should lower");
    types.facts.symbol_types.remove(&later_symbol);

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
        }),
        "{:?}",
        output.diagnostics
    );
    assert_eq!(
        output.facts.interpreter_support.items.get(&later_id),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        )),
        "missing top-level value type facts must block latent effect solving"
    );
}

#[test]
fn check_program_rejects_top_level_latent_effect_escaping_explicit_pure_function_bound() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

let Later: unit -> unit ![] = () => {
  perform Payment.ping();
  return;
};

flow main() -> unit {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::EscapedEffect)
                && diagnostic.message.contains("latent flow effect escapes")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_generates_public_flow_effect_contract() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

public flow main() -> unit ![Network] {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let contract = output
        .facts
        .public_contracts
        .iter()
        .find(|contract| contract.exported_name.segments == ["app", "main", "main"])
        .expect("public flow should publish an effect contract");
    assert!(matches!(
        contract.source,
        etas_effects::PublicEffectContractSource::ExplicitSourceAnnotation
    ));
    assert!(contract.declared.is_some());
    assert!(contract.latent_flows.is_empty());
    assert!(!contract.public_row.effects.is_empty());
}

#[test]
fn check_program_rejects_public_contract_without_checked_item_signature() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

public flow main() -> unit ![Network] {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    types.facts.item_signatures.remove(&flow_item(&hir, "main"));

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic.message.contains(
                    "public effect contract generation requires checked exported value facts",
                )
        }),
        "{:?}",
        output.diagnostics
    );
    assert!(
        output
            .facts
            .public_contracts
            .iter()
            .all(|contract| contract.exported_name.segments != ["app", "main", "main"]),
        "public contract generation must not publish fallback metadata when checked value facts are missing"
    );
}

#[test]
fn check_program_generates_public_tool_effect_contract_from_explicit_row() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

public tool search(q: string) -> string ![Network] {
  return q;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let contract = output
        .facts
        .public_contracts
        .iter()
        .find(|contract| contract.exported_name.segments == ["app", "main", "search"])
        .expect("public tool should publish an explicit effect contract");
    assert!(matches!(
        contract.source,
        etas_effects::PublicEffectContractSource::ExplicitSourceAnnotation
    ));
    assert!(contract.declared.is_some());
    assert!(!contract.public_row.effects.is_empty());
}

#[test]
fn check_program_generates_public_agent_effect_contract_metadata() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

public agent Reviewer(input: Prompt) -> string {
  return input;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let contract = output
        .facts
        .public_contracts
        .iter()
        .find(|contract| contract.exported_name.segments == ["app", "main", "Reviewer"])
        .expect("public agent should publish generated checked effect metadata");
    assert!(matches!(
        contract.source,
        etas_effects::PublicEffectContractSource::GeneratedCheckedMetadata
    ));
    assert!(contract.declared.is_none());
    assert!(
        !has_agentic_infer(&contract.requested_actions),
        "agent declaration public metadata should not publish call-only Agentic.infer requested actions"
    );
    assert!(
        !contract.public_row.effects.iter().any(|effect| matches!(
            effect,
            Effect::AppliedAction(action)
                if action.action.tag == AGENTIC_TAG
                    && action.action.action == AGENTIC_INFER_ACTION
        )),
        "generated public row should not publish default-handled Agentic inference as escaping"
    );
}

#[test]
fn check_program_rejects_tool_without_explicit_effect_contract() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

public tool search(q: string) -> string;
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::MissingToolProviderBinding)
        }),
        "{:?}",
        output.diagnostics
    );
    assert!(
        output
            .facts
            .public_contracts
            .iter()
            .all(
                |contract| contract.exported_name.segments != ["app", "main", "search"]
                    || !matches!(
                        contract.source,
                        etas_effects::PublicEffectContractSource::GeneratedCheckedMetadata
                    )
            ),
        "bodyless source tools must not publish generated metadata in place of provider metadata"
    );
    let tool_id = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Tool(tool)
                if hir
                    .symbols
                    .get(tool.symbol)
                    .is_some_and(|symbol| symbol.name == "search") =>
            {
                Some(id)
            }
            _ => None,
        })
        .expect("search tool should lower");
    assert_eq!(
        output.facts.interpreter_support.items.get(&tool_id),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        ))
    );
}

#[test]
fn check_program_rejects_local_bodyless_tool_even_with_explicit_effect_contract() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

tool search(q: string) -> string ![Network];
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::MissingToolProviderBinding)
                && diagnostic.message.contains("provider binding")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_accepts_bound_bodyless_tool_with_provider_metadata() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

tool search(q: string) -> string ![Network];
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let binding = etas_effects::ToolProviderBindingMetadata {
        tool: vec!["app".into(), "main".into(), "search".into()],
        provider: "mcp:browser:search".into(),
        effect_row: vec!["Network".into()],
        action_row: Vec::new(),
    };
    let output = RunEffectPipeline::run(etas_effects::EffectPipelineInput {
        hir: &hir,
        types: &types,
        std_registry: test_std_registry(),
        dependency_metadata: None,
        tool_bindings: std::slice::from_ref(&binding),
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .expect("effect pipeline should run");

    assert!(
        !output.effects.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::MissingToolProviderBinding)
        }),
        "{:?}",
        output.effects.diagnostics
    );
    assert!(
        output.effects.diagnostics.is_empty(),
        "{:?}",
        output.effects.diagnostics
    );
    let tool_id = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Tool(tool)
                if hir
                    .symbols
                    .get(tool.symbol)
                    .is_some_and(|symbol| symbol.name == "search") =>
            {
                Some(id)
            }
            _ => None,
        })
        .expect("search tool should lower");
    assert_ne!(
        output.effects.facts.interpreter_support.items.get(&tool_id),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        ))
    );
}

#[test]
fn check_program_infers_etas_tool_body_effects_and_actions() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

effect Payment extends Network {
  action ping() -> unit;
}

public tool emit() -> unit ![Network]
{
    perform Payment.ping();
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summary = output
        .facts
        .item_effects
        .get(&tool_item(&hir, "emit"))
        .expect("tool should have effect facts");
    assert!(
        summary
            .requested_actions
            .effects
            .iter()
            .any(|effect| { matches!(effect, Effect::Action(_)) }),
        "tool body should record performed action footprint: {summary:?}"
    );
}

#[test]
fn check_program_accepts_pure_etas_tool_body_without_explicit_contract() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

tool echo(input: string) -> string {
    return input;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summary = output
        .facts
        .item_effects
        .get(&tool_item(&hir, "echo"))
        .expect("tool should have effect facts");
    assert!(summary.escaping_effects.effects.is_empty(), "{summary:?}");
    assert!(summary.requested_actions.effects.is_empty(), "{summary:?}");
}

#[test]
fn check_program_rejects_etas_tool_body_effects_without_explicit_contract() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

effect Payment extends Network {
  action ping() -> unit;
}

tool emit() -> unit
{
    perform Payment.ping();
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::EffectOutsideDeclaredRow)
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_generates_public_top_level_flow_value_latent_contract() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

effect Payment extends Network {
  action ping() -> unit;
}

public let Later: unit -> unit ![Network] = () => {
  perform Payment.ping();
  return;
};

flow main() -> unit {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let contract = output
        .facts
        .public_contracts
        .iter()
        .find(|contract| contract.exported_name.segments == ["app", "main", "Later"])
        .expect("public top-level flow value should publish an effect contract");
    assert!(matches!(
        contract.value,
        etas_effects::PublicEffectValue::ValueType(_)
    ));
    assert_eq!(contract.latent_flows.len(), 1);
    assert!(contract.declared.is_some());
    assert!(!contract.inferred.effects.is_empty());
}

#[test]
fn public_contract_keeps_handled_requested_actions_out_of_escaping_boundary() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

effect Approval {
  action request() -> unit;
}

public flow main() -> unit ![Approval.request] {
  handle {
    perform Approval.request();
  } with {
    Approval.request() => {
      resume;
    }
  };
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = flow_item(&hir, "main");
    let summary = output
        .facts
        .item_effects
        .get(&main)
        .expect("main should have effect summary");
    assert!(
        summary
            .requested_actions
            .effects
            .contains(&Effect::Action(ActionRef {
                tag: APPROVAL_TAG,
                action: APPROVAL_REQUEST_ACTION,
            })),
        "{summary:?}"
    );
    assert!(
        !summary
            .escaping_effects
            .effects
            .contains(&Effect::Action(ActionRef {
                tag: APPROVAL_TAG,
                action: APPROVAL_REQUEST_ACTION,
            })),
        "handled requested action must not remain in escaping effects: {summary:?}"
    );
}

#[test]
fn check_program_generates_public_container_flow_value_latent_contract() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

effect Payment extends Network {
  action ping() -> unit;
}

public let Later: Array<unit -> unit ![Network]> = [
  () => {
    perform Payment.ping();
    return;
  }
];

flow main() -> unit {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let contract = output
        .facts
        .public_contracts
        .iter()
        .find(|contract| contract.exported_name.segments == ["app", "main", "Later"])
        .expect("public top-level container with flow values should publish an effect contract");
    assert_eq!(contract.latent_flows.len(), 1);
    assert!(
        !contract.inferred.effects.is_empty(),
        "public contract inferred row should include exported container latent effects"
    );
    assert!(
        !contract.public_row.effects.is_empty(),
        "generated public row should include exported container latent effects"
    );
}

#[test]
fn check_program_rejects_top_level_flow_container_without_latent_source_facts() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

effect Payment extends Network {
  action ping() -> unit;
}

let later: unit -> unit ![Network] = () => {
  perform Payment.ping();
  return;
};

public let Later: Array<unit -> unit ![Network]> = [later];

flow main() -> unit {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let later_ref = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            HirExpr::Array { elems, .. } => elems.first().copied().map(|_| id),
            _ => None,
        })
        .and_then(|array| match hir.exprs.get(array) {
            Some(HirExpr::Array { elems, .. }) => elems.first().copied(),
            _ => None,
        })
        .expect("Later initializer should contain a later path");
    let HirExpr::Path(path) = hir
        .exprs
        .get_mut(later_ref)
        .expect("later path should be mutable")
    else {
        unreachable!("Later initializer element should lower as a path");
    };
    path.resolution = ResolveResult::Unresolved;

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic
                    .message
                    .contains("top-level flow-value container requires checked latent source facts")
        }),
        "{:?}",
        output.diagnostics
    );
    let later_item = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::TopLevelLet(top_level)
                if hir
                    .symbols
                    .get(top_level.symbol)
                    .is_some_and(|symbol| symbol.name == "Later") =>
            {
                Some(id)
            }
            _ => None,
        })
        .expect("Later top-level let should lower");
    assert_eq!(
        output.facts.interpreter_support.items.get(&later_item),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        )),
        "flow-value containers with missing latent source facts must not publish empty summaries"
    );
}

#[test]
fn check_program_generates_public_reusable_handler_value_contract() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

effect Approval {
  action request() -> unit;
}

public let AutoApproval: ![Approval => []] = handler {
  Approval.request() => {
    resume;
  }
};

flow main() -> unit {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let contract = output
        .facts
        .public_contracts
        .iter()
        .find(|contract| contract.exported_name.segments == ["app", "main", "AutoApproval"])
        .expect("public reusable handler value should publish an effect contract");
    let etas_effects::PublicEffectValue::ValueType(value_ty) = contract.value else {
        panic!("public handler contract should expose the checked handler value type");
    };
    assert!(
        matches!(
            types.store.get(value_ty),
            Some(etas_types::Type::Handler(_))
        ),
        "public reusable handler contract must carry the checked handler type"
    );
    assert!(matches!(
        contract.source,
        etas_effects::PublicEffectContractSource::ExplicitSourceAnnotation
    ));
    assert!(contract.latent_flows.is_empty());
    assert!(
        output.facts.handler_values.values().any(|fact| {
            fact.handled.effects.iter().any(is_approval_request_effect)
                && fact.produced.effects.is_empty()
        }),
        "public reusable handler value must also materialize checked handler facts"
    );
}

#[test]
fn check_program_does_not_generate_public_contract_for_private_flow() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

flow helper() -> unit ![Network] {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(
        output.facts.public_contracts.is_empty(),
        "{:?}",
        output.facts.public_contracts
    );
}

#[test]
fn public_network_contract_does_not_require_acknowledgement_by_default() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

public flow main() -> unit ![Network] {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let contract = output
        .facts
        .public_contracts
        .iter()
        .find(|contract| contract.exported_name.segments == ["app", "main", "main"])
        .expect("public flow should publish an effect contract");
    assert!(contract.high_impact_ack.is_none());
}

#[test]
fn public_explicit_high_impact_contract_records_acknowledgement() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

public flow main() -> unit ![Command] {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let contract = output
        .facts
        .public_contracts
        .iter()
        .find(|contract| contract.exported_name.segments == ["app", "main", "main"])
        .expect("public flow should publish the contract fact");
    assert!(
        contract.high_impact_ack.is_some(),
        "explicit source row is the current item-boundary acknowledgement"
    );
}

#[test]
fn public_inferred_high_impact_contract_requires_acknowledgement() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

effect Shell extends Command {
  action run() -> unit;
}

public flow main() -> unit {
  perform Shell.run();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::MissingHighImpactAcknowledgement)
        }),
        "{:?}",
        output.diagnostics
    );
    let contract = output
        .facts
        .public_contracts
        .iter()
        .find(|contract| contract.exported_name.segments == ["app", "main", "main"])
        .expect("public flow should still publish the generated contract fact for diagnostics");
    assert!(contract.high_impact_ack.is_none());
}

#[test]
fn effect_registry_records_user_action_owner_identity() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action charge(amount: i32) -> bool;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let registry = etas_effects::EffectRegistry::from_hir_and_types(&hir, &types)
        .expect("source effects should build a registry");

    let payment = registry
        .tag_by_name("Payment")
        .expect("user effect should enter registry");
    let action_symbol = hir
        .symbols
        .iter()
        .find(|symbol| matches!(symbol.def, etas_hir::SymbolDef::EffectAction { .. }))
        .map(|symbol| symbol.id)
        .expect("effect action symbol should lower");
    let action = registry
        .action(action_symbol)
        .expect("effect action signature should enter registry");

    assert_eq!(action.owner, payment);
    assert_eq!(
        registry.actions_by_owner(payment),
        Some(&[action_symbol][..])
    );
}

#[test]
fn effect_registry_resolves_core_tags_by_identity() {
    let registry = EffectRegistry::with_standard_effects();

    assert_eq!(registry.tag_by_core(CoreEffect::Agentic), Some(AGENTIC_TAG));
    assert_eq!(registry.tag_by_name("Approval"), Some(APPROVAL_TAG));
    assert_eq!(
        registry.runtime_requirement_reason(
            registry
                .tag_by_name("Approval")
                .expect("Approval standard effect should be registered"),
        ),
        Some(RuntimeRequirementReason::Approval)
    );
    assert_eq!(
        registry.runtime_requirement_reason(CONSOLE_TAG),
        Some(RuntimeRequirementReason::Console)
    );
}

#[test]
fn effect_registry_declares_minimal_standard_effect_action_vocabulary() {
    let registry = EffectRegistry::with_standard_effects();

    for (effect, parent) in [
        ("Agentic", None),
        ("Network", None),
        ("FileIO", None),
        ("Command", None),
        ("Memory", None),
        ("Secret", None),
        ("Time", None),
        ("Human", None),
        ("Error", None),
        ("Console", Some(FILE_IO_TAG)),
        ("Approval", Some(HUMAN_TAG)),
        ("Clock", Some(TIME_TAG)),
    ] {
        let tag = registry
            .tag_by_name(effect)
            .unwrap_or_else(|| panic!("missing standard effect tag {effect}"));
        assert_eq!(
            registry.tag_by_name(&format!("std.effects.{effect}")),
            Some(tag)
        );
        if let Some(parent) = parent {
            assert!(
                registry.tag_extends(tag, parent),
                "{effect} should extend parent tag {parent:?}"
            );
        }
    }

    for action in [
        "Console.stdin_read_line",
        "Console.stdin_read_all",
        "Console.stdout_write",
        "Console.stderr_write",
        "Command.run",
        "Secret.read",
        "Agentic.infer",
        "Memory.read",
        "Memory.write",
        "Approval.request",
        "Clock.now",
        "Clock.sleep",
    ] {
        let action_ref = registry
            .action_by_name(action)
            .unwrap_or_else(|| panic!("missing standard action {action}"));
        assert!(
            registry
                .runtime_requirement_reason_for_action(action_ref.action)
                .is_some(),
            "{action} should have an explicit runtime requirement mapping"
        );
    }

    for action in [
        "Console.stdin_read_line",
        "Console.stdin_read_all",
        "Console.stdout_write",
        "Console.stderr_write",
        "Agentic.infer",
        "Command.run",
        "Secret.read",
        "Clock.now",
        "Clock.sleep",
        "Memory.read",
        "Memory.write",
        "Approval.request",
    ] {
        let action_ref = registry
            .action_by_name(action)
            .unwrap_or_else(|| panic!("missing standard action {action}"));
        assert!(
            registry.standard_action(action_ref.action).is_some(),
            "{action} should have a standard action signature"
        );
    }

    let error_raise = registry
        .action_by_name("Error.raise")
        .expect("missing standard action Error.raise");
    let error_raise_sig = registry
        .standard_action(error_raise.action)
        .expect("Error.raise should be a standard action signature");
    assert!(error_raise_sig.returns_never);
    assert_eq!(error_raise_sig.runtime_requirement, None);

    for effect in [
        "Web",
        "Workspace",
        "File",
        "Db",
        "Vector",
        "Email",
        "Calendar",
        "UI",
        "Random",
        "Crypto",
        "Log",
        "Trace",
        "Metric",
        "Queue",
        "Cache",
        "ObjectStore",
        "Payment",
        "Identity",
        "Package",
        "Deploy",
    ] {
        assert!(
            registry.tag_by_name(effect).is_none(),
            "old standard effect tag should not resolve: {effect}"
        );
        assert!(
            registry
                .tag_by_name(&format!("std.effects.{effect}"))
                .is_none(),
            "old qualified standard effect tag should not resolve: {effect}"
        );
    }

    for action in [
        "Web.search",
        "Command.spawn",
        "Secret.write",
        "Secret.list",
        "Agentic.embed",
        "Agentic.rerank",
        "Memory.migrate",
        "Memory.compact",
        "Clock.schedule",
        "Payment.charge",
    ] {
        assert!(
            registry.action_by_name(action).is_none(),
            "old standard action should not resolve: {action}"
        );
    }
}

#[test]
fn check_program_materializes_standard_error_raise_action() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(err: ValidationError) -> never ![Error<ValidationError>] {
    perform Error<ValidationError>.raise(err);
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = flow_item(&hir, "main");
    let validation_error = types
        .facts
        .item_signatures
        .get(&main)
        .and_then(|signature| match signature {
            etas_types::ItemSignature::Flow(flow) => flow.params.first().copied(),
            _ => None,
        })
        .expect("main parameter type should be recorded");
    let summary = output
        .facts
        .item_effects
        .get(&main)
        .expect("main should have effect facts");
    let perform_expr = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| matches!(expr, HirExpr::Perform { .. }).then_some(id))
        .expect("perform expression should lower");
    let fact = output
        .facts
        .performed_actions
        .get(&perform_expr)
        .expect("standard Error.raise should materialize a performed-action fact");
    assert_eq!(fact.effect_segments, vec!["Error"]);
    assert_eq!(fact.action, "raise");
    assert!(
        fact.action_symbol.is_some(),
        "standard Error.raise should resolve to a synthetic action symbol"
    );
    assert!(
        summary
            .escaping_effects
            .effects
            .iter()
            .any(|effect| { matches!(effect, Effect::Error(error) if *error == validation_error) }),
        "Error<ValidationError>.raise should escape a typed Error<ValidationError>: {summary:?}"
    );
    assert!(
        fact.summary
            .escaping_effects
            .effects
            .contains(&Effect::Error(validation_error)),
        "{fact:?}"
    );
}

#[test]
fn effect_registry_marks_high_impact_acknowledgement_metadata() {
    let registry = EffectRegistry::with_standard_effects();
    let command = registry
        .tag_by_core(CoreEffect::Command)
        .expect("Command core effect should be registered");
    let secret = registry
        .tag_by_core(CoreEffect::Secret)
        .expect("Secret core effect should be registered");
    let approval = registry
        .tag_by_name("Approval")
        .expect("Approval standard effect should be registered");
    let memory_write = registry
        .core_action(CoreEffect::Memory, MEMORY_WRITE_ACTION)
        .expect("Memory.write standard action should be registered");

    assert!(registry.tag_requires_high_impact_ack(command));
    assert!(registry.tag_requires_high_impact_ack(secret));
    assert!(registry.tag_requires_high_impact_ack(approval));
    assert!(registry.action_requires_high_impact_ack(&memory_write));
    assert!(
        !registry
            .core_action(CoreEffect::Memory, MEMORY_READ_ACTION)
            .is_some_and(|action| registry.action_requires_high_impact_ack(&action)),
        "Memory.read should not require high-impact acknowledgement"
    );
}

#[test]
fn effect_registry_does_not_fallback_to_text_for_unresolved_extension_refs() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect PartnerBilling extends Network;
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut hir = lower_program(&parsed.value);
    let effect_id = hir
        .items
        .iter()
        .find_map(|(id, item)| matches!(item, HirItem::Effect(_)).then_some(id))
        .expect("effect should lower");
    let HirItem::Effect(effect) = hir
        .items
        .get_mut(effect_id)
        .expect("effect item should exist")
    else {
        unreachable!("effect item id came from effect match");
    };
    effect
        .extends
        .as_mut()
        .expect("effect should preserve extends ref")
        .path
        .resolution = ResolveResult::Unresolved;
    let types = etas_types::check_program(&hir);
    let registry = etas_effects::EffectRegistry::from_hir_and_types(&hir, &types)
        .expect("source effects should build a registry");
    let billing = registry
        .tag_by_name("PartnerBilling")
        .expect("user effect should still be registered");

    assert!(
        !registry.tag_extends(billing, NETWORK_TAG),
        "registry must not recover unresolved extension refs by text"
    );
}

#[test]
fn effect_registry_does_not_register_empty_name_for_missing_effect_symbol() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect MissingSymbolEffect;
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let effect_id = hir
        .items
        .iter()
        .find_map(|(id, item)| matches!(item, HirItem::Effect(_)).then_some(id))
        .expect("effect should lower");
    let HirItem::Effect(effect) = hir
        .items
        .get_mut(effect_id)
        .expect("effect item should exist")
    else {
        unreachable!("effect item id came from effect match");
    };
    effect.symbol = SymbolId(u32::MAX);

    let registry = etas_effects::EffectRegistry::from_hir_and_types(&hir, &types)
        .expect("source effects should build a registry");

    assert!(
        registry.tag_by_name("").is_none(),
        "registry must not register an empty effect name when the HIR effect symbol is missing"
    );
}

#[test]
fn check_program_records_stable_path_memory_action_args() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
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
  let existing = ProjectMemory.Papers.get("paper-1");
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = flow_item(&hir, "main");
    let summary = output
        .facts
        .item_effects
        .get(&main)
        .expect("main should have effect facts");
    assert!(
        summary.escaping_effects.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.args
                        == vec![etas_types::EffectArgRef::Path(vec![
                            "app".to_owned(),
                            "main".to_owned(),
                            "ProjectMemory".to_owned(),
                            "Papers".to_owned()
                        ])]
            )
        }),
        "Memory.read<ProjectMemory> is an authority action and must remain in escaping effects: {summary:?}"
    );
    assert!(
        summary.requested_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.args
                        == vec![etas_types::EffectArgRef::Path(vec![
                            "app".to_owned(),
                            "main".to_owned(),
                            "ProjectMemory".to_owned(),
                            "Papers".to_owned()
                        ])]
            )
        }),
        "requested action summary must preserve the memory action footprint: {summary:?}"
    );
    assert!(
        !summary.default_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.args
                        == vec![etas_types::EffectArgRef::Path(vec![
                            "app".to_owned(),
                            "main".to_owned(),
                            "ProjectMemory".to_owned(),
                            "Papers".to_owned()
                        ])]
            )
        }),
        "Memory.read must not be marked as default-handled: {summary:?}"
    );
}

#[test]
fn check_program_treats_registry_declared_memory_selection_limit_as_pure_support() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
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
  let selected = ProjectMemory.Papers.select("topic").limit(Tokens(1));
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = flow_item(&hir, "main");
    let summary = output
        .facts
        .item_effects
        .get(&main)
        .expect("main should have effect facts");
    assert_eq!(
        summary
            .requested_actions
            .effects
            .iter()
            .filter(|effect| matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.action.action == MEMORY_READ_ACTION
            ))
            .count(),
        1,
        "MemorySelection.limit must not add a requested action beyond the select footprint: {summary:?}"
    );
    assert!(
        !matches!(
            output.facts.interpreter_support.items.get(&main),
            Some(InterpreterSupport::RequiresInterpreterOrchestration(_))
        ),
        "pure support limit must not require runtime handler orchestration"
    );
}

#[test]
fn check_program_maps_store_upsert_to_memory_read_and_write_actions() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> unit ![Memory.read<ProjectMemory>, Memory.write<ProjectMemory>]
{
  ProjectMemory.Papers.upsert("paper-1", "draft");
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = flow_item(&hir, "main");
    let summary = output
        .facts
        .item_effects
        .get(&main)
        .expect("main should have effect facts");
    for expected in [MEMORY_READ_ACTION, MEMORY_WRITE_ACTION] {
        assert!(
            summary
                .requested_actions
                .effects
                .iter()
                .any(|effect| matches!(
                    effect,
                    Effect::AppliedAction(action)
                        if action.action.action == expected
                )),
            "Store.upsert should request checked Memory.read and Memory.write actions: {summary:?}"
        );
    }
}

#[test]
fn check_program_treats_memory_transaction_support_types_as_local_only() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(
  transaction: MemoryTransaction,
  version: MemoryVersion,
  conflict: MemoryConflict
) -> unit ![] {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = flow_item(&hir, "main");
    let summary = output
        .facts
        .item_effects
        .get(&main)
        .expect("main should have effect facts");
    assert!(summary.escaping_effects.effects.is_empty());
    assert!(summary.requested_actions.effects.is_empty());
    assert!(summary.default_actions.effects.is_empty());
    assert_eq!(
        output.facts.interpreter_support.items.get(&main),
        Some(&InterpreterSupport::LocalOnly)
    );
}

#[test]
fn check_program_rejects_missing_memory_place_fact_instead_of_empty_memory_summary() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
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
  let existing = ProjectMemory.Papers.get("paper-1");
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let receiver = hir
        .exprs
        .iter()
        .find_map(|(_, expr)| {
            if let HirExpr::MethodCall {
                receiver, method, ..
            } = expr
                && method == "get"
            {
                Some(*receiver)
            } else {
                None
            }
        })
        .expect("store receiver should lower as a method-call receiver");
    types.facts.expr_memory_places.remove(&receiver);

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
        }),
        "{:?}",
        output.diagnostics
    );
    let main = flow_item(&hir, "main");
    assert_eq!(
        output.facts.interpreter_support.items.get(&main),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        )),
        "missing memory place facts must block memory effect solving"
    );
}

#[test]
fn check_program_rejects_store_method_without_receiver_type_fact() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
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
  let existing = ProjectMemory.Papers.get("paper-1");
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let receiver = hir
        .exprs
        .iter()
        .find_map(|(_, expr)| {
            if let HirExpr::MethodCall {
                receiver, method, ..
            } = expr
                && method == "get"
            {
                Some(*receiver)
            } else {
                None
            }
        })
        .expect("store receiver should lower as a method-call receiver");
    types.facts.expr_types.remove(&receiver);

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic.message.contains(
                    "std store method effect solving requires a checked receiver type fact",
                )
        }),
        "{:?}",
        output.diagnostics
    );
    let main = flow_item(&hir, "main");
    assert_eq!(
        output.facts.interpreter_support.items.get(&main),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        )),
        "missing store receiver type facts must not fall back to runtime handler support"
    );
}

#[test]
fn check_program_rejects_std_memory_wrapper_without_receiver_arg() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

import std.memory.{get};

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
  let existing = get();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    let _call = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            HirExpr::Call { callee, args, .. } if args.is_empty() => match &hir.exprs[*callee] {
                HirExpr::Path(path)
                    if path.segments.last().is_some_and(|seg| seg.name == "get") =>
                {
                    Some(id)
                }
                _ => None,
            },
            _ => None,
        })
        .expect("std.memory.get call should lower");

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic
                    .message
                    .contains("std memory wrapper effect solving requires a receiver argument")
        }),
        "{:?}",
        output.diagnostics
    );
    let main = flow_item(&hir, "main");
    assert_eq!(
        output.facts.interpreter_support.items.get(&main),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        )),
        "std memory wrappers without a checked receiver argument must not look pure"
    );
}

#[test]
fn check_program_rejects_unresolved_std_intrinsic_path_without_text_fallback() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

import std.memory.{get};

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
  let existing = get(ProjectMemory.Papers, "paper-1");
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    let callee = hir
        .exprs
        .iter()
        .find_map(|(_, expr)| match expr {
            HirExpr::Call { callee, args, .. } if args.len() == 2 => match &hir.exprs[*callee] {
                HirExpr::Path(path)
                    if path.segments.last().is_some_and(|seg| seg.name == "get") =>
                {
                    Some(*callee)
                }
                _ => None,
            },
            _ => None,
        })
        .expect("std.memory.get callee should lower");
    let HirExpr::Path(path) = hir
        .exprs
        .get_mut(callee)
        .expect("callee expression should be mutable")
    else {
        unreachable!("callee id came from path expression");
    };
    path.resolution = ResolveResult::Unresolved;
    types.facts.expr_types.remove(&callee);

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic
                    .message
                    .contains("first-class flow call requires a checked callee function type")
        }),
        "{:?}",
        output.diagnostics
    );
    assert!(
        !output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("std memory wrapper effect solving requires a receiver argument")),
        "unresolved std intrinsic paths must not be classified through raw text fallback: {:?}",
        output.diagnostics
    );
    let main = flow_item(&hir, "main");
    assert_eq!(
        output.facts.interpreter_support.items.get(&main),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        ))
    );
}

#[test]
fn effect_unit_collector_records_items_nested_handlers_and_first_class_calls() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
  action request() -> unit;
}

flow apply(callback: unit -> unit ![]) -> unit ![] {
  callback();
  return;
}

flow main() -> unit ![] {
  let local = () => {
    return;
  };
  let reusable: ![Approval => []] = handler {
    Approval.request() => {
      resume;
    }
  };
  apply(local);
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let main = flow_item(&hir, "main");
    let apply = flow_item(&hir, "apply");
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let units = RunEffectPipeline::artifacts(etas_effects::EffectPipelineInput {
        hir: &hir,
        types: &types,
        std_registry: test_std_registry(),
        dependency_metadata: None,
        tool_bindings: &[],
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .expect("effect pipeline should run")
    .units;

    assert!(units.contains(&EffectUnit::Item(main)));
    assert!(units.contains(&EffectUnit::Item(apply)));
    assert!(
        units.iter().any(|unit| matches!(
            unit,
            EffectUnit::AnonymousFlow { owner, .. } if *owner == main
        )),
        "anonymous/local flow bodies must be explicit effect units: {units:?}"
    );
    assert!(
        units.iter().any(|unit| matches!(
            unit,
            EffectUnit::HandlerArm { owner, .. } if *owner == main
        )),
        "handler arm bodies must be explicit effect units: {units:?}"
    );
    assert!(
        units.iter().any(|unit| matches!(
            unit,
            EffectUnit::FirstClassFlowCall { owner, .. } if *owner == apply
        )),
        "first-class flow-value call sites must be explicit effect units: {units:?}"
    );
}

#[test]
fn effect_inference_planner_builds_registry_and_units_without_private_graph() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> unit {
  helper();
  return;
}

flow helper() -> unit {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);

    let main = flow_item(&hir, "main");
    let plan = RunEffectPipeline::plan(etas_effects::EffectPipelineInput {
        hir: &hir,
        types: &types,
        std_registry: test_std_registry(),
        dependency_metadata: None,
        tool_bindings: &[],
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .expect("effect pipeline should plan");

    assert!(plan.registry.tag_extends(CONSOLE_TAG, FILE_IO_TAG));
    let output = RunEffectPipeline::run(etas_effects::EffectPipelineInput {
        hir: &hir,
        types: &types,
        std_registry: test_std_registry(),
        dependency_metadata: None,
        tool_bindings: &[],
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .expect("effect pipeline should run");
    assert!(output.artifacts.units.contains(&EffectUnit::Item(main)));
    assert!(
        output
            .effects
            .facts
            .unit_effects
            .contains_key(&EffectUnit::Item(main))
    );
}

#[test]
fn run_effect_pipeline_has_documented_internal_stage_order() {
    let schedule = RunEffectPipeline::schedule_text();
    let expected = [
        "group effects.run",
        "pass effects.build_registry",
        "pass effects.collect_units",
        "pass effects.solve_summaries",
        "pass effects.materialize_facts",
        "pass effects.validate_contracts",
        "pass effects.trace_spec.materialize_models",
        "pass effects.trace_spec.analyze_monitors",
        "pass effects.trace_spec.validate",
        "pass effects.emit_output",
    ];

    let mut offset = 0;
    for expected_line in expected {
        let Some(index) = schedule[offset..].find(expected_line) else {
            panic!("missing effect pipeline stage `{expected_line}` in:\n{schedule}");
        };
        offset += index + expected_line.len();
    }
}

#[test]
fn run_effect_pipeline_materializes_direct_call_summary_with_generic_solver() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

flow helper() -> unit ![Network] {
  perform Payment.ping();
  return;
}

flow main() -> unit ![Network] {
  helper();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);

    let helper = flow_item(&hir, "helper");
    let main = flow_item(&hir, "main");
    let output = RunEffectPipeline::run(etas_effects::EffectPipelineInput {
        hir: &hir,
        types: &types,
        std_registry: test_std_registry(),
        dependency_metadata: None,
        tool_bindings: &[],
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .expect("effect pipeline should run");

    assert!(
        output
            .artifacts
            .registry
            .tag_extends(CONSOLE_TAG, FILE_IO_TAG)
    );
    assert!(output.artifacts.units.contains(&EffectUnit::Item(main)));
    assert!(output.artifacts.units.contains(&EffectUnit::Item(helper)));
    assert!(output.effects.facts.item_effects.contains_key(&main));
    assert!(output.effects.facts.item_effects.contains_key(&helper));
    assert!(
        output
            .effects
            .facts
            .item_effects
            .get(&main)
            .expect("main summary")
            .escaping_effects
            .effects
            .iter()
            .any(is_action_effect),
        "{:?}",
        output.effects.facts.item_effects.get(&main)
    );
    assert!(
        output.effects.diagnostics.is_empty(),
        "{:?}",
        output.effects.diagnostics
    );
}

#[test]
fn effect_registry_consumes_dependency_effect_metadata() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> unit {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let metadata = etas_effects::DependencyEffectMetadata {
        tags: vec![etas_effects::DependencyEffectTag {
            path: vec!["dep".into(), "payments".into(), "PartnerBilling".into()],
            runtime_requirement: Some(RuntimeRequirementReason::Network),
        }],
        actions: Vec::new(),
        extensions: vec![etas_effects::DependencyEffectExtension {
            package: None,
            child: vec!["dep".into(), "payments".into(), "PartnerBilling".into()],
            parent: vec!["Network".into()],
        }],
    };

    let registry =
        etas_effects::EffectRegistry::from_hir_types_and_dependencies(&hir, &types, &metadata)
            .expect("valid dependency metadata should build a registry");
    let billing = registry
        .tag_by_name("dep.payments.PartnerBilling")
        .expect("dependency effect metadata should enter registry");

    assert!(registry.tag_extends(billing, NETWORK_TAG));
    assert_eq!(
        registry.runtime_requirement_reason(billing),
        Some(RuntimeRequirementReason::Network)
    );
    assert!(
        registry.tag_by_name("PartnerBilling").is_none(),
        "dependency metadata must not leak unqualified names into source lookup"
    );
}

#[test]
fn run_effect_pipeline_does_not_specialize_action_args_from_record_payload() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Transport extends Network {
  action request(method: string, host: string) -> unit;
}

type Request = {
  method: string,
  host: string,
};

flow dispatch(request: Request) -> unit ![Transport] {
  perform Transport.request(request.method, request.host);
  return;
}

flow main() -> unit ![Transport] {
  let request = Request { method = "GET", host = "example.test" };
  dispatch(request);
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);

    let main = flow_item(&hir, "main");
    let output = RunEffectPipeline::run(etas_effects::EffectPipelineInput {
        hir: &hir,
        types: &types,
        std_registry: test_std_registry(),
        dependency_metadata: None,
        tool_bindings: &[],
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .expect("effect pipeline should run");

    assert!(
        output.effects.diagnostics.is_empty(),
        "{:?}",
        output.effects.diagnostics
    );
    let summary = output
        .effects
        .facts
        .item_effects
        .get(&main)
        .expect("main summary");
    assert!(
        summary
            .requested_actions
            .effects
            .iter()
            .any(|effect| matches!(
                effect,
                Effect::Action(action)
                    if output.artifacts.registry.action_signature(action)
                        .is_some_and(|signature| signature.name.ends_with("request"))
            )),
        "{summary:?}"
    );
    assert!(
        !summary
            .requested_actions
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::AppliedAction(_))),
        "runtime payload fields must not become static action selectors: {summary:?}"
    );
}

#[test]
fn run_effect_pipeline_keeps_pure_record_projection_payloads_out_of_static_action_args() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.text.{trim, uppercase};

effect Transport extends Network {
  action request(method: string, host: string) -> unit;
}

type Url = {
  host: string,
};

type Request = {
  method: string,
  url: Url,
};

flow normalize_method(method: string) -> string {
  return uppercase(trim(method));
}

flow normalize_request(request: Request) -> Request {
  return Request {
    method = normalize_method(request.method),
    url = request.url,
  };
}

flow dispatch(request: Request) -> unit ![Transport] {
  let normalized = normalize_request(request);
  perform Transport.request(normalized.method, normalized.url.host);
  return;
}

flow main() -> unit ![Transport] {
  let request = Request {
    method = "GET",
    url = Url { host = "api.example.test" },
  };
  dispatch(request);
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);

    let main = flow_item(&hir, "main");
    let output = RunEffectPipeline::run(etas_effects::EffectPipelineInput {
        hir: &hir,
        types: &types,
        std_registry: test_std_registry(),
        dependency_metadata: None,
        tool_bindings: &[],
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .expect("effect pipeline should run");

    assert!(
        output.effects.diagnostics.is_empty(),
        "{:?}",
        output.effects.diagnostics
    );
    let summary = output
        .effects
        .facts
        .item_effects
        .get(&main)
        .expect("main summary");
    assert!(
        summary
            .requested_actions
            .effects
            .iter()
            .any(|effect| matches!(
                effect,
                Effect::Action(action)
                    if output.artifacts.registry.action_signature(action)
                        .is_some_and(|signature| signature.name.ends_with("request"))
            )),
        "{summary:?}"
    );
    assert!(
        !summary
            .requested_actions
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::AppliedAction(_))),
        "runtime payload projections must not become static action selectors: {summary:?}"
    );
}

#[test]
fn effect_registry_prefers_checked_action_signature_over_dependency_summary() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Payload = {
  body: string,
};

effect EdkHttp extends Network {
  action request<Method, Host>(method: string, host: string, payload: Payload) -> i32;
}

flow main() -> unit {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::build_signature_facts(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let metadata = etas_effects::DependencyEffectMetadata {
        tags: vec![etas_effects::DependencyEffectTag {
            path: vec!["EdkHttp".into()],
            runtime_requirement: Some(RuntimeRequirementReason::Network),
        }],
        actions: vec![etas_effects::DependencyEffectAction {
            path: vec!["EdkHttp".into(), "request".into()],
            effect_args: Vec::new(),
            selector_param_names: Vec::new(),
            selector_defaults: Vec::new(),
            returns_never: false,
            runtime_requirement: Some(RuntimeRequirementReason::Network),
        }],
        extensions: Vec::new(),
    };

    let registry =
        etas_effects::EffectRegistry::from_hir_types_and_dependencies(&hir, &types, &metadata)
            .expect("valid dependency metadata should build a registry");
    let action = registry
        .action_by_name("EdkHttp.request")
        .expect("action should be registered");
    let signature = registry
        .action_signature(&action)
        .expect("action signature should resolve");

    assert_eq!(
        signature.params.len(),
        3,
        "checked action params must not be shadowed by dependency summary"
    );
    assert_eq!(
        signature.effect_args,
        vec![
            etas_effects::EffectActionArgKind::Type,
            etas_effects::EffectActionArgKind::Type,
        ]
    );
}

#[test]
fn effect_pipeline_artifacts_preserve_dependency_metadata() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> unit {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let metadata = etas_effects::DependencyEffectMetadata {
        tags: vec![etas_effects::DependencyEffectTag {
            path: vec!["dep".into(), "audit".into(), "AuditTrail".into()],
            runtime_requirement: Some(RuntimeRequirementReason::FileIO),
        }],
        actions: Vec::new(),
        extensions: vec![etas_effects::DependencyEffectExtension {
            package: None,
            child: vec!["dep".into(), "audit".into(), "AuditTrail".into()],
            parent: vec!["FileIO".into()],
        }],
    };
    let output = RunEffectPipeline::run(etas_effects::EffectPipelineInput {
        hir: &hir,
        types: &types,
        std_registry: test_std_registry(),
        dependency_metadata: Some(&metadata),
        tool_bindings: &[],
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .expect("effect pipeline should run");

    assert_eq!(output.artifacts.dependency_metadata, metadata);
    let audit = output
        .artifacts
        .registry
        .tag_by_name("dep.audit.AuditTrail")
        .expect("dependency effect should enter artifact registry");
    assert!(output.artifacts.registry.tag_extends(audit, FILE_IO_TAG));
}

#[test]
fn effect_pipeline_reports_unresolved_dependency_effect_extensions() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import dep.audit;

flow main() -> unit {
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let metadata = etas_effects::DependencyEffectMetadata {
        tags: vec![etas_effects::DependencyEffectTag {
            path: vec!["dep".into(), "audit".into(), "AuditTrail".into()],
            runtime_requirement: Some(RuntimeRequirementReason::FileIO),
        }],
        actions: Vec::new(),
        extensions: vec![etas_effects::DependencyEffectExtension {
            package: Some(7),
            child: vec!["dep".into(), "audit".into(), "AuditTrail".into()],
            parent: vec!["dep".into(), "missing".into(), "UnknownParent".into()],
        }],
    };
    let anchors = [
        etas_effects::ExternalArtifactAnchor {
            package: 7,
            package_label: "dep-audit@1.0.0#dep".to_owned(),
            item: vec!["dep".into(), "audit".into(), "AuditTrail".into()],
            span: Span::empty(etas_core::SourceId(0), TextSize(1)),
        },
        etas_effects::ExternalArtifactAnchor {
            package: 8,
            package_label: "other-audit@1.0.0#dep".to_owned(),
            item: vec!["dep".into(), "audit".into(), "AuditTrail".into()],
            span: Span::empty(etas_core::SourceId(9), TextSize(1)),
        },
    ];
    let missing_anchor_error = RunEffectPipeline::run(etas_effects::EffectPipelineInput {
        hir: &hir,
        types: &types,
        std_registry: test_std_registry(),
        dependency_metadata: Some(&metadata),
        tool_bindings: &[],
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .expect_err("unanchored external metadata diagnostics must fail closed");
    assert!(matches!(
        missing_anchor_error,
        etas_effects::EffectPipelineError::MissingDiagnosticAnchor { .. }
    ));
    let output = RunEffectPipeline::run(etas_effects::EffectPipelineInput {
        hir: &hir,
        types: &types,
        std_registry: test_std_registry(),
        dependency_metadata: Some(&metadata),
        tool_bindings: &[],
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &anchors,
        reachable_items: None,
    })
    .expect("effect pipeline should run");

    assert_eq!(
        output
            .artifacts
            .registry
            .unresolved_dependency_extensions()
            .len(),
        1,
        "unresolved dependency coverage metadata must be preserved for diagnostics"
    );
    assert!(
        output.effects.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic
                    .message
                    .contains("dependency effect extension could not be resolved")
        }),
        "invalid dependency coverage metadata must be reported as a blocking effect diagnostic: {:?}",
        output.effects.diagnostics
    );
}

#[test]
fn check_program_rejects_reverse_extension_coverage() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
  action ping() -> unit;
}

effect Wire extends Payment;

flow main() -> unit ![Wire] {
  perform Payment.ping();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::EffectOutsideDeclaredRow)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_uses_standard_console_extends_file_io() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
alias IOError = string;

flow write_line() -> unit ![Console.stdout_write, Error<IOError>] {
  return;
}

flow main() -> unit ![FileIO, Error<IOError>] {
  write_line();
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().all(|diagnostic| diagnostic.code
            != DiagnosticCode::Effect(EffectDiagnosticCode::EscapedEffect)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn effect_rows_and_requirements_join_by_union() {
    let inference = Effect::Tag(EffectTagId(0));
    let network = Effect::Tag(EffectTagId(1));

    let mut row = EffectRow::closed(EffectSet::one(inference.clone()));
    let other = EffectRow::closed(EffectSet::one(network.clone()));
    assert!(row.union_assign(&other));
    assert!(row.effects.contains(&inference));
    assert!(row.effects.contains(&network));

    let mut requirements = RequirementSet::one(RequirementFact::Limit(LimitRequirement {
        kind: LimitKind::Iterations,
        budget: LimitBudgetKind::Count,
        value: Some(LimitValue::Count(1)),
    }));
    assert!(!requirements.union_assign(&RequirementSet::new()));
}

#[test]
fn effect_summary_joins_toward_runtime_or_rejected_support() {
    let mut summary = EffectSummary::local();
    summary.record_escaping_effect(Effect::Tag(EffectTagId(0)));
    let mut runtime = EffectSummary::local();
    runtime.record_escaping_effect(Effect::Tag(EffectTagId(1)));
    runtime.determinism = Determinism::RuntimeMediated;
    runtime.support = requires_host(HostRequirementKind::Network);

    assert!(summary.join_assign(&runtime));
    assert_eq!(summary.determinism, Determinism::RuntimeMediated);
    assert!(
        summary
            .escaping_effects
            .effects
            .contains(&Effect::Tag(EffectTagId(0)))
    );
    assert!(
        summary
            .escaping_effects
            .effects
            .contains(&Effect::Tag(EffectTagId(1)))
    );
    assert!(matches!(
        summary.support,
        InterpreterSupport::RequiresHost(ref requirements)
            if requirements.kinds.contains(&HostRequirementKind::Network)
    ));

    let rejected = EffectSummary {
        support: InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect),
        ..EffectSummary::local()
    };
    summary.join_assign(&rejected);
    assert!(matches!(
        summary.support,
        InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect)
    ));
}

#[test]
fn check_program_records_ordered_action_trace_for_sequential_performs() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Audit {
    action first() -> unit;
    action second() -> unit;
}

flow main() -> unit ![Audit] {
    perform Audit.first();
    perform Audit.second();
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = flow_item(&hir, "main");
    let summary = output
        .facts
        .item_effects
        .get(&main)
        .expect("main should have a summary");
    let ActionTraceDomain::Seq(events) = &summary.action_trace else {
        panic!(
            "sequential performs must produce an ordered Seq trace, got {:?}",
            summary.action_trace
        );
    };
    assert_eq!(events.len(), 2, "{events:?}");
    assert!(
        events
            .iter()
            .all(|event| matches!(event, ActionTraceDomain::Event(_)))
    );
}

#[test]
fn check_program_records_choice_action_trace_for_branch_performs() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Audit {
    action first() -> unit;
    action second() -> unit;
}

flow main(flag: bool) -> unit ![Audit] {
    if flag {
        perform Audit.first();
    } else {
        perform Audit.second();
    }
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = flow_item(&hir, "main");
    let summary = output
        .facts
        .item_effects
        .get(&main)
        .expect("main should have a summary");
    let ActionTraceDomain::Choice(branches) = &summary.action_trace else {
        panic!(
            "branch performs must produce a Choice trace, got {:?}",
            summary.action_trace
        );
    };
    assert_eq!(branches.len(), 2, "{branches:?}");
}

#[test]
fn action_trace_widening_preserves_only_unknown_order_for_branch_or_recursive_shapes() {
    let approval = Effect::Action(ActionRef {
        tag: APPROVAL_TAG,
        action: APPROVAL_REQUEST_ACTION,
    });
    let memory = Effect::Action(ActionRef {
        tag: MEMORY_TAG,
        action: MEMORY_READ_ACTION,
    });
    let event = |action| {
        ActionTraceDomain::Event(ActionEvent {
            action,
            span: etas_core::Span::empty(etas_core::SourceId(0), etas_core::TextSize::ZERO),
            source: ActionEventSource::Perform,
        })
    };
    let trace = ActionTraceDomain::Choice(vec![event(approval.clone()), event(memory.clone())]);
    let current = ActionTraceDomain::Repeat(Box::new(ActionTraceDomain::Seq(vec![
        event(approval.clone()),
        event(memory.clone()),
    ])));

    let widened = trace.stabilize_for_fixpoint(Some(&current));

    let ActionTraceDomain::UnknownOrder(actions) = widened else {
        panic!("widening branch/recursive trace must not invent an order: {widened:?}");
    };
    assert!(actions.contains(&approval), "{actions:?}");
    assert!(actions.contains(&memory), "{actions:?}");
}

#[test]
fn effect_facts_store_interpreter_support_by_item() {
    let mut facts = EffectFacts::default();
    facts.interpreter_support.items.insert(
        etas_hir::HirItemId(0),
        requires_host(HostRequirementKind::ToolCall),
    );
    facts.interpreter_support.entry = Some(local_only());

    assert!(matches!(
        facts
            .interpreter_support
            .items
            .get(&etas_hir::HirItemId(0)),
        Some(InterpreterSupport::RequiresHost(requirements))
            if requirements.kinds.contains(&HostRequirementKind::ToolCall)
    ));
    assert!(matches!(
        facts.interpreter_support.entry,
        Some(InterpreterSupport::LocalOnly)
    ));
}

#[test]
fn effect_summary_classifies_runtime_requirements_in_domain() {
    let mut summary = EffectSummary::local();

    summary.require_runtime(etas_effects::RuntimeRequirementReason::Checkpoint);

    assert_eq!(summary.determinism, Determinism::RuntimeMediated);
    assert!(matches!(
        summary.support,
        InterpreterSupport::RequiresInterpreterOrchestration(_)
    ));
}

#[test]
fn interpreter_support_removes_handled_host_requirements_in_domain() {
    let mut summary = EffectSummary::local();
    summary.require_runtime(etas_effects::RuntimeRequirementReason::RuntimeHandler);
    summary
        .support
        .join_assign(&requires_host(HostRequirementKind::Console));

    let reduced = summary
        .support
        .without_host_requirements(&[HostRequirementKind::Console]);

    assert!(matches!(
        reduced,
        InterpreterSupport::RequiresInterpreterOrchestration(requirements)
            if requirements.host.kinds.is_empty()
                && requirements.features.kinds.contains(&etas_effects::InterpreterFeatureKind::EffectHandler)
    ));
}

#[test]
fn check_program_accepts_command_tool_with_sandbox_action_parameter() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
		public tool shell(cmd: string) -> string ![Command.run<DefaultCommandSandbox>] {
		    return cmd;
		}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let contract = output
        .facts
        .public_contracts
        .iter()
        .find(|contract| {
            contract
                .exported_name
                .segments
                .last()
                .is_some_and(|name| name == "shell")
        })
        .expect("shell tool should publish its declared effect contract");
    assert!(
        contract.public_row.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.action.action == COMMAND_RUN_ACTION
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
        "sandbox profile must be represented in the declared public contract: {contract:?}"
    );
}

#[test]
fn check_program_materializes_std_command_wrapper_sandbox_action_parameter() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.host.command.{Command, CommandResult, run};

flow main(cmd: Command) -> CommandResult ![Command.run<DefaultCommandSandbox>]
{
    return run(cmd, DefaultCommandSandbox);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = flow_item(&hir, "main");
    let summary = output
        .facts
        .item_effects
        .get(&main)
        .expect("main flow should have effect summary");
    assert!(
        summary.requested_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.action.action == COMMAND_RUN_ACTION
                        && action.args == vec![etas_types::EffectArgRef::Path(vec![
                            "std".to_owned(),
                            "host".to_owned(),
                            "sandbox".to_owned(),
                            "DefaultCommandSandbox".to_owned()
                        ])]
            )
        }),
        "{:?}",
        summary.requested_actions.effects.iter().collect::<Vec<_>>()
    );
}

#[test]
fn check_program_rejects_runtime_sandbox_selector_in_declared_effect_row() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.host.command.{Command, CommandResult, run};

flow main(cmd: Command, sandbox: SandboxProfile) -> CommandResult ![Command.run<sandbox>]
{
    return run(cmd, sandbox);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);

    assert!(
        types.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Type(etas_core::TypeDiagnosticCode::InvalidEffectArgument)
        }),
        "{:?}",
        types.diagnostics
    );
}

#[test]
fn source_perform_uses_static_generic_selectors_not_runtime_payload_paths() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Region;

effect Workspace {
    action read<R>(path: string) -> string;
}

flow main(path: string) -> string ![Workspace.read<_>]
{
    return perform Workspace.read<_>(path);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let pipeline = RunEffectPipeline::run(etas_effects::EffectPipelineInput {
        hir: &hir,
        types: &types,
        std_registry: test_std_registry(),
        dependency_metadata: None,
        tool_bindings: &[],
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .expect("effect pipeline should run");
    let output = pipeline.effects;
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);

    let action_symbol = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Effect(effect) => effect.body.actions().first().map(|action| action.symbol),
            _ => None,
        })
        .expect("workspace action should lower");
    let action = pipeline
        .artifacts
        .registry
        .action(action_symbol)
        .expect("workspace action should be registered");
    let summary = output
        .facts
        .item_effects
        .get(&flow_item(&hir, "main"))
        .expect("main summary");
    let expected = Effect::AppliedAction(etas_effects::ActionInstanceRef {
        action: ActionRef {
            tag: action.owner,
            action: action.id,
        },
        args: vec![etas_types::EffectArgRef::Wildcard],
    });

    assert!(
        summary.requested_actions.effects.contains(&expected),
        "{:?}",
        summary.requested_actions.effects.iter().collect::<Vec<_>>()
    );
    assert!(
        !summary
            .requested_actions
            .effects
            .iter()
            .any(|effect| matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.args == vec![etas_types::EffectArgRef::Path(vec!["path".to_owned()])]
            )),
        "runtime payload parameter must not become a static action selector: {:?}",
        summary.requested_actions.effects.iter().collect::<Vec<_>>()
    );
}

#[test]
fn console_effect_maps_to_console_host_requirement() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;
import std.io.{println};

flow main() -> unit ![Error<IOError>]
{
    println("hello");
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(matches!(
        output.facts.interpreter_support.entry,
        Some(InterpreterSupport::RequiresHost(requirements))
            if requirements.kinds.contains(&HostRequirementKind::Console)
    ));
    let summary = output
        .facts
        .item_effects
        .values()
        .find(|summary| !summary.requested_actions.effects.is_empty())
        .expect("std.io call should record requested console action");
    assert!(
        summary.requested_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::Action(action)
                    if *action == ActionRef {
                        tag: CONSOLE_TAG,
                        action: CONSOLE_STDOUT_WRITE_ACTION,
                    }
            )
        }),
        "std.io.println must record Console.stdout_write as a requested action: {summary:?}"
    );
    assert!(
        summary.default_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::Action(action)
                    if *action == ActionRef {
                        tag: CONSOLE_TAG,
                        action: CONSOLE_STDOUT_WRITE_ACTION,
                    }
            )
        }),
        "std.io.println must record Console.stdout_write as a default action action: {summary:?}"
    );
    assert!(
        !summary.escaping_effects.effects.iter().any(|effect| {
            matches!(
            effect,
            Effect::Action(action)
                if *action == ActionRef {
                    tag: CONSOLE_TAG,
                    action: CONSOLE_STDOUT_WRITE_ACTION,
                }
            )
        }),
        "std.io.println must not expose default action Console.stdout_write as an escaping public action boundary: {summary:?}"
    );
    assert!(
        matches!(
            &summary.action_trace,
            ActionTraceDomain::Event(event)
                if matches!(
                    &event.action,
                    Effect::Action(action)
                        if *action == ActionRef {
                            tag: CONSOLE_TAG,
                            action: CONSOLE_STDOUT_WRITE_ACTION,
                        }
                ) && event.source == ActionEventSource::StdIntrinsic
        ),
        "std.io.println must record exactly one ordered Console.stdout_write trace event: {summary:?}"
    );
}

#[test]
fn trace_spec_deny_rejects_requested_console_action() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;
import std.io.{println};

spec Safe: trace = -Console.stdout_write;

flow main() -> unit ![Error<IOError>] ~ Safe
{
    println("x");
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecDenied)
                && diagnostic
                    .message
                    .contains("trace spec deny clause rejects")
        }),
        "trace spec deny should reject Console.stdout_write requested by println: {:#?}",
        output.diagnostics
    );
}

#[test]
fn inline_trace_spec_deny_rejects_requested_console_action() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;
import std.io.{println};

flow main() -> unit ![Error<IOError>] ~ (-Console.stdout_write)
{
    println("x");
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecDenied)
                && diagnostic
                    .message
                    .contains("trace spec deny clause rejects")
        }),
        "inline trace spec deny should reject Console.stdout_write requested by println: {:#?}",
        output.diagnostics
    );
}

#[test]
fn trace_spec_disjunction_accepts_satisfied_alternative() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;
import std.io.{println};

spec Safe: trace = -Console.stdout_write | +Console.stdout_write;

flow main() -> unit ![Error<IOError>] ~ Safe
{
    println("x");
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        !output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecDenied)
                || diagnostic.code
                    == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecAllowViolation)
        }),
        "trace spec disjunction should pass when one alternative accepts the trace: {:#?}",
        output.diagnostics
    );
}

#[test]
fn trace_spec_disjunction_rejects_when_all_alternatives_fail() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;
import std.io.{println};

spec Safe: trace = -Console.stdout_write | -Console;

flow main() -> unit ![Error<IOError>] ~ Safe
{
    println("x");
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecDenied)
                && diagnostic
                    .message
                    .contains("trace spec deny clause rejects")
        }),
        "trace spec disjunction should fail when every alternative rejects the trace: {:#?}",
        output.diagnostics
    );
}

#[test]
fn trace_spec_type_selector_rejects_value_symbol() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

effect Gate {
    action request<T>() -> unit;
}

let RuntimeScope = "not-a-type";

spec Safe: trace = +Gate.request<RuntimeScope>;

flow main() -> unit ![Gate.request<_>] ~ Safe
{
    perform Gate.request();
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("trace spec selector must name a checked type")),
        "trace spec type selector must not accept value symbols: {:#?}",
        output.diagnostics
    );
}

#[test]
fn std_tcp_group_import_records_default_action() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;
import std.net.tcp.{Host, NetworkError, Port, TcpOptions, connect as tcp_connect};

flow main() -> unit ![Error<NetworkError>]
{
    let _stream = tcp_connect(
        Host { name = "example.com" },
        Port { value = 80 },
        TcpOptions {}
    );
    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let registry = EffectRegistry::with_standard_effects();
    let action = registry
        .action_by_name("Net.tcp_connect")
        .expect("Net.tcp_connect should resolve");
    let summary = output
        .facts
        .item_effects
        .values()
        .find(|summary| !summary.requested_actions.effects.is_empty())
        .expect("std.net.tcp.connect should record requested action");
    assert!(
        summary.requested_actions.effects.iter().any(
            |effect| matches!(effect, Effect::AppliedAction(instance) if instance.action == action)
        ),
        "std.net.tcp.connect must record Net.tcp_connect as requested action: {summary:?}"
    );
    assert!(
        summary.default_actions.effects.iter().any(
            |effect| matches!(effect, Effect::AppliedAction(instance) if instance.action == action)
        ),
        "std.net.tcp.connect must record Net.tcp_connect as default action: {summary:?}"
    );
}

#[test]
fn handler_application_propagates_handler_body_std_default_actions() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;
import std.net.tcp.{Host, NetworkError, Port, TcpOptions, connect as tcp_connect};

effect EdkHttp extends Network {
  action request(host: Host, port: Port, options: TcpOptions) -> string;
}

let H: ![EdkHttp => Error<NetworkError> for string] = handler {
  EdkHttp.request(host, port, options) => {
    let _stream = tcp_connect(host, port, options);
    finish "ok";
  }
};

flow main(host: Host, port: Port, options: TcpOptions) -> unit ![Error<NetworkError>]
{
  let _value = perform EdkHttp.request(host, port, options) with H;
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let registry = EffectRegistry::with_standard_effects();
    let action = registry
        .action_by_name("Net.tcp_connect")
        .expect("Net.tcp_connect should resolve");
    let summary = output
        .facts
        .item_effects
        .get(&flow_item(&hir, "main"))
        .expect("main summary");
    assert!(
        summary.default_actions.effects.iter().any(
            |effect| matches!(effect, Effect::AppliedAction(instance) if instance.action == action)
        ),
        "handler body std.net.tcp.connect must propagate default action to handle expression: {summary:?}"
    );
}

#[test]
fn direct_perform_std_action_is_not_default_handled() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

flow main() -> unit ![Console.stdout_write]
{
    perform Console.stdout_write("hello");
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = flow_item(&hir, "main");
    let summary = output
        .facts
        .item_effects
        .get(&main)
        .expect("main summary should be materialized");
    let stdout = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(
        summary.requested_actions.effects.contains(&stdout),
        "direct perform should record requested action: {summary:?}"
    );
    assert!(
        summary.escaping_effects.effects.contains(&stdout),
        "direct perform should escape unless scoped handler handles it: {summary:?}"
    );
    assert!(
        !summary.default_actions.effects.contains(&stdout),
        "direct perform must not become default-handled through action signature metadata: {summary:?}"
    );
}

#[test]
fn try_expr_captures_typed_error_effect_and_keeps_other_effects() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
  action request() -> unit;
}

flow read_line(values: Array<string>) -> string ![Approval, Error<IndexError>]
{
    perform Approval.request();
    return values[0];
}

flow main(values: Array<string>) -> Result<string, IndexError> ![Approval]

{
    return read_line(values)?;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.try_captures.len(), 1);
    let capture = output
        .facts
        .try_captures
        .values()
        .next()
        .expect("try capture fact");
    assert!(
        capture.conversions.is_empty(),
        "checked index ? should capture Error<IndexError> without implicit conversions: {capture:?}"
    );
    let main = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some(id)
            }
            _ => None,
        })
        .expect("main flow");
    let summary = output.facts.item_effects.get(&main).expect("main effects");
    assert!(
        summary.requested_actions.effects.iter().any(|effect| {
            matches!(effect, Effect::Action(_)) || matches!(effect, Effect::AppliedAction(_))
        }),
        "try capture must keep the non-error action footprint in requested actions: {summary:?}"
    );
    assert!(
        summary.escaping_effects.effects.iter().any(|effect| {
            matches!(effect, Effect::Action(_)) || matches!(effect, Effect::AppliedAction(_))
        }),
        "try capture must keep non-error actions in escaping effects after capturing Error<E>: {:?}",
        summary.escaping_effects
    );
    assert!(
        !summary
            .escaping_effects
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::Error(_))),
        "{:?}",
        summary.escaping_effects
    );
}

#[test]
fn try_expr_captures_error_effect_from_whole_block_operand() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Gate {
  action request() -> unit;
}

flow read_value(values: Array<string>) -> string ![Gate, Error<IndexError>]
{
    perform Gate.request();
    return values[0];
}

flow main(values: Array<string>) -> Result<string, IndexError> ![Gate]

{
    return {
        let value = read_value(values);
        value
    }?;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.try_captures.len(), 1);
    let main = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some(id)
            }
            _ => None,
        })
        .expect("main flow");
    let summary = output.facts.item_effects.get(&main).expect("main effects");
    assert!(
        summary.requested_actions.effects.iter().any(|effect| {
            matches!(effect, Effect::Action(_)) || matches!(effect, Effect::AppliedAction(_))
        }),
        "block operand `?` must keep non-error action footprint in requested actions: {:?}",
        summary.requested_actions
    );
    assert!(
        summary.escaping_effects.effects.iter().any(|effect| {
            matches!(effect, Effect::Action(_)) || matches!(effect, Effect::AppliedAction(_))
        }),
        "block operand `?` must keep non-error actions in escaping effects after capturing Error<E>: {:?}",
        summary.escaping_effects
    );
    assert!(
        !summary
            .escaping_effects
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::Error(_))),
        "block operand `?` must capture Error<E> raised by statements in the block: {:?}",
        summary.escaping_effects
    );
}

#[test]
fn try_expr_rejects_plain_result_value_without_error_effect() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
alias IOError = string;

flow main(value: Result<string, IOError>) -> Result<Result<string, IOError>, IOError> {
    return value?;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::InvalidTryCapture)
    }));
}

#[test]
fn try_expr_rejects_ambiguous_error_capture_without_target() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
alias IOError = string;
alias ParseError = i32;

flow read_value() -> string ![Error<IOError>, Error<ParseError>]
{
    return "";
}

flow main() -> unit {
    let result = read_value()?;
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::InvalidTryCapture)
    }));
}

#[test]
fn try_expr_rejects_multiple_error_effects_even_with_target_error() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
alias IOError = string;
alias ParseError = i32;

flow read_value(values: Array<string>) -> string ![Error<IndexError>, Error<ParseError>]
{
    let value = values[0];
    perform Error<ParseError>.raise(1);
    return value;
}

flow main(values: Array<string>) -> Result<string, IndexError> ![Error<ParseError>] {
    return read_value(values)?;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::InvalidTryCapture)
                && diagnostic.message.contains("multiple Error")
                && diagnostic.message.contains("explicit handler")
        }),
        "{:?}",
        output.diagnostics
    );
    assert!(
        output.facts.try_captures.is_empty(),
        "multi-error `?` must not materialize a capture fact; an explicit handler is required"
    );
}

#[test]
fn try_expr_rejects_missing_error_conversion() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
alias IOError = string;
alias ParseError = i32;

flow read_value() -> string ![Error<ParseError>]
{
    return "";
}

flow main() -> Result<string, IOError> {
    return read_value()?;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::InvalidTryCapture)
    }));
}

#[test]
fn check_program_rejects_unsupported_tool_requirement_constructor() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
	tool shell(cmd: string) -> string ![Command]
	    require UnsupportedRequirement("host.shell.sandboxed");

"#,
    ));
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Syntax(SyntaxDiagnosticCode::UnexpectedToken)
                && diagnostic
                    .message
                    .contains("tool requirement clauses are obsolete")
        }),
        "{:?}",
        parsed.diagnostics
    );
}

#[test]
fn check_program_reports_command_tool_without_sandbox_requirement() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
tool shell(cmd: string) -> string ![Command] {
    return cmd;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::MissingSandboxActionArgument)
        }),
        "{:?}",
        output.diagnostics
    );
    let (tool_id, _) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Tool(tool)
                if hir
                    .symbols
                    .get(tool.symbol)
                    .is_some_and(|symbol| symbol.name == "shell") =>
            {
                Some((id, tool))
            }
            _ => None,
        })
        .expect("shell tool should lower");
    assert!(matches!(
        output.facts.interpreter_support.items.get(&tool_id),
        Some(InterpreterSupport::Rejected(
            FrontendRejectionReason::MissingRequirement
        ))
    ));
}

#[test]
fn check_program_reports_command_extension_tool_without_sandbox_requirement() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Shell extends Command {
  action run() -> unit;
}

tool shell(cmd: string) -> string ![Shell] {
    return cmd;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::MissingSandboxActionArgument)
        }),
        "{:?}",
        output.diagnostics
    );
    let (tool_id, _) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Tool(tool)
                if hir
                    .symbols
                    .get(tool.symbol)
                    .is_some_and(|symbol| symbol.name == "shell") =>
            {
                Some((id, tool))
            }
            _ => None,
        })
        .expect("shell tool should lower");
    assert!(matches!(
        output.facts.interpreter_support.items.get(&tool_id),
        Some(InterpreterSupport::Rejected(
            FrontendRejectionReason::MissingRequirement
        ))
    ));
}

#[test]
fn check_program_records_propagated_requirements_by_item_and_symbol() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
tool search(q: string) -> string ![Network] {
    return q;
}

flow main(q: string) -> string {
    return search(q);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let (main_id, main) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some((id, flow))
            }
            _ => None,
        })
        .expect("main flow should lower");
    assert!(
        output
            .facts
            .requirements
            .items
            .get(&main_id)
            .is_none_or(|requirements| requirements.iter().next().is_none()),
        "{:?}",
        output.facts.requirements
    );
    assert!(
        output
            .facts
            .requirements
            .symbols
            .get(&main.symbol)
            .is_none_or(|requirements| requirements.iter().next().is_none()),
        "{:?}",
        output.facts.requirements
    );
}

#[test]
fn check_program_agent_declaration_summary_does_not_record_inference_action() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
agent Reviewer(input: Prompt) -> string {
    return input;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let reviewer = agent_item(&hir, "Reviewer");
    let summary = output
        .facts
        .item_effects
        .get(&reviewer)
        .expect("agent summary should exist");
    assert!(
        !has_agentic_infer(&summary.requested_actions),
        "{summary:?}"
    );
    assert!(!has_agentic_infer(&summary.default_actions), "{summary:?}");
    assert!(
        !trace_has_agentic_infer(&summary.action_trace),
        "{summary:?}"
    );
    assert!(
        !has_agentic_escape(&summary.escaping_effects),
        "{summary:?}"
    );
}

#[test]
fn check_program_rejects_agent_public_agentic_infer_row() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
agent Reviewer(input: Prompt) -> string ![Agentic.infer<Reviewer>] {
    return input;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::EffectOutsideDeclaredRow)
                && diagnostic
                    .message
                    .contains("Agentic.infer<A> is an internal requested action")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_treats_registry_declared_prompt_builder_methods_as_pure_support() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.agent.prompt.Prompt;

flow main(input: string) -> Prompt ![] {
    return Prompt.new().system(Trusted("review")).user(Public(input));
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_id = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
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
    assert_eq!(
        output.facts.interpreter_support.items.get(&main_id),
        Some(&InterpreterSupport::LocalOnly)
    );
}

#[test]
fn check_program_treats_trust_prompt_encoding_as_pure_support() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.agent.prompt.Prompt;

flow main(input: string) -> Prompt ![] {
    let trusted: Trusted<string> = Trusted(input);
    return Prompt.new().system(trusted).data(Sanitized(input));
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_id = flow_item(&hir, "main");
    let summary = &output.facts.item_effects[&main_id];
    assert!(summary.escaping_effects.effects.is_empty());
    assert!(summary.requested_actions.effects.is_empty());
    assert!(summary.default_actions.effects.is_empty());
    assert_eq!(
        output.facts.interpreter_support.items.get(&main_id),
        Some(&InterpreterSupport::LocalOnly)
    );
}

#[test]
fn check_program_treats_registry_declared_message_methods_as_pure_support() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.agent.message.Message;

flow main(input: string) -> Option<Message<string>> ![] {
    let message = Message.new(input);
    return message.cast<string>();
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_id = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
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
    assert_eq!(
        output.facts.interpreter_support.items.get(&main_id),
        Some(&InterpreterSupport::LocalOnly)
    );
}

#[test]
fn check_program_treats_agent_runtime_support_types_as_local_only() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.agent.schema.{ModelResponse, ResponseDecode};
import std.agent.message.Provenance;

flow main(
    response: ModelResponse,
    decoder: ResponseDecode<string>,
    provenance: Provenance
) -> unit ![] {
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_id = flow_item(&hir, "main");
    let summary = &output.facts.item_effects[&main_id];
    assert!(summary.escaping_effects.effects.is_empty());
    assert!(summary.requested_actions.effects.is_empty());
    assert!(summary.default_actions.effects.is_empty());
    assert_eq!(
        output.facts.interpreter_support.items.get(&main_id),
        Some(&InterpreterSupport::LocalOnly)
    );
}

#[test]
fn check_program_treats_registry_declared_session_policy_constructors_as_pure_support() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(limit: Limit) -> unit ![] {
    let recent: ContextPolicy = LastTurns(8);
    let summarized: ContextPolicy = SummaryPlusRecent(4);
    let retention: RetentionPolicy = Days(90);
    let compaction: CompactionPolicy = SummarizeWhen(limit);
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_id = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
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
    assert_eq!(
        output.facts.interpreter_support.items.get(&main_id),
        Some(&InterpreterSupport::LocalOnly)
    );
}

#[test]
fn check_program_treats_session_config_continue_or_new_as_pure_support() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(ticket: string) -> SessionConfig ![] {
    return SessionConfig.continue_or_new(ticket);
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_id = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
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
    assert_eq!(
        output.facts.interpreter_support.items.get(&main_id),
        Some(&InterpreterSupport::LocalOnly)
    );
}

#[test]
fn check_program_records_conversation_load_memory_read_action() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(ticket: string) -> Conversation ![Memory.read<SessionId>] {
    let session = SessionConfig.continue_or_new(ticket);
    return Conversation.load(session);
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = flow_item(&hir, "main");
    let summary = output
        .facts
        .item_effects
        .get(&main)
        .expect("main should have effect facts");
    assert!(
        summary.requested_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.action.action == MEMORY_READ_ACTION
                        && action.args
                            == vec![etas_types::EffectArgRef::Path(vec![
                                "std".to_owned(),
                                "agent".to_owned(),
                                "session".to_owned(),
                                "SessionId".to_owned()
                            ])]
            )
        }),
        "Conversation.load must request Memory.read<SessionId>: {summary:?}"
    );
    assert!(
        summary.default_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.action.action == MEMORY_READ_ACTION
                        && action.args
                            == vec![etas_types::EffectArgRef::Path(vec![
                                "std".to_owned(),
                                "agent".to_owned(),
                                "session".to_owned(),
                                "SessionId".to_owned()
                            ])]
            )
        }),
        "Conversation.load must mark Memory.read<SessionId> as default-handled: {summary:?}"
    );
    assert!(
        summary.escaping_effects.effects.is_empty(),
        "Conversation.load default-handled Memory.read<SessionId> must not escape: {summary:?}"
    );
}

#[test]
fn check_program_records_conversation_compact_memory_write_default_action() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(ticket: SessionId, limit: Limit) -> Conversation ![] {
    let session = SessionConfig {
        id = ticket,
        context = SummaryPlusRecent(1),
        retention = Days(90),
        compaction = SummarizeWhen(limit),
    };
    return Conversation.compact(session);
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = flow_item(&hir, "main");
    let summary = output
        .facts
        .item_effects
        .get(&main)
        .expect("main should have effect facts");
    assert!(
        summary.requested_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.action.action == MEMORY_WRITE_ACTION
                        && action.args
                            == vec![etas_types::EffectArgRef::Path(vec![
                                "std".to_owned(),
                                "agent".to_owned(),
                                "session".to_owned(),
                                "SessionId".to_owned()
                            ])]
            )
        }),
        "Conversation.compact must request Memory.write<SessionId>: {summary:?}"
    );
    assert!(
        summary.default_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.action.action == MEMORY_WRITE_ACTION
                        && action.args
                            == vec![etas_types::EffectArgRef::Path(vec![
                                "std".to_owned(),
                                "agent".to_owned(),
                                "session".to_owned(),
                                "SessionId".to_owned()
                            ])]
            )
        }),
        "Conversation.compact must mark Memory.write<SessionId> as default-handled: {summary:?}"
    );
    assert!(
        summary.escaping_effects.effects.is_empty(),
        "Conversation.compact default-handled Memory.write<SessionId> must not escape: {summary:?}"
    );
}

#[test]
fn check_program_rejects_method_call_without_receiver_type_fact() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(values: Array<i32>) -> usize {
  return values.len();
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let receiver = hir
        .exprs
        .iter()
        .find_map(|(_, expr)| {
            if let HirExpr::MethodCall {
                receiver, method, ..
            } = expr
                && method == "len"
            {
                Some(*receiver)
            } else {
                None
            }
        })
        .expect("len receiver should lower");
    types.facts.expr_types.remove(&receiver);

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic
                    .message
                    .contains("method effect propagation requires a checked receiver type fact")
        }),
        "{:?}",
        output.diagnostics
    );
    let main = flow_item(&hir, "main");
    assert_eq!(
        output.facts.interpreter_support.items.get(&main),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        )),
        "missing method receiver type facts must not fall back to runtime handler support"
    );
}

#[test]
fn check_program_classifies_agent_pipeline_stage_as_inference() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
agent Reviewer(input: Prompt) -> string {
    return input;
}

flow main(input: Prompt) -> string {
    return input ~> Reviewer;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_id = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
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
    let summary = output
        .facts
        .item_effects
        .get(&main_id)
        .expect("main summary should exist");
    assert!(has_agentic_infer(&summary.requested_actions), "{summary:?}");
    assert!(has_agentic_infer(&summary.default_actions), "{summary:?}");
    assert!(
        trace_has_agentic_infer(&summary.action_trace),
        "{summary:?}"
    );
    assert!(
        trace_has_agentic_infer_source(&summary.action_trace, ActionEventSource::AgentCall),
        "agent pipeline trace should be compiler-generated call metadata, not a declared row: {summary:?}"
    );
    assert!(
        !has_agentic_escape(&summary.escaping_effects),
        "{summary:?}"
    );
    assert!(matches!(
        output.facts.interpreter_support.items.get(&main_id),
        Some(InterpreterSupport::RequiresHost(requirements))
            if requirements.kinds.contains(&HostRequirementKind::Agentic)
    ));
}

#[test]
fn check_program_classifies_agent_run_method_call_as_inference() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
agent Reviewer(input: Prompt) -> string {
    return input;
}

flow main(input: Prompt) -> string {
    return Reviewer.run(input);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_id = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
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
    let summary = output
        .facts
        .item_effects
        .get(&main_id)
        .expect("main summary should exist");
    assert!(has_agentic_infer(&summary.requested_actions), "{summary:?}");
    assert!(has_agentic_infer(&summary.default_actions), "{summary:?}");
    assert!(
        trace_has_agentic_infer(&summary.action_trace),
        "{summary:?}"
    );
    assert!(
        trace_has_agentic_infer_source(&summary.action_trace, ActionEventSource::AgentCall),
        "agent run trace should be compiler-generated call metadata, not a declared row: {summary:?}"
    );
    assert!(
        !has_agentic_escape(&summary.escaping_effects),
        "{summary:?}"
    );
    assert!(matches!(
        output.facts.interpreter_support.items.get(&main_id),
        Some(InterpreterSupport::RequiresHost(requirements))
            if requirements.kinds.contains(&HostRequirementKind::Agentic)
    ));
}

#[test]
fn check_program_classifies_source_import_agent_run_method_call_as_inference() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

agent Reviewer(input: Prompt) -> string {
    return input;
}

flow main(input: Prompt) -> string {
    return Reviewer.run(input);
}
"#,
    ));
    let mut hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let reviewer = agent_item(&hir, "Reviewer");
    let receiver = hir
        .exprs
        .iter()
        .find_map(|(_, expr)| match expr {
            HirExpr::MethodCall {
                receiver, method, ..
            } if method == "run" => Some(*receiver),
            _ => None,
        })
        .expect("agent run receiver should lower");
    let reviewer_symbol = match hir.items.get(reviewer) {
        Some(HirItem::Agent(agent)) => agent.symbol,
        _ => unreachable!("reviewer id came from agent item"),
    };
    let (defining_module, definition_span) = hir
        .symbols
        .get(reviewer_symbol)
        .map(|symbol| (symbol.defining_module, symbol.definition_span))
        .expect("reviewer symbol should exist");
    let alias = hir.symbols.alloc(SymbolData {
        name: "Reviewer".to_owned(),
        kind: SymbolKind::Import,
        visibility: Visibility::Private,
        defining_module,
        defining_item: None,
        def: SymbolDef::ImportAlias {
            path: vec!["app".to_owned(), "main".to_owned(), "Reviewer".to_owned()],
            origin: ImportAliasOrigin::SourceImport,
        },
        declared_type: None,
        definition_span,
    });
    let HirExpr::Path(path) = hir
        .exprs
        .get_mut(receiver)
        .expect("receiver expression should be mutable")
    else {
        unreachable!("receiver id came from path expression");
    };
    path.resolution = ResolveResult::Resolved(alias);

    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = flow_item(&hir, "main");
    let summary = output
        .facts
        .item_effects
        .get(&main)
        .expect("main summary should exist");
    assert!(
        has_agentic_infer(&summary.requested_actions),
        "source-import agent run should record the target agent inference action"
    );
    assert!(has_agentic_infer(&summary.default_actions), "{summary:?}");
    assert!(
        trace_has_agentic_infer(&summary.action_trace),
        "{summary:?}"
    );
    assert!(
        trace_has_agentic_infer_source(&summary.action_trace, ActionEventSource::AgentCall),
        "source-import agent run trace should be compiler-generated call metadata, not a declared row: {summary:?}"
    );
    assert!(
        !has_agentic_escape(&summary.escaping_effects),
        "{summary:?}"
    );
}

#[test]
fn check_program_rejects_unresolved_agent_run_receiver() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
agent Reviewer(input: Prompt) -> string {
    return input;
}

flow main(input: Prompt) -> string {
    return Reviewer.run(input);
}
"#,
    ));
    let mut hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let receiver = hir
        .exprs
        .iter()
        .find_map(|(_, expr)| match expr {
            HirExpr::MethodCall {
                receiver, method, ..
            } if method == "run" => Some(*receiver),
            _ => None,
        })
        .expect("agent run receiver should lower");
    let HirExpr::Path(path) = hir
        .exprs
        .get_mut(receiver)
        .expect("receiver expression should be mutable")
    else {
        unreachable!("receiver id came from path expression");
    };
    path.resolution = ResolveResult::Unresolved;

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic
                    .message
                    .contains("agent run effect propagation requires a resolved agent receiver")
        }),
        "{:?}",
        output.diagnostics
    );
    let main = flow_item(&hir, "main");
    assert_eq!(
        output.facts.interpreter_support.items.get(&main),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        )),
        "unresolved agent run receiver must not fall back to generic runtime handler support"
    );
}

#[test]
fn check_program_rejects_missing_source_import_agent_run_receiver() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

agent Reviewer(input: Prompt) -> string {
    return input;
}

flow main(input: Prompt) -> string {
    return Reviewer.run(input);
}
"#,
    ));
    let mut hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let receiver = hir
        .exprs
        .iter()
        .find_map(|(_, expr)| match expr {
            HirExpr::MethodCall {
                receiver, method, ..
            } if method == "run" => Some(*receiver),
            _ => None,
        })
        .expect("agent run receiver should lower");
    let alias = hir.symbols.alloc(SymbolData {
        name: "Reviewer".to_owned(),
        kind: SymbolKind::Import,
        visibility: Visibility::Private,
        defining_module: etas_hir::HirModuleId(0),
        defining_item: None,
        def: SymbolDef::ImportAlias {
            path: vec![
                "missing".to_owned(),
                "module".to_owned(),
                "Reviewer".to_owned(),
            ],
            origin: ImportAliasOrigin::SourceImport,
        },
        declared_type: None,
        definition_span: hir.exprs[receiver].span(&hir.blocks),
    });
    let HirExpr::Path(path) = hir
        .exprs
        .get_mut(receiver)
        .expect("receiver expression should be mutable")
    else {
        unreachable!("receiver id came from path expression");
    };
    path.resolution = ResolveResult::Resolved(alias);

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic.message.contains("source import target")
        }),
        "{:?}",
        output.diagnostics
    );
    let main = flow_item(&hir, "main");
    assert_eq!(
        output.facts.interpreter_support.items.get(&main),
        Some(&InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        )),
        "missing source-import agent run receiver must not fall back to runtime handler support"
    );
}

#[test]
fn check_program_rejects_unsupported_tool_requirement_expressions() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
tool search(q: string) -> string ![Network]
    require "host.network.search"
{
    return q;
}
"#,
    ));
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Syntax(SyntaxDiagnosticCode::UnexpectedToken)
                && diagnostic
                    .message
                    .contains("tool requirement clauses are obsolete")
        }),
        "{:?}",
        parsed.diagnostics
    );
}

#[test]
fn check_program_records_local_flow_effects() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        "flow main() -> unit { return; }",
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let (flow_id, _) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow) => Some((id, flow)),
            _ => None,
        })
        .expect("flow should lower");
    assert!(matches!(
        output.facts.interpreter_support.items.get(&flow_id),
        Some(InterpreterSupport::LocalOnly)
    ));
    assert!(matches!(
        output.facts.interpreter_support.entry,
        Some(InterpreterSupport::LocalOnly)
    ));
}

#[test]
fn check_program_marks_explicit_runtime_effects() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;
import std.io.println;

flow main() -> unit ![Error<IOError>] {
    println("hello");
    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let (flow_id, _) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow) => Some((id, flow)),
            _ => None,
        })
        .expect("flow should lower");
    assert!(matches!(
        output.facts.interpreter_support.items.get(&flow_id),
        Some(InterpreterSupport::RequiresHost(requirements))
            if requirements.kinds.contains(&HostRequirementKind::Console)
    ));
}

#[test]
fn check_program_reports_flow_effects_outside_declared_row() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
    action request() -> unit;
}

effect Payment {
    action ping() -> unit;
}

flow main() -> unit ![Approval]

{
    perform Payment.ping();
    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::EffectOutsideDeclaredRow)
        }),
        "{:?}",
        output.diagnostics
    );
    let (main_id, _) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some((id, flow))
            }
            _ => None,
        })
        .expect("main flow should lower");
    assert!(matches!(
        output.facts.interpreter_support.items.get(&main_id),
        Some(InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        ))
    ));
}

#[test]
fn check_program_agent_declaration_public_row_does_not_record_inference_action() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
agent Reviewer(input: Prompt) -> string ![Network] {
    return input;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let reviewer = agent_item(&hir, "Reviewer");
    let summary = output
        .facts
        .item_effects
        .get(&reviewer)
        .expect("agent summary should exist");
    assert!(
        !has_agentic_infer(&summary.requested_actions),
        "{summary:?}"
    );
    assert!(!has_agentic_infer(&summary.default_actions), "{summary:?}");
    assert!(
        !trace_has_agentic_infer(&summary.action_trace),
        "{summary:?}"
    );
    assert!(
        !has_agentic_escape(&summary.escaping_effects),
        "{summary:?}"
    );
}

#[test]
fn check_program_records_pipeline_agent_inference_without_flow_row_escape() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
agent Reviewer(input: Prompt) -> string {
    return input;
}

flow main(input: Prompt) -> string ![Network]

{
    return input ~> Reviewer;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = flow_item(&hir, "main");
    let summary = output
        .facts
        .item_effects
        .get(&main)
        .expect("main summary should exist");
    assert!(
        summary
            .requested_actions
            .effects
            .iter()
            .any(|effect| matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.action.tag == AGENTIC_TAG
                        && action.action.action == AGENTIC_INFER_ACTION
            )),
        "{summary:?}"
    );
    assert!(
        summary
            .default_actions
            .effects
            .iter()
            .any(|effect| matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.action.tag == AGENTIC_TAG
                        && action.action.action == AGENTIC_INFER_ACTION
            )),
        "{summary:?}"
    );
    assert!(
        !has_agentic_escape(&summary.escaping_effects),
        "{summary:?}"
    );
}

#[test]
fn check_program_infers_performed_effect_action_owner() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
    action request() -> unit;
}

flow main() -> unit {
    perform Approval.request();
    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let (main_id, _) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some((id, flow))
            }
            _ => None,
        })
        .expect("main flow should lower");
    let summary = output
        .facts
        .item_effects
        .get(&main_id)
        .expect("main should have effect summary");
    assert!(summary.requested_actions.effects.iter().any(|effect| {
        matches!(effect, Effect::Action(action) if action.tag == EffectTagId(5))
    }));
    assert!(matches!(
        output.facts.interpreter_support.items.get(&main_id),
        Some(InterpreterSupport::RequiresHost(requirements))
            if requirements.kinds.contains(&HostRequirementKind::Approval)
    ));
}

#[test]
fn check_program_records_performed_action_fact() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
    action request(reason: string) -> unit;
}

flow main() -> unit {
    perform Approval.request("ship");
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let perform_expr = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| matches!(expr, HirExpr::Perform { .. }).then_some(id))
        .expect("perform expression should lower");
    let fact = output
        .facts
        .performed_actions
        .get(&perform_expr)
        .expect("resolved perform should materialize an action fact");
    assert_eq!(fact.expr, perform_expr);
    assert_eq!(fact.effect_segments, vec!["Approval"]);
    assert_eq!(fact.action, "request");
    assert_eq!(fact.args.len(), 1);
    assert!(matches!(
        hir.symbols
            .get(
                fact.action_symbol
                    .expect("source perform should record its action symbol")
            )
            .map(|symbol| &symbol.def),
        Some(SymbolDef::EffectAction { .. })
    ));
    assert!(
        fact.summary
            .requested_actions
            .effects
            .contains(&Effect::Action(ActionRef {
                tag: APPROVAL_TAG,
                action: APPROVAL_REQUEST_ACTION,
            })),
        "{:?}",
        fact.summary
    );
    assert!(
        !fact
            .summary
            .default_actions
            .effects
            .contains(&Effect::Action(ActionRef {
                tag: APPROVAL_TAG,
                action: APPROVAL_REQUEST_ACTION,
            })),
        "source Approval.request perform must not be marked as default-handled: {:?}",
        fact.summary
    );
    assert!(matches!(
        fact.summary.action_trace,
        ActionTraceDomain::Event(ActionEvent {
            action: Effect::Action(ActionRef {
                tag: APPROVAL_TAG,
                action: APPROVAL_REQUEST_ACTION,
            }),
            source: ActionEventSource::Perform,
            ..
        })
    ));
}

#[test]
fn check_program_rejects_resolved_perform_without_action_signature_fact() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
    action request() -> unit;
}

flow main() -> unit {
    perform Approval.request();
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    types.facts.action_signatures.clear();
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic.message.contains("checked action signature fact")
        }),
        "{:?}",
        output.diagnostics
    );
    assert!(
        output.facts.performed_actions.is_empty(),
        "resolved performed actions without checked action metadata must not materialize fallback facts"
    );
    let summary = output
        .facts
        .item_effects
        .get(&flow_item(&hir, "main"))
        .expect("main should have effect summary");
    assert!(matches!(
        summary.support,
        InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect)
    ));
}

#[test]
fn check_program_rejects_handler_without_action_signature_fact() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
    action request() -> unit;
}

flow main() -> unit {
    handle {
        return;
    } with {
        Approval.request() => {
            resume;
        }
    };
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    types.facts.action_signatures.clear();

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic
                    .message
                    .contains("handler arm effect solving requires a checked action signature fact")
        }),
        "{:?}",
        output.diagnostics
    );
    let summary = output
        .facts
        .item_effects
        .get(&flow_item(&hir, "main"))
        .expect("main should have effect summary");
    assert!(matches!(
        summary.support,
        InterpreterSupport::Rejected(FrontendRejectionReason::UnsupportedHandler)
    ));
}

#[test]
fn check_program_allows_perform_payload_with_explicit_wildcard_static_selector() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Memory {
    action read<Scope>(value: i32) -> unit;
}

flow main() -> unit ![Memory.read<_>] {
    perform Memory.read<_>(1 + 2);
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summary = output
        .facts
        .item_effects
        .get(&flow_item(&hir, "main"))
        .expect("main should have effect summary");
    assert!(
        summary
            .requested_actions
            .effects
            .iter()
            .any(|effect| matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.action.tag == MEMORY_TAG
                        && action.args == vec![etas_types::EffectArgRef::Wildcard]
            )),
        "perform payload values should not be required as static action selectors: {summary:?}"
    );
}

#[test]
fn check_program_materializes_omitted_perform_static_action_selector_as_wildcard() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Memory {
    action read<Scope>(value: i32) -> unit;
}

flow main() -> unit ![Memory.read<_>] {
    perform Memory.read(1 + 2);
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summary = output
        .facts
        .item_effects
        .get(&flow_item(&hir, "main"))
        .expect("main should have effect summary");
    assert!(
        summary.requested_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.action.tag == MEMORY_TAG
                        && action.args == vec![etas_types::EffectArgRef::Wildcard]
            )
        }),
        "omitted Memory.read selector should materialize as wildcard: {summary:?}"
    );
}

#[test]
fn source_action_explicit_static_type_selector_materializes_scoped_action_arg() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect EdkHttp extends Network {
    action request<GetExample>(method: string, host: string, request_id: i32) -> string;
}

type ExampleScope;

flow main() -> string ![EdkHttp.request<ExampleScope>] {
    return perform EdkHttp.request<ExampleScope>("GET", "example.com", 0);
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summary = output
        .facts
        .item_effects
        .get(&flow_item(&hir, "main"))
        .expect("main should have effect summary");
    assert!(
        summary.requested_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.args
                        == vec![
                            etas_types::EffectArgRef::Type(type_id_by_symbol_name(
                                &hir,
                                &types,
                                "ExampleScope"
                            ))
                        ]
            )
        }),
        "EdkHttp.request must preserve explicit static selector args: {summary:?}"
    );
}

#[test]
fn source_action_type_param_selector_materializes_scoped_action_arg() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Workspace {
    action read<R>(path: string) -> string;
}

spec ReadablePath<R>;

flow main<R>(path: string) -> string ![Workspace.read<R>] {
    return perform Workspace.read<R>(path);
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summary = output
        .facts
        .item_effects
        .get(&flow_item(&hir, "main"))
        .expect("main should have effect summary");
    assert!(
        summary.requested_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.args.iter().any(|arg| matches!(
                        arg,
                        etas_types::EffectArgRef::Type(ty)
                            if matches!(
                                types.store.get(*ty),
                                Some(etas_types::Type::Named(named)) if named.name == "R"
                            )
                    ))
            )
        }),
        "Workspace.read<R> must preserve the flow type-param selector: {summary:?}"
    );
}

#[test]
fn source_action_payload_generic_selector_preserves_declared_arity() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec WorkspaceRegion;
spec ReadablePath<R>;

type ReportsRoot;
type ReportPath;

impl ReportsRoot ~ WorkspaceRegion;
impl ReportPath ~ ReadablePath<ReportsRoot>;

effect Workspace {
    action read<R ~ WorkspaceRegion, P ~ ReadablePath<R>>(path: P) -> string;
}

flow main<R ~ WorkspaceRegion, P ~ ReadablePath<R>>(path: P) -> string ![Workspace.read<R, P>] {
    return perform Workspace.read<R, P>(path);
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summary = output
        .facts
        .item_effects
        .get(&flow_item(&hir, "main"))
        .expect("main should have effect summary");
    assert!(
        summary.requested_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.args.len() == 2
                        && action.args.iter().any(|arg| matches!(
                            arg,
                            etas_types::EffectArgRef::Type(ty)
                                if matches!(
                                    types.store.get(*ty),
                                    Some(etas_types::Type::Named(named)) if named.name == "R"
                                )
                        ))
                        && action.args.iter().any(|arg| matches!(
                            arg,
                            etas_types::EffectArgRef::Type(ty)
                                if matches!(
                                    types.store.get(*ty),
                                    Some(etas_types::Type::Named(named)) if named.name == "P"
                                )
                        ))
            )
        }),
        "Workspace.read<R, P> must preserve both declared static selectors: {summary:?}"
    );
}

#[test]
fn dynamic_source_action_payload_params_materialize_wildcard_action_args() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect EdkHttp extends Network {
    action request<Scope>(method: string, host: string, request_id: i32) -> string;
}

flow main(method: string, host: string) -> string ![EdkHttp.request<_>] {
    return perform EdkHttp.request<_>(method, host, 0);
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summary = output
        .facts
        .item_effects
        .get(&flow_item(&hir, "main"))
        .expect("main should have effect summary");
    assert!(
        summary.requested_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.args
                        == vec![
                            etas_types::EffectArgRef::Wildcard,
                        ]
            )
        }),
        "runtime EdkHttp.request payload params should not become static action selectors: {summary:?}"
    );
}

#[test]
fn source_action_omitted_perform_selector_materializes_wildcard() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect EdkHttp extends Network {
    action request<Scope>(method: string, host: string, request_id: i32) -> string;
}

flow main(method: string, host: string) -> string ![EdkHttp.request<_>] {
    return perform EdkHttp.request(method, host, 0);
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summary = output
        .facts
        .item_effects
        .get(&flow_item(&hir, "main"))
        .expect("main should have effect summary");
    assert!(
        summary.requested_actions.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::AppliedAction(action)
                    if action.args == vec![etas_types::EffectArgRef::Wildcard]
            )
        }),
        "omitted EdkHttp.request selector must materialize as wildcard without a declaration-level `_` workaround: {summary:?}"
    );
}

#[test]
fn check_program_rejects_perform_when_owner_effect_arg_cannot_be_materialized() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
alias IOError = string;

effect Failure<E> {
    action raise() -> unit;
}

flow main() -> unit ![Failure<IOError>] {
    perform Failure<IOError>.raise();
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let owner_arg = hir
        .exprs
        .iter()
        .find_map(|(_, expr)| match expr {
            HirExpr::Perform { action, .. } => action.effect.args.first(),
            _ => None,
        })
        .expect("perform owner effect arg should lower");
    let etas_hir::HirEffectArg::Path(owner_arg) = owner_arg else {
        panic!("perform owner effect arg should lower as a path ref");
    };
    let ResolveResult::Resolved(owner_arg_symbol) = owner_arg.resolution else {
        panic!("perform owner effect arg should resolve to the IOError type symbol");
    };
    types.facts.symbol_types.remove(&owner_arg_symbol);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic
                    .message
                    .contains("owner effect arguments require checked type argument facts")
        }),
        "{:?}",
        output.diagnostics
    );
    assert!(
        output.facts.performed_actions.is_empty(),
        "perform with unmaterialized owner effect args must not materialize fallback action facts"
    );
    let summary = output
        .facts
        .item_effects
        .get(&flow_item(&hir, "main"))
        .expect("main should have effect summary");
    assert!(matches!(
        summary.support,
        InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect)
    ));
}

#[test]
fn check_program_rejects_handler_when_handled_owner_arg_cannot_be_materialized() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
alias IOError = string;

effect Failure<E> {
    action raise() -> unit;
}

flow main() -> unit ![Failure<IOError>] {
    handle {
        perform Failure<IOError>.raise();
    } with {
        Failure<IOError>.raise() => {
            resume;
        }
    };
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let io_error_symbol = hir
        .symbols
        .iter()
        .find_map(|symbol| (symbol.name == "IOError").then_some(symbol.id))
        .expect("IOError type symbol should lower");
    types.facts.symbol_types.remove(&io_error_symbol);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic.message.contains(
                    "handler arm effect solving requires checked handled-action type argument facts",
                )
        }),
        "{:?}",
        output.diagnostics
    );
    let summary = output
        .facts
        .item_effects
        .get(&flow_item(&hir, "main"))
        .expect("main should have effect summary");
    assert!(matches!(summary.support, InterpreterSupport::Rejected(_)));
}

#[test]
fn check_program_reports_unresolved_performed_action() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
}

flow main() -> unit {
    perform Approval.request();
    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::UnresolvedPerformedAction)
        }),
        "{:?}",
        output.diagnostics
    );
    assert!(
        output.facts.performed_actions.is_empty(),
        "unresolved performed actions must not materialize fallback facts"
    );
}

#[test]
fn check_program_does_not_resolve_undeclared_web_search_from_std() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(q: string) -> unit {
    perform Web.search(q);
    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::UnresolvedPerformedAction)
        }),
        "{:?}",
        output.diagnostics
    );
    assert!(
        output.facts.performed_actions.is_empty(),
        "undeclared Web.search must not materialize through std fallback"
    );
}

#[test]
fn check_program_resolves_source_declared_web_search() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Web extends Network {
    action search(q: string) -> unit;
}

flow main(q: string) -> unit ![Web.search] {
    perform Web.search(q);
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(
        !output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::UnresolvedPerformedAction)
        }),
        "{:?}",
        output.diagnostics
    );

    let registry = EffectRegistry::from_hir_and_types(&hir, &types)
        .expect("source effects should build a registry");
    let web = registry
        .tag_by_name("Web")
        .expect("source-declared Web effect should enter registry");
    assert!(registry.tag_extends(web, NETWORK_TAG));
    let search = registry
        .action_by_name("Web.search")
        .expect("source-declared Web.search action should enter registry");
    assert_eq!(search.tag, web);
}

#[test]
fn check_program_rejects_unknown_declared_effect_instead_of_pure_fallback() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> unit ![MissingEffect] {
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::UnknownEffectTag)
        }),
        "{:?}",
        output.diagnostics
    );
    let main = flow_item(&hir, "main");
    assert!(matches!(
        output.facts.interpreter_support.items.get(&main),
        Some(InterpreterSupport::Rejected(
            FrontendRejectionReason::UnresolvedEffect
        ))
    ));
}

#[test]
fn check_program_rejects_effect_arg_path_that_only_matches_by_last_segment() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module app.main;

alias IOError = string;

flow main() -> unit ![Error<other.module.IOError>] {
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(
        types.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Type(etas_core::TypeDiagnosticCode::InvalidEffectArgument)
        }),
        "{:?}",
        types.diagnostics
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_rejects_effect_arg_type_path_without_checked_type_fact() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
alias IOError = string;

flow main() -> unit ![Error<IOError>] {
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let main = flow_item(&hir, "main");
    let io_error_symbol = hir
        .symbols
        .iter()
        .find_map(|symbol| (symbol.name == "IOError").then_some(symbol.id))
        .expect("IOError type symbol should lower");
    types.facts.symbol_types.remove(&io_error_symbol);
    let signature = types
        .facts
        .item_signatures
        .get_mut(&main)
        .expect("main signature should be checked");
    let etas_types::ItemSignature::Flow(signature) = signature else {
        panic!("main should have a flow signature");
    };
    signature
        .effects
        .as_mut()
        .expect("main effect row should be checked")
        .effects[0]
        .args = vec![etas_types::EffectArgRef::Path(vec!["IOError".to_owned()])];

    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic
                    .message
                    .contains("effect tag arguments require checked type argument facts")
        }),
        "{:?}",
        output.diagnostics
    );
    assert!(matches!(
        output.facts.interpreter_support.items.get(&main),
        Some(InterpreterSupport::Rejected(
            FrontendRejectionReason::EscapedEffect
        ))
    ));
}

#[test]
fn check_program_includes_handler_arm_effects_and_requirements() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
    action request() -> unit;
}

effect Payment extends Network {
    action ping() -> unit;
}

flow main() -> unit {
    handle {
        perform Approval.request();
    } with {
        Approval.request() => {
            perform Payment.ping();
            resume;
        }
    };

    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let (main_id, main) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some((id, flow))
            }
            _ => None,
        })
        .expect("main flow should lower");
    let summary = output
        .facts
        .item_effects
        .get(&main_id)
        .expect("main should have effect summary");
    assert!(
        summary
            .escaping_effects
            .effects
            .iter()
            .any(is_action_effect)
    );
    assert!(
        !summary
            .escaping_effects
            .effects
            .iter()
            .any(is_approval_request_effect),
        "handled Approval effect should not escape the handle expression"
    );

    assert!(
        output
            .facts
            .requirements
            .symbols
            .get(&main.symbol)
            .is_none_or(|requirements| requirements.iter().next().is_none()),
        "{:?}",
        output.facts.requirements
    );
    let application = output
        .facts
        .handle_applications
        .values()
        .next()
        .expect("handle application fact");
    assert!(
        application
            .handled
            .effects
            .iter()
            .any(is_approval_request_effect),
        "{application:?}"
    );
    assert!(
        application.produced.effects.iter().any(is_action_effect),
        "{application:?}"
    );
}

#[test]
fn check_program_does_not_hide_handler_arm_effects() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
    action request() -> unit;
}

effect Payment extends Network {
    action ping() -> unit;
}


flow main() -> unit {
    handle {
        perform Approval.request();
    } with {
        Approval.request() => {
            perform Payment.ping();
            resume;
        }
    };

    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let (main_id, _) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some((id, flow))
            }
            _ => None,
        })
        .expect("main flow should lower");
    let summary = output
        .facts
        .item_effects
        .get(&main_id)
        .expect("main should have effect summary");
    assert!(
        summary
            .escaping_effects
            .effects
            .iter()
            .any(is_action_effect),
        "handler arm Network effect must still escape the handle expression: {summary:?}"
    );
    assert!(support_requires_host(
        output.facts.interpreter_support.items.get(&main_id),
        HostRequirementKind::Network,
    ));
}

#[test]
fn check_program_removes_handled_escaping_effects_but_keeps_requested_actions() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
    action request() -> unit;
}

flow main() -> unit {
    handle {
        perform Approval.request();
    } with {
        Approval.request() => {
            resume;
        }
    };

    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let (main_id, _) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some((id, flow))
            }
            _ => None,
        })
        .expect("main flow should lower");
    let summary = output
        .facts
        .item_effects
        .get(&main_id)
        .expect("main should have effect summary");

    assert!(
        !summary
            .escaping_effects
            .effects
            .iter()
            .any(is_approval_request_effect),
        "{summary:?}"
    );
    assert!(
        summary
            .requested_actions
            .effects
            .iter()
            .any(is_approval_request_effect),
        "handlers must not erase requested action footprints needed for policy/grants/trace: {summary:?}"
    );
    let application = output
        .facts
        .handle_applications
        .values()
        .next()
        .expect("handle application fact");
    assert!(matches!(application.handler, HandlerValueRef::Expr { .. }));
    assert!(
        application
            .handled
            .effects
            .iter()
            .any(is_approval_request_effect),
        "{application:?}"
    );
    assert!(
        !application
            .remaining
            .effects
            .iter()
            .any(is_approval_request_effect),
        "{application:?}"
    );
    assert!(
        summary
            .requested_actions
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::Action(action) if action.tag == EffectTagId(5))),
        "handler elimination must not erase the performed Approval action footprint: {summary:?}"
    );
}

#[test]
fn check_program_records_handler_value_fact_for_handler_literal() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
    action request() -> unit;
}

flow main() -> unit {
    let reusable: ![Approval => []] = handler {
        Approval.request() => {
            resume;
        }
    };

    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let fact = output
        .facts
        .handler_values
        .values()
        .next()
        .expect("handler value fact");
    assert!(
        fact.handled.effects.iter().any(is_approval_request_effect),
        "{fact:?}"
    );
    assert!(
        fact.produced.effects.is_empty(),
        "resume-only handler must not produce effects: {fact:?}"
    );
    assert_eq!(fact.arms.len(), 1);
    assert!(fact.arms[0].resumable);
    assert_eq!(fact.arms[0].resumes, ResumeSummary::Once);
}

#[test]
fn check_program_records_top_level_handler_value_fact() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
    action request(message: string) -> bool;
}

let AutoApproval: ![Approval => []] = handler {
    Approval.request(message) => {
        resume true;
    }
};

flow main() -> unit {
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_top_level_items(&hir, etas_types::check_program(&hir));
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let handler_expr = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::TopLevelLet(item)
                if hir
                    .symbols
                    .get(item.symbol)
                    .is_some_and(|symbol| symbol.name == "AutoApproval") =>
            {
                Some(item.value)
            }
            _ => None,
        })
        .expect("top-level handler let should lower");
    let fact = output
        .facts
        .handler_values
        .get(&handler_expr)
        .expect("top-level handler value fact");
    assert!(
        fact.handled.effects.iter().any(is_approval_request_effect),
        "{fact:?}"
    );
    assert!(fact.produced.effects.is_empty(), "{fact:?}");
}

#[test]
fn check_program_records_expression_handler_application_fact() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
    action request() -> unit;
}

flow main() -> unit {
    handle {
        perform Approval.request();
    } with {
        Approval.request() => {
            resume;
        }
    };

    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let application = output
        .facts
        .handle_applications
        .values()
        .next()
        .expect("handle application fact");
    assert!(matches!(application.handler, HandlerValueRef::Expr { .. }));
    assert!(
        application
            .handled
            .effects
            .iter()
            .any(is_approval_request_effect),
        "{application:?}"
    );
    assert!(
        !application
            .remaining
            .effects
            .iter()
            .any(is_approval_request_effect),
        "{application:?}"
    );
}

#[test]
fn check_program_records_first_class_handler_application_fact() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
    action request(message: string) -> bool;
}

let AutoApproval: ![Approval => []] = handler {
    Approval.request(message) => {
        resume true;
    }
};

flow choose(approval_handler: ![Approval => []]) -> ![Approval => []] {
    return approval_handler;
}

flow main() -> bool {
    let selected = choose(AutoApproval);
    return handle perform Approval.request("ship") with selected;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_top_level_items(&hir, etas_types::check_program(&hir));
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let application = output
        .facts
        .handle_applications
        .values()
        .next()
        .expect("first-class handle application fact");
    assert!(matches!(application.handler, HandlerValueRef::Expr { .. }));
    assert!(
        application
            .handled
            .effects
            .iter()
            .any(is_approval_request_effect),
        "{application:?}"
    );
    assert!(
        application.remaining.effects.is_empty(),
        "handled Approval effect should not escape: {application:?}"
    );
}

#[test]
fn check_program_rejects_handler_produced_effect_outside_explicit_bound() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
    action request() -> unit;
}

effect Payment extends Network {
    action ping() -> unit;
}

flow main() -> unit {
    let reusable: ![Approval => []] = handler {
        Approval.request() => {
            perform Payment.ping();
            resume;
        }
    };

    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Effect(
                    EffectDiagnosticCode::HandlerProducedEffectOutsideDeclaredRow,
                )
                && diagnostic.message.contains("produced effects")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_unresolved_handler_action() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
}

flow main() -> unit {
    handle {
        return;
    } with {
        Approval.request() => {
            resume;
        }
    };

    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    let (main_id, _) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some((id, flow))
            }
            _ => None,
        })
        .expect("main flow should lower");
    assert!(matches!(
        output.facts.interpreter_support.items.get(&main_id),
        Some(InterpreterSupport::Rejected(
            FrontendRejectionReason::UnsupportedHandler
        ))
    ));
}

#[test]
fn check_program_rejects_resume_outside_handler_arm() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> unit {
    resume;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::ResumeOutsideHandler)
        }),
        "{:?}",
        output.diagnostics
    );
    let (main_id, _) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some((id, flow))
            }
            _ => None,
        })
        .expect("main flow should lower");
    assert!(matches!(
        output.facts.interpreter_support.items.get(&main_id),
        Some(InterpreterSupport::Rejected(
            FrontendRejectionReason::UnsupportedHandler
        ))
    ));
}

#[test]
fn check_program_rejects_multiple_resumes_in_one_handler_arm() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
    action request() -> unit;
}

flow main() -> unit {
    handle {
        perform Approval.request();
    } with {
        Approval.request() => {
            resume;
            resume;
        }
    };

    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::ResumeUsedMoreThanOnce)
        }),
        "{:?}",
        output.diagnostics
    );
    let (main_id, _) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some((id, flow))
            }
            _ => None,
        })
        .expect("main flow should lower");
    assert!(
        !matches!(
            output.facts.interpreter_support.items.get(&main_id),
            Some(InterpreterSupport::Rejected(
                FrontendRejectionReason::UnsupportedHandler
            ))
        ),
        "stable handler diagnostics must not be reclassified as UnsupportedHandler"
    );
}

#[test]
fn check_program_rejects_resume_captured_by_lambda_in_handler_arm() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
    action request() -> unit;
}

flow main() -> unit {
    handle {
        perform Approval.request();
    } with {
        Approval.request() => {
            let later = () => {
                resume;
            };
            resume;
        }
    };

    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::InvalidResume)
                && diagnostic
                    .message
                    .contains("cannot be captured by an anonymous flow")
        }),
        "{:?}",
        output.diagnostics
    );
    let (main_id, _) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some((id, flow))
            }
            _ => None,
        })
        .expect("main flow should lower");
    assert!(
        !matches!(
            output.facts.interpreter_support.items.get(&main_id),
            Some(InterpreterSupport::Rejected(
                FrontendRejectionReason::UnsupportedHandler
            ))
        ),
        "stable handler diagnostics must not be reclassified as UnsupportedHandler"
    );
}

#[test]
fn check_program_rejects_resume_for_never_returning_action() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Error {
    action stop() -> never;
}

flow main() -> unit {
    handle {
        perform Error.stop();
    } with {
        Error.stop() => {
            resume;
        }
    };

    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::CannotResumeNeverAction)
        }),
        "{:?}",
        output.diagnostics
    );
    let (main_id, _) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "main") =>
            {
                Some((id, flow))
            }
            _ => None,
        })
        .expect("main flow should lower");
    assert!(
        !matches!(
            output.facts.interpreter_support.items.get(&main_id),
            Some(InterpreterSupport::Rejected(
                FrontendRejectionReason::UnsupportedHandler
            ))
        ),
        "stable handler diagnostics must not be reclassified as UnsupportedHandler"
    );
}

#[test]
fn check_program_reports_missing_while_and_retry_limits() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(ready: bool) -> unit {
    while ready {
        break;
    }

    retry {
        break;
    }

    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    let missing_limits = output
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::MissingEffectLoopLimit)
        })
        .count();
    assert_eq!(missing_limits, 2, "{:?}", output.diagnostics);
}

#[test]
fn check_program_accepts_explicit_while_and_retry_limits() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(ready: bool) -> unit {
    while ready limit Iterations(1) {
        break;
    }

    retry limit Attempts(1) {
        break;
    }

    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(
        !output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::MissingEffectLoopLimit)
        }),
        "{:?}",
        output.diagnostics
    );
    assert!(
        output
            .facts
            .requirements
            .items
            .values()
            .any(
                |requirements| requirements.contains(&RequirementFact::Limit(LimitRequirement {
                    kind: LimitKind::Iterations,
                    budget: LimitBudgetKind::Count,
                    value: Some(LimitValue::Count(1)),
                }))
            ),
        "while limit should record an Iterations count requirement"
    );
    assert!(
        output
            .facts
            .requirements
            .items
            .values()
            .any(
                |requirements| requirements.contains(&RequirementFact::Limit(LimitRequirement {
                    kind: LimitKind::Attempts,
                    budget: LimitBudgetKind::Count,
                    value: Some(LimitValue::Count(1)),
                }))
            ),
        "retry limit should record an Attempts count requirement"
    );
}

#[test]
fn check_program_records_pipeline_stage_limits() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow Writer(input: string) -> string {
    input
}

flow main(topic: string) -> string {
    topic ~> Writer limit Tokens(4096)
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(
        output
            .facts
            .requirements
            .items
            .values()
            .any(
                |requirements| requirements.contains(&RequirementFact::Limit(LimitRequirement {
                    kind: LimitKind::Tokens,
                    budget: LimitBudgetKind::Count,
                    value: Some(LimitValue::Count(4096)),
                }))
            ),
        "pipeline stage limit should record a Tokens count requirement"
    );
}

#[test]
fn check_program_rejects_user_defined_pipeline_stage_limit_constructor() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow Tokens(value: i64) -> i64 {
    value
}

flow Writer(input: string) -> string {
    input
}

flow main(topic: string) -> string {
    topic ~> Writer limit Tokens(4096)
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
        }),
        "user-defined Tokens(...) must not be accepted as a std runtime limit: {:?}",
        output.diagnostics
    );
    assert!(
        output
            .facts
            .requirements
            .items
            .values()
            .all(
                |requirements| !requirements.contains(&RequirementFact::Limit(LimitRequirement {
                    kind: LimitKind::Tokens,
                    budget: LimitBudgetKind::Count,
                    value: Some(LimitValue::Count(4096)),
                }))
            ),
        "invalid local constructor must not materialize a Tokens runtime requirement"
    );
}

#[test]
fn check_program_propagates_later_flow_effect_to_caller() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Payment extends Network {
    action ping() -> unit;
}

flow main() -> unit {
    later();
    return;
}

flow later() -> unit ![Network]

{
    perform Payment.ping();
    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let (main_id, _) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|s| s.name == "main") =>
            {
                Some((id, flow))
            }
            _ => None,
        })
        .expect("main flow should lower");
    assert!(matches!(
        output.facts.interpreter_support.items.get(&main_id),
        Some(InterpreterSupport::RequiresHost(requirements))
            if requirements.kinds.contains(&HostRequirementKind::Network)
    ));
}

#[test]
fn check_program_converges_mutually_recursive_flow_effects() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow first() -> unit {
    second();
    return;
}

effect Payment extends Network {
    action ping() -> unit;
}

flow second() -> unit ![Network]

{
    perform Payment.ping();
    first();
    return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    let output = check_program(&hir, &types);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    for name in ["first", "second"] {
        let (flow_id, _) = hir
            .items
            .iter()
            .find_map(|(id, item)| match item {
                HirItem::Flow(flow)
                    if hir
                        .symbols
                        .get(flow.symbol)
                        .is_some_and(|symbol| symbol.name == name) =>
                {
                    Some((id, flow))
                }
                _ => None,
            })
            .expect("flow should lower");
        assert!(
            matches!(
                output.facts.interpreter_support.items.get(&flow_id),
                Some(InterpreterSupport::RequiresHost(requirements))
                    if requirements.kinds.contains(&HostRequirementKind::Network)
            ),
            "{name} should require runtime network support"
        );
    }
}
