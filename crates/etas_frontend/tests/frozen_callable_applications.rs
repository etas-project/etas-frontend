use std::{collections::BTreeMap, sync::Arc};

use etas_code::{
    CodeFingerprint, PackageKey,
    definition::CodeDefinitionOperations,
    descriptor::{DescriptorLimits, PrimitiveInfo, TraceInfo, TypeInfo},
    syntax::CodeSyntaxOperations,
};
use etas_frontend::{
    CheckedCallableApplication, CheckedProgram, EntryPolicy, Frontend, ProjectInput, SourceInput,
    code::{
        definition::{DefinitionCatalog, NativeDefinitionStore, ReflectionOptions},
        identity::DefinitionOrigins,
        service::NativeCodeSyntaxOperations,
    },
};
use etas_hir::{HirExprId, HirItem, HirItemId};
use etas_types::{EffectRef, EffectRowRef, PrimitiveType, Type, TypeId};

fn prepare(source: &str, retain: &str) -> (CheckedProgram, Arc<DefinitionCatalog>) {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    input.options.reflection = ReflectionOptions {
        retain_bodies: vec![format!("app.{retain}")],
        export_bodies: vec![],
    };
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let catalog = catalog(&checked);
    (checked, catalog)
}

fn catalog(checked: &CheckedProgram) -> Arc<DefinitionCatalog> {
    let package = |name: &[u8]| {
        PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
            "test.frozen-callable-application.v1",
            name,
        ))
    };
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(checked, package(b"app"))
            .unwrap(),
        standard_package: package(b"std"),
        external_definitions: BTreeMap::new(),
    };
    Arc::new(
        DefinitionCatalog::build(checked.clone(), &origins, DescriptorLimits::default()).unwrap(),
    )
}

fn item(checked: &CheckedProgram, name: &str) -> HirItemId {
    checked
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow) if checked.symbols.get(flow.symbol).unwrap().name == name => {
                Some(id)
            }
            _ => None,
        })
        .unwrap()
}

fn primitive(checked: &CheckedProgram, primitive: PrimitiveType) -> TypeId {
    checked
        .type_store
        .iter()
        .find_map(|(id, ty)| (ty == &Type::Primitive(primitive)).then_some(id))
        .unwrap()
}

fn application(
    checked: &CheckedProgram,
    name: &str,
    ty: PrimitiveType,
) -> CheckedCallableApplication {
    checked
        .checked_callable_application(
            item(checked, name),
            &[("T".into(), primitive(checked, ty))],
            &[],
        )
        .unwrap()
}

fn reflection(checked: &CheckedProgram) -> HirExprId {
    assert_eq!(checked.types.reflections.len(), 1);
    *checked.types.reflections.keys().next().unwrap()
}

const IDENTITY: &str = "module app;
    flow identity<T>(value: T) -> T ![] { return value; }
    flow inspect() -> unit ![] {
        let selected: (bool) -> bool ![] = identity;
        let definition = ^selected;
        let other: (i32) -> i32 ![] = identity;
        return;
    }";

#[test]
fn freezing_an_application_projects_its_bound_header_without_mutating_the_template() {
    let (checked, catalog) = prepare(IDENTITY, "identity");
    let application = application(&checked, "identity", PrimitiveType::Bool);
    let frozen = catalog
        .freeze_callable_application(&application, DescriptorLimits::default())
        .unwrap();
    assert!(Arc::ptr_eq(frozen.checked(), &checked));
    let proof = frozen.callable_application().unwrap();
    assert_eq!(proof.arguments(), application.arguments());
    assert!(Arc::ptr_eq(proof.program(), &checked));
    let contract = frozen.contract();
    let header = contract.callable(contract.entry().unwrap()).unwrap();
    let TypeInfo::Function(signature) = contract.descriptors().type_info(header.signature).unwrap()
    else {
        panic!("applied function signature");
    };
    assert!(signature.generics.is_empty());
    assert_eq!(signature.parameters.len(), 1);
    assert_eq!(
        contract.descriptors().type_info(signature.output).unwrap(),
        &TypeInfo::Primitive(PrimitiveInfo::Bool)
    );
    let Some(etas_types::ItemSignature::Flow(template)) = checked
        .types
        .item_signatures
        .get(&item(&checked, "identity"))
    else {
        panic!("source template");
    };
    assert_eq!(template.generic_params.len(), 1);
    assert_ne!(template.output, application.signature().output);
}

