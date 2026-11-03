use etas_effects::{Effect, HostRequirementKind, InterpreterSupport};
use etas_frontend::{EntryPolicy, Frontend, ProjectInput, SourceInput};

fn summary<'a>(
    output: &'a etas_frontend::ProjectOutput,
    name: &str,
) -> &'a etas_effects::EffectSummary {
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.as_ref().unwrap();
    let item = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if checked
                    .hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == name) =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    let summary = &output.effects.as_ref().unwrap().facts.item_effects[&item];
    assert!(
        !matches!(summary.support, InterpreterSupport::Rejected(_)),
        "{name}: {summary:#?}"
    );
    summary
}

fn requires_compiler(summary: &etas_effects::EffectSummary) -> bool {
    match &summary.support {
        InterpreterSupport::RequiresHost(host) => {
            host.kinds.contains(&HostRequirementKind::Compiler)
        }
        InterpreterSupport::RequiresInterpreterOrchestration(requirements) => requirements
            .host
            .kinds
            .contains(&HostRequirementKind::Compiler),
        _ => false,
    }
}

fn requested_names(
    output: &etas_frontend::ProjectOutput,
    name: &str,
) -> std::collections::BTreeSet<String> {
    let summary = summary(output, name);
    let checked = output.checked.as_ref().unwrap();
    summary
        .requested_actions
        .effects
        .iter()
        .map(|effect| {
            let action = match effect {
                Effect::Action(action) => action,
                Effect::AppliedAction(instance) => &instance.action,
                other => panic!("unexpected request {other:?}"),
            };
            let tag = checked.effect_registry.tag_name(action.tag).unwrap();
            format!(
                "{tag}.{}",
                checked
                    .effect_registry
                    .action_name(action.tag, action.action)
                    .unwrap()
            )
        })
        .collect()
}

#[test]
fn compiled_flow_calls_apply_frozen_contracts_only_when_invoked() {
    for expression in [
        "compile((^target).with_body(body))",
        "perform Code.compile((^target).with_body(body))",
    ] {
        let output = check(&format!(
            r#"
alias Task = () -> unit ![Console.stdout_write];
flow target() -> unit ![Console.stdout_write] {{ perform Console.stdout_write("base"); return; }}
flow produce(body: SyntaxBlock) -> Task ![Error<CompileError>] {{ return {expression}; }}
flow invoke(body: SyntaxBlock) -> unit ![Console.stdout_write, Error<CompileError>] {{ let candidate = produce(body); candidate(); return; }}
"#
        ));
        assert_eq!(
            requested_names(&output, "produce"),
            ["Code.compile".to_owned()].into()
        );
        assert_eq!(
            requested_names(&output, "invoke"),
            ["Code.compile".to_owned(), "Console.stdout_write".to_owned()].into()
        );
        let returned = summary(&output, "produce")
            .exits
            .as_ref()
            .unwrap()
            .callable_return_sources()
            .unwrap();
        assert!(returned.iter().any(|source| matches!(
            source,
            etas_effects::CallableReturnSource::FrozenContract { .. }
        )));
        assert!(
            output
                .checked
                .as_ref()
                .unwrap()
                .types
                .definition_operations
                .values()
                .any(|fact| fact.operation
                    == etas_types::facts::reflection::DefinitionOperation::Compile)
        );
    }
}

#[test]
fn compiled_contract_origins_survive_descriptor_helpers_branches_and_result_ok_extraction() {
    let output = check(
        r#"
alias Task = () -> unit ![];
flow first() -> unit ![] { println("one")?; return; }
flow second() -> unit ![] { eprintln("two")?; return; }
flow revise(draft: CallableDef<Task>) -> Task ![Error<CompileError>] { return compile(draft); }
flow inspect(body: SyntaxBlock, choose: bool) -> unit ![] {
    let descriptor = if choose { ^first } else { ^second };
    let result = revise(descriptor.with_body(body))?;
    match result { Ok(candidate) => { candidate(); }, Err(_) => {}, }
    return;
}
"#,
    );
    assert_eq!(
        requested_names(&output, "inspect"),
        [
            "Code.compile".to_owned(),
            "Console.stdout_write".to_owned(),
            "Console.stderr_write".to_owned()
        ]
        .into()
    );
    assert_eq!(
        summary(&output, "inspect")
            .escaping_effects
            .effects
            .iter()
            .count(),
        0
    );
}

