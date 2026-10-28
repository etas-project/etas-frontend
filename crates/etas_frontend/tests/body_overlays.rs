use std::{collections::BTreeMap, sync::Arc};

use etas_code::{
    CodeFingerprint, PackageKey, descriptor::DescriptorLimits, syntax::CodeSyntaxOperations,
};
use etas_core::{DiagnosticCode, SourceId, TypeDiagnosticCode};
use etas_frontend::{
    CheckedProgram, Frontend, ProjectInput, SourceInput,
    code::{
        definition::{DefinitionCatalog, FrozenDefinitionEnvironment},
        draft::{DefaultBodyOverlay, OverlayError},
        identity::DefinitionOrigins,
        service::NativeCodeSyntaxOperations,
    },
};
use etas_hir::{HirExpr, HirItem, HirProgram, HirProjectLowering, HirProjectModule};

fn freeze(source: &str) -> (CheckedProgram, Arc<FrozenDefinitionEnvironment>) {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = etas_frontend::EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let key = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.overlay.package",
        b"app",
    ));
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, key).unwrap(),
        standard_package: key,
        external_definitions: BTreeMap::new(),
    };
    let catalog =
        DefinitionCatalog::build(checked.clone(), &origins, DescriptorLimits::default()).unwrap();
    let expression = checked
        .hir
        .exprs
        .iter()
        .find_map(|(id, expr)| matches!(expr, HirExpr::Reflect(_)).then_some(id))
        .unwrap();
    let frozen = catalog.freeze_agent(expression).unwrap();
    (checked, frozen)
}

fn lower(overlay: &DefaultBodyOverlay) -> HirProgram {
    let checked = overlay.frozen().checked();
    let source = checked
        .definition_sources
        .source(overlay.slot().source)
        .unwrap();
    let parts = [&source.program.value];
    let modules = [HirProjectModule {
        parts: &parts,
        namespace: None,
    }];
    let mut lowering =
        HirProjectLowering::new_with_std_registry(&modules, checked.std_registry.clone());
    overlay.install(&mut lowering).unwrap();
    lowering.normalize_part(0, &source.program.value);
    lowering.lower_part(0, &source.program.value);
    lowering.finish().unwrap()
}

