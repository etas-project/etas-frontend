use etas_frontend::{Frontend, ProjectInput, SourceInput};
use etas_types::{ItemSignature, Type};

fn signature_output(source: &str) -> (etas_types::TypeId, std::sync::Arc<etas_types::TypeStore>) {
    let output =
        Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let ItemSignature::Flow(signature) = &checked.types.item_signatures[&checked.entry.unwrap()]
    else {
        panic!("expected flow signature");
    };
    (signature.output, checked.type_store.clone())
}

#[test]
fn renamed_standard_types_keep_their_canonical_runtime_type() {
    let (ty, store) = signature_output(
        r#"
module app.main;
import std.agent.message.Message as Envelope;
flow main(input: string) -> Envelope<string> { return Envelope.new(input); }
"#,
    );
    assert!(
        matches!(store.get(ty), Some(Type::Message(_))),
        "{:?}",
        store.get(ty)
    );

    let (ty, store) = signature_output(
        r#"
module app.main;
import std.collections.List as Array;
flow main(input: Array<i32>) -> Array<i32> { return input; }
"#,
    );
    assert!(
        matches!(store.get(ty), Some(Type::List(_))),
        "{:?}",
        store.get(ty)
    );
}

#[test]
fn source_types_can_shadow_standard_display_names_without_changing_identity() {
    let (ty, store) = signature_output(
        r#"
module app.main;
type Message<T> = { value: T };
flow main() -> Message<i32> { return Message<i32> { value = 42 }; }
"#,
    );
    let Some(Type::Applied { constructor, .. }) = store.get(ty) else {
        panic!("expected applied source nominal type");
    };
    assert!(matches!(
        store.get(etas_types::TypeId(constructor.0)),
        Some(Type::Nominal(_))
    ));
}
