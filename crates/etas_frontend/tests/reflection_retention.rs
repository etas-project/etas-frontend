use etas_code::{
    CodeFingerprint, PackageKey, descriptor::DescriptorLimits, syntax::CodeSyntaxOperations,
};
use etas_core::{DiagnosticCode, SourceId, TypeDiagnosticCode};
use etas_frontend::{
    CheckedProgram, EntryPolicy, Frontend, ProjectInput, ProjectOutput, SourceInput, SourceKind,
    code::{
        definition::{DefinitionCatalog, ReflectionOptions},
        identity::{DefinitionOrigins, SourceModuleOrigin},
        service::NativeCodeSyntaxOperations,
    },
};
use etas_hir::{HirExpr, HirExprId};
use std::{collections::BTreeMap, sync::Arc};

fn check(sources: &[&str], retain: &[&str], export: &[&str]) -> ProjectOutput {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
    input.options.entry_policy = EntryPolicy::Optional;
    input.options.reflection = ReflectionOptions {
        retain_bodies: retain.iter().map(|name| (*name).into()).collect(),
        export_bodies: export.iter().map(|name| (*name).into()).collect(),
    };
    input.sources = sources
        .iter()
        .enumerate()
        .map(|(index, text)| SourceInput {
            id: SourceId(index as u32 + 1),
            path: None,
            text: (*text).into(),
            kind: SourceKind::SourceProjectFile,
        })
        .collect();
    Frontend.check_project(input)
}

fn catalog(checked: &CheckedProgram) -> DefinitionCatalog {
    let key = |name: &[u8]| {
        PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
            "etas.test.retention-package.v1",
            name,
        ))
    };
    let origins = DefinitionOrigins {
        source_modules: checked
            .hir
            .modules
            .iter()
            .map(|module| {
                let source = checked.modules_arena[*module]
                    .name
                    .as_ref()
                    .map(|path| path.segments[0].name.as_str());
                (
                    *module,
                    SourceModuleOrigin::declared(
                        checked,
                        *module,
                        if source == Some("foreign") {
                            key(b"foreign")
                        } else {
                            key(b"app")
                        },
                    )
                    .unwrap(),
                )
            })
            .collect(),
        standard_package: key(b"std"),
        external_definitions: BTreeMap::new(),
    };
    DefinitionCatalog::build(checked.clone(), &origins, DescriptorLimits::default()).unwrap()
}

fn reflections(checked: &CheckedProgram) -> Vec<HirExprId> {
    checked
        .hir
        .exprs
        .iter()
        .filter_map(|(id, expr)| matches!(expr, HirExpr::Reflect(_)).then_some(id))
        .collect()
}

