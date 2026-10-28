use std::{collections::BTreeMap, sync::Arc};

use etas_code::{CodeFingerprint, PackageKey, descriptor::*};
use etas_frontend::{
    CheckedProgram, EntryPolicy, Frontend, ProjectInput, SourceInput, SourceKind,
    code::{
        definition::{DefinitionCatalog, StaticReflectionView},
        identity::DefinitionOrigins,
    },
};

fn checked(sources: &[&str]) -> CheckedProgram {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
    input.options.entry_policy = EntryPolicy::Optional;
    input.sources = sources
        .iter()
        .enumerate()
        .map(|(id, text)| SourceInput {
            id: etas_core::SourceId(70 + id as u32),
            path: None,
            text: (*text).into(),
            kind: SourceKind::SourceProjectFile,
        })
        .collect();
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.unwrap()
}

fn catalog(
    checked: &CheckedProgram,
) -> Result<DefinitionCatalog, etas_frontend::code::descriptor::ProjectionError> {
    let key = PackageKey::from_canonical_fingerprint(CodeFingerprint::from_bytes([7; 32]));
    DefinitionCatalog::build(
        checked.clone(),
        &DefinitionOrigins {
            source_modules: DefinitionOrigins::declared_source_modules(checked, key).unwrap(),
            standard_package: key,
            external_definitions: BTreeMap::new(),
        },
        Default::default(),
    )
}

fn agent(catalog: &DefinitionCatalog) -> AgentHeaderInfo {
    let expression = *catalog
        .checked_program()
        .types
        .reflections
        .keys()
        .next()
        .unwrap();
    let StaticReflectionView::Agent(header) = catalog.static_reflection(expression).unwrap() else {
        panic!("agent");
    };
    header
}

#[test]
fn agent_spec_descriptions_use_checked_aliases_arguments_and_inline_alternatives() {
    let checked = checked(&[
        "module api; public spec Stage<I, O>: callable I => O ![]; public spec Quiet: trace = -Console.stdout_write;",
        "module app; import api.Stage as S; import api.Quiet as Q; agent Worker(value: i32) -> i32 ~ S<i32, i32> + Q + (-Console.stdout_write | -Console) { return value; } flow inspect() -> unit { let d = ^Worker; return; }",
    ]);
    let catalog = catalog(&checked).unwrap();
    let header = agent(&catalog);
    assert_eq!(header.specs.len(), 3);
    let AttachedSpecInfo::Named(stage) = &header.specs[0] else {
        panic!("stage");
    };
    assert_eq!(stage.definition.module(), ["api"]);
    assert_eq!(stage.definition.declaration(), ["Stage"]);
    assert_eq!(stage.arguments.len(), 2);
    for argument in &stage.arguments {
        assert_eq!(
            catalog.descriptors().type_info(*argument),
            Some(&TypeInfo::Primitive(PrimitiveInfo::I32))
        );
    }
    let AttachedSpecInfo::Named(quiet) = &header.specs[1] else {
        panic!("quiet");
    };
    assert_eq!(quiet.definition.declaration(), ["Quiet"]);
    let AttachedSpecInfo::InlineTrace(constraint) = &header.specs[2] else {
        panic!("inline");
    };
    assert_eq!(constraint.alternatives.len(), 2);
    for alternative in &constraint.alternatives {
        assert!(matches!(&alternative[..], [TraceRuleInfo::Deny(_)]));
    }
    constraint.validate(catalog.descriptors()).unwrap();
}

