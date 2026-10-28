use etas_core::{SourceFile, SourceId};
use etas_types::{ItemSignature, Type};

fn check(source: &str) -> (etas_hir::HirProgram, etas_types::TypeOutput) {
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(91), None, source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    let output = etas_types::check_program(&hir);
    (hir, output)
}

#[test]
fn compiler_wrappers_infer_agent_identity_and_capture_only_the_declared_error() {
    let (hir, output) = check(
        r#"
import std.compiler.AgentDef;
import std.compiler.CheckReport;
import std.compiler.CompileError;
import std.compiler.check;
import std.compiler.compile;
agent Worker;
flow verify(definition: AgentDef<Worker>) -> Result<CheckReport, CompileError> {
    return check(definition)?;
}
flow build(definition: AgentDef<Worker>) -> Result<Worker, CompileError> {
    return compile(definition)?;
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let build = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "build") =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    let ItemSignature::Flow(signature) = &output.facts.item_signatures[&build] else {
        panic!("flow")
    };
    let Some(Type::Result { ok, .. }) = output.store.get(signature.output) else {
        panic!("result")
    };
    assert!(
        matches!(output.store.get(*ok), Some(Type::Nominal(nominal)) if nominal.name == "Worker")
    );
    assert!(
        output
            .facts
            .generic_instantiations
            .values()
            .any(|fact| !fact.type_bindings.is_empty())
    );
}

#[test]
fn compiler_service_rejects_wrong_agent_identity_callable_and_report_as_a_definition() {
    for source in [
        "import std.compiler.compile; agent A; agent B; flow bad() -> B { return compile(^A); }",
        "import std.compiler.check; flow target() -> unit { return; } flow bad() -> unit { check(^target); return; }",
        "import std.compiler.CheckReport; import std.compiler.compile; flow bad(report: CheckReport) -> unit { compile(report); return; }",
    ] {
        let (_, output) = check(source);
        assert!(
            !output.diagnostics.is_empty(),
            "unexpected acceptance: {source}"
        );
    }
}

#[test]
fn compiler_action_signature_keeps_payload_generic_and_default_handler_effect_types() {
    let (_, output) = check(
        r#"
agent Worker;
flow verify() -> unit ![Code.check] { let report = perform Code.check(^Worker); return; }
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let signature = output
        .facts
        .symbol_types
        .values()
        .find_map(|fact| match fact {
            etas_types::SymbolTypeFact::EffectAction { signature }
                if signature.default_handler_effects.is_some() =>
            {
                Some(signature)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(signature.generic_params.len(), 1);
    assert!(signature.effect_args.is_empty());
    let row = signature.default_handler_effects.as_ref().unwrap();
    let [effect] = row.effects.as_slice() else {
        panic!("default error")
    };
    assert_eq!(effect.name, "Error");
    let [etas_types::EffectArgRef::Type(error)] = effect.args.as_slice() else {
        panic!("typed error")
    };
    assert!(
        matches!(output.store.get(*error), Some(Type::Nominal(nominal)) if nominal.name == "std.compiler.CompileError")
    );
}

#[test]
fn compiler_handler_infers_the_request_identity_for_resume() {
    let (_, output) = check(
        r#"
import std.compiler.compile as build;
agent Worker() -> i32 { return 7; }
flow main() -> Worker {
    return handle { build(^Worker) } with {
        Code.compile(_) => { resume Worker; }
    };
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let handlers = output.facts.handler_actions.values().collect::<Vec<_>>();
    let [fact] = handlers.as_slice() else {
        panic!("one checked handler")
    };
    assert!(
        matches!(output.store.get(fact.output), Some(Type::Nominal(nominal)) if nominal.name == "Worker")
    );
}

#[test]
fn compiler_handler_request_inference_is_local_to_each_handle() {
    let (_, output) = check(
        r#"
import std.compiler.compile;
agent First() -> i32 { return 1; }
agent Second() -> i32 { return 2; }
flow main() -> unit {
    let first = handle { compile(^First) } with {
        Code.compile(_) => { resume First; }
    };
    let second = handle { perform Code.compile(^Second) } with {
        Code.compile(_) => { resume Second; }
    };
    return;
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn compiler_handler_infers_multiple_requests_of_the_same_identity() {
    let (_, output) = check(
        r#"
import std.compiler.compile;
agent Worker() -> i32 { return 7; }
flow main() -> Worker {
    return handle {
        let first = compile(^Worker);
        compile(^Worker)
    } with {
        Code.compile(_) => { resume Worker; }
    };
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn compiler_handler_does_not_collect_requests_from_an_uncalled_lambda() {
    let (_, output) = check(
        r#"
import std.compiler.compile;
agent Worker() -> i32 { return 1; }
agent Other() -> i32 { return 2; }
flow main() -> Worker {
    return handle {
        let deferred = () => compile(^Other);
        compile(^Worker)
    } with { Code.compile(_) => { resume Worker; } };
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn generic_action_handler_does_not_collect_requests_from_an_uncalled_lambda() {
    let (_, output) = check(
        r#"
effect Echo { action echo<T>(value: T) -> T; }
flow main() -> i32 {
    return handle {
        let deferred = () => perform Echo.echo("unused");
        perform Echo.echo(42)
    } with { Echo.echo(_) => { resume 42; } };
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn a_lambda_still_infers_its_own_nested_handler_requests() {
    let (_, output) = check(
        r#"
effect Echo { action echo<T>(value: T) -> T; }
flow main() -> i32 {
    let deferred = () => handle {
        perform Echo.echo(42)
    } with { Echo.echo(_) => { resume 42; } };
    return deferred();
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn compiler_handler_preserves_the_obligation_to_validate_indirect_requests() {
    let (_, output) = check(
        r#"
import std.compiler.compile;
agent Worker() -> i32 { return 1; }
agent Other() -> i32 { return 2; }
flow hidden() -> Other { return compile(^Other); }
flow main() -> Other {
    return handle {
        let first = compile(^Worker);
        hidden()
    } with {
        Code.compile(_) => { resume Worker; }
    };
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(
        output.facts.deferred_handler_instantiations.len(),
        1,
        "a local solver result must not erase the cross-call proof obligation"
    );
}
#[test]
fn compiler_handler_does_not_infer_identity_from_resume_or_unrelated_requests() {
    for body in [
        "compile(^Other)",
        "let first = compile(^Worker); compile(^Other)",
        "7",
    ] {
        let (_, output) = check(&format!(
            r#"
import std.compiler.compile;
agent Worker() -> i32 {{ return 1; }}
agent Other() -> i32 {{ return 2; }}
flow main() -> unit {{
    let result = handle {{ {body} }} with {{
        Code.compile(_) => {{ resume Worker; }}
    }};
    return;
}}
"#,
        ));
        assert!(
            output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("resume payload")),
            "request {body} must not permit resume Worker: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn a_handler_shared_by_distinct_requests_remains_parametric() {
    let (_, output) = check(
        r#"
effect Echo { action value<T>(payload: T) -> T; }
flow main() -> unit {
    let values = handle {
        let number = perform Echo.value(1);
        let text = perform Echo.value("x");
        (number, text)
    } with {
        Echo.value<_>(payload) => { resume payload; }
    };
    return;
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn an_inner_total_handler_owns_its_request_instances() {
    let (_, output) = check(
        r#"
import std.compiler.compile;
agent Worker() -> i32 { return 1; }
agent Other() -> i32 { return 2; }
flow main() -> Worker {
    return handle {
        let inner = handle { compile(^Other) } with {
            Code.compile(_) => { resume Other; }
        };
        compile(^Worker)
    } with {
        Code.compile(_) => { resume Worker; }
    };
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn pure_definition_editing_inside_a_request_preserves_its_inferred_identity() {
    let (_, output) = check(
        r#"
import std.compiler.{compile, parse_body};
agent Worker() -> i32 { return 1; }
flow main() -> Worker {
    let body = match parse_body("return 2;") {
        Ok(body) => body,
        Err(_) => { return abort("invalid body"); },
    };
    return handle { compile((^Worker).with_body(body)) } with {
        Code.compile(_) => { resume Worker; }
    };
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}
