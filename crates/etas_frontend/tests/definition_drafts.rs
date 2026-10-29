use std::sync::Arc;

use etas_code::{
    CallableId, CodeFingerprint, DefinitionKey, DefinitionKind, ImplementationHeader,
    ImplementationKey, MethodId, PackageKey,
    definition::{
        AnalysisUnavailableReason, BaseRevisionInput, DefinitionAnalysisState, DefinitionError,
        DefinitionSnapshot, FrozenDefinition, FrozenDefinitionInput,
    },
    descriptor::{
        DescriptorGraph, DescriptorGraphInput, DescriptorLimits, PrimitiveInfo, SignatureInfo,
        TypeInfo, TypeInfoRef,
    },
    syntax::CodeSyntaxOperations,
};
use etas_frontend::code::service::NativeCodeSyntaxOperations;

fn fingerprint(value: &str) -> CodeFingerprint {
    CodeFingerprint::of_bytes("etas.code.test.draft.v1", value.as_bytes())
}

fn base_input(kind: DefinitionKind, has_entry: bool) -> FrozenDefinitionInput<String> {
    let identity = DefinitionKey::new(
        PackageKey::from_canonical_fingerprint(fingerprint("package")),
        vec!["app".into()],
        vec!["Worker".into()],
        kind,
    )
    .unwrap();
    let default_callable = has_entry.then(|| {
        CallableId::method(
            MethodId::new(
                ImplementationKey::from_header(
                    identity.package(),
                    identity.module().to_vec(),
                    identity.clone(),
                    None,
                    &ImplementationHeader::new(vec![]).unwrap(),
                )
                .unwrap(),
                "run".into(),
            )
            .unwrap(),
        )
    });
    FrozenDefinitionInput {
        identity,
        descriptors: DescriptorGraph::new(
            DescriptorGraphInput {
                types: vec![
                    TypeInfo::Primitive(PrimitiveInfo::Unit),
                    TypeInfo::Function(SignatureInfo {
                        generics: vec![],
                        parameters: vec![],
                        output: TypeInfoRef(0),
                        declared_output: Some(TypeInfoRef(0)),
                        effects: None,
                    }),
                ],
                ..DescriptorGraphInput::default()
            },
            DescriptorLimits::default(),
        )
        .unwrap(),
        default_entry: has_entry.then_some(TypeInfoRef(1)),
        default_callable,
        environment: fingerprint("locked environment"),
        contract_fingerprint: fingerprint("frozen contract"),
        contract: Arc::new("authoritative contract data".into()),
        compilation: fingerprint("compiler / backend / semantic config"),
    }
}

#[test]
fn body_edit_shares_frozen_base_and_invalidates_only_the_new_snapshot() {
    let syntax = NativeCodeSyntaxOperations::default();
    let body = syntax.parse_body("return;").unwrap();
    let original = DefinitionSnapshot::from_base(
        FrozenDefinition::new(base_input(DefinitionKind::Agent, true)).unwrap(),
        BaseRevisionInput {
            content: body.identity().content(),
            body: Some(body),
            parent: None,
            analysis_configuration: fingerprint("analysis"),
            summaries: Some(Arc::new(vec!["old behavior".to_owned()])),
        },
    )
    .unwrap();
    let replacement = syntax.parse_body("let changed = 42; return;").unwrap();
    let edited = original.with_body(replacement.clone());
    assert!(std::ptr::eq(original.base(), edited.base()));
    assert_eq!(edited.base().identity(), original.base().identity());
    assert_eq!(edited.base().signature(), original.base().signature());
    assert_eq!(syntax.format(original.body().unwrap()).unwrap(), "return;");
    assert!(
        syntax
            .format(edited.body().unwrap())
            .unwrap()
            .contains("changed")
    );
    assert_eq!(original.current_analysis().unwrap(), &["old behavior"]);
    assert_eq!(
        edited.current_analysis(),
        Err(AnalysisUnavailableReason::EditedBody)
    );
    assert!(matches!(
        edited.analysis(),
        DefinitionAnalysisState::BaselineOnly { .. }
    ));
    assert_ne!(edited.revision().key(), original.revision().key());
    assert_eq!(
        edited.revision().inputs().parent,
        Some(original.revision().key())
    );
    assert_eq!(
        edited.revision().inputs().content,
        replacement.identity().content()
    );
    assert_eq!(
        edited.revision().inputs().contract,
        original.revision().inputs().contract
    );
    assert_eq!(
        edited.revision().inputs().environment,
        original.revision().inputs().environment
    );
    let edited_again = edited.with_body(replacement);
    assert_eq!(
        edited_again.revision().inputs().parent,
        Some(edited.revision().key())
    );
    assert_eq!(
        edited_again.current_analysis(),
        Err(AnalysisUnavailableReason::EditedBody)
    );
    assert!(std::ptr::eq(edited_again.base(), original.base()));
}

