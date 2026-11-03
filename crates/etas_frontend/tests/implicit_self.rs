use etas_frontend::{Frontend, ProjectInput, SourceInput};

fn check(source: &str) -> etas_frontend::ProjectOutput {
    Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)))
}

#[test]
fn implicit_spec_receivers_support_two_nominals_and_generic_evidence() {
    let output = check(
        r#"
module app.main;
spec SnapshotEncoder { flow encode(self) -> i32; }
type Packet = { number: i32 };
type Snapshot = { number: i32 };
impl Packet ~ SnapshotEncoder { flow encode(self) -> i32 { return self.number; } }
impl Snapshot ~ SnapshotEncoder { flow encode(self) -> i32 { return self.number + 1; } }
flow encode<T ~ SnapshotEncoder>(value: T) -> i32 { return value::SnapshotEncoder.encode(); }
flow main() -> i32 {
    return encode(Packet { number = 7 }) + encode(Snapshot { number = 11 });
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.checked.is_some());
}

#[test]
fn implicit_spec_receiver_does_not_accept_missing_or_incorrect_evidence() {
    for implementation in [
        "",
        "impl Packet ~ SnapshotEncoder { flow encode(self) -> string { return \"bad\"; } }",
        "impl Packet ~ SnapshotEncoder { flow encode(self, extra: i32) -> i32 { return extra; } }",
        "impl Packet ~ SnapshotEncoder { flow encode(self) -> i32 { return \"wrong body\"; } }",
    ] {
        let output = check(&format!(
            r#"
module app.main;
spec SnapshotEncoder {{ flow encode(self) -> i32; }}
type Packet = {{ number: i32 }};
{implementation}
flow main() -> i32 {{ return Packet {{ number = 7 }}::SnapshotEncoder.encode(); }}
"#
        ));
        assert!(output.checked.is_none(), "accepted {implementation}");
    }
}

#[test]
fn spec_method_calls_do_not_drop_the_implementations_requested_actions() {
    let output = check(
        r#"
module app.main;
spec Reporter { flow report(self) -> unit ![Console.stdout_write]; }
type Packet = { number: i32 };
impl Packet ~ Reporter {
    flow report(self) -> unit ![Console.stdout_write] { perform Console.stdout_write("reported"); }
}
flow main() -> unit ![Console.stdout_write] { Packet { number = 7 }::Reporter.report(); }
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.expect("checked reporter");
    let method = checked
        .types
        .spec_impls
        .iter()
        .flat_map(|implementation| &implementation.methods)
        .find(|method| method.name == "report")
        .expect("checked report method");
    let etas_hir::SymbolDef::Item {
        item: implementation,
    } = checked.hir.symbols.get(method.symbol).unwrap().def
    else {
        panic!("normalized report method");
    };
    assert!(
        !checked.effects.item_effects[&implementation]
            .requested_actions
            .effects
            .is_empty()
    );
    let summary = checked
        .effects
        .item_effects
        .get(&checked.entry.unwrap())
        .expect("main summary");
    assert!(
        !summary.requested_actions.effects.is_empty(),
        "method request disappeared: {summary:?}"
    );
    assert_eq!(
        summary.requested_actions,
        checked.effects.item_effects[&implementation].requested_actions
    );
    assert_eq!(
        summary.escaping_effects,
        checked.effects.item_effects[&implementation].escaping_effects
    );
    assert_eq!(
        summary.action_trace,
        checked.effects.item_effects[&implementation].action_trace
    );
}

#[test]
fn concrete_spec_method_invocations_preserve_trace_multiplicity() {
    let output = check(
        r#"
module app.main;
spec Reporter { flow report(self) -> unit ![Console.stdout_write]; }
type Packet = { number: i32 };
impl Packet ~ Reporter {
    flow report(self) -> unit ![Console.stdout_write] { perform Console.stdout_write("reported"); }
}
flow main() -> unit ![Console.stdout_write] {
    let packet = Packet { number = 7 };
    packet::Reporter.report();
    packet::Reporter.report();
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.expect("checked reporter");
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    let etas_effects::ActionTraceDomain::Seq(events) = &summary.action_trace else {
        panic!(
            "two calls must remain two trace positions: {:?}",
            summary.action_trace
        );
    };
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| matches!(event, etas_effects::ActionTraceDomain::Event(_)))
    );
}

#[test]
fn trace_spec_rejects_an_action_performed_by_a_spec_method() {
    let output = check(
        r#"
module app.main;
spec Reporter { flow report(self) -> unit ![Console.stdout_write]; }
spec Quiet: trace = -Console.stdout_write;
type Packet = { number: i32 };
impl Packet ~ Reporter {
    flow report(self) -> unit ![Console.stdout_write] { perform Console.stdout_write("reported"); }
}

flow main() -> unit ![Console.stdout_write] ~ Quiet {
    Packet { number = 7 }::Reporter.report();
}
"#,
    );
    assert!(
        output.checked.is_none(),
        "trace spec accepted the method's forbidden action"
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic.code, etas_core::DiagnosticCode::Effect(_))),
        "{:?}",
        output.diagnostics
    );
    assert!(
        !output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Effect(
                etas_core::EffectDiagnosticCode::IncompleteEffectFacts
            )),
        "must fail on the actual trace, not missing summaries: {:?}",
        output.diagnostics
    );
}

