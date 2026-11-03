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
        "import std.compiler.check; flow target() -> unit { return; } flow bad() -> unit { check(target); return; }",
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
fn compiler_check_accepts_a_typed_flow_descriptor_but_not_a_raw_callable() {
    let (_, output) = check(
        "import std.compiler.check; flow target() -> unit ![] { return; } flow verify() -> unit { check(^target); return; }",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(
        output
            .facts
            .std_intrinsic_calls
            .values()
            .any(|fact| fact.overload == 1)
    );
}

#[test]
fn direct_compiler_actions_select_typed_flow_overloads() {
    for operation in ["check", "compile"] {
        let (_, output) = check(&format!(
            "flow target(text: string) -> string ![] {{ return text; }} flow invoke() -> unit {{ let result = perform Code.{operation}(^target); return; }}"
        ));
        assert!(
            output.diagnostics.is_empty(),
            "{operation}: {:?}",
            output.diagnostics
        );
        let fact = output.facts.action_calls.values().next().unwrap();
        assert_eq!(fact.overload, 1);
        let callable = output
            .facts
            .known_std_types
            .code
            .callable_definition
            .unwrap();
        assert!(
            matches!(output.store.get(fact.signature.params[0]), Some(Type::Applied { constructor, .. }) if constructor.0 == callable.0)
        );
        if operation == "compile" {
            assert!(matches!(
                output.store.get(fact.signature.output),
                Some(Type::Function(_))
            ));
        }
    }
}

#[test]
fn direct_compiler_actions_reject_erasure_and_wrong_expected_callable() {
    for body in [
        "import std.compiler.CallableDef; flow invalid(info: CallableDef) -> (string) -> string ![] { return perform Code.compile(info); }",
        "flow target(text: string) -> string ![] { return text; } flow invalid() -> (i32) -> i32 ![] { return perform Code.compile(^target); }",
        "flow target(text: string) -> string ![] { return text; } flow invalid() -> unit { let report = perform Code.check<(string) -> string ![]>(^target); return; }",
    ] {
        let (_, output) = check(body);
        assert!(!output.diagnostics.is_empty(), "{body}");
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
fn compiler_handler_uses_the_selected_flow_request_input_signature() {
    for request in ["build(^target)", "perform Code.compile(^target)"] {
        let (_, output) = check(&format!(
            r#"
import std.compiler.compile as build;
flow target(value: i32) -> i32 ![] {{ return value; }}
flow main() -> i32 {{
    let candidate = handle {{ {request} }} with {{
        Code.compile(_) => {{ resume target; }}
    }};
    return candidate(7);
}}
"#
        ));
        assert!(
            output.diagnostics.is_empty(),
            "{request}: {:?}",
            output.diagnostics
        );
        let handler = output.facts.handler_actions.values().next().unwrap();
        assert!(matches!(
            output.store.get(handler.output),
            Some(Type::Function(_))
        ));
        let callable = output
            .facts
            .known_std_types
            .code
            .callable_definition
            .unwrap();
        assert!(
            matches!(output.store.get(handler.params[0]), Some(Type::Applied { constructor, .. }) if constructor.0 == callable.0)
        );
    }
}

#[test]
fn compiler_check_handler_payload_uses_inputs_even_when_both_overloads_return_the_same_report() {
    for request in ["inspect(^target)", "perform Code.check(^target)"] {
        let (_, output) = check(&format!(
            r#"
import std.compiler.{{check as inspect, CheckReport, CallableDef}};
flow target(value: i32) -> i32 ![] {{ return value; }}
flow main(report: CheckReport) -> CheckReport {{
    return handle {{ {request} }} with {{
        Code.check(definition) => {{
            let typed: CallableDef<(i32) -> i32 ![]> = definition;
            resume report;
        }}
    }};
}}
"#
        ));
        assert!(
            output.diagnostics.is_empty(),
            "{request}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn compiler_handler_rejects_mixed_request_families_and_wrong_resume_signatures() {
    for (requests, resume) in [
        ("let agent = compile(^Worker); compile(^target)", "target"),
        ("compile(^target)", "other"),
    ] {
        let (_, output) = check(&format!(
            r#"
import std.compiler.compile;
agent Worker() -> i32 {{ return 7; }}
flow target(value: i32) -> i32 ![] {{ return value; }}
flow other(value: string) -> string ![] {{ return value; }}
flow main() -> unit {{
    let result = handle {{ {requests} }} with {{ Code.compile(_) => {{ resume {resume}; }} }};
    return;
}}
"#
        ));
        assert!(
            !output.diagnostics.is_empty(),
            "{requests}: resume {resume}"
        );
    }
}

#[test]
fn handler_finish_cannot_replace_the_checked_nominal_or_callable_result_type() {
    for source in [
        "import std.compiler.compile; agent A() -> i32 { return 1; } agent B() -> i32 { return 2; } flow bad() -> A { return handle { compile(^A) } with { Code.compile(_) => { finish B; } }; }",
        "import std.compiler.compile; flow original(x: i32) -> i32 ![] { return x; } flow other(x: string) -> string ![] { return x; } flow bad() -> unit { let selected = handle { compile(^original) } with { Code.compile(_) => { finish other; } }; return; }",
    ] {
        let (_, output) = check(source);
        assert!(!output.diagnostics.is_empty(), "{source}");
    }
}

#[test]
fn handler_finish_infers_a_value_result_when_the_body_cannot_return() {
    let (_, output) = check(
        "flow main() -> i32 { let result = handle { return perform Error<string>.raise(\"fail\"); } with { Error<string>.raise(_) => { finish 7; } }; return result; }",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.facts.symbol_types.values().any(|fact| matches!(fact, etas_types::SymbolTypeFact::Local { ty, .. } if matches!(output.store.get(*ty), Some(Type::Primitive(etas_types::PrimitiveType::I32))))));
}

#[test]
fn unit_literal_and_zero_tuple_type_have_one_checked_identity() {
    let mut interner = etas_types::TypeInterner::new();
    let unit = interner.primitive(etas_types::PrimitiveType::Unit);
    assert_eq!(interner.intern(Type::Tuple(Vec::new())), unit);
    let (_, output) = check(
        "flow identity(value: unit) -> unit { return value; } flow main() -> unit { return identity(()); }",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
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