#[test]
fn bare_agent_edit_does_not_invent_a_default_entry_or_analysis() {
    let syntax = NativeCodeSyntaxOperations::default();
    let bare: DefinitionSnapshot<_, Vec<String>> = DefinitionSnapshot::from_base(
        FrozenDefinition::new(base_input(DefinitionKind::Agent, false)).unwrap(),
        BaseRevisionInput {
            body: None,
            content: fingerprint("bare declaration"),
            parent: None,
            analysis_configuration: fingerprint("analysis"),
            summaries: None,
        },
    )
    .unwrap();
    assert!(bare.base().signature().is_none());
    assert!(bare.body().is_none());
    let edited = bare.with_body(syntax.parse_body("return;").unwrap());
    assert!(edited.body().is_some());
    assert!(edited.base().signature().is_none());
    assert_eq!(
        edited.current_analysis(),
        Err(AnalysisUnavailableReason::MissingDefaultEntry)
    );
}

#[test]
fn unavailable_retained_bodies_do_not_erase_actual_revision_content() {
    let base = FrozenDefinition::new(base_input(DefinitionKind::Agent, true)).unwrap();
    let snapshot = |content| {
        DefinitionSnapshot::<_, Vec<String>>::from_base(
            base.clone(),
            BaseRevisionInput {
                body: None,
                content,
                parent: None,
                analysis_configuration: fingerprint("analysis"),
                summaries: None,
            },
        )
        .unwrap()
    };
    let first = snapshot(fingerprint("hidden body one"));
    let second = snapshot(fingerprint("hidden body two"));
    assert!(first.body().is_none() && second.body().is_none());
    assert_ne!(first.revision().key(), second.revision().key());
}

#[test]
fn snapshot_rejects_missing_entry_and_mismatched_retained_content() {
    let syntax = NativeCodeSyntaxOperations::default();
    let body = syntax.parse_body("return;").unwrap();
    let snapshot = |base, content| {
        DefinitionSnapshot::<_, Vec<String>>::from_base(
            base,
            BaseRevisionInput {
                body: Some(body.clone()),
                content,
                parent: None,
                analysis_configuration: fingerprint("analysis"),
                summaries: None,
            },
        )
    };
    let bare = FrozenDefinition::new(base_input(DefinitionKind::Agent, false)).unwrap();
    assert!(matches!(
        snapshot(bare, body.identity().content()),
        Err(DefinitionError::MissingDefaultEntry)
    ));
    let callable = FrozenDefinition::new(base_input(DefinitionKind::Agent, true)).unwrap();
    assert!(matches!(
        snapshot(callable, fingerprint("different body")),
        Err(DefinitionError::BodyContentMismatch)
    ));
}

#[test]
fn frozen_base_requires_an_editable_owner_and_real_callable_descriptor() {
    assert!(matches!(
        FrozenDefinition::new(base_input(DefinitionKind::Type, true)),
        Err(DefinitionError::UnsupportedDefinitionKind)
    ));
    assert!(matches!(
        FrozenDefinition::new(base_input(DefinitionKind::Flow, true)),
        Err(DefinitionError::InvalidDefaultEntry)
    ));
    let mut input = base_input(DefinitionKind::Agent, true);
    input.default_entry = Some(TypeInfoRef(0));
    assert!(matches!(
        FrozenDefinition::new(input),
        Err(DefinitionError::InvalidDefaultEntry)
    ));
    let mut input = base_input(DefinitionKind::Agent, true);
    input.default_entry = Some(TypeInfoRef(u32::MAX));
    assert!(matches!(
        FrozenDefinition::new(input),
        Err(DefinitionError::InvalidDefaultEntry)
    ));
}

#[test]
fn frozen_flow_uses_its_own_declaration_not_an_agent_method() {
    let mut input = base_input(DefinitionKind::Flow, true);
    input.default_callable = Some(CallableId::declaration(input.identity.clone()).unwrap());
    let base = FrozenDefinition::new(input).unwrap();
    assert_eq!(base.identity().kind(), DefinitionKind::Flow);
    assert_eq!(
        base.default_callable().unwrap().as_declaration(),
        Some(base.identity())
    );
    assert!(base.signature().is_some());
    assert!(matches!(
        FrozenDefinition::new(base_input(DefinitionKind::Flow, false)),
        Err(DefinitionError::InvalidDefaultEntry)
    ));
}

#[test]
fn frozen_default_requires_a_paired_canonical_method_identity() {
    let mut input = base_input(DefinitionKind::Agent, true);
    input.default_callable = None;
    assert!(matches!(
        FrozenDefinition::new(input),
        Err(DefinitionError::InvalidDefaultEntry)
    ));
    let mut input = base_input(DefinitionKind::Agent, true);
    input.default_entry = None;
    assert!(matches!(
        FrozenDefinition::new(input),
        Err(DefinitionError::InvalidDefaultEntry)
    ));
    let mut input = base_input(DefinitionKind::Agent, true);
    input.default_callable = Some(CallableId::declaration(input.identity.clone()).unwrap());
    assert!(matches!(
        FrozenDefinition::new(input),
        Err(DefinitionError::InvalidDefaultEntry)
    ));
    let mut input = base_input(DefinitionKind::Agent, true);
    let original = input
        .default_callable
        .as_ref()
        .unwrap()
        .as_method()
        .unwrap();
    input.default_callable = Some(CallableId::method(
        MethodId::new(original.implementation().clone(), "other".into()).unwrap(),
    ));
    assert!(matches!(
        FrozenDefinition::new(input),
        Err(DefinitionError::InvalidDefaultEntry)
    ));
}