#[test]
fn auxiliary_body_access_is_not_shared_across_same_revision_reflection_leases() {
    let output = check(
        &[
            "module foreign.lib; public agent Worker; impl agent Worker { public flow extra() -> i32 { return 1; } } flow inspect() -> unit { let own = ^Worker; return; }",
            "module app.main; import foreign.lib.Worker; flow main() -> unit { let imported = ^Worker; return; }",
        ],
        &["foreign.lib.Worker.extra"],
        &[],
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let store = etas_frontend::code::definition::NativeDefinitionStore::new(
        Arc::new(catalog(&checked)),
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    );
    let expression = |source| {
        checked
            .hir
            .exprs
            .iter()
            .find_map(|(id, expr)| match expr {
                HirExpr::Reflect(reflection) if reflection.span.source == SourceId(source) => {
                    Some(id)
                }
                _ => None,
            })
            .unwrap()
    };
    let own = store.reflect_agent(expression(1)).unwrap();
    let imported = store.reflect_agent(expression(2)).unwrap();
    assert_eq!(own.identity(), imported.identity());
    assert_eq!(own.revision(), imported.revision());
    assert!(!own.is_same_object(&imported));
    let own_methods = store.methods(&own).unwrap();
    let imported_methods = store.methods(&imported).unwrap();
    assert_eq!(own_methods.len(), 1);
    assert_eq!(imported_methods.len(), 1);
    assert_eq!(own_methods[0].header.id, imported_methods[0].header.id);
    assert!(own_methods[0].body.is_some());
    assert!(imported_methods[0].body.is_none());
}

#[test]
fn private_method_visibility_does_not_change_an_agents_revision() {
    let output = check(
        &[
            "module foreign.lib; public agent Worker(value: i32) -> i32 { return value; } impl agent Worker { public flow extra() -> i32 { return 1; } flow hidden() -> i32 { return 2; } } flow inspect() -> unit { let own = ^Worker; return; }",
            "module app.main; import foreign.lib.Worker; flow main() -> unit { let imported = ^Worker; return; }",
        ],
        &[
            "foreign.lib.Worker",
            "foreign.lib.Worker.extra",
            "foreign.lib.Worker.hidden",
        ],
        &[],
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let store = etas_frontend::code::definition::NativeDefinitionStore::new(
        Arc::new(catalog(&checked)),
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    );
    let expression = |source| {
        reflections(&checked)
            .into_iter()
            .find(|id| checked.hir.exprs[*id].span(&checked.hir.blocks).source == SourceId(source))
            .unwrap()
    };
    let own = store.reflect_agent(expression(1)).unwrap();
    let imported = store.reflect_agent(expression(2)).unwrap();
    assert_eq!(own.identity(), imported.identity());
    assert_eq!(own.revision(), imported.revision());
    let own_methods = store.methods(&own).unwrap();
    let imported_methods = store.methods(&imported).unwrap();
    assert_eq!(own_methods.len(), 2);
    assert_eq!(imported_methods.len(), 1);
    let method_name = |method: &etas_frontend::code::definition::NativeAgentMethodView| {
        method.header.id.as_method().unwrap().name().to_owned()
    };
    assert!(
        own_methods
            .iter()
            .any(|method| method_name(method) == "hidden")
    );
    assert_eq!(method_name(&imported_methods[0]), "extra");
    assert!(own_methods.iter().all(|method| method.body.is_some()));
    assert!(imported_methods[0].body.is_none());
    assert!(store.resolve(&own).unwrap().body().is_some());
    assert!(store.resolve(&imported).unwrap().body().is_none());
}

#[test]
fn frozen_tool_surface_uses_declaration_identity_not_import_spelling() {
    let fingerprint = |name: &str, alias: &str| {
        let agent = format!(
            "module app.main; import app.tools.{name} as {alias}; @tools([{alias}]) agent Worker() -> i32 {{ return 1; }} flow inspect() -> unit {{ let definition = ^Worker; return; }}"
        );
        let output = check(
            &[
                "module app.tools; public tool lookup() -> i32 { return 1; } public tool render() -> i32 { return 2; }",
                &agent,
            ],
            &[],
            &[],
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let checked = output.checked.unwrap();
        let frozen = catalog(&checked)
            .freeze_agent(reflections(&checked)[0])
            .unwrap();
        let tools = &frozen.contract().agent_header().unwrap().tools;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].declaration(), [name]);
        assert_eq!(tools[0].module(), ["app", "tools"]);
        frozen.contract().fingerprint()
    };
    assert_eq!(
        fingerprint("lookup", "First"),
        fingerprint("lookup", "Second")
    );
    assert_ne!(
        fingerprint("lookup", "First"),
        fingerprint("render", "First")
    );
}

#[test]
fn default_retention_hides_body_without_erasing_signature() {
    let output = check(
        &[
            "module app; agent Worker() -> i32 { return 42; } flow inspect() -> unit { let d = ^Worker; return; }",
        ],
        &[],
        &[],
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let catalog = catalog(&checked);
    let expression = reflections(&checked)[0];
    assert!(
        catalog
            .retained_body(expression, &NativeCodeSyntaxOperations::default())
            .unwrap()
            .is_none()
    );
    assert!(
        catalog
            .freeze_agent(expression)
            .unwrap()
            .default_body()
            .is_some()
    );
}

#[test]
fn primary_agent_and_run_reuse_one_retained_native_body() {
    for target in ["app.Worker", "app.Worker.run"] {
        let output = check(
            &[
                "module app; agent Worker(x: i32) -> i32 { return x + 1; } flow inspect() -> unit { let a = ^Worker; let b = ^Worker.run; return; }",
            ],
            &[target],
            &[],
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let checked = output.checked.unwrap();
        let catalog = catalog(&checked);
        let syntax = NativeCodeSyntaxOperations::default();
        let bodies = reflections(&checked)
            .into_iter()
            .map(|expr| catalog.retained_body(expr, &syntax).unwrap().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(bodies[0].identity(), bodies[1].identity());
        let first = syntax.resolve(&bodies[0]).unwrap();
        let second = syntax.resolve(&bodies[1]).unwrap();
        let (etas_syntax::SourceBodyView::Block(first), etas_syntax::SourceBodyView::Block(second)) =
            (first.body().unwrap(), second.body().unwrap())
        else {
            panic!("native blocks")
        };
        assert!(std::ptr::eq(first, second));
        assert_eq!(syntax.format(&bodies[0]).unwrap(), "return x + 1;");
        let source = checked.definition_sources.source(SourceId(1)).unwrap();
        assert!(Arc::ptr_eq(
            &source.file.text,
            &checked.sources.sources[0].text
        ));
    }
}

#[test]
fn export_implies_retention_but_plain_retention_does_not_expose_cross_package_body() {
    let sources = [
        "module app.lib; public agent Worker() -> i32 { return 42; }",
        "module foreign.client; import app.lib.Worker; flow inspect() -> unit { let a = ^Worker; return; }",
    ];
    for exported in [false, true] {
        let output = if exported {
            check(&sources, &[], &["app.lib.Worker"])
        } else {
            check(&sources, &["app.lib.Worker"], &[])
        };
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let checked = output.checked.unwrap();
        let catalog = catalog(&checked);
        let body = catalog
            .retained_body(
                reflections(&checked)[0],
                &NativeCodeSyntaxOperations::default(),
            )
            .unwrap();
        assert_eq!(body.is_some(), exported);
    }
}

#[test]
fn invalid_retention_requests_have_registered_configuration_diagnostics() {
    for (retain, export) in [
        (vec!["app.Missing"], vec![]),
        (vec![], vec!["app.Private"]),
        (vec!["app.Private.run.nope"], vec![]),
    ] {
        let output = check(
            &["module app; agent Private() -> i32 { return 1; }"],
            &retain,
            &export,
        );
        assert!(output.checked.is_none());
        let diagnostic = output
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.code
                    == DiagnosticCode::Type(TypeDiagnosticCode::InvalidReflectionConfiguration)
            })
            .unwrap();
        let source = output
            .sources
            .as_ref()
            .unwrap()
            .files
            .iter()
            .find(|source| source.id == diagnostic.primary.span.source)
            .unwrap();
        assert_eq!(source.kind, SourceKind::VirtualGenerated);
        assert!(source.text.contains("reflection body entry:"));
        assert!(diagnostic.primary.span.range.end.to_usize() <= source.text.len());
    }
}

#[test]
fn source_views_and_frozen_environment_outlive_the_check_output() {
    let output = check(
        &[
            "module app; agent Worker(x: i32) -> i32 { let previous_local = x; return previous_local; } flow inspect() -> unit { let caller_local = 9; let a = ^Worker; return; }",
        ],
        &["app.Worker"],
        &[],
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.as_ref().unwrap();
    let catalog = catalog(checked);
    let expression = reflections(checked)[0];
    let frozen = catalog.freeze_agent(expression).unwrap();
    assert!(Arc::ptr_eq(frozen.checked(), checked));
    let scope = frozen.declaration_scope().unwrap();
    assert!(frozen.checked().scopes.lookup_local(scope, "x").is_some());
    assert!(
        frozen
            .checked()
            .scopes
            .lookup_local(scope, "caller_local")
            .is_none()
    );
    assert!(
        frozen
            .checked()
            .scopes
            .lookup_local(scope, "previous_local")
            .is_none()
    );
    let syntax = NativeCodeSyntaxOperations::default();
    let body = catalog.retained_body(expression, &syntax).unwrap().unwrap();
    drop(output);
    drop(catalog);
    assert!(syntax.format(&body).unwrap().contains("previous_local"));
    assert!(
        frozen
            .checked()
            .definition_sources
            .source(SourceId(1))
            .is_some()
    );
}

#[test]
fn bare_agents_and_unretained_auxiliary_methods_do_not_invent_a_body() {
    let output = check(
        &[
            "module app; agent Worker; impl agent Worker { flow explain() -> i32 { return 42; } } flow inspect() -> unit { let a = ^Worker; let b = ^Worker.explain; return; }",
        ],
        &["app.Worker"],
        &[],
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let catalog = catalog(&checked);
    let syntax = NativeCodeSyntaxOperations::default();
    for expression in reflections(&checked) {
        assert!(
            catalog
                .retained_body(expression, &syntax)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn public_reexport_retention_resolves_through_the_existing_module_catalog() {
    let output = check(
        &[
            "module app.lib; public agent Worker() -> i32 { return 42; }",
            "module app.api; public import app.lib.Worker as Selected;",
            "module app.main; import app.api.Selected; flow inspect() -> unit { let a = ^Selected; return; }",
        ],
        &["app.api.Selected"],
        &[],
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let syntax = NativeCodeSyntaxOperations::default();
    let body = catalog(&checked)
        .retained_body(reflections(&checked)[0], &syntax)
        .unwrap()
        .unwrap();
    assert_eq!(syntax.format(&body).unwrap(), "return 42;");
}

#[test]
fn a_consumer_cannot_authorize_dependency_overlay_body_retention() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        "module app; flow inspect() -> unit { return; }",
    ));
    input.sources.push(SourceInput {
        id: SourceId(2),
        path: None,
        text: "module dep; public agent Worker() -> i32 { return 42; }".into(),
        kind: SourceKind::DependencySourceOverlay {
            package: etas_frontend::ExternalPackageId(9),
            import_root: "dep".into(),
        },
    });
    input.options.entry_policy = EntryPolicy::Optional;
    input
        .options
        .reflection
        .retain_bodies
        .push("dep.Worker".into());
    input
        .environment
        .external_packages
        .push(etas_frontend::ProjectExternalPackageInput {
            id: etas_frontend::ExternalPackageId(9),
            name: "dep".into(),
            version: "0.1.0".into(),
            edition: "2026".into(),
            import_root: "dep".into(),
        });
    let output = Frontend.check_project(input);
    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Type(TypeDiagnosticCode::InvalidReflectionConfiguration)
            && diagnostic.message.contains("another package")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn frozen_default_uses_its_implementation_module_not_the_nominal_declaration_module() {
    let output = check(
        &[
            "module app.component; public agent Worker;",
            "module app.implementation; import app.component.Worker; impl agent Worker { public flow run(value: i32) -> i32 { return value; } }",
            "module app.main; import app.component.Worker; flow inspect() -> unit { let a = ^Worker; return; }",
        ],
        &["app.component.Worker"],
        &[],
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let frozen = catalog(&checked)
        .freeze_agent(reflections(&checked)[0])
        .unwrap();
    let module = checked
        .modules_arena
        .get(frozen.definition_module())
        .unwrap();
    let path = module
        .name
        .as_ref()
        .unwrap()
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(path, ["app", "implementation"]);
}

#[test]
fn frozen_environment_identity_ignores_session_arena_order() {
    let agent = "module app.lib; public agent Worker() -> i32 { return 42; }";
    let caller = "module app.main; import app.lib.Worker; flow inspect() -> unit { let a = ^Worker; return; }";
    let fingerprint = |sources: &[&str]| {
        let output = check(sources, &["app.lib.Worker"], &[]);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let checked = output.checked.unwrap();
        catalog(&checked)
            .freeze_agent(reflections(&checked)[0])
            .unwrap()
            .fingerprint()
    };
    assert_eq!(fingerprint(&[agent, caller]), fingerprint(&[caller, agent]));
}

#[test]
fn spec_selected_and_primary_views_share_the_actual_body_lease() {
    let output = check(
        &[
            "module app; spec Stage<I, O>: callable I => O ![]; agent Worker; impl agent Worker ~ Stage<i32, i32> { flow run(value: i32) -> i32 { return value; } } flow inspect() -> unit { let a = ^Worker; let b = ^Worker::Stage<i32, i32>.run; return; }",
        ],
        &["app.Worker"],
        &[],
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let catalog = catalog(&checked);
    let syntax = NativeCodeSyntaxOperations::default();
    let bodies = reflections(&checked)
        .into_iter()
        .map(|expr| catalog.retained_body(expr, &syntax).unwrap().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(bodies[0].identity(), bodies[1].identity());
}