#[test]
fn implicit_receiver_does_not_consume_the_specs_public_generic_argument() {
    let output = check(
        r#"
module app.main;
spec Encoder<Output> { flow encode(self) -> Output; }
type Packet = { number: i32 };
impl Packet ~ Encoder<i32> { flow encode(self) -> i32 { return self.number; } }
flow encode<T ~ Encoder<i32>>(value: T) -> i32 { return value::Encoder<i32>.encode(); }
flow main() -> i32 { return encode(Packet { number = 7 }); }
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.checked.is_some());
}

#[test]
fn concrete_spec_method_effects_use_implementation_generic_parameter_identity() {
    let output = check(
        r#"
module app.main;
spec Failure { flow fail<T>(self, error: T) -> never ![Error<T>]; }
type Packet = { number: i32 };
impl Packet ~ Failure {
    flow fail<U>(self, error: U) -> never ![Error<U>] { return perform Error<U>.raise(error); }
}
flow main() -> Result<never, string> {
    let packet = Packet { number = 7 };
    return packet::Failure.fail("failed")?;
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.expect("checked generic method call");
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert!(summary.escaping_effects.is_empty());
    let (_, invoked) = checked
        .effects
        .expr_effects
        .iter()
        .find(|(expr, _)| {
            matches!(
                checked.hir.exprs.get(**expr),
                Some(etas_hir::HirExpr::SpecMethodCall { .. })
            )
        })
        .expect("specialized invocation before try capture");
    let effects = invoked.escaping_effects.effects.iter().collect::<Vec<_>>();
    let [etas_effects::Effect::Error(error)] = effects.as_slice() else {
        panic!("expected one specialized typed error: {effects:?}");
    };
    assert!(matches!(
        checked.type_store.get(*error),
        Some(etas_types::Type::Primitive(
            etas_types::PrimitiveType::String
        ))
    ));
}

#[test]
fn implicit_receiver_identity_survives_source_metadata_projection_and_binary_codec() {
    let output = check(
        r#"
module app.main;
public spec SnapshotEncoder { flow encode(self) -> i32; }
public type Packet = { number: i32 };
impl Packet ~ SnapshotEncoder { flow encode(self) -> i32 { return self.number; } }
public flow main() -> i32 { return Packet { number = 7 }::SnapshotEncoder.encode(); }
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.expect("checked receiver package");
    let artifact = etas_frontend::build_package_metadata_artifact_from_checked(
        etas_frontend::PackageMetadataBuildInput {
            definition_origin: Some(etas_code::PackageKey::from_canonical_fingerprint(
                etas_code::CodeFingerprint::of_bytes(
                    "etas.test.receiver-package",
                    b"receiver-package",
                ),
            )),
            package_id: "receiver-package".into(),
            package_version: "0.1.0".into(),
            package_edition: "2026".into(),
            compiler_version: etas_frontend::FRONTEND_COMPILER_VERSION.into(),
            source_payload_hash: "blake3:test-source".into(),
            manifest_hash: "blake3:test-manifest".into(),
            dependency_lock_hash: "blake3:test-lock".into(),
            bins: Vec::new(),
            dependencies: Vec::new(),
            tool_bindings: Vec::new(),
        },
        &checked,
    )
    .expect("source metadata projection");
    let (_, metadata) = etas_package_metadata::package_metadata_from_artifact(
        std::path::Path::new("package.etasmeta"),
        &artifact.bytes,
    )
    .expect("binary receiver replay");
    let spec = metadata
        .public_metadata
        .spec_signatures
        .iter()
        .find(|spec| {
            spec.path
                .last()
                .is_some_and(|name| name == "SnapshotEncoder")
        })
        .expect("exported spec");
    assert!(
        spec.param_names.is_empty(),
        "Self must not add a public spec type argument"
    );
    let method = &spec.methods[0];
    assert_eq!(method.receiver_param, Some(0));
    let signature = method.signature.as_ref().unwrap();
    assert_eq!(signature.generic_params.len(), 1);
    assert_eq!(signature.generic_params[0].name, signature.input[0].name);
    assert_eq!(
        signature.input[0].kind,
        etas_package_metadata::TypeKind::Var
    );
}
