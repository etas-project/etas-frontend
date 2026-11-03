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
fn direct_generic_reflection_rejects_an_unresolved_phantom_parameter() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        r#"module app;
import std.compiler.CallableDef;
flow normalize<T>(value: i32) -> i32 ![] { return value; }
flow prepare() -> unit { normalize<bool>(1); normalize<string>(1); return; }
flow inspect() -> CallableDef<(i32) -> i32 ![]> { return ^normalize; }
"#,
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("generic callable value has an unresolved type argument")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn direct_generic_reflection_rejects_a_forged_type_application() {
    let (store, expr) = definitions_from(
        r#"module app;
import std.compiler.CallableDef;
flow normalize<T>(value: T) -> T ![] { return value; }
flow prepare() -> unit { normalize<bool>(true); normalize<string>("value"); return; }
flow inspect() -> CallableDef<(bool) -> bool ![]> { return ^normalize; }
"#,
    );
    let checked = store.catalog().checked_program();
    let owner = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if checked
                    .hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "normalize") =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    for (ty, accepted) in [
        (etas_types::PrimitiveType::Bool, true),
        (etas_types::PrimitiveType::String, false),
    ] {
        let ty = checked
            .type_store
            .iter()
            .find_map(|(id, value)| (value == &etas_types::Type::Primitive(ty)).then_some(id))
            .unwrap();
        let application = checked
            .checked_callable_application(owner, &[("T".into(), ty)], &[])
            .unwrap();
        assert_eq!(
            store
                .reflect_callable_application(expr, &application, Default::default())
                .is_ok(),
            accepted,
        );
    }
}

#[test]
fn generic_reflection_requires_the_matching_checked_lexical_application() {
    let (store, expr) = definitions_from(
        r#"module app;
import std.compiler.CallableDef;
flow normalize<T>(value: T) -> T ![] { return value; }
flow outer<U>() -> CallableDef<(U) -> U ![]> { return ^normalize; }
flow prepare() -> unit { outer<bool>(); outer<string>(); return; }
"#,
    );
    let checked = store.catalog().checked_program();
    let item = |name: &str| {
        checked
            .hir
            .items
            .iter()
            .find_map(|(id, item)| match item {
                etas_hir::HirItem::Flow(flow)
                    if checked
                        .hir
                        .symbols
                        .get(flow.symbol)
                        .is_some_and(|symbol| symbol.name == name) =>
                {
                    Some(id)
                }
                _ => None,
            })
            .unwrap()
    };
    let primitive = |primitive| {
        checked
            .type_store
            .iter()
            .find_map(|(id, ty)| (ty == &etas_types::Type::Primitive(primitive)).then_some(id))
            .unwrap()
    };
    let boolean = primitive(etas_types::PrimitiveType::Bool);
    let string = primitive(etas_types::PrimitiveType::String);
    let target = checked
        .checked_callable_application(item("normalize"), &[("T".into(), boolean)], &[])
        .unwrap();
    let lexical = checked
        .checked_callable_application(item("outer"), &[("U".into(), boolean)], &[])
        .unwrap();
    let other = checked
        .checked_callable_application(item("outer"), &[("U".into(), string)], &[])
        .unwrap();
    assert!(
        store
            .reflect_callable_application_in_context(
                expr,
                &target,
                Some(&lexical),
                Default::default()
            )
            .is_ok()
    );
    assert!(
        store
            .reflect_callable_application_in_context(
                expr,
                &target,
                Some(&other),
                Default::default()
            )
            .is_err()
    );
    assert!(
        store
            .reflect_callable_application_in_context(expr, &target, None, Default::default())
            .is_err()
    );
}