#[test]
fn descriptor_assignment_updates_only_subsequent_compile_contracts() {
    let output = check(
        r#"
flow first() -> unit ![] { println("first")?; return; }
flow second() -> unit ![] { eprintln("second")?; return; }
flow keep_old() -> unit ![Error<CompileError>] {
    var definition = ^first;
    let old = compile(definition);
    definition = ^second;
    old(); return;
}
flow use_new() -> unit ![Error<CompileError>] {
    var definition = ^first;
    definition = ^second;
    let candidate = compile(definition);
    candidate(); return;
}
flow replace<F>(first: CallableDef<F>, second: CallableDef<F>) -> F ![Error<CompileError>] {
    var candidate = compile(first);
    candidate = compile(second);
    return candidate;
}
flow generic_replace() -> unit ![Error<CompileError>] {
    let candidate = replace(^first, ^second); candidate(); return;
}
"#,
    );
    assert_eq!(
        requested_names(&output, "keep_old"),
        ["Code.compile".to_owned(), "Console.stdout_write".to_owned()].into()
    );
    for name in ["use_new", "generic_replace"] {
        assert_eq!(
            requested_names(&output, name),
            ["Code.compile".to_owned(), "Console.stderr_write".to_owned()].into()
        );
    }
}

#[test]
fn generic_compiler_helper_preserves_symbolic_contracts_through_local_bindings() {
    let output = check(
        r#"
flow target() -> unit ![] { println("latent")?; return; }
flow revise<F>(draft: CallableDef<F>) -> F ![Error<CompileError>] {
    let result = compile(draft);
    return result;
}
flow inspect() -> unit ![Error<CompileError>] { let candidate = revise(^target); candidate(); return; }
"#,
    );
    assert_eq!(
        requested_names(&output, "revise"),
        ["Code.compile".to_owned()].into()
    );
    assert_eq!(
        requested_names(&output, "inspect"),
        ["Code.compile".to_owned(), "Console.stdout_write".to_owned()].into()
    );
}

#[test]
fn flow_compiler_overloads_preserve_requests_without_executing_the_reflected_flow() {
    let output = check(
        r#"
alias Normalize = (string) -> string ![Console.stdout_write];
flow normalize(text: string) -> string ![Console.stdout_write] { perform Console.stdout_write(text); return text; }
flow inspect(body: SyntaxBlock) -> Normalize ![Error<CompileError>] {
    let draft = (^normalize).with_body(body);
    let report = check(draft);
    return compile(draft);
}
"#,
    );
    let behavior = summary(&output, "inspect");
    assert!(requires_compiler(behavior), "{behavior:#?}");
    let checked = output.checked.as_ref().unwrap();
    let requested = behavior
        .requested_actions
        .effects
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let expected = [146, 147].map(|id| {
        let action = checked
            .effect_registry
            .standard_action(etas_effects::EffectActionId(id))
            .unwrap();
        Effect::Action(etas_effects::ActionRef {
            tag: action.owner,
            action: action.id,
        })
    });
    assert_eq!(requested.len(), expected.len());
    for action in expected {
        assert!(requested.contains(&action), "{requested:?}");
    }
    assert_eq!(behavior.escaping_effects.effects.iter().count(), 1);
    assert!(matches!(
        behavior.escaping_effects.effects.iter().next(),
        Some(Effect::Error(_))
    ));
    assert!(behavior.requested_actions.open.is_none());
}

#[test]
fn direct_perform_uses_the_same_default_error_contract_as_the_wrapper() {
    let output = check(
        "flow inspect() -> Result<CheckReport, CompileError> { return (perform Code.check(^Worker))?; }",
    );
    let summary = summary(&output, "inspect");
    assert!(summary.escaping_effects.is_empty());
    assert!(requires_compiler(summary));
    let defaults = summary.default_handler_effects.as_ref().unwrap();
    assert_eq!(defaults.handlers.len(), 1);
    assert!(defaults.handlers[0].produced.is_empty());
    assert_eq!(summary.default_actions.effects.iter().count(), 1);
}

#[test]
fn direct_flow_compiler_actions_keep_checked_instantiations_and_default_errors() {
    for (operation, result) in [("check", "CheckReport"), ("compile", "Normalize")] {
        let output = check(&format!(
            r#"
alias Normalize = (string) -> string ![];
flow normalize(text: string) -> string ![] {{ return text; }}
flow inspect() -> Result<{result}, CompileError> {{ return (perform Code.{operation}(^normalize))?; }}
"#
        ));
        let behavior = summary(&output, "inspect");
        assert!(behavior.escaping_effects.is_empty(), "{behavior:#?}");
        assert_eq!(behavior.requested_actions.effects.iter().count(), 1);
        assert_eq!(behavior.default_actions.effects.iter().count(), 1);
        assert!(requires_compiler(behavior));
        let types = &output.checked.as_ref().unwrap().types;
        assert!(types.action_calls.values().any(|call| call.overload == 1));
    }
}

