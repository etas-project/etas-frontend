use etas_code::syntax::CodeSyntaxOperations;
use etas_code::{CallableId, CodeFingerprint, DefinitionKey, DefinitionKind, PackageKey};
use etas_core::{SourceFile, SourceId};
use etas_frontend::code::service::NativeCodeSyntaxOperations;
use std::sync::Arc;

fn callable() -> CallableId {
    let package =
        PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes("roundtrip", b"package"));
    CallableId::declaration(
        DefinitionKey::new(
            package,
            vec!["app".into()],
            vec!["main".into()],
            DefinitionKind::Flow,
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn retained_source_reexport_preserves_original_unit_range_form_and_content() {
    for declaration in [
        "flow main() -> i32 { /* retained block */ return 42; }",
        "flow main() -> i32 = 42;",
        "flow main() -> i32 { return perform Probe.read(); } with { Probe.read() => { resume 42; } }",
    ] {
        let source = Arc::new(SourceFile::new(
            SourceId(2),
            None,
            format!("module app; {declaration}"),
        ));
        let program = Arc::new(etas_syntax::parse_program((*source).clone()));
        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        let backend = NativeCodeSyntaxOperations::default();
        let target = callable();
        let original = backend
            .retain_source_body(
                target.fingerprint(),
                source,
                program,
                etas_syntax::ProgramBodySelection {
                    item: 0,
                    impl_member: None,
                },
            )
            .unwrap();
        let saved = backend.export_body_source(&original).unwrap();
        assert!(saved.source_start > 0);
        let restored = NativeCodeSyntaxOperations::default();
        let body = restored.import_retained_body(&target, &saved).unwrap();
        assert_eq!(restored.export_body_source(&body).unwrap(), saved);
        let again = NativeCodeSyntaxOperations::default();
        let second = again
            .import_retained_body(&target, &restored.export_body_source(&body).unwrap())
            .unwrap();
        assert_eq!(again.export_body_source(&second).unwrap(), saved);
        assert_eq!(original.identity(), body.identity());
        assert_eq!(body.identity(), second.identity());
        assert_eq!(
            backend.format(&original).unwrap(),
            again.format(&second).unwrap()
        );
    }
}
