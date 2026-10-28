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
    &output.effects.as_ref().unwrap().facts.item_effects[&item]
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

fn check(body: &str) -> etas_frontend::ProjectOutput {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(format!(
        r#"
module app.main;
import std.compiler.check;
import std.compiler.compile;
import std.compiler.CompileError;
import std.compiler.CheckReport;
import std.io.println;
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