#[test]
fn custom_handler_replaces_default_errors_for_both_wrapper_and_direct_perform() {
    for expression in ["check(^Worker)", "perform Code.check(^Worker)"] {
        let output = check(&format!(
            "flow inspect(report: CheckReport) -> CheckReport {{ return handle {{ return {expression}; }} with {{ Code.check(_) => {{ resume report; }} }}; }}"
        ));
        let summary = summary(&output, "inspect");
        assert!(summary.escaping_effects.is_empty(), "{summary:#?}");
        assert!(summary.default_actions.is_empty());
        assert!(summary.default_handler_effects.is_none());
        assert!(!requires_compiler(summary), "{summary:#?}");
        assert_eq!(summary.requested_actions.effects.iter().count(), 1);
    }
}

#[test]
fn custom_code_handler_does_not_erase_explicit_source_compile_error() {
    let output = check(
        r#"
flow inspect(report: CheckReport, error: CompileError, fail: bool) -> CheckReport ![Error<CompileError>] {
    return handle {
        if fail { perform Error<CompileError>.raise(error); }
        return check(^Worker);
    } with { Code.check(_) => { resume report; } };
}
"#,
    );
    let summary = summary(&output, "inspect");
    assert_eq!(summary.escaping_effects.effects.iter().count(), 1);
    assert!(matches!(
        summary.escaping_effects.effects.iter().next(),
        Some(Effect::Error(_))
    ));
    assert!(!requires_compiler(summary));
}

#[test]
fn overriding_one_code_action_does_not_remove_another_default_provider_requirement() {
    let output = check(
        r#"
flow inspect(report: CheckReport) -> CheckReport ![Error<CompileError>] {
    return handle {
        let candidate = compile(^Worker);
        return check(^Worker);
    } with { Code.check(_) => { resume report; } };
}
"#,
    );
    let summary = summary(&output, "inspect");
    assert!(requires_compiler(summary));
    assert_eq!(summary.default_actions.effects.iter().count(), 1);
    assert_eq!(
        summary
            .default_handler_effects
            .as_ref()
            .unwrap()
            .handlers
            .len(),
        1
    );
    assert!(matches!(
        summary.escaping_effects.effects.iter().next(),
        Some(Effect::Error(_))
    ));
}

#[test]
fn custom_flow_compile_handler_preserves_the_frozen_callable_contract_after_resumption() {
    for request in ["compile(^target)", "perform Code.compile(^target)"] {
        let output = check(&format!(
            r#"
flow target() -> unit ![] {{ println("latent request")?; return; }}
flow inspect() -> unit {{
    let candidate = handle {{ {request} }} with {{ Code.compile(_) => {{ resume target; }} }};
    candidate();
    return;
}}
"#
        ));
        assert_eq!(
            requested_names(&output, "inspect"),
            ["Code.compile".into(), "Console.stdout_write".into()].into()
        );
        let summary = summary(&output, "inspect");
        assert!(summary.escaping_effects.is_empty());
        assert!(!requires_compiler(summary));
        assert_eq!(summary.default_actions.effects.iter().count(), 1);
    }
}

#[test]
fn indirect_compile_handlers_select_the_request_payload_overload() {
    for request in ["compile(draft)", "perform Code.compile(draft)"] {
        let output = check(&format!(
            r#"
alias Task = () -> unit ![];
flow target() -> unit ![] {{ println("latent request")?; return; }}
flow request(draft: CallableDef<Task>) -> Task ![Error<CompileError>] {{ return {request}; }}
flow inspect() -> unit {{
    let candidate = handle {{ request(^target) }} with {{ Code.compile(_) => {{ resume target; }} }};
    candidate();
    return;
}}
"#
        ));
        assert_eq!(
            requested_names(&output, "inspect"),
            ["Code.compile".into(), "Console.stdout_write".into()].into()
        );
        assert!(summary(&output, "inspect").escaping_effects.is_empty());
        assert!(!requires_compiler(summary(&output, "inspect")));
    }
}

#[test]
fn indirect_compile_handlers_reject_inconsistent_payload_and_resume_abis() {
    for body in [
        "let ignored = request_agent(); request_flow()",
        "request_flow()",
    ] {
        let output = check(&format!(
            r#"
alias Task = () -> unit ![];
agent Worker() -> unit {{ return; }}
flow target() -> unit ![] {{ return; }}
flow request_agent() -> Worker ![Error<CompileError>] {{ return compile(^Worker); }}
flow request_flow() -> Task ![Error<CompileError>] {{ return compile(^target); }}
flow inspect() -> unit {{
    let selected = handle {{ {body} }} with {{ Code.compile(_) => {{ resume Worker; }} }};
    return;
}}
"#
        ));
        assert!(
            output.diagnostics.iter().any(|diagnostic| diagnostic.code
                == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::TypeMismatch)),
            "{body}: {:?}",
            output.diagnostics
        );
        assert!(output.checked.is_none());
    }
}

