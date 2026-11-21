use std::collections::HashMap;

use etas_hir::HirItem;
use etas_types::{ItemSignature, Type, TypeSubstitutionEngine};

fn check(source: &str) -> (etas_hir::HirProgram, etas_types::TypeOutput) {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    let output = etas_types::check_program(&hir);
    (hir, output)
}

#[test]
fn registry_opaque_types_have_closed_distinct_nominal_identities() {
    let (hir, output) = check(
        "module app;
        import std.fs.IOError as FsError;
        import std.io.IOError as ConsoleError;
        import std.json.JsonValue;
        flow retain(fs: FsError, console: ConsoleError, json: JsonValue) -> unit ![] { return; }",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let item = hir
        .items
        .iter()
        .find_map(|(id, node)| matches!(node, HirItem::Flow(_)).then_some(id))
        .unwrap();
    let ItemSignature::Flow(signature) = &output.facts.item_signatures[&item] else {
        panic!("flow signature");
    };
    let names = HashMap::new();
    let validation = TypeSubstitutionEngine::existing(&output.store, &names);
    for (ty, name) in signature.params[..2]
        .iter()
        .zip(["std.fs.IOError", "std.io.IOError"])
    {
        let Some(Type::Nominal(nominal)) = output.store.get(*ty) else {
            panic!(
                "registry-backed opaque type must have a closed nominal identity: {:?}",
                output.store.get(*ty)
            );
        };
        assert_eq!(nominal.name, name);
        assert!(nominal.representation.is_none());
        validation.validate_closed_argument(*ty).unwrap();
    }
    assert_ne!(signature.params[0], signature.params[1]);
    let json = signature.params[2];
    assert!(
        matches!(output.store.get(json), Some(Type::Enum(value)) if value.name == "std.json.JsonValue")
    );
    validation.validate_closed_argument(json).unwrap();
    assert_eq!(
        output.facts.enum_layouts[&json]
            .variants
            .iter()
            .map(|variant| variant.name.as_str())
            .collect::<Vec<_>>(),
        ["Null", "Bool", "Number", "String", "Array", "Object"]
    );
    assert_eq!(
        output.facts.known_std_types.json_value,
        Some(signature.params[2])
    );
}

#[test]
fn opaque_registry_identity_does_not_authorize_anonymous_record_construction() {
    let (_, output) = check(
        "module app;
        import std.fs.IOError as FsError;
        flow forge() -> FsError ![] { return { message = \"not an error handle\" }; }",
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.code,
            etas_core::DiagnosticCode::Type(
                etas_core::TypeDiagnosticCode::Mismatch
                    | etas_core::TypeDiagnosticCode::TypeMismatch
            )
        )),
        "anonymous representation cannot construct an opaque std value: {:?}",
        output.diagnostics
    );
}

#[test]
fn opaque_std_type_arity_is_diagnosed_once() {
    let (_, output) = check(
        "module app;
        import std.fs.IOError as FsError;
        flow invalid(value: FsError<i32>) -> unit ![] { return; }",
    );
    let arity = output
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code
                == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::ArityMismatch)
        })
        .count();
    assert_eq!(arity, 1, "{:?}", output.diagnostics);
}
