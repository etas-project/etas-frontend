use std::{collections::BTreeMap, sync::Arc};

use etas_code::{CodeFingerprint, PackageKey, syntax::CodeSyntaxOperations};
use etas_frontend::{
    CheckedProgram, EntryPolicy, Frontend, FrontendSession, ProjectInput, SourceInput,
    code::{
        definition::{DefinitionCatalog, FrozenDefinitionEnvironment, StaticReflectionView},
        draft::DefaultBodyOverlay,
        identity::{DefinitionOrigins, SourceModuleOrigin},
        service::NativeCodeSyntaxOperations,
    },
};

fn package(name: &[u8]) -> PackageKey {
    PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.module-origin",
        name,
    ))
}

#[test]
fn source_effect_name_does_not_replace_the_standard_effect_identity() {
    use etas_code::descriptor::EffectTargetInfo;
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app;
effect Net extends Network { action request() -> unit; }
agent Worker() -> unit ![Net.request] { perform Net.request(); return; }
flow main() -> unit { let definition = ^Worker; return; }
"#,
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let canonical = origins(&checked, &["app"]);
    let catalog = DefinitionCatalog::build(checked, &canonical, Default::default()).unwrap();
    assert!(catalog.descriptors().data().rows.iter().flat_map(|row| &row.effects).any(|effect| {
        matches!(&effect.target, EffectTargetInfo::Action { effect, name } if name == "request" && effect.package() == package(b"locked dependency") && effect.module() == ["app"] && effect.declaration() == ["Net"])
    }));
}

#[test]
fn qualified_standard_annotation_members_keep_resolved_canonical_identity() {
    use etas_code::descriptor::{AnnotationReferenceInfo, StaticValueInfo};
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app;
import std.text as Text;
@custom(Text, symbol = Text.trim)
agent Worker() -> i32 { return 1; }
flow inspect() -> unit { let definition = ^Worker; return; }
"#,
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let expression = *checked.types.reflections.keys().next().unwrap();
    let catalog = catalog(checked);
    let StaticReflectionView::Agent(header) = catalog.static_reflection(expression).unwrap() else {
        panic!("agent header");
    };
    let arguments = &header.annotations[0].arguments;
    assert!(
        matches!(arguments[0].value.as_ref(), StaticValueInfo::Reference(AnnotationReferenceInfo::Module { package: owner, path })
        if *owner == package(b"builtin std") && path == &["std", "text"])
    );
    assert!(
        matches!(arguments[1].value.as_ref(), StaticValueInfo::Reference(AnnotationReferenceInfo::Declaration(key))
        if key.package() == package(b"builtin std") && key.module() == ["std", "text"] && key.declaration() == ["trim"])
    );
}

#[test]
fn runtime_value_member_is_not_a_static_annotation_reference() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app;
flow request() -> { host: string } { return { host = "runtime" }; }
let endpoint = request();
@custom(endpoint.host)
agent Worker() -> i32 { return 1; }
"#,
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::InvalidAnnotation)
                && diagnostic.message.contains("static literal")
        }),
        "{:?}",
        output.diagnostics
    );
    assert!(output.checked.is_none());
}

fn checked(namespace: &str) -> CheckedProgram {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(format!(
        "module {namespace}; type Ticket = string; agent Worker() -> i32 {{ return 1; }} impl agent Worker {{ flow extra(value: Ticket) -> Ticket {{ return value; }} }} flow main() -> unit {{ let definition = ^Worker; return; }}"
    )));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.unwrap()
}

fn origins(program: &CheckedProgram, canonical: &[&str]) -> DefinitionOrigins {
    DefinitionOrigins {
        source_modules: program
            .hir
            .modules
            .iter()
            .map(|module| {
                (
                    *module,
                    SourceModuleOrigin::new(
                        package(b"locked dependency"),
                        canonical.iter().map(|s| (*s).into()).collect(),
                    )
                    .unwrap(),
                )
            })
            .collect(),
        standard_package: package(b"builtin std"),
        external_definitions: BTreeMap::new(),
    }
}