#[test]
fn flow_candidate_with_unknown_trace_selector_is_not_accepted() {
    for constraint in [
        "-Gate.request<Restricted>",
        "-Gate.request<_> | -Gate.request<Restricted>",
    ] {
        let (store, expr) = definitions_from(&format!(
            r#"module app;
type Restricted;
effect Gate {{ action request<T>() -> unit; }}
spec Safe: trace = {constraint};
flow normalize() -> unit ![Gate.request<_>] ~ Safe {{ return; }}
flow inspect() -> unit {{ let definition = ^normalize; return; }}"#,
        ));
        let original = store.reflect_flow(expr).unwrap();
        let draft = store
            .with_body(
                &original,
                store
                    .syntax_operations()
                    .parse_body("perform Gate.request(); return;")
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
        assert!(
            analysis.output().diagnostics.iter().any(|diagnostic| {
                diagnostic.code
                    == etas_core::DiagnosticCode::Effect(
                        etas_core::EffectDiagnosticCode::IncompleteEffectFacts,
                    )
            }),
            "candidate must report an unproved selector: {:?}",
            analysis.output().diagnostics
        );
        assert!(analysis.into_checked_candidate(Default::default()).is_err());
    }
}

#[test]
fn flow_candidate_cannot_hide_callback_requests_behind_an_empty_effect_row() {
    let (store, expr) = definitions_from(
        r#"module app;
spec Safe: trace = -Console.stdout_write;
flow normalize(callback: () -> unit ![]) -> unit ![] ~ Safe { return; }
flow inspect() -> unit { let definition = ^normalize; return; }"#,
    );
    let original = store.reflect_flow(expr).unwrap();
    let draft = store
        .with_body(
            &original,
            store
                .syntax_operations()
                .parse_body("callback(); return;")
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
    assert!(
        analysis.output().diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == etas_core::DiagnosticCode::Effect(
                    etas_core::EffectDiagnosticCode::IncompleteEffectFacts,
                )
        }),
        "candidate callback requests are not proven empty: {:?}",
        analysis.output().diagnostics
    );
    assert!(analysis.into_checked_candidate(Default::default()).is_err());
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
            .any(|diagnostic| diagnostic.code == "code::ActionTraceNotContained"),
        "{report:?}"
    );
}

#[test]
fn callback_effectful_handler_preserves_arm_trace_order() {
    let (store, expr) = definitions_from(
        r#"module app;
import std.io.{println, eprintln};
effect Choice { action decide() -> unit; }
flow normalize(f: () -> unit ![Choice.decide]) -> unit ![] {
    handle { f(); } with { Choice.decide() => { println("a")?; eprintln("b")?; resume; } };
    return;
}
flow inspect() -> unit { let definition = ^normalize; return; }"#,
    );
    let original = store.reflect_flow(expr).unwrap();
    let report_for = |source| {
        let body = store.syntax_operations().parse_body(source).unwrap();
        let draft = store.with_body(&original, body).unwrap();
        let overlay = store
            .resolve(&draft)
            .unwrap()
            .body_overlay(store.syntax_operations())
            .unwrap();
        FrontendSession::new()
            .analyze_body_overlay(overlay)
            .unwrap()
            .check_report(Default::default())
            .unwrap()
    };
    let same = report_for(
        "handle { f(); } with { Choice.decide() => { println(\"a\")?; eprintln(\"b\")?; resume; } }; return;",
    );
    assert_eq!(same.status, CheckStatus::Accepted, "{same:?}");
    let reordered = report_for(
        "handle { f(); } with { Choice.decide() => { eprintln(\"b\")?; println(\"a\")?; resume; } }; return;",
    );
    assert_ne!(reordered.status, CheckStatus::Accepted, "{reordered:?}");
    assert!(
        reordered
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "code::ActionTraceNotContained"),
        "{reordered:?}"
    );
}