#[test]
fn deferred_handler_overloads_keep_body_local_type_variables_independent() {
    let output = check(
        r#"
alias NumberTask = () -> i32 ![];
alias TextTask = () -> string ![];
flow number() -> i32 ![] { return 9; }
flow text() -> string ![] { return "value"; }
flow request_number(draft: CallableDef<NumberTask>) -> NumberTask ![Error<CompileError>] { return compile(draft); }
flow request_text(draft: CallableDef<TextTask>) -> TextTask ![Error<CompileError>] { return compile(draft); }
flow select_number() -> i32 {
    let selected = handle { request_number(^number) } with { Code.compile(_) => { resume number; } };
    return selected();
}
flow select_text() -> string {
    let selected = handle { request_text(^text) } with { Code.compile(_) => { resume text; } };
    return selected();
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.checked.is_some());
}

#[test]
fn a_compile_handler_finish_preserves_its_own_callable_origin() {
    for completion in [
        "finish alternative;",
        "if replace { finish alternative; } resume target;",
    ] {
        let output = check(&format!(
            r#"
flow target() -> unit ![] {{ return; }}
flow alternative() -> unit ![] {{ println("finished callable")?; return; }}
flow inspect(replace: bool) -> unit {{
    let selected = handle {{ compile(^target) }} with {{ Code.compile(_) => {{ {completion} }} }};
    selected();
    return;
}}
"#
        ));
        assert_eq!(
            requested_names(&output, "inspect"),
            ["Code.compile".into(), "Console.stdout_write".into()].into()
        );
        assert!(!requires_compiler(summary(&output, "inspect")));
    }
}

#[test]
fn generic_handler_finish_preserves_a_captured_replacement_contract() {
    let output = check(
        r#"
flow quiet() -> unit ![] { return; }
flow loud() -> unit ![] { println("replacement")?; return; }
flow revise<F>(draft: CallableDef<F>, replacement: CallableDef<F>, replace: bool) -> F ![Error<CompileError>] {
    let other = compile(replacement);
    return handle { compile(draft) } with {
        Code.compile(_) => { if replace { finish other; } resume compile(draft); }
    };
}
flow inspect(replace: bool) -> unit ![Error<CompileError>] {
    let selected = revise(^quiet, ^loud, replace);
    selected();
    return;
}
"#,
    );
    assert_eq!(
        requested_names(&output, "revise"),
        ["Code.compile".into()].into()
    );
    assert_eq!(
        requested_names(&output, "inspect"),
        ["Code.compile".into(), "Console.stdout_write".into()].into()
    );
}

#[test]
fn generic_finish_does_not_assign_the_drafts_contract_to_a_different_callable_parameter() {
    let output = check(
        r#"
flow quiet() -> unit ![] { return; }
flow loud() -> unit ![] { println("replacement")?; return; }
flow choose<F>(draft: CallableDef<F>, replacement: F) -> F {
    return handle { compile(draft) } with { Code.compile(_) => { finish replacement; } };
}
flow inspect() -> unit {
    let selected = choose(^quiet, loud);
    selected();
    return;
}
flow inspect_quiet() -> unit {
    let selected = choose(^loud, quiet);
    selected();
    return;
}
"#,
    );
    assert_eq!(
        requested_names(&output, "choose"),
        ["Code.compile".into()].into()
    );
    assert_eq!(
        requested_names(&output, "inspect"),
        ["Code.compile".into(), "Console.stdout_write".into()].into()
    );
    assert_eq!(
        requested_names(&output, "inspect_quiet"),
        ["Code.compile".into()].into()
    );
}

#[test]
fn prior_requests_do_not_make_a_conditional_handled_request_unconditional() {
    let output = check(
        r#"
flow quiet() -> unit ![] { return; }
flow loud() -> unit ![] { println("normal body result")?; return; }
flow choose<F>(draft: CallableDef<F>, replacement: F, request: bool) -> F ![Error<CompileError>] {
    let prior = compile(draft);
    return handle {
        if request { let ignored = compile(draft); }
        replacement
    } with { Code.compile(_) => { finish compile(draft); } };
}
flow inspect(request: bool) -> unit ![Error<CompileError>] {
    let selected = choose(^quiet, loud, request);
    selected();
    return;
}
"#,
    );
    assert_eq!(
        requested_names(&output, "inspect"),
        ["Code.compile".into(), "Console.stdout_write".into()].into()
    );
    assert!(requires_compiler(summary(&output, "inspect")));
}

fn check(body: &str) -> etas_frontend::ProjectOutput {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(format!(
        r#"
module app.main;
import std.compiler.check;
import std.compiler.compile;
import std.compiler.CompileError;
import std.compiler.CheckReport;
import std.compiler.SyntaxBlock;
import std.compiler.CallableDef;
import std.io.println;
import std.io.eprintln;
agent Worker() -> unit ![Console.stdout_write] {{ perform Console.stdout_write("candidate must not be invoked by compilation"); return; }}
{body}
"#
    )));
    input.options.entry_policy = EntryPolicy::Optional;
    Frontend.check_project(input)
}