fn catalog(program: CheckedProgram) -> DefinitionCatalog {
    let origin = origins(&program, &["library", "workers"]);
    DefinitionCatalog::build(program, &origin, Default::default()).unwrap()
}

fn frozen(catalog: &DefinitionCatalog) -> Arc<FrozenDefinitionEnvironment> {
    let expr = *catalog
        .checked_program()
        .types
        .reflections
        .keys()
        .next()
        .unwrap();
    catalog.freeze_agent(expr).unwrap()
}

#[test]
fn consumer_namespaces_do_not_change_agent_or_method_identity() {
    let first = catalog(checked("alias_one.workers"));
    let second = catalog(checked("alias_two.workers"));
    let header = |catalog: &DefinitionCatalog| {
        let expr = *catalog
            .checked_program()
            .types
            .reflections
            .keys()
            .next()
            .unwrap();
        let StaticReflectionView::Agent(header) = catalog.static_reflection(expr).unwrap() else {
            panic!("agent header");
        };
        header
    };
    let (a, b) = (header(&first), header(&second));
    assert_eq!(a.identity, b.identity);
    assert_eq!(a.identity.module(), &["library", "workers"]);
    assert_eq!(a.default_entry, b.default_entry);
    assert_eq!(a.methods, b.methods);
    assert_eq!(
        first.descriptors().type_fingerprint(a.component).unwrap(),
        second.descriptors().type_fingerprint(b.component).unwrap()
    );
    for id in &a.methods {
        assert_eq!(
            first
                .descriptors()
                .type_fingerprint(first.callable(id).unwrap().signature)
                .unwrap(),
            second
                .descriptors()
                .type_fingerprint(second.callable(id).unwrap().signature)
                .unwrap()
        );
    }
    let method = a.default_entry.unwrap();
    assert_eq!(
        method.as_method().unwrap().implementation().module(),
        &["library", "workers"]
    );
}

#[test]
fn candidate_rebasing_preserves_canonical_module_origin_and_local_resolution() {
    let original = catalog(checked("consumer_alias.workers"));
    let base = frozen(&original);
    let syntax = NativeCodeSyntaxOperations::default();
    let overlay = DefaultBodyOverlay::prepare(
        base.clone(),
        syntax
            .parse_body("return match Ticket(\"id\") { _ => 2 };")
            .unwrap(),
        &syntax,
    )
    .unwrap();
    let analysis = FrontendSession::new()
        .analyze_body_overlay(overlay.clone())
        .unwrap();
    let report = analysis.check_report(Default::default()).unwrap();
    let candidate = FrontendSession::new()
        .compile_body_overlay(overlay)
        .unwrap();
    assert_eq!(candidate.identity(), base.identity());
    assert_eq!(candidate.identity().module(), &["library", "workers"]);
    assert!(
        candidate
            .program()
            .hir
            .modules_arena
            .iter()
            .any(|(_, module)| module
                .name
                .as_ref()
                .is_some_and(|path| path.segments[0].name == "consumer_alias"))
    );
    assert_eq!(report.dependencies.modules.len(), 1);
    assert_eq!(
        report.dependencies.modules[0].module,
        ["library", "workers"]
    );
    assert_eq!(
        report.dependencies.modules[0].package,
        package(b"locked dependency")
    );
}

#[test]
fn canonical_origin_is_part_of_the_frozen_environment_not_a_display_name() {
    let program = checked("consumer_alias.workers");
    let first = catalog(program.clone());
    let different = DefinitionCatalog::build(
        program.clone(),
        &origins(&program, &["other", "workers"]),
        Default::default(),
    )
    .unwrap();
    let (first, different) = (frozen(&first), frozen(&different));
    assert_ne!(first.identity(), different.identity());
    assert_ne!(first.fingerprint(), different.fingerprint());
    assert!(SourceModuleOrigin::new(package(b"owner"), vec!["".into()]).is_err());
    assert!(SourceModuleOrigin::new(package(b"owner"), vec!["invalid\0module".into()]).is_err());
}

