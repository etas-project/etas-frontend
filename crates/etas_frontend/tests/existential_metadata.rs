use etas_frontend::{Frontend, ProjectInput, SourceInput};
use etas_package_metadata::{CallableValueSource, ExistentialWitnessTarget};

#[test]
fn source_object_factories_export_typed_witnesses_through_the_binary_artifact() {
    let source = r#"
module api;
public spec Named { flow name(self) -> string ![Console.stdout_write]; }
public type Label = string;
impl Label ~ Named {
    flow name(self) -> string ![Console.stdout_write] {
        perform Console.stdout_write("label");
        return "label";
    }
}
public flow make() -> ? ~ Named ![] { return Label("value"); }
public flow pack<T ~ Named>(value: T) -> ? ~ Named ![] { return value; }
flow main() -> unit ![] { return; }
"#;
    let output =
        Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let artifact = etas_frontend::build_package_metadata_artifact_from_checked(
        etas_frontend::PackageMetadataBuildInput {
            definition_origin: Some(
                etas_code::PackageKey::from_name_and_version("objects", "1.0.0").unwrap(),
            ),
            package_id: "objects".into(),
            package_version: "1.0.0".into(),
            package_edition: "2026".into(),
            compiler_version: etas_frontend::FRONTEND_COMPILER_VERSION.into(),
            source_payload_hash: "blake3:source".into(),
            manifest_hash: "blake3:manifest".into(),
            dependency_lock_hash: "blake3:lock".into(),
            bins: vec![],
            dependencies: vec![],
            tool_bindings: vec![],
        },
        &checked,
    )
    .unwrap();
    let (_, metadata) = etas_package_metadata::package_metadata_from_artifact(
        std::path::Path::new("objects.etasmeta"),
        &artifact.bytes,
    )
    .unwrap();
    for (name, concrete) in [("make", true), ("pack", false)] {
        let graph = metadata
            .public_metadata
            .effect_summaries
            .iter()
            .find(|summary| summary.item == ["api", name])
            .unwrap()
            .returned_callables
            .as_ref()
            .unwrap();
        let [CallableValueSource::Existential(index)] = graph.sources.as_slice() else {
            panic!("checked object root")
        };
        let object = &graph.objects[*index as usize];
        assert_eq!(object.object_type.bounds[0].spec, ["api", "Named"]);
        assert_eq!(object.witnesses.len(), 1);
        assert_eq!(object.witnesses[0].member, "name");
        match (&object.witnesses[0].target, concrete) {
            (ExistentialWitnessTarget::Implementation { method, value }, true) => {
                assert_eq!(method.name(), "name");
                assert_eq!(method.implementation().subject().declaration(), ["Label"]);
                assert_eq!(graph.values[*value as usize].param_names, ["self"]);
                assert_eq!(
                    graph.values[*value as usize]
                        .requested_actions
                        .effects
                        .len(),
                    1
                );
                assert!(matches!(
                    graph.values[*value as usize].action_trace,
                    etas_package_metadata::ActionTrace::Event { .. }
                ));
            }
            (ExistentialWitnessTarget::Bound, false) => {
                assert_eq!(
                    object.source_type.kind,
                    etas_package_metadata::TypeKind::Var
                );
                assert_eq!(object.source_type.name, "T");
                assert_eq!(object.payload_sources.len(), 1);
            }
            other => panic!("incorrect witness: {other:?}"),
        }
    }
}