#[test]
fn callback_handler_contract_preserves_mixed_resume_and_finish_arms() {
    let (store, expr) = definitions_from(
        r#"module app;
flow normalize(f: () -> unit ![]) -> unit ![] {
    handle { f(); } with {
        Console.stdout_write(_) => { finish (); }
        Console.stderr_write(_) => { resume; }
    };
    return;
}
flow inspect() -> unit { let definition = ^normalize; return; }"#,
    );
    let original = store.reflect_flow(expr).unwrap();
    let report_for = |source| {
        let body = store.syntax_operations().parse_body(source).unwrap();
        let draft = store.with_body(&original, body).unwrap();
        let overlay = store
            .resolve(&draft)
            .unwrap()
            .body_overlay(store.syntax_operations())
            .unwrap();
        FrontendSession::new()
            .analyze_body_overlay(overlay)
            .unwrap()
            .check_report(Default::default())
            .unwrap()
    };
    let same = report_for(
        "handle { f(); } with { Console.stdout_write(_) => { finish (); } Console.stderr_write(_) => { resume; } }; return;",
    );
    assert_eq!(same.status, CheckStatus::Accepted, "{same:?}");
    let changed = report_for(
        "handle { f(); } with { Console.stdout_write(_) => { resume; } Console.stderr_write(_) => { finish (); } }; return;",
    );
    assert_ne!(changed.status, CheckStatus::Accepted, "{changed:?}");
    assert!(
        changed
            .diagnostics
            .iter()
            .any(|d| d.code == "code::ActionTraceNotContained"),
        "{changed:?}"
    );
}

#[test]
fn callback_finish_contract_preserves_control_and_rejects_skipped_continuation() {
    let (store, expr) = definitions_from(
        r#"module app;
import std.io.println;
flow normalize(f: () -> unit ![]) -> unit ![] {
    handle { f(); } with { Console.stdout_write(_) => { finish (); } };
    return;
}
flow inspect() -> unit { let definition = ^normalize; return; }"#,
    );
    let original = store.reflect_flow(expr).unwrap();
    let report_for = |source| {
        let body = store.syntax_operations().parse_body(source).unwrap();
        let draft = store.with_body(&original, body).unwrap();
        let overlay = store
            .resolve(&draft)
            .unwrap()
            .body_overlay(store.syntax_operations())
            .unwrap();
        FrontendSession::new()
            .analyze_body_overlay(overlay)
            .unwrap()
            .check_report(Default::default())
            .unwrap()
    };
    let unchanged =
        report_for("handle { f(); } with { Console.stdout_write(_) => { finish (); } }; return;");
    assert_eq!(unchanged.status, CheckStatus::Accepted, "{unchanged:?}");
    let changed =
        report_for("handle { f(); } with { Console.stdout_write(_) => { resume; } }; return;");
    assert_ne!(changed.status, CheckStatus::Accepted, "{changed:?}");
    assert!(
        changed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "code::ActionTraceNotContained"),
        "{changed:?}"
    );
    let skipped = report_for(
        "handle { f(); println(\"skipped\")?; } with { Console.stdout_write(_) => { finish (); } }; return;",
    );
    assert_ne!(skipped.status, CheckStatus::Accepted, "{skipped:?}");
}

#[test]
fn changing_the_intercepted_action_changes_the_frozen_callback_contract() {
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
                    "handle { f(); } with { Console.stderr_write(_) => { resume; } }; return;",
                )
                .unwrap(),
        )
        .unwrap();
    let overlay = store
        .resolve(&draft)
        .unwrap()
        .body_overlay(store.syntax_operations())
        .unwrap();
    let report = FrontendSession::new()
        .analyze_body_overlay(overlay)
        .unwrap()
        .check_report(Default::default())
        .unwrap();
    assert_ne!(report.status, CheckStatus::Accepted, "{report:?}");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "code::ActionTraceNotContained"),
        "{report:?}"
    );
}

#[test]
fn changing_non_unit_resume_value_is_not_treated_as_a_unit_transformer() {
    let (store, expr) = definitions_from(
        r#"module app;
effect Choice { action decide() -> bool; }
flow normalize(f: () -> unit ![Choice.decide]) -> unit ![] {
    handle { f(); } with { Choice.decide() => { resume true; } };
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
                    "handle { f(); } with { Choice.decide() => { resume false; } }; return;",
                )
                .unwrap(),
        )
        .unwrap();
    let overlay = store
        .resolve(&draft)
        .unwrap()
        .body_overlay(store.syntax_operations())
        .unwrap();
    let report = FrontendSession::new()
        .analyze_body_overlay(overlay)
        .unwrap()
        .check_report(Default::default())
        .unwrap();
    assert_ne!(report.status, CheckStatus::Accepted, "{report:?}");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "code::ActionTraceNotContained"),
        "{report:?}"
    );
}

