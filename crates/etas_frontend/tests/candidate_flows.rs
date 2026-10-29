use std::{collections::BTreeMap, sync::Arc};

use etas_code::{
    CodeFingerprint, DefinitionKind, PackageKey, definition::CodeDefinitionOperations,
    report::CheckStatus, syntax::CodeSyntaxOperations,
};
use etas_frontend::{
    EntryPolicy, Frontend, FrontendSession, ProjectInput, SourceInput,
    code::{
        definition::{DefinitionCatalog, NativeDefinitionStore, ReflectionOptions},
        draft::DefaultBodyOverlay,
        identity::DefinitionOrigins,
        service::NativeCodeSyntaxOperations,
    },
};

fn definitions() -> (Arc<NativeDefinitionStore>, etas_hir::HirExprId) {
    definitions_from(
        r#"
module app;
flow normalize(text: string) -> string ![] { return std.text.trim(text); }
flow inspect() -> unit { let definition = ^normalize; return; }
"#,
    )
}

fn definitions_from(source: &str) -> (Arc<NativeDefinitionStore>, etas_hir::HirExprId) {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    input.options.reflection = ReflectionOptions {
        retain_bodies: vec!["app.normalize".into()],
        export_bodies: vec![],
    };
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let expression = *checked.types.reflections.keys().next().unwrap();
    let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.flow-candidates",
        b"app",
    ));
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
        standard_package: package,
        external_definitions: BTreeMap::new(),
    };
    let catalog =
        Arc::new(DefinitionCatalog::build(checked, &origins, Default::default()).unwrap());
    (
        Arc::new(NativeDefinitionStore::new(
            catalog,
            Arc::new(NativeCodeSyntaxOperations::default()),
            Default::default(),
        )),
        expression,
    )
}

#[test]
fn narrower_flow_analysis_is_accepted_without_replacing_its_frozen_contract() {
    let (store, expr) = definitions_from(
        r#"
module app;
import std.io.println;
flow normalize(write: bool) -> unit ![] {
    if write { println("optional")?; }
    return;
}

flow inspect() -> unit { let definition = ^normalize; return; }
"#,
    );
    let original = store.reflect_flow(expr).unwrap();
    let frozen = store.catalog().freeze_flow(expr).unwrap();
    let fingerprint = frozen.contract().fingerprint();
    let draft = store
        .with_body(
            &original,
            store.syntax_operations().parse_body("return;").unwrap(),
        )
        .unwrap();
    let overlay = store
        .resolve(&draft)
        .unwrap()
        .body_overlay(store.syntax_operations())
        .unwrap();
    let analysis = FrontendSession::new()
        .analyze_body_overlay(overlay)
        .unwrap();
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(
        report.status,
        CheckStatus::Accepted,
        "{:?}",
        report.diagnostics
    );
    let candidate = analysis.into_checked_candidate(Default::default()).unwrap();
    assert_eq!(candidate.frozen().contract().fingerprint(), fingerprint);
    let summary = &candidate.program().effects.item_effects[&candidate.entry()];
    assert!(summary.requested_actions.is_empty());
    let revision = store
        .reflect_checked(&draft, &candidate, Default::default())
        .unwrap();
    let view = store.resolve(&revision).unwrap();
    assert!(view.current_analysis().is_ok());
    assert_eq!(
        view.environment().unwrap().contract().fingerprint(),
        fingerprint
    );
}

