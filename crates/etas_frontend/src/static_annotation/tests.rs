use super::*;
use crate::{EntryPolicy, Frontend, ProjectInput, SourceInput};

fn checked(source: &str) -> crate::CheckedProgram {
    let mut input = ProjectInput::single_source(SourceInput {
        id: etas_core::SourceId(17),
        path: None,
        text: source.into(),
        kind: crate::SourceKind::SourceProjectFile,
    });
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.unwrap()
}

#[test]
fn annotation_projection_rejects_missing_reference_identity_at_the_real_expression() {
    let checked = checked("module app; let label = \"value\"; @custom(label) agent Worker;");
    let annotation = checked
        .hir
        .item_annotations
        .values()
        .flatten()
        .next()
        .unwrap();
    let projector = AnnotationProjection::new(
        &checked,
        |_| -> Result<Vec<String>, String> { Err("missing canonical binding".into()) },
        Default::default(),
    );
    let error = projector.project(annotation).unwrap_err();
    assert_eq!(error.span.source, etas_core::SourceId(17));
    let HirAnnotationArg::Positional { value, .. } = annotation.args[0] else {
        panic!("argument")
    };
    assert_eq!(
        error.span,
        checked.hir.exprs[value].span(&checked.hir.blocks)
    );
    assert_eq!(error.reason, "missing canonical binding");
    assert_eq!(error.resource_limit, None);
}

#[test]
fn annotation_projection_reports_resource_limits_without_leaving_depth_state() {
    let checked =
        checked("module app; @custom([[[[[\"deep\"]]]]]) @other(\"valid\") agent Worker;");
    let annotations = checked.hir.item_annotations.values().next().unwrap();
    let projector = AnnotationProjection::new(
        &checked,
        |_| -> Result<Vec<String>, String> { Err("unexpected reference".into()) },
        DescriptorLimits {
            max_expansion_depth: 3,
            ..Default::default()
        },
    );
    let error = projector.project(&annotations[0]).unwrap_err();
    assert_eq!(error.span.source, etas_core::SourceId(17));
    assert!(error.reason.contains("depth limit"));
    assert_eq!(
        error.resource_limit,
        Some(("annotation expansion depth", 3))
    );
    assert_eq!(projector.depth.get(), 0);
    assert!(projector.project(&annotations[1]).is_ok());
    for (limits, resource, limit) in [
        (
            DescriptorLimits {
                max_nodes: 1,
                ..Default::default()
            },
            "annotation nodes",
            1,
        ),
        (
            DescriptorLimits {
                max_text_bytes: 8,
                ..Default::default()
            },
            "annotation text bytes",
            8,
        ),
    ] {
        let projector = AnnotationProjection::new(
            &checked,
            |_| -> Result<Vec<String>, String> { Err("unexpected reference".into()) },
            limits,
        );
        let error = projector.project(&annotations[0]).unwrap_err();
        assert_eq!(error.span.source, etas_core::SourceId(17));
        assert!(error.reason.contains("limit exceeded"));
        assert_eq!(error.resource_limit, Some((resource, limit)));
    }
}

#[test]
fn imported_handler_values_do_not_become_static_annotation_constants() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
    input.options.entry_policy = EntryPolicy::Optional;
    input.sources = [
        "module app.lib; public effect Gate { action request() -> i32; } public let H: ![Gate => []] = handler { Gate.request() => { resume 7; } };",
        "module app.main; import app.lib.H; @custom(H) agent Worker; flow main() -> unit { return; }",
    ].into_iter().enumerate().map(|(index, text)| SourceInput { id: etas_core::SourceId(index as u32 + 17), path: None, text: text.into(), kind: crate::SourceKind::SourceProjectFile }).collect();
    let output = Frontend.check_project(input);
    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::InvalidAnnotation)
            && diagnostic.primary.span.source == etas_core::SourceId(18)),
        "{:?}",
        output.diagnostics
    );
}