#[test]
fn branch_with_checked_completion_preserves_callback_trace_order() {
    let (store, expr) = definitions_from(
        r#"module app;
import std.io.{println, eprintln};
flow normalize(flag: bool, f: () -> unit ![]) -> unit ![] {
    if flag { println("a")?; } else { eprintln("b")?; }
    f();
    return;
}
flow inspect() -> unit { let definition = ^normalize; return; }"#,
    );
    let original = store.reflect_flow(expr).unwrap();
    let report_for = |source| {
        let body = store.syntax_operations().parse_body(source).unwrap();
        let draft = store.with_body(&original, body).unwrap();
        let overlay = store
            .resolve(&draft)
            .unwrap()
            .body_overlay(store.syntax_operations())
            .unwrap();
        FrontendSession::new()
            .analyze_body_overlay(overlay)
            .unwrap()
            .check_report(Default::default())
            .unwrap()
    };
    let unchanged =
        report_for("if flag { println(\"a\")?; } else { eprintln(\"b\")?; } f(); return;");
    assert_eq!(unchanged.status, CheckStatus::Accepted, "{unchanged:?}");
    let reordered =
        report_for("f(); if flag { println(\"a\")?; } else { eprintln(\"b\")?; } return;");
    assert_ne!(reordered.status, CheckStatus::Accepted, "{reordered:?}");
    assert!(
        reordered
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "code::ActionTraceNotContained"),
        "{reordered:?}"
    );
}

