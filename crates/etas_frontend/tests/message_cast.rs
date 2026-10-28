use etas_frontend::{Frontend, SourceInput};

#[test]
fn chained_message_cast_keeps_constructor_and_target_payloads_distinct() {
    let output = Frontend.check(SourceInput::anonymous(
        r#"
import std.agent.message.Message;
import std.json.JsonValue;
import std.crypto.sha256;
flow main(raw: bytes) -> Option<Message<JsonValue>> {
    return Message.new(raw).cast<JsonValue>();
}
flow identity(raw: bytes) -> Option<Message<bytes>> {
    return Message.new(raw).cast<bytes>();
}
flow hashed(raw: bytes) -> Option<Message<bytes>> {
    return Message.new(sha256(raw)).cast<bytes>();
}
flow local(text: string) -> Option<Message<i32>> {
    let result = Message.new(text).cast<i32>();
    return result;
}
"#,
    ));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.expect("fully checked project");
    let mut constructors = 0;
    let mut casts = 0;
    for (expr, data) in checked.hir.exprs.iter() {
        let etas_hir::HirExpr::MethodCall { method, .. } = data else {
            continue;
        };
        if method == "new" {
            assert!(matches!(
                checked.type_store.get(checked.types.expr_types[&expr]),
                Some(etas_types::Type::Message(_))
            ));
            constructors += 1;
        } else if method == "cast" {
            let Some(etas_types::Type::Option(inner)) =
                checked.type_store.get(checked.types.expr_types[&expr])
            else {
                panic!("checked Option cast result");
            };
            assert!(matches!(
                checked.type_store.get(*inner),
                Some(etas_types::Type::Message(_))
            ));
            casts += 1;
        }
    }
    assert_eq!((constructors, casts), (4, 4));
}
