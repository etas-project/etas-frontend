use std::{collections::BTreeMap, sync::Arc};

use etas_code::{CodeFingerprint, PackageKey, descriptor::DescriptorLimits};
use etas_frontend::{
    CallableApplicationError, CheckedProgram, EntryPolicy, Frontend, ProjectInput, SourceInput,
    code::{definition::DefinitionCatalog, identity::DefinitionOrigins},
};
use etas_hir::{HirItem, HirItemId};
use etas_types::{CallableGenericArg, EffectRef, EffectRowRef, PrimitiveType, Type, TypeId};

fn check(source: &str) -> CheckedProgram {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.unwrap()
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

fn nominal(checked: &CheckedProgram, name: &str) -> TypeId {
    let symbol = checked
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap()
        .id;
    match &checked.types.symbol_types[&symbol] {
        etas_types::SymbolTypeFact::NominalType { constructor, .. }
        | etas_types::SymbolTypeFact::Type { constructor } => TypeId(constructor.0),
        _ => panic!("nominal declaration"),
    }
}

fn catalog(checked: &CheckedProgram) -> DefinitionCatalog {
    let package = |name: &[u8]| {
        PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
            "test.checked-callable-application.v1",
            name,
        ))
    };
    DefinitionCatalog::build(
        checked.clone(),
        &DefinitionOrigins {
            source_modules: DefinitionOrigins::declared_source_modules(checked, package(b"app"))
                .unwrap(),
            standard_package: package(b"std"),
            external_definitions: BTreeMap::new(),
        },
        DescriptorLimits::default(),
    )
    .unwrap()
}

fn empty_row() -> EffectRowRef {
    EffectRowRef {
        effects: vec![],
        tail: None,
    }
}

const BOUND_SOURCE: &str = "module app;
    flow Worker<T, effect E>(value: T, callback: () -> unit ![E]) -> T ![E] { callback(); return value; }
    flow noop() -> unit ![] { return; }
    flow prepare() -> unit ![] { Worker<bool, ![]>(true, noop); return; }";

#[test]
fn a_typed_generic_reference_prepares_its_application_before_invocation() {
    let checked = check(
        "module app;
        flow identity<T>(value: T) -> T ![] { return value; }
        flow prepare() -> unit ![] {
            let selected: (bool) -> bool ![] = identity;
            return;
        }",
    );
    let selected = checked
        .symbols
        .iter()
        .find(|symbol| symbol.name == "selected")
        .unwrap()
        .id;
    let etas_types::SymbolTypeFact::Local { ty, .. } = checked.types.symbol_types[&selected] else {
        panic!("checked reference binding");
    };
    let Some(Type::Function(expected)) = checked.type_store.get(ty) else {
        panic!("checked reference signature");
    };
    let application = checked
        .checked_callable_application(
            item(&checked, "identity"),
            &[("T".into(), expected.output)],
            &[],
        )
        .unwrap();
    assert_eq!(application.signature().params, expected.input);
    assert_eq!(application.signature().output, expected.output);
    assert_eq!(application.signature().effects, expected.effects);
    assert_eq!(
        application.arguments(),
        &[CallableGenericArg::Type(expected.output)]
    );
}