#[test]
fn unchanged_flow_with_an_intercepted_callback_can_be_recompiled() {
    let body = r#"handle { f(); } with { Console.stdout_write(_) => { resume; } }; return;"#;
    let (store, expr) = definitions_from(
        r#"module app;
flow normalize(f: () -> unit ![Console.stdout_write]) -> unit ![] {
    handle { f(); } with { Console.stdout_write(_) => { resume; } };
    return;
}
flow inspect() -> unit { let definition = ^normalize; return; }"#,
    );
    let original = store.reflect_flow(expr).unwrap();
    let draft = store
        .with_body(
            &original,
            store.syntax_operations().parse_body(body).unwrap(),
        )
        .unwrap();
    let overlay = store
        .resolve(&draft)
        .unwrap()
        .body_overlay(store.syntax_operations())
        .unwrap();
    let analysis = FrontendSession::new()
        .analyze_body_overlay(overlay)
        .unwrap();
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(
        report.status,
        CheckStatus::Accepted,
        "{:?}",
        report.diagnostics
    );
    analysis.into_checked_candidate(Default::default()).unwrap();
}

#[test]
fn callback_with_empty_escaping_row_can_still_be_intercepted_by_handler() {
    let (store, expr) = definitions_from(
        r#"module app;
flow normalize(f: () -> unit ![]) -> unit ![] {
    handle { f(); } with { Console.stdout_write(_) => { resume; } };
    return;
}
flow inspect() -> unit { let definition = ^normalize; return; }"#,
    );
    let original = store.reflect_flow(expr).unwrap();
    let draft = store
        .with_body(
            &original,
            store
                .syntax_operations()
                .parse_body(
                    "handle { f(); } with { Console.stdout_write(_) => { finish (); } }; return;",
                )
                .unwrap(),
        )
        .unwrap();
    let overlay = store
        .resolve(&draft)
        .unwrap()
        .body_overlay(store.syntax_operations())
        .unwrap();
    let analysis = FrontendSession::new()
        .analyze_body_overlay(overlay)
        .unwrap();
    let report = analysis.check_report(Default::default()).unwrap();
    assert_ne!(report.status, CheckStatus::Accepted, "{report:?}");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "code::MissingOutcomeSensitiveSummary"),
        "{report:?}"
    );
}

#[test]
fn flow_edit_preserves_callable_identity_without_becoming_an_agent() {
    let (store, expr) = definitions();
    let original = store.reflect_flow(expr).unwrap();
    assert_eq!(original.identity().kind(), DefinitionKind::Flow);
    assert_eq!(
        original.revision().inputs().definition.as_declaration(),
        Some(original.identity())
    );
    let view = store.resolve(&original).unwrap();
    assert!(view.signature().is_some());
    assert!(view.declared_tools().is_err());
    let body = store
        .syntax_operations()
        .parse_body("let cleaned = std.text.trim(text); return cleaned;")
        .unwrap();
    let draft = store.with_body(&original, body).unwrap();
    assert_eq!(draft.identity(), original.identity());
    assert_eq!(
        draft.revision().inputs().parent,
        Some(original.revision().key())
    );
    assert_eq!(
        store.resolve(&original).unwrap().revision(),
        original.revision()
    );
    let overlay = store
        .resolve(&draft)
        .unwrap()
        .body_overlay(store.syntax_operations())
        .unwrap();
    let analysis = FrontendSession::new()
        .analyze_body_overlay(overlay)
        .unwrap();
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(
        report.status,
        CheckStatus::Accepted,
        "{:?}",
        report.diagnostics
    );
    let candidate = analysis.into_checked_candidate(Default::default()).unwrap();
    assert_eq!(candidate.identity().kind(), DefinitionKind::Flow);
    assert!(matches!(
        candidate.program().hir.items[candidate.entry()],
        etas_hir::HirItem::Flow(_)
    ));
    assert_eq!(candidate.frozen().contract().callables().count(), 1);
    assert!(candidate.frozen().contract().agent_header().is_none());
    let revised = store
        .reflect_checked(&draft, &candidate, Default::default())
        .unwrap();
    assert_eq!(revised.revision(), candidate.revision());
    assert!(store.resolve(&revised).unwrap().current_analysis().is_ok());
}