#[test]
fn overlay_preserves_headers_and_uses_only_definition_lexical_bindings() {
    let (checked, frozen) = freeze(
        "module app; flow helper(x: i32) -> i32 { return x + 1; } agent Worker(x: i32) -> i32 { let previous = 4; return x; } flow inspect() -> unit { let caller_local = 99; let d = ^Worker; return; }",
    );
    let syntax = NativeCodeSyntaxOperations::default();
    let overlay = DefaultBodyOverlay::prepare(
        frozen.clone(),
        syntax.parse_body("return helper(x);").unwrap(),
        &syntax,
    )
    .unwrap();
    assert!(Arc::ptr_eq(overlay.frozen(), &frozen));
    assert_eq!(overlay.relocated().source().id, SourceId(1));
    assert!(!overlay.exposure().retained);
    let hir = lower(&overlay);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    let types = etas_types::check_project(etas_types::CheckProjectRequest { hir: &hir });
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let agent = hir
        .items
        .iter()
        .find_map(|(_, item)| {
            if let HirItem::Agent(agent) = item {
                Some(agent)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(agent.params.len(), 1);
    assert_eq!(agent.span.source, SourceId(0));
    assert!(checked.hir.exprs.iter().any(|(_, expr)| matches!(expr, HirExpr::Literal(etas_hir::HirLiteral::Int { text, .. }) if text == "4")));
    for name in ["previous", "caller_local"] {
        let rejected = DefaultBodyOverlay::prepare(
            frozen.clone(),
            syntax.parse_body(&format!("return {name};")).unwrap(),
            &syntax,
        )
        .unwrap();
        let lowered = lower(&rejected);
        assert!(
            lowered
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.primary.span.source == SourceId(1)),
            "{:?}",
            lowered.diagnostics
        );
    }
}

#[test]
fn type_failures_point_at_the_registered_generated_body_not_the_old_declaration() {
    let (_, frozen) = freeze(
        "module app; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let syntax = NativeCodeSyntaxOperations::default();
    let text = "\nreturn \"not an integer\";";
    let overlay =
        DefaultBodyOverlay::prepare(frozen, syntax.parse_body(text).unwrap(), &syntax).unwrap();
    let hir = lower(&overlay);
    let types = etas_types::check_project(etas_types::CheckProjectRequest { hir: &hir });
    let diagnostic = types
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
        })
        .unwrap();
    assert_eq!(
        diagnostic.primary.span.source,
        overlay.relocated().source().id
    );
    let span = diagnostic.primary.span;
    assert!(
        overlay
            .relocated()
            .source()
            .text
            .get(span.range.start.to_usize()..span.range.end.to_usize())
            .is_some()
    );
    assert_eq!(overlay.relocated().source().text(), text);
}

#[test]
fn explicit_default_method_is_the_only_overlaid_slot_and_bare_agents_have_none() {
    let (_, frozen) = freeze(
        "module app; agent Worker; impl agent Worker { flow run(x: i32) -> i32 { return x; } flow extra() -> i32 { return 7; } } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let syntax = NativeCodeSyntaxOperations::default();
    let overlay = DefaultBodyOverlay::prepare(
        frozen,
        syntax.parse_body("return x + 10;").unwrap(),
        &syntax,
    )
    .unwrap();
    assert_eq!(overlay.slot().impl_member, Some(0));
    let hir = lower(&overlay);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    assert!(hir.exprs.iter().any(|(_, expr)| matches!(expr, HirExpr::Literal(etas_hir::HirLiteral::Int { text, .. }) if text == "7")));
    assert_eq!(
        hir.exprs
            .iter()
            .filter(|(_, expr)| expr.span(&hir.blocks).source == SourceId(1))
            .count(),
        3
    );
    let (_, bare) =
        freeze("module app; agent Worker; flow inspect() -> unit { let d = ^Worker; return; }");
    assert!(matches!(
        DefaultBodyOverlay::prepare(bare, syntax.parse_body("return;").unwrap(), &syntax),
        Err(OverlayError::MissingDefaultEntry)
    ));
}

#[test]
fn replacing_body_does_not_reuse_the_old_trailing_handler() {
    let (_, frozen) = freeze(
        "module app; agent Worker; impl agent Worker { flow run() -> i32 { return 1; } with { Error<i32>.raise(_) => { finish 2; } } } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let syntax = NativeCodeSyntaxOperations::default();
    let overlay =
        DefaultBodyOverlay::prepare(frozen, syntax.parse_body("return 3;").unwrap(), &syntax)
            .unwrap();
    let hir = lower(&overlay);
    assert!(
        !hir.exprs
            .iter()
            .any(|(_, expr)| matches!(expr, HirExpr::Handle { .. }))
    );
    assert!(!hir.exprs.iter().any(|(_, expr)| matches!(expr, HirExpr::Literal(etas_hir::HirLiteral::Int { text, .. }) if text == "2")));
}

#[test]
fn body_overlay_installation_rejects_duplicate_or_unapplied_targets() {
    let (_, frozen) = freeze(
        "module app; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let syntax = NativeCodeSyntaxOperations::default();
    let overlay =
        DefaultBodyOverlay::prepare(frozen, syntax.parse_body("return 2;").unwrap(), &syntax)
            .unwrap();
    let checked = overlay.frozen().checked();
    let source = checked
        .definition_sources
        .source(overlay.slot().source)
        .unwrap();
    let parts = [&source.program.value];
    let modules = [HirProjectModule {
        parts: &parts,
        namespace: None,
    }];
    let mut lowering =
        HirProjectLowering::new_with_std_registry(&modules, checked.std_registry.clone());
    overlay.install(&mut lowering).unwrap();
    assert!(matches!(
        overlay.install(&mut lowering),
        Err(etas_hir::lower::BodyOverlayError::DuplicateCallable(_))
    ));
    assert!(matches!(
        lowering.finish(),
        Err(etas_hir::lower::BodyOverlayError::UnappliedCallable(_))
    ));
}