#[test]
fn applied_behavior_keeps_handled_requests_even_when_the_escaping_row_is_empty() {
    let checked = check(
        "module app; import std.io.println;
        flow echo<T>(value: T) -> T ![] { println(\"debug\")?; return value; }
        flow prepare() -> bool ![] { return echo(true); }",
    );
    let owner = item(&checked, "echo");
    let Some(etas_types::ItemSignature::Flow(prepare)) = checked
        .types
        .item_signatures
        .get(&item(&checked, "prepare"))
    else {
        panic!("prepare signature");
    };
    let application = checked
        .checked_callable_application(owner, &[("T".into(), prepare.output)], &[])
        .unwrap();
    let behavior = application
        .applied_behavior(DescriptorLimits::default(), None)
        .unwrap();
    assert!(Arc::ptr_eq(behavior.application().program(), &checked));
    assert_eq!(behavior.application().owner(), owner);
    let summary = behavior.summary();
    assert!(summary.escaping_effects.is_empty());
    let action = checked
        .effect_registry
        .action_by_name("Console.stdout_write")
        .unwrap();
    assert!(
        summary
            .requested_actions
            .effects
            .contains(&etas_effects::Effect::Action(action))
    );
    assert!(!summary.action_trace.action_set().is_empty());
    assert!(
        summary
            .exits
            .as_ref()
            .unwrap()
            .exits
            .iter()
            .any(|exit| matches!(exit.outcome,
        etas_effects::CallableOutcome::Returned { ty, .. } if ty == prepare.output))
    );
    assert_eq!(
        summary.requested_actions,
        checked.effects.item_effects[&owner].requested_actions
    );
    assert_ne!(
        application.signature().params[0],
        match &checked.types.item_signatures[&owner] {
            etas_types::ItemSignature::Flow(signature) => signature.params[0],
            _ => panic!("source template"),
        }
    );
}

#[test]
fn applied_region_behavior_matches_normal_call_specialization_without_wildcards() {
    let checked = check(
        "module app;
        import std.effects.Fs;
        import std.fs.{IOError, Region, WorkspacePath, read_bytes};
        type Root; impl Root ~ Region;
        flow load<R ~ Region>(path: WorkspacePath<R>) -> bytes ![Fs.read<R>, Error<IOError>] {
            return read_bytes(path);
        }
        flow prepare(path: WorkspacePath<Root>) -> bytes ![Fs.read<Root>, Error<IOError>] {
            return load<Root>(path);
        }",
    );
    let owner = item(&checked, "load");
    let root = nominal(&checked, "Root");
    let application = checked
        .checked_callable_application(owner, &[("R".into(), root)], &[])
        .unwrap();
    let behavior = application
        .applied_behavior(DescriptorLimits::default(), None)
        .unwrap();
    let applied = behavior.summary();
    let call = checked
        .effects
        .native_call_targets
        .iter()
        .find_map(|(expr, targets)| {
            (targets.contains(&etas_effects::EffectUnit::Item(owner))
                && matches!(
                    checked.exprs.get(*expr),
                    Some(etas_hir::HirExpr::Call { .. })
                ))
            .then_some(*expr)
        })
        .unwrap();
    let normal = &checked.effects.expr_effects[&call];
    assert_eq!(applied.escaping_effects, normal.escaping_effects);
    assert_eq!(applied.requested_actions, normal.requested_actions);
    assert_eq!(applied.default_actions, normal.default_actions);
    assert_eq!(applied.handled_actions, normal.handled_actions);
    assert_eq!(applied.action_trace, normal.action_trace);
    assert_eq!(applied.handler_requests, normal.handler_requests);
    let action = checked.effect_registry.action_by_name("Fs.read").unwrap();
    assert!(
        applied
            .requested_actions
            .effects
            .contains(&etas_effects::Effect::AppliedAction(
                etas_effects::ActionInstanceRef {
                    action,
                    args: vec![etas_types::EffectArgRef::Type(root)],
                }
            ))
    );
    assert_ne!(
        applied.requested_actions,
        checked.effects.item_effects[&owner].requested_actions
    );
}

#[test]
fn bound_rows_keep_the_callback_trace_template_instead_of_realizing_it_once() {
    let checked = check(BOUND_SOURCE);
    let owner = item(&checked, "Worker");
    let Some(etas_types::ItemSignature::Flow(prepare)) = checked
        .types
        .item_signatures
        .get(&item(&checked, "prepare"))
    else {
        panic!("prepare signature");
    };
    let boolean = checked
        .type_store
        .iter()
        .find_map(|(id, ty)| matches!(ty, Type::Primitive(PrimitiveType::Bool)).then_some(id))
        .unwrap();
    assert!(prepare.params.is_empty());
    let application = checked
        .checked_callable_application(
            owner,
            &[("T".into(), boolean)],
            &[("E".into(), empty_row())],
        )
        .unwrap();
    let behavior = application
        .applied_behavior(DescriptorLimits::default(), None)
        .unwrap();
    assert!(behavior.summary().escaping_effects.is_empty());
    assert_eq!(
        behavior.summary().action_trace,
        checked.effects.item_effects[&owner].action_trace
    );
    assert!(!matches!(
        behavior.summary().action_trace,
        etas_effects::ActionTraceDomain::Empty
    ));
}

#[test]
fn applied_behavior_is_bounded_cancellable_and_rejects_a_missing_source_summary() {
    let mut checked = check(BOUND_SOURCE);
    let owner = item(&checked, "Worker");
    let boolean = checked
        .type_store
        .iter()
        .find_map(|(id, ty)| matches!(ty, Type::Primitive(PrimitiveType::Bool)).then_some(id))
        .unwrap();
    let application = checked
        .checked_callable_application(
            owner,
            &[("T".into(), boolean)],
            &[("E".into(), empty_row())],
        )
        .unwrap();
    assert!(matches!(
        application.applied_behavior(
            DescriptorLimits {
                max_nodes: 1,
                ..Default::default()
            },
            None
        ),
        Err(etas_frontend::CallableBehaviorError::Substitution(
            etas_effects::substitution::SummarySubstitutionError::ResourceLimit
        ))
    ));
    struct Stop;
    impl etas_utils::PipelineExecutionControl for Stop {
        fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
            Err(etas_utils::PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::Cancelled,
                reason: "test stop".into(),
            })
        }
    }
    assert!(matches!(
        application.applied_behavior(DescriptorLimits::default(), Some(&Stop)),
        Err(etas_frontend::CallableBehaviorError::Substitution(
            etas_effects::substitution::SummarySubstitutionError::Interrupted(_)
        ))
    ));
    drop(application);
    let data = Arc::make_mut(&mut checked);
    Arc::make_mut(&mut data.effects).item_effects.remove(&owner);
    let application = checked
        .checked_callable_application(
            owner,
            &[("T".into(), boolean)],
            &[("E".into(), empty_row())],
        )
        .unwrap();
    assert!(
        matches!(application.applied_behavior(DescriptorLimits::default(), None), Err(etas_frontend::CallableBehaviorError::MissingSummary(item)) if item == owner)
    );
}

