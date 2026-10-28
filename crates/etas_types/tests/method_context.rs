use etas_types::{PrimitiveType, Type, check_program};

#[test]
fn method_context_keeps_inferred_constructor_payload_separate_from_explicit_cast_target() {
    for (input, payload, target) in [
        ("bytes", PrimitiveType::Bytes, "JsonValue"),
        ("bytes", PrimitiveType::Bytes, "bytes"),
        ("string", PrimitiveType::String, "i32"),
        ("i32", PrimitiveType::I32, "string"),
    ] {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
            etas_core::SourceId(9),
            None,
            format!(
                r#"
import std.agent.message.Message;
import std.json.JsonValue;
flow main(raw: {input}) -> Option<Message<{target}>> {{
    return Message.new(raw).cast<{target}>();
}}
"#
            ),
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = etas_hir::lower_program(&parsed.value);
        let output = check_program(&hir);
        assert!(
            output.diagnostics.is_empty(),
            "{input} -> {target}: {:?}",
            output.diagnostics
        );
        let constructor = hir
            .exprs
            .iter()
            .find_map(|(expr, value)| {
                matches!(value, etas_hir::HirExpr::MethodCall { method, .. } if method == "new")
                    .then_some(expr)
            })
            .unwrap();
        let Type::Message(actual) = output
            .store
            .get(output.facts.expr_types[&constructor])
            .unwrap()
        else {
            panic!("constructor must have a checked Message type");
        };
        assert_eq!(output.store.get(*actual), Some(&Type::Primitive(payload)));
    }
}

#[test]
fn method_context_preserves_caller_generic_identity_and_nested_call_payloads() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(9),
        None,
        r#"
import std.agent.message.Message;
import std.json.JsonValue;
import std.crypto.sha256;
flow generic<T>(raw: T) -> Option<Message<bytes>> {
    return Message.new(raw).cast<bytes>();
}
flow hashed(raw: bytes) -> Option<Message<bytes>> {
    return Message.new(sha256(raw)).cast<bytes>();
}
flow repeated(raw: bytes, text: string) -> (Option<Message<JsonValue>>, Option<Message<i32>>) {
    return (Message.new(raw).cast<JsonValue>(), Message.new(text).cast<i32>());
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    let output = check_program(&hir);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let generic = hir
        .items
        .iter()
        .find_map(|(item, data)| {
            matches!(data, etas_hir::HirItem::Flow(flow) if hir.symbols.get(flow.symbol).unwrap().name == "generic").then_some(item)
        })
        .unwrap();
    let etas_types::ItemSignature::Flow(signature) = &output.facts.item_signatures[&generic] else {
        panic!("generic flow");
    };
    let payload = signature.params[0];
    assert!(
        matches!(output.store.get(payload), Some(Type::Named(parameter)) if parameter.source_parameter.is_some())
    );
    assert_eq!(hir.exprs.iter().filter(|(expr, value)| matches!(value, etas_hir::HirExpr::MethodCall { method, .. } if method == "new") && matches!(output.store.get(output.facts.expr_types[expr]), Some(Type::Message(inner)) if *inner == payload)).count(), 1);
}

#[test]
fn method_context_does_not_relax_cast_arity_or_result_type_checks() {
    for source in [
        "flow main(raw: bytes) -> Option<Message<i32>> { return Message.new(raw).cast<string>(); }",
        "flow main(raw: bytes) -> Option<Message<i32>> { return Message.new(raw).cast<i32>(1); }",
        "flow main(raw: bytes) -> Option<Message<i32>> { return Message.new(raw).cast<i32, string>(); }",
        "flow main(raw: bytes) -> Option<Message<i32>> { return raw.cast<i32>(); }",
        "flow main(raw: bytes) -> Option<Message<string>> { return Message.new<i32>(raw).cast<string>(); }",
    ] {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
            etas_core::SourceId(9),
            None,
            source,
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let output = check_program(&etas_hir::lower_program(&parsed.value));
        assert!(
            !output.diagnostics.is_empty(),
            "invalid call accepted: {source}"
        );
    }
}