#[test]
fn instantiated_flow_candidate_checks_body_under_bound_type() {
    use etas_types::{PrimitiveType, Type};

    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app;
flow identity<T>(value: T) -> T ![] { return value; }
flow inspect(selected: (bool) -> bool ![]) -> unit { let definition = ^selected; return; }
flow main() -> unit { identity<bool>(true); return; }
"#,
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    input.options.reflection = ReflectionOptions {
        retain_bodies: vec!["app.identity".into()],
        export_bodies: vec![],
    };
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let owner = checked
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if checked.symbols.get(flow.symbol).unwrap().name == "identity" =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    let boolean = checked
        .type_store
        .iter()
        .find_map(|(id, ty)| (ty == &Type::Primitive(PrimitiveType::Bool)).then_some(id))
        .unwrap();
    let application = checked
        .checked_callable_application(owner, &[("T".into(), boolean)], &[])
        .unwrap();
    let expression = *checked.types.reflections.keys().next().unwrap();
    let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.flow-candidates",
        b"app",
    ));
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
        standard_package: package,
        external_definitions: BTreeMap::new(),
    };
    let catalog =
        Arc::new(DefinitionCatalog::build(checked, &origins, Default::default()).unwrap());
    let store = NativeDefinitionStore::new(
        catalog,
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    );
    let original = store
        .reflect_callable_application(expression, &application, Default::default())
        .unwrap();
    let draft = store
        .with_body(
            &original,
            store
                .syntax_operations()
                .parse_body("return !value;")
                .unwrap(),
        )
        .unwrap();
    let overlay = store
        .resolve(&draft)
        .unwrap()
        .body_overlay(store.syntax_operations())
        .unwrap();
    let analysis = FrontendSession::new()
        .analyze_body_overlay(overlay)
        .unwrap();
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(
        report.status,
        CheckStatus::Accepted,
        "{:?}",
        report.diagnostics
    );
    assert!(analysis.into_checked_candidate(Default::default()).is_ok());
    let invalid = store
        .with_body(
            &original,
            store.syntax_operations().parse_body("return 42;").unwrap(),
        )
        .unwrap();
    let overlay = store
        .resolve(&invalid)
        .unwrap()
        .body_overlay(store.syntax_operations())
        .unwrap();
    let report = FrontendSession::new()
        .analyze_body_overlay(overlay)
        .unwrap()
        .check_report(Default::default())
        .unwrap();
    assert_eq!(report.status, CheckStatus::Rejected);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "type::TypeMismatch")
    );
}

#[test]
fn flow_candidate_rejects_output_changes_and_additional_handled_requests() {
    let (store, expr) = definitions();
    let frozen = store.catalog().freeze_flow(expr).unwrap();
    for body in [
        "return 42;",
        "println(\"extra hidden action\")?; return text;",
    ] {
        let overlay = DefaultBodyOverlay::prepare(
            frozen.clone(),
            store.syntax_operations().parse_body(body).unwrap(),
            store.syntax_operations(),
        )
        .unwrap();
        let analysis = FrontendSession::new()
            .analyze_body_overlay(overlay)
            .unwrap();
        let report = analysis.check_report(Default::default()).unwrap();
        assert_eq!(
            report.status,
            CheckStatus::Rejected,
            "{body}: {:?}",
            report.diagnostics
        );
        assert!(analysis.into_checked_candidate(Default::default()).is_err());
    }
}

#[test]
fn repeated_flow_reflection_reuses_the_same_immutable_source_lease() {
    let (store, expr) = definitions();
    let first = store.reflect_flow(expr).unwrap();
    let second = store.reflect_flow(expr).unwrap();
    assert!(first.is_same_object(&second));
}