#[test]
fn checked_application_keeps_its_source_owner_all_arguments_and_applied_signature() {
    let checked = check(BOUND_SOURCE);
    let owner = item(&checked, "Worker");
    let boolean = checked
        .type_store
        .iter()
        .find_map(|(id, ty)| matches!(ty, Type::Primitive(PrimitiveType::Bool)).then_some(id))
        .unwrap();
    let application = checked
        .checked_callable_application(
            owner,
            &[("T".into(), boolean)],
            &[("E".into(), empty_row())],
        )
        .unwrap();
    assert!(Arc::ptr_eq(application.program(), &checked));
    assert_eq!(application.owner(), owner);
    assert_eq!(
        application.arguments(),
        &[
            CallableGenericArg::Type(boolean),
            CallableGenericArg::EffectRow(empty_row())
        ]
    );
    assert_eq!(application.signature().params[0], boolean);
    assert_eq!(application.signature().output, boolean);
    assert_eq!(application.signature().effects, Some(empty_row()));
    assert!(application.signature().generic_params.is_empty());
    let Some(Type::Function(callback)) = checked.type_store.get(application.signature().params[1])
    else {
        panic!("applied callback type");
    };
    assert_eq!(callback.effects, Some(empty_row()));
    let Some(etas_types::ItemSignature::Flow(template)) = checked.types.item_signatures.get(&owner)
    else {
        panic!("template");
    };
    assert_eq!(template.generic_params.len(), 2);
}