#[test]
fn reflected_applied_draft_keeps_the_same_contract_and_selected_source_body() {
    let (checked, catalog) = prepare(IDENTITY, "identity");
    let application = application(&checked, "identity", PrimitiveType::Bool);
    let store = NativeDefinitionStore::new(
        catalog,
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    );
    let original = store
        .reflect_callable_application(
            reflection(&checked),
            &application,
            DescriptorLimits::default(),
        )
        .unwrap();
    let repeated = store
        .reflect_callable_application(
            reflection(&checked),
            &application,
            DescriptorLimits::default(),
        )
        .unwrap();
    assert_eq!(original.revision(), repeated.revision());
    let original_view = store.resolve(&original).unwrap();
    assert!(original_view.body().is_some());
    let contract = original_view
        .environment()
        .unwrap()
        .contract()
        .fingerprint();
    let draft = store
        .with_body(
            &original,
            store
                .syntax_operations()
                .parse_body("return true;")
                .unwrap(),
        )
        .unwrap();
    let view = store.resolve(&draft).unwrap();
    assert!(view.current_analysis().is_err());
    assert_eq!(
        view.environment().unwrap().contract().fingerprint(),
        contract
    );
    assert_eq!(
        view.environment()
            .unwrap()
            .callable_application()
            .unwrap()
            .arguments(),
        application.arguments()
    );
    assert!(original_view.current_analysis().is_ok());
}

#[test]
fn phantom_bindings_with_the_same_arrow_have_distinct_frozen_contracts() {
    let (checked, catalog) = prepare(
        "module app; flow phantom<T>() -> bool ![] { return true; }
         flow inspect() -> unit ![] { phantom<bool>(); phantom<i32>(); return; }",
        "phantom",
    );
    let a = application(&checked, "phantom", PrimitiveType::Bool);
    let b = application(&checked, "phantom", PrimitiveType::I32);
    assert_eq!(a.signature(), b.signature());
    let a = catalog
        .freeze_callable_application(&a, DescriptorLimits::default())
        .unwrap();
    let b = catalog
        .freeze_callable_application(&b, DescriptorLimits::default())
        .unwrap();
    assert_eq!(a.identity(), b.identity());
    assert_ne!(a.contract().fingerprint(), b.contract().fingerprint());
    assert_ne!(a.fingerprint(), b.fingerprint());
}

#[test]
fn application_freezing_rejects_foreign_proofs_and_missing_behavior() {
    let (checked, source_catalog) = prepare(IDENTITY, "identity");
    let (foreign, _) = prepare(IDENTITY, "identity");
    let foreign = application(&foreign, "identity", PrimitiveType::Bool);
    assert!(
        source_catalog
            .freeze_callable_application(&foreign, DescriptorLimits::default())
            .is_err()
    );
    let mut corrupt = (*checked).clone();
    Arc::make_mut(&mut corrupt.effects)
        .item_effects
        .remove(&item(&checked, "identity"));
    let corrupt = Arc::new(corrupt);
    let corrupt_catalog = catalog(&corrupt);
    let proof = application(&corrupt, "identity", PrimitiveType::Bool);
    assert!(
        corrupt_catalog
            .freeze_callable_application(&proof, DescriptorLimits::default())
            .is_err()
    );
}

