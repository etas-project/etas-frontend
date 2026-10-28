use super::*;
use crate::{EntryPolicy, Frontend, ProjectInput, SourceInput, code::identity::DefinitionOrigins};
use etas_code::PackageKey;
use std::collections::BTreeMap;

fn prepared() -> (DefinitionCatalog, metadata::CallableValueGraph) {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        "module app; type UserId = string; type OtherId = string; agent Worker(value: UserId) -> (() -> UserId ![]) { return () => value; } flow inspect() -> unit { let definition = ^Worker; return; }",
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.canonical-latent-graph",
        b"app",
    ));
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
        standard_package: package,
        external_definitions: BTreeMap::new(),
    };
    let reflection = *checked.types.reflections.keys().next().unwrap();
    let catalog = DefinitionCatalog::build(checked, &origins, Default::default()).unwrap();
    let frozen = catalog.freeze_agent(reflection).unwrap();
    let callable = frozen.contract().header().default_entry.as_ref().unwrap();
    let graph = (*frozen.contract().latent_graph(callable).unwrap().graph).clone();
    (catalog, graph)
}

#[test]
fn nominal_identity_changes_the_latent_contract_even_with_the_same_representation() {
    let (catalog, graph) = prepared();
    let before = fingerprint(&catalog, &graph).unwrap();
    let mut changed = graph.clone();
    changed.values[0].output.path = vec!["app".into(), "OtherId".into()];
    assert_ne!(before, fingerprint(&catalog, &changed).unwrap());
}

#[test]
fn a_missing_canonical_identity_cannot_be_replaced_by_a_descriptive_path() {
    let (catalog, mut graph) = prepared();
    graph.values[0].output.path = vec!["missing".into(), "UserId".into()];
    assert!(matches!(
        fingerprint(&catalog, &graph),
        Err(ProjectionError::MissingIdentity(_))
    ));
}

#[test]
fn latent_requested_actions_and_their_static_selectors_remain_part_of_the_contract() {
    let (catalog, mut graph) = prepared();
    let before = fingerprint(&catalog, &graph).unwrap();
    let effect = metadata::EffectRef {
        path: vec!["Console".into(), "stdout_write".into()],
        args: vec![metadata::EffectArg {
            kind: metadata::EffectArgKind::String,
            value: "first".into(),
            ..Default::default()
        }],
    };
    graph.values[0]
        .requested_actions
        .effects
        .push(effect.clone());
    graph.values[0].action_trace = metadata::ActionTrace::Event(metadata::ActionTraceEvent {
        action: effect,
        source: metadata::ActionTraceEventSource::Perform,
    });
    let first = fingerprint(&catalog, &graph).unwrap();
    assert_ne!(before, first);
    graph.values[0].requested_actions.effects[0].args[0].value = "second".into();
    assert_ne!(first, fingerprint(&catalog, &graph).unwrap());
}