#[test]
fn unrelated_or_duplicate_native_source_origins_are_rejected() {
    let program = checked("consumer_alias.workers");
    let mut extra = origins(&program, &["library", "workers"]);
    extra.source_modules.insert(
        etas_hir::HirModuleId(u32::MAX),
        SourceModuleOrigin::new(package(b"unowned"), vec!["unowned".into()]).unwrap(),
    );
    let error = DefinitionCatalog::build(program, &extra, Default::default())
        .err()
        .expect("invalid source origin");
    assert!(error.to_string().contains("no owning checked module"));

    let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
    input.options.entry_policy = EntryPolicy::Optional;
    input.sources = [
        "module first; agent One() -> i32 { return 1; }",
        "module second; agent Two() -> i32 { return 2; }",
    ]
    .iter()
    .enumerate()
    .map(|(index, text)| SourceInput {
        id: etas_core::SourceId(index as u32),
        path: None,
        text: (*text).into(),
        kind: etas_frontend::SourceKind::SourceProjectFile,
    })
    .collect();
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let program = output.checked.unwrap();
    let duplicate = origins(&program, &["same", "module"]);
    let error = DefinitionCatalog::build(program, &duplicate, Default::default())
        .err()
        .expect("duplicate source origin");
    assert!(error.to_string().contains("same canonical origin"));
}

#[test]
fn a_dependency_overlay_cannot_be_claimed_by_the_declared_source_package_helper() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        "module library.workers; agent Worker() -> i32 { return 1; }",
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let dependency = etas_frontend::ExternalPackageId(7);
    input.sources[0].kind = etas_frontend::SourceKind::DependencySourceOverlay {
        package: dependency,
        import_root: "consumer_alias".into(),
    };
    input
        .environment
        .external_packages
        .push(etas_frontend::ProjectExternalPackageInput {
            id: dependency,
            name: "workers".into(),
            version: "1.0.0".into(),
            edition: "2025".into(),
            import_root: "consumer_alias".into(),
        });
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let program = output.checked.unwrap();
    assert!(DefinitionOrigins::declared_source_modules(&program, package(b"consumer")).is_err());
    let module = program.hir.modules[0];
    let origin = SourceModuleOrigin::declared(&program, module, package(b"dependency")).unwrap();
    assert_eq!(origin.module(), &["library", "workers"]);
    assert_eq!(origin.package(), package(b"dependency"));
    let indexed = program.module_index.by_source[&program.modules_arena[module].span.source];
    assert!(matches!(
        &program.module_index.modules[indexed].origin,
        etas_frontend::ModuleOrigin::DependencySourceOverlay { package, import_root }
            if *package == dependency && import_root == "consumer_alias"
    ));
}

#[test]
fn a_dependency_module_part_cannot_inherit_the_first_local_parts_package() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        "module app; agent Local() -> i32 { return 1; }",
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let dependency = etas_frontend::ExternalPackageId(7);
    input.sources.push(SourceInput {
        id: etas_core::SourceId(1),
        path: None,
        text: "module app; agent Foreign() -> i32 { return 2; }".into(),
        kind: etas_frontend::SourceKind::DependencySourceOverlay {
            package: dependency,
            import_root: "dependency".into(),
        },
    });
    input
        .environment
        .external_packages
        .push(etas_frontend::ProjectExternalPackageInput {
            id: dependency,
            name: "dependency".into(),
            version: "1.0.0".into(),
            edition: "2025".into(),
            import_root: "dependency".into(),
        });
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    assert_eq!(checked.hir.modules.len(), 1);
    let error =
        DefinitionOrigins::declared_source_modules(&checked, package(b"local")).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("module part requires an explicit package origin")
    );
}