#[test]
fn unprepared_phantom_arguments_are_rejected_even_with_the_same_arrow_type() {
    let checked = check(
        "module app; type Allowed; type Unchecked;
        flow phantom<R>() -> i32 ![] { return 0; }
        flow prepare() -> unit ![] { phantom<Allowed>(); return; }",
    );
    let owner = item(&checked, "phantom");
    assert!(
        checked
            .checked_callable_application(owner, &[("R".into(), nominal(&checked, "Allowed"))], &[])
            .is_ok()
    );
    assert_eq!(
        checked
            .checked_callable_application(
                owner,
                &[("R".into(), nominal(&checked, "Unchecked"))],
                &[]
            )
            .unwrap_err(),
        CallableApplicationError::UnpreparedApplication(owner)
    );
}

#[test]
fn generic_binding_arity_kind_unknown_names_and_duplicates_fail_closed() {
    let checked = check(BOUND_SOURCE);
    let owner = item(&checked, "Worker");
    let boolean = checked
        .type_store
        .iter()
        .find_map(|(id, ty)| matches!(ty, Type::Primitive(PrimitiveType::Bool)).then_some(id))
        .unwrap();
    for (types, rows) in [
        (vec![], vec![]),
        (vec![("E".into(), boolean)], vec![("T".into(), empty_row())]),
        (
            vec![("unknown".into(), boolean)],
            vec![("E".into(), empty_row())],
        ),
        (vec![("T".into(), boolean), ("T".into(), boolean)], vec![]),
        (
            vec![],
            vec![("E".into(), empty_row()), ("E".into(), empty_row())],
        ),
    ] {
        assert!(matches!(
            checked.checked_callable_application(owner, &types, &rows),
            Err(CallableApplicationError::InvalidBinding(_))
        ));
    }
}

#[test]
fn a_known_effect_row_is_not_an_authorization_to_invent_another_instance() {
    let checked = check(BOUND_SOURCE);
    let owner = item(&checked, "Worker");
    let boolean = checked
        .type_store
        .iter()
        .find_map(|(id, ty)| matches!(ty, Type::Primitive(PrimitiveType::Bool)).then_some(id))
        .unwrap();
    let row = EffectRowRef {
        effects: vec![EffectRef {
            name: "Console.stdout_write".into(),
            args: vec![],
        }],
        tail: None,
    };
    assert_eq!(
        checked
            .checked_callable_application(owner, &[("T".into(), boolean)], &[("E".into(), row)])
            .unwrap_err(),
        CallableApplicationError::UnpreparedApplication(owner)
    );
}

