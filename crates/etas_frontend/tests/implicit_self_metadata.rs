use etas_frontend::{Frontend, ProjectInput, SourceInput};

#[test]
fn source_generic_spec_method_evidence_survives_real_metadata_projection_and_binary_codec() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module api;
public spec Encoder { flow encode(self) -> i32; }
public flow encode<T ~ Encoder>(value: T) -> i32 { return value::Encoder.encode(); }
flow main() -> unit { return; }
"#,
    )));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.expect("checked producer");
    let artifact = etas_frontend::build_package_metadata_artifact_from_checked(
        etas_frontend::PackageMetadataBuildInput {
            definition_origin: None,
            package_id: "spec-evidence-producer".into(), package_version: "0.1.0".into(),
            package_edition: "2026".into(), compiler_version: etas_frontend::FRONTEND_COMPILER_VERSION.into(),
            source_payload_hash: "blake3:source".into(), manifest_hash: "blake3:manifest".into(),
            dependency_lock_hash: "blake3:lock".into(), bins: Vec::new(), dependencies: Vec::new(), tool_bindings: Vec::new(),
        }, &checked,
    ).expect("checked generic spec invocation must be exportable, not erased or replaced with a row-only summary");
    let (_, metadata) = etas_package_metadata::package_metadata_from_artifact(
        std::path::Path::new("producer.etasmeta"),
        &artifact.bytes,
    )
    .expect("binary evidence replay");
    let summary = metadata
        .public_metadata
        .effect_summaries
        .iter()
        .find(|summary| summary.item == ["api", "encode"])
        .expect("exported generic wrapper summary");
    let etas_package_metadata::ActionTrace::ParameterCall { parameter } = &summary.action_trace
    else {
        panic!(
            "the method invocation must remain explicit: {:?}",
            summary.action_trace
        );
    };
    let etas_package_metadata::CallableRequestTarget::SpecMethod(method) = parameter else {
        panic!("a spec method is not a positional callable parameter");
    };
    assert_eq!(method.spec, ["api", "Encoder"]);
    assert_eq!(method.method, "encode");
    assert_eq!(method.arity, 1);
    assert_eq!(method.receiver.kind, etas_package_metadata::TypeKind::Var);
    assert_eq!(method.receiver.name, "T");
    assert!(method.generic_arguments.is_empty());
    assert!(method.callable_arguments.is_empty());
    let requests = summary
        .handler_requests
        .as_ref()
        .expect("checked request template");
    assert!(requests.actions.is_empty());
    assert_eq!(requests.parameters.len(), 1);
    assert_eq!(&requests.parameters[0].parameter, parameter);
}