#[test]
fn frozen_empty_escaping_row_does_not_erase_handled_requests_or_their_trace() {
    let (checked, catalog) = prepare(
        "module app; import std.io.println;
         flow echo<T>(value: T) -> T ![] { println(\"debug\")?; return value; }
         flow inspect() -> unit ![] {
             let selected: (bool) -> bool ![] = echo; let definition = ^selected; return;
         }",
        "echo",
    );
    let proof = application(&checked, "echo", PrimitiveType::Bool);
    let store = NativeDefinitionStore::new(
        catalog,
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    );
    let reference = store
        .reflect_callable_application(reflection(&checked), &proof, DescriptorLimits::default())
        .unwrap();
    let view = store.resolve(&reference).unwrap();
    let analysis = view.current_analysis().unwrap();
    let graph = view.current_analysis_descriptors().unwrap();
    assert!(
        graph
            .effect_row(analysis.effects.escaping.unwrap())
            .unwrap()
            .effects
            .is_empty()
    );
    let requested = graph
        .effect_row(analysis.effects.requested.unwrap())
        .unwrap();
    assert_eq!(requested.effects.len(), 1);
    assert!(requested.tail.is_none());
    assert!(matches!(
        graph.trace(analysis.trace.unwrap()),
        Some(TraceInfo::Event(_))
    ));
    let frozen = view.environment().unwrap();
    assert!(
        !frozen
            .contract()
            .applied_behavior()
            .unwrap()
            .summary()
            .requested_actions
            .is_empty()
    );
    assert_eq!(view.signature().unwrap().generics.len(), 0);
}

#[test]
fn bound_effect_row_is_closed_but_callback_trace_keeps_its_parameter_identity() {
    let (checked, catalog) = prepare(
        "module app;
         flow twice<effect E>(callback: () -> unit ![E]) -> unit ![E] { callback(); callback(); return; }
         flow emit() -> unit ![Console.stdout_write] { perform Console.stdout_write(\"x\"); return; }
         flow inspect() -> unit ![] {
             let selected: (() -> unit ![Console.stdout_write]) -> unit ![Console.stdout_write] = twice;
             let definition = ^selected; return;
         }",
        "twice",
    );
    let proof = checked
        .checked_callable_application(
            item(&checked, "twice"),
            &[],
            &[(
                "E".into(),
                EffectRowRef {
                    effects: vec![EffectRef {
                        name: "Console.stdout_write".into(),
                        args: vec![],
                    }],
                    tail: None,
                },
            )],
        )
        .unwrap();
    let frozen = catalog
        .freeze_callable_application(&proof, DescriptorLimits::default())
        .unwrap();
    let contract = frozen.contract();
    let header = contract.callable(contract.entry().unwrap()).unwrap();
    let graph = contract.descriptors();
    let escaping = graph
        .effect_row(header.analysis.effects.escaping.unwrap())
        .unwrap();
    assert_eq!(escaping.effects.len(), 1);
    assert!(escaping.tail.is_none());
    let TraceInfo::Seq(parts) = graph.trace(header.analysis.trace.unwrap()).unwrap() else {
        panic!("two callback call sites");
    };
    assert_eq!(parts.len(), 2);
    assert!(
        parts
            .iter()
            .all(|part| matches!(graph.trace(*part), Some(TraceInfo::ParameterCall(_))))
    );
    assert_eq!(graph.trace(parts[0]), graph.trace(parts[1]));
}

#[test]
fn returned_closure_provenance_stays_paired_with_the_root_application() {
    let (checked, catalog) = prepare(
        "module app;
         flow capture<T>(value: T) -> () -> T ![] ![] { return () => value; }
         flow inspect() -> unit ![] { let f: (bool) -> (() -> bool ![]) ![] = capture; let definition = ^f; return; }",
        "capture",
    );
    let proof = application(&checked, "capture", PrimitiveType::Bool);
    let frozen = catalog
        .freeze_callable_application(&proof, DescriptorLimits::default())
        .unwrap();
    let behavior = frozen.contract().applied_behavior().unwrap();
    let before = checked.effects.item_effects[&proof.owner()]
        .exits
        .as_ref()
        .unwrap()
        .callable_return_sources();
    let after = behavior
        .summary()
        .exits
        .as_ref()
        .unwrap()
        .callable_return_sources();
    assert!(before.is_some());
    assert_eq!(before, after);
    assert_eq!(behavior.application().arguments(), proof.arguments());
    let header = frozen
        .contract()
        .callable(frozen.contract().entry().unwrap())
        .unwrap();
    let TypeInfo::Function(outer) = frozen
        .contract()
        .descriptors()
        .type_info(header.signature)
        .unwrap()
    else {
        panic!("outer flow");
    };
    let TypeInfo::Function(inner) = frozen
        .contract()
        .descriptors()
        .type_info(outer.output)
        .unwrap()
    else {
        panic!("returned closure");
    };
    assert_eq!(
        frozen.contract().descriptors().type_info(inner.output),
        Some(&TypeInfo::Primitive(PrimitiveInfo::Bool))
    );
}

#[test]
fn reflected_binding_cannot_claim_another_prepared_instantiation() {
    let (checked, catalog) = prepare(IDENTITY, "identity");
    let proof = application(&checked, "identity", PrimitiveType::I32);
    let store = NativeDefinitionStore::new(
        catalog,
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    );
    assert!(
        store
            .reflect_callable_application(reflection(&checked), &proof, DescriptorLimits::default())
            .is_err()
    );
}

#[test]
fn inferred_output_is_not_relabelled_as_an_explicit_annotation() {
    let (checked, catalog) = prepare(
        "module app; flow infer<T>(value: T) { return value; }
         flow inspect() -> unit ![] { let f: (bool) -> bool ![] = infer; let definition = ^f; return; }",
        "infer",
    );
    let proof = application(&checked, "infer", PrimitiveType::Bool);
    let frozen = catalog
        .freeze_callable_application(&proof, DescriptorLimits::default())
        .unwrap();
    let header = frozen
        .contract()
        .callable(frozen.contract().entry().unwrap())
        .unwrap();
    let TypeInfo::Function(signature) = frozen
        .contract()
        .descriptors()
        .type_info(header.signature)
        .unwrap()
    else {
        panic!("inferred output");
    };
    assert_eq!(
        frozen.contract().descriptors().type_info(signature.output),
        Some(&TypeInfo::Primitive(PrimitiveInfo::Bool))
    );
    assert!(signature.declared_output.is_none());
}

#[test]
fn source_view_cache_and_source_restore_do_not_exchange_phantom_applications() {
    let (checked, catalog) = prepare(
        "module app; flow phantom<T>() -> bool ![] { return true; }
         flow inspect(selected: () -> bool ![]) -> unit ![] {
             let definition = ^selected; phantom<bool>(); phantom<i32>(); return;
         }",
        "phantom",
    );
    let a = application(&checked, "phantom", PrimitiveType::Bool);
    let b = application(&checked, "phantom", PrimitiveType::I32);
    let store = NativeDefinitionStore::new(
        catalog,
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    );
    let expr = reflection(&checked);
    let a = store
        .reflect_callable_application(expr, &a, DescriptorLimits::default())
        .unwrap();
    let b = store
        .reflect_callable_application(expr, &b, DescriptorLimits::default())
        .unwrap();
    assert_ne!(a.revision(), b.revision());
    assert!(!a.is_same_object(&b));
    let source = store.save_source(&a, Default::default()).unwrap();
    assert!(
        store
            .restore_source_draft(&a, &source, Default::default())
            .is_ok()
    );
    assert!(matches!(
        store.restore_source_draft(&b, &source, Default::default()),
        Err(
            etas_frontend::code::definition::NativeDefinitionError::SourceBundle(
                etas_code::definition::SourceBundleError::ContractMismatch
            )
        )
    ));
}

#[test]
fn equivalent_effect_row_bindings_reuse_the_same_native_source_view() {
    let (checked, catalog) = prepare(
        "module app;
         flow accept<effect E>(callback: () -> unit ![E]) -> unit ![E] { callback(); return; }
         flow inspect() -> unit ![] {
             let selected: (() -> unit ![Console.stdout_write, Console.stderr_write]) -> unit ![Console.stdout_write, Console.stderr_write] = accept;
             let definition = ^selected; return;
         }",
        "accept",
    );
    let mut row = EffectRowRef {
        effects: vec![
            EffectRef {
                name: "Console.stdout_write".into(),
                args: vec![],
            },
            EffectRef {
                name: "Console.stderr_write".into(),
                args: vec![],
            },
        ],
        tail: None,
    };
    let owner = item(&checked, "accept");
    let first = checked
        .checked_callable_application(owner, &[], &[("E".into(), row.clone())])
        .unwrap();
    row.effects.reverse();
    row.effects.push(row.effects[0].clone());
    let second = checked
        .checked_callable_application(owner, &[], &[("E".into(), row)])
        .unwrap();
    let store = NativeDefinitionStore::new(
        catalog,
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    );
    let a = store
        .reflect_callable_application(reflection(&checked), &first, DescriptorLimits::default())
        .unwrap();
    let b = store
        .reflect_callable_application(reflection(&checked), &second, DescriptorLimits::default())
        .unwrap();
    assert!(a.is_same_object(&b));
}

#[test]
fn freezing_an_application_obeys_descriptor_resource_limits() {
    let (checked, catalog) = prepare(IDENTITY, "identity");
    let proof = application(&checked, "identity", PrimitiveType::Bool);
    assert!(
        catalog
            .freeze_callable_application(
                &proof,
                DescriptorLimits {
                    max_nodes: 0,
                    ..Default::default()
                }
            )
            .is_err()
    );
}