#[test]
fn agent_spec_descriptions_include_component_markers_and_only_the_default_method_constraints() {
    let checked = checked(&[r#"module app;
spec Marker;
spec Stage<I, O>: callable I => O ![];
spec Extra: trace = -Console.stdout_write;
agent Worker;
impl agent Worker ~ Marker {}
impl agent Worker ~ Stage<i32, i32> { flow run(value: i32) -> i32 { return value; } }
impl agent Worker { flow helper() -> unit ~ Extra { return; } }
flow inspect() -> unit { let d = ^Worker; return; }
"#]);
    let catalog = catalog(&checked).unwrap();
    let header = agent(&catalog);
    let names = header
        .specs
        .iter()
        .map(|spec| match spec {
            AttachedSpecInfo::Named(spec) => spec.definition.declaration()[0].as_str(),
            _ => panic!("named"),
        })
        .collect::<Vec<_>>();
    assert_eq!(names, ["Marker", "Stage"]);
}

#[test]
fn missing_inline_checked_model_cannot_become_an_empty_spec_description() {
    let mut checked = checked(&[
        "module app; agent Worker() -> unit ~ (-Console.stdout_write) { return; } flow inspect() -> unit { let d = ^Worker; return; }",
    ]);
    Arc::make_mut(&mut Arc::make_mut(&mut checked).effects)
        .trace_specs
        .conformances
        .clear();
    assert!(
        matches!(catalog(&checked), Err(etas_frontend::code::descriptor::ProjectionError::Invalid(reason)) if reason.contains("checked trace model"))
    );
}

#[test]
fn default_method_specs_keep_their_checked_generic_binder_scope() {
    let checked = checked(&[r#"module app;
spec Identity<T>: callable T => T ![];
agent Worker;
impl agent Worker {
    flow run<T>(value: T) -> T ~ Identity<T> { return value; }
}
flow inspect() -> unit { let d = ^Worker; return; }
"#]);
    let catalog = catalog(&checked).unwrap();
    let header = agent(&catalog);
    let AttachedSpecInfo::Named(spec) = &header.specs[0] else {
        panic!("named");
    };
    let Some(TypeInfo::Parameter(binder)) = catalog.descriptors().type_info(spec.arguments[0])
    else {
        panic!("parameter");
    };
    let parameter = catalog.descriptors().binder(*binder).unwrap();
    assert_eq!(parameter.name, "T");
    let entry = header.default_entry.as_ref().unwrap();
    assert_eq!(
        &parameter.owner,
        &GenericOwner::Method(entry.as_method().unwrap().clone())
    );
    let callable = catalog.callable(entry).unwrap();
    let Some(TypeInfo::Function(signature)) = catalog.descriptors().type_info(callable.signature)
    else {
        panic!("signature");
    };
    assert_eq!(signature.generics, [*binder]);
    assert_eq!(signature.parameters[0].ty, spec.arguments[0]);
}

#[test]
fn frozen_contract_includes_inline_constraints_and_ignores_import_alias_spelling() {
    let build = |import: &str, spelling: &str, inline: &str| {
        let source = format!(
            "module app; {import} agent Worker() -> i32 ~ {spelling} + ({inline}) {{ return 1; }} flow inspect() -> unit {{ let d = ^Worker; return; }}"
        );
        let checked = checked(&[
            "module api; public spec Quiet: trace = -Console.stdout_write;",
            &source,
        ]);
        let catalog = catalog(&checked).unwrap();
        let expression = *checked.types.reflections.keys().next().unwrap();
        catalog
            .freeze_agent(expression)
            .unwrap()
            .contract()
            .fingerprint()
    };
    let direct = build("import api.Quiet;", "Quiet", "-Console.stdout_write");
    let alias = build("import api.Quiet as Q;", "Q", "-Console.stdout_write");
    let changed = build("import api.Quiet;", "Quiet", "-Console");
    assert_eq!(direct, alias);
    assert_ne!(direct, changed);
}

#[test]
fn protocol_constraints_are_not_mistaken_for_missing_spec_facts() {
    for declaration in [
        "agent Worker() -> unit ~ R { return; }",
        "agent Worker; impl agent Worker { flow run() -> unit ~ R { return; } }",
    ] {
        let source = format!(
            "module app; import api.Review as R; {declaration} flow inspect() -> unit {{ let d = ^Worker; return; }}"
        );
        let checked = checked(&["module api; public protocol Review {}", &source]);
        let catalog = catalog(&checked).unwrap();
        let header = agent(&catalog);
        assert_eq!(header.specs.len(), 1);
        assert!(
            matches!(&header.specs[0], AttachedSpecInfo::Protocol { definition, arguments } if definition.kind() == etas_code::DefinitionKind::Protocol && definition.module() == ["api"] && definition.declaration() == ["Review"] && arguments.is_empty())
        );
    }
}