#[test]
fn scoped_flow_bindings_reject_foreign_catalogs_and_the_edited_body_slot() {
    use etas_frontend::code::definition::{FrozenReflectionBindings, NativeDefinitionError};

    let (store, expr) = definitions_from(
        "module app; flow helper(text: string) -> string ![] { return text; } flow normalize(text: string) -> string ![] { return text; } flow inspect() -> unit { let definition = ^normalize; return; }",
    );
    let syntax = store.syntax_operations();
    let overlay = DefaultBodyOverlay::prepare(
        store.catalog().freeze_flow(expr).unwrap(),
        syntax
            .parse_body("let selected = helper; let definition = ^selected; return text;")
            .unwrap(),
        syntax,
    )
    .unwrap();
    let candidate = FrontendSession::new()
        .compile_body_overlay(overlay)
        .unwrap();
    let catalog = candidate.definition_catalog(Default::default()).unwrap();
    let bindings = FrozenReflectionBindings::build(store.clone(), &candidate, &catalog).unwrap();
    let (identity, helper) = catalog
        .native_callables()
        .find(|(id, _)| {
            id.as_declaration()
                .is_some_and(|id| id.declaration() == ["helper"])
        })
        .unwrap();
    let original = bindings.frozen_flow_item(&catalog, helper).unwrap();
    assert_eq!(store.catalog().item_owner(identity), Some(original));
    assert!(matches!(
        bindings.frozen_flow_item(store.catalog(), helper),
        Err(NativeDefinitionError::CandidateInputMismatch),
    ));
    assert!(matches!(
        bindings.frozen_flow_item(&catalog, candidate.entry()),
        Err(NativeDefinitionError::CandidateInputMismatch),
    ));
    let expression = candidate
        .program()
        .checked_reflections()
        .iter()
        .find_map(|(expr, fact)| {
            matches!(
                fact.target,
                etas_types::CheckedReflectionTarget::CallableValue { .. }
            )
            .then_some(*expr)
        })
        .unwrap();
    assert!(matches!(
        bindings.reflect_flow_binding(store.catalog(), expression, original),
        Err(NativeDefinitionError::CandidateInputMismatch),
    ));
    let reflected = bindings
        .reflect_flow_binding(&catalog, expression, original)
        .unwrap();
    assert_eq!(&reflected.header.id, identity);
    assert_eq!(
        reflected.definition.unwrap().identity(),
        identity.as_declaration().unwrap()
    );
}

#[test]
fn flow_source_roundtrip_requires_the_same_frozen_contract_and_recompilation() {
    let (store, expr) = definitions();
    let original = store.reflect_flow(expr).unwrap();
    let body = store
        .syntax_operations()
        .parse_body("let normalized = std.text.trim(text); return normalized;")
        .unwrap();
    let edited = store.with_body(&original, body).unwrap();
    let saved = store.save_source(&edited, Default::default()).unwrap();
    assert_eq!(saved.target.kind(), DefinitionKind::Flow);
    assert_eq!(
        saved.schema,
        etas_code::definition::DEFINITION_SOURCE_BUNDLE_SCHEMA
    );
    let restored = store
        .restore_source_draft(&original, &saved, Default::default())
        .unwrap();
    assert_eq!(restored.revision(), edited.revision());
    assert!(
        store
            .resolve(&restored)
            .unwrap()
            .checked_implementation()
            .is_none()
    );
    let overlay = store
        .resolve(&restored)
        .unwrap()
        .body_overlay(store.syntax_operations())
        .unwrap();
    let candidate = FrontendSession::new()
        .analyze_body_overlay(overlay)
        .unwrap()
        .into_checked_candidate(Default::default())
        .unwrap();
    assert_eq!(candidate.identity(), &saved.target);
    let mut wrong_target = saved.clone();
    wrong_target.target = etas_code::DefinitionKey::new(
        saved.target.package(),
        saved.target.module().to_vec(),
        vec!["other".into()],
        DefinitionKind::Flow,
    )
    .unwrap();
    assert!(
        store
            .restore_source_draft(&original, &wrong_target, Default::default())
            .is_err()
    );
    let mut legacy = saved;
    legacy.schema = 1;
    assert!(matches!(
        legacy.validate(Default::default()),
        Err(etas_code::definition::SourceBundleError::UnsupportedSchema(
            1
        ))
    ));
}