#[test]
fn compiler_wrappers_preserve_code_requests_without_requesting_candidate_actions() {
    for (operation, result, expression) in [
        ("check", "CheckReport", "check(^Worker)"),
        ("compile", "Worker", "compile(^Worker)"),
    ] {
        for captured in [false, true] {
            let body = if captured {
                format!(
                    "flow inspect() -> Result<{result}, CompileError> {{ return {expression}?; }}"
                )
            } else {
                format!(
                    "flow inspect() -> {result} ![Error<CompileError>] {{ return {expression}; }}"
                )
            };
            let output = check(&body);
            assert!(
                output.diagnostics.is_empty(),
                "{operation} captured={captured}: {:?}",
                output.diagnostics
            );
            let checked = output.checked.as_ref().unwrap();
            let id = checked
                .hir
                .items
                .iter()
                .find_map(|(id, item)| match item {
                    etas_hir::HirItem::Flow(flow)
                        if checked
                            .hir
                            .symbols
                            .get(flow.symbol)
                            .is_some_and(|symbol| symbol.name == "inspect") =>
                    {
                        Some(id)
                    }
                    _ => None,
                })
                .unwrap();
            let effects = output.effects.as_ref().unwrap();
            let summary = &effects.facts.item_effects[&id];
            let action = checked
                .effect_registry
                .action_by_name(&format!("Code.{operation}"))
                .unwrap();
            let expected = Effect::Action(action);
            assert_eq!(
                summary.requested_actions.effects.iter().collect::<Vec<_>>(),
                [&expected]
            );
            assert_eq!(
                summary.default_actions.effects.iter().collect::<Vec<_>>(),
                [&expected]
            );
            assert!(summary.requested_actions.open.is_none());
            if captured {
                assert!(summary.escaping_effects.is_empty(), "{summary:#?}");
            } else {
                let escaping = summary
                    .escaping_effects
                    .effects
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>();
                let [Effect::Error(error)] = escaping.as_slice() else {
                    panic!("wrong escaping row: {summary:#?}")
                };
                assert!(
                    matches!(checked.type_store.get(*error), Some(etas_types::Type::Nominal(nominal)) if nominal.name == "std.compiler.CompileError")
                );
            }
            match &summary.support {
                InterpreterSupport::RequiresHost(host) => assert_eq!(
                    host.kinds.iter().copied().collect::<Vec<_>>(),
                    [HostRequirementKind::Compiler]
                ),
                other => panic!(
                    "compiler must not acquire candidate Console/network/model requirements: {other:?}"
                ),
            }
        }
    }
}

#[test]
fn rejected_reports_remain_readonly_descriptive_data() {
    let output = check(
        r#"
flow inspect(report: CheckReport) -> bool {
    let status: string = report.status;
    let snapshot: string = report.snapshot_fingerprint;
    let parent: Option<string> = report.revision.parent;
    let environment: string = report.dependencies.environment;
    return status == "rejected" && snapshot != environment;
}

"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let output = check(
        r#"flow mutate(report: CheckReport) -> unit { var edited = report; edited.status = "accepted"; return; }"#,
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::TypeMismatch)
            && diagnostic.message.contains("read-only")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn copying_report_data_does_not_require_or_create_checked_execution_evidence() {
    let output = check(
        r#"
flow copy_report(report: CheckReport) -> CheckReport {
    return CheckReport {
        status = "accepted",
        diagnostics = report.diagnostics,
        revision = report.revision,
        dependencies = report.dependencies,
        snapshot_fingerprint = report.snapshot_fingerprint,
        effects = report.effects,
        determinism = report.determinism,
        trace = report.trace,
        runtime_checks = report.runtime_checks,
    };
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}