#[test]
fn branch_before_callback_does_not_discard_unproved_host_outcomes() {
    let (store, expr) = definitions_from(
        r#"module app;
flow normalize(flag: bool, f: () -> unit ![]) -> unit ![Console.stdout_write, Console.stderr_write] {
    if flag { perform Console.stdout_write("a"); } else { perform Console.stderr_write("b"); }
    f();
    return;
}
flow inspect() -> unit { let definition = ^normalize; return; }"#,
    );
    let original = store.reflect_flow(expr).unwrap();
    let body = store.syntax_operations().parse_body(
        "if flag { perform Console.stdout_write(\"a\"); } else { perform Console.stderr_write(\"b\"); } f(); return;",
    ).unwrap();
    let draft = store.with_body(&original, body).unwrap();
    let overlay = store
        .resolve(&draft)
        .unwrap()
        .body_overlay(store.syntax_operations())
        .unwrap();
    let report = FrontendSession::new()
        .analyze_body_overlay(overlay)
        .unwrap()
        .check_report(Default::default())
        .unwrap();
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
fn unchanged_boolean_resume_value_preserves_callback_contract() {
    let (store, expr) = definitions_from(
        r#"module app;
effect Choice { action decide() -> bool; }
flow normalize(f: () -> unit ![Choice.decide]) -> unit ![] {
    handle { f(); } with { Choice.decide() => { resume true; } };
    return;
}
flow inspect() -> unit { let definition = ^normalize; return; }"#,
    );
    let original = store.reflect_flow(expr).unwrap();
    let body = store
        .syntax_operations()
        .parse_body("handle { f(); } with { Choice.decide() => { resume true; } }; return;")
        .unwrap();
    let draft = store.with_body(&original, body).unwrap();
    let overlay = store
        .resolve(&draft)
        .unwrap()
        .body_overlay(store.syntax_operations())
        .unwrap();
    let report = FrontendSession::new()
        .analyze_body_overlay(overlay)
        .unwrap()
        .check_report(Default::default())
        .unwrap();
    assert_eq!(report.status, CheckStatus::Accepted, "{report:?}");
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
fn candidate_prepares_new_frozen_reflection_instances_without_mutating_source() {
    candidate_prepares_new_frozen_reflection_instances(true, false);
}

#[test]
fn candidate_prepares_unannotated_frozen_reflection_instances_without_mutating_source() {
    candidate_prepares_new_frozen_reflection_instances(false, false);
}

#[test]
fn candidate_prepares_unannotated_frozen_reflection_with_wider_interface() {
    candidate_prepares_new_frozen_reflection_instances(false, true);
}

fn candidate_prepares_new_frozen_reflection_instances(
    explicit_effects: bool,
    wider_interface: bool,
) {
    use etas_types::{PrimitiveType, Type};

    let source = r#"module app;
import std.compiler.CallableDef;
flow helper<T>(value: T) -> T ![] { return value; }
flow normalize<U>(value: U) -> U ![] { return value; }
flow inspect(selected: (bool) -> bool ![]) -> unit { let definition = ^selected; return; }
flow prepare() -> unit { normalize<bool>(true); return; }
"#;
    let source = if explicit_effects {
        source.to_owned()
    } else {
        source.replace(
            "flow helper<T>(value: T) -> T ![]",
            "flow helper<T>(value: T) -> T",
        )
    };
    let (store, expression) = definitions_from(&source);
    let source = store.catalog().checked_program();
    let item = |name: &str| {
        source
            .items
            .iter()
            .find_map(|(id, item)| match item {
                etas_hir::HirItem::Flow(flow)
                    if source.symbols.get(flow.symbol).unwrap().name == name =>
                {
                    Some(id)
                }
                _ => None,
            })
            .unwrap()
    };
    let boolean = source
        .type_store
        .iter()
        .find_map(|(id, ty)| (ty == &Type::Primitive(PrimitiveType::Bool)).then_some(id))
        .unwrap();
    let helper = item("helper");
    let bindings = [("T".into(), boolean)];
    assert!(
        source
            .checked_callable_application(helper, &bindings, &[])
            .is_err()
    );
    let selected = source
        .checked_callable_application(item("normalize"), &[("U".into(), boolean)], &[])
        .unwrap();
    let original = store
        .reflect_callable_application(expression, &selected, Default::default())
        .unwrap();
    let body_source = if wider_interface {
        "let definition: CallableDef<(U) -> U ![Console.stdout_write]> = ^helper; return value;"
    } else {
        "let definition: CallableDef<(U) -> U ![]> = ^helper; return value;"
    };
    let body = store.syntax_operations().parse_body(body_source).unwrap();
    let draft = store.with_body(&original, body).unwrap();
    let overlay = store
        .resolve(&draft)
        .unwrap()
        .body_overlay(store.syntax_operations())
        .unwrap();
    let candidate = FrontendSession::new()
        .compile_body_overlay(overlay)
        .unwrap();
    assert!(
        source
            .checked_callable_application(helper, &bindings, &[])
            .is_err()
    );
    let catalog = candidate.definition_catalog(Default::default()).unwrap();
    let frozen = etas_frontend::code::definition::FrozenReflectionBindings::build(
        store.clone(),
        &candidate,
        &catalog,
    )
    .unwrap();
    let candidate_store = NativeDefinitionStore::new(
        catalog,
        store.syntax_operations().clone(),
        Default::default(),
    );
    let program = candidate.program();
    let candidate_helper = program
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if program.symbols.get(flow.symbol).unwrap().name == "helper" =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    let candidate_boolean = program
        .type_store
        .iter()
        .find_map(|(id, ty)| (ty == &Type::Primitive(PrimitiveType::Bool)).then_some(id))
        .unwrap();
    let target = program
        .checked_callable_application(candidate_helper, &[("T".into(), candidate_boolean)], &[])
        .unwrap();
    let site = program
        .checked_reflections()
        .iter()
        .find_map(|(expr, fact)| {
            (program.hir.index().expr_item.get(expr) == Some(&candidate.entry())
                && matches!(
                    fact.target,
                    etas_types::CheckedReflectionTarget::Callable { .. }
                ))
            .then_some(*expr)
        })
        .unwrap();
    let (backend, definition) = frozen
        .reflect_callable_application(&candidate_store, site, &target, None, Default::default())
        .unwrap();
    assert!(backend.resolve(&definition).is_ok());
    assert!(!Arc::ptr_eq(source, backend.catalog().checked_program()));
    assert!(
        source
            .checked_callable_application(helper, &bindings, &[])
            .is_err()
    );
    assert!(
        backend
            .catalog()
            .checked_program()
            .checked_callable_application(helper, &bindings, &[])
            .is_ok()
    );
}

#[test]
fn frozen_generic_reflection_rejects_foreign_or_mismatched_applications() {
    use etas_frontend::code::definition::FrozenReflectionBindings;
    use etas_types::{PrimitiveType, Type};

    let (source, expression) = definitions_from(
        r#"module app;
import std.compiler.CallableDef;
flow helper<T>(value: T) -> T ![] { return value; }
flow normalize<U>(value: U) -> U ![] { return value; }
flow inspect(selected: (bool) -> bool ![]) -> unit { let definition = ^selected; return; }
flow prepare() -> unit { normalize<bool>(true); helper<bool>(true); helper<string>("x"); return; }
"#,
    );
    let checked = source.catalog().checked_program();
    let item = |name: &str| {
        checked
            .items
            .iter()
            .find_map(|(id, item)| match item {
                etas_hir::HirItem::Flow(flow)
                    if checked.symbols.get(flow.symbol).unwrap().name == name =>
                {
                    Some(id)
                }
                _ => None,
            })
            .unwrap()
    };
    let boolean = checked
        .type_store
        .iter()
        .find_map(|(id, ty)| (ty == &Type::Primitive(PrimitiveType::Bool)).then_some(id))
        .unwrap();
    let selected = checked
        .checked_callable_application(item("normalize"), &[("U".into(), boolean)], &[])
        .unwrap();
    let original = source
        .reflect_callable_application(expression, &selected, Default::default())
        .unwrap();
    let body = source
        .syntax_operations()
        .parse_body("let definition: CallableDef<(U) -> U ![]> = ^helper; return value;")
        .unwrap();
    let draft = source.with_body(&original, body).unwrap();
    let overlay = source
        .resolve(&draft)
        .unwrap()
        .body_overlay(source.syntax_operations())
        .unwrap();
    let candidate = FrontendSession::new()
        .compile_body_overlay(overlay)
        .unwrap();
    let catalog = candidate.definition_catalog(Default::default()).unwrap();
    let frozen = FrozenReflectionBindings::build(source.clone(), &candidate, &catalog).unwrap();
    let candidate_store = NativeDefinitionStore::new(
        catalog.clone(),
        source.syntax_operations().clone(),
        Default::default(),
    );
    let program = candidate.program();
    let helper = program
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if program.symbols.get(flow.symbol).unwrap().name == "helper" =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    let primitive = |kind| {
        program
            .type_store
            .iter()
            .find_map(|(id, ty)| (ty == &Type::Primitive(kind)).then_some(id))
            .unwrap()
    };
    let boolean = primitive(PrimitiveType::Bool);
    let string = primitive(PrimitiveType::String);
    let target = program
        .checked_callable_application(helper, &[("T".into(), boolean)], &[])
        .unwrap();
    let wrong = program
        .checked_callable_application(helper, &[("T".into(), string)], &[])
        .unwrap();
    let lexical = program
        .checked_callable_application(candidate.entry(), &[("U".into(), boolean)], &[])
        .unwrap();
    let expression = program
        .checked_reflections()
        .iter()
        .find_map(|(expr, fact)| {
            (program.hir.index().expr_item.get(expr) == Some(&candidate.entry())
                && matches!(
                    fact.target,
                    etas_types::CheckedReflectionTarget::Callable { .. }
                ))
            .then_some(*expr)
        })
        .unwrap();
    assert!(
        frozen
            .reflect_callable_application(
                &candidate_store,
                expression,
                &target,
                Some(&lexical),
                Default::default()
            )
            .is_ok()
    );
    assert!(
        frozen
            .reflect_callable_application(
                &candidate_store,
                expression,
                &wrong,
                Some(&lexical),
                Default::default()
            )
            .is_err()
    );
    assert!(
        frozen
            .reflect_callable_application(
                &candidate_store,
                expression,
                &target,
                None,
                Default::default()
            )
            .is_ok()
    );
    assert!(
        frozen
            .reflect_callable_application(
                &source,
                expression,
                &target,
                Some(&lexical),
                Default::default()
            )
            .is_err()
    );
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