#[test]
fn a_recursive_nominal_argument_uses_checked_identity_and_respects_projection_limits() {
    let checked = check("module app;
        enum Tree<T> { Leaf(T), Pair(Tree<T>, Tree<T>), }
        flow keep<T>(value: T) -> T ![] { return value; }
        flow prepare() -> Tree<bool> ![] { return keep(Tree.Pair(Tree.Leaf(true), Tree.Leaf(false))); }");
    let owner = item(&checked, "keep");
    let Some(etas_types::ItemSignature::Flow(prepare)) = checked
        .types
        .item_signatures
        .get(&item(&checked, "prepare"))
    else {
        panic!("prepared signature");
    };
    let application = checked
        .checked_callable_application(owner, &[("T".into(), prepare.output)], &[])
        .unwrap();
    assert_eq!(application.signature().output, prepare.output);
    let catalog = catalog(&checked);
    assert!(
        catalog
            .callable_application_fingerprint(&application, DescriptorLimits::default())
            .is_ok()
    );
    assert!(
        catalog
            .callable_application_fingerprint(
                &application,
                DescriptorLimits {
                    max_nodes: 1,
                    ..DescriptorLimits::default()
                }
            )
            .is_err()
    );
}

#[test]
fn application_identity_includes_phantom_arguments_not_just_the_arrow() {
    let checked = check(
        "module app; type Left; type Right;
        flow phantom<R>() -> i32 ![] { return 0; }
        flow prepare() -> unit ![] { phantom<Left>(); phantom<Right>(); return; }",
    );
    let catalog = catalog(&checked);
    let owner = item(&checked, "phantom");
    let left = checked
        .checked_callable_application(owner, &[("R".into(), nominal(&checked, "Left"))], &[])
        .unwrap();
    let right = checked
        .checked_callable_application(owner, &[("R".into(), nominal(&checked, "Right"))], &[])
        .unwrap();
    assert_eq!(left.signature(), right.signature());
    assert_ne!(
        catalog
            .callable_application_fingerprint(&left, DescriptorLimits::default())
            .unwrap(),
        catalog
            .callable_application_fingerprint(&right, DescriptorLimits::default())
            .unwrap()
    );
    assert_eq!(
        catalog
            .callable_application_fingerprint(&left, DescriptorLimits::default())
            .unwrap(),
        catalog
            .callable_application_fingerprint(&left.clone(), DescriptorLimits::default())
            .unwrap()
    );
}

#[test]
fn application_identity_is_independent_of_local_type_id_order_and_rejects_foreign_pins() {
    let mut fingerprints = Vec::new();
    let mut programs = Vec::new();
    for declarations in ["type Left; type Right;", "type Right; type Left;"] {
        let checked = check(&format!(
            "module app; {declarations}
            flow phantom<R>() -> i32 ![] {{ return 0; }}
            flow prepare() -> unit ![] {{ phantom<Left>(); phantom<Right>(); return; }}"
        ));
        let catalog = catalog(&checked);
        let application = checked
            .checked_callable_application(
                item(&checked, "phantom"),
                &[("R".into(), nominal(&checked, "Left"))],
                &[],
            )
            .unwrap();
        fingerprints.push(
            catalog
                .callable_application_fingerprint(&application, DescriptorLimits::default())
                .unwrap(),
        );
        programs.push((catalog, application));
    }
    assert_eq!(fingerprints[0], fingerprints[1]);
    assert!(
        programs[0]
            .0
            .callable_application_fingerprint(&programs[1].1, DescriptorLimits::default())
            .is_err()
    );
}

#[test]
fn equivalent_effect_argument_sets_have_the_same_instance_identity() {
    let checked = check(
        r#"module app;
        flow writes() -> unit ![Console.stdout_write, Console.stderr_write] {
            perform Console.stdout_write("out"); perform Console.stderr_write("err"); return;
        }
        flow accept<effect E>(callback: () -> unit ![E]) -> unit ![E] { callback(); }
        flow prepare() -> unit ![Console.stdout_write, Console.stderr_write] { accept<![Console.stdout_write, Console.stderr_write]>(writes); return; }"#,
    );
    let owner = item(&checked, "accept");
    let row = EffectRowRef {
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
    let mut reordered = row.clone();
    reordered.effects.reverse();
    reordered.effects.push(reordered.effects[0].clone());
    let first = checked
        .checked_callable_application(owner, &[], &[("E".into(), row)])
        .unwrap();
    let second = checked
        .checked_callable_application(owner, &[], &[("E".into(), reordered)])
        .unwrap();
    let catalog = catalog(&checked);
    let first_behavior = first
        .applied_behavior(DescriptorLimits::default(), None)
        .unwrap();
    let second_behavior = second
        .applied_behavior(DescriptorLimits::default(), None)
        .unwrap();
    assert_eq!(first_behavior.summary(), second_behavior.summary());
    assert_eq!(
        first_behavior.summary().escaping_effects,
        checked.effects.item_effects[&item(&checked, "writes")].escaping_effects
    );
    assert_eq!(
        first_behavior.summary().action_trace,
        checked.effects.item_effects[&owner].action_trace,
        "applying a type/row environment must not execute or erase callback trace slots"
    );
    assert_eq!(
        catalog
            .callable_application_fingerprint(&first, DescriptorLimits::default())
            .unwrap(),
        catalog
            .callable_application_fingerprint(&second, DescriptorLimits::default())
            .unwrap()
    );
}
