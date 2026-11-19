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

#[test]
fn suspension_proofs_follow_source_calls_without_treating_empty_effects_as_evidence() {
    use etas_effects::suspension::SuspensionSummary as S;
    use etas_frontend::CheckedCallableRoot;
    let source = r#"
module app;
import std.io.println;
import std.task.start;
import std.task.run as within;
import app.CallbackLookup.{Available as Found, Empty as Missing};
public flow invoke_pair<A, B, effect E>(jobs: (() -> A ![E], () -> B ![E])) -> (A, B) ![E] {
    let (first, second) = jobs;
    return (first(), second());
}
public flow capture_pair<A, B, effect E>(jobs: (() -> A ![E], () -> B ![E])) -> (() -> (A, B) ![E]) ![] {
    let (first, second) = jobs;
    return () => (first(), second());
}
public flow invoke_fields<A, B, effect E>(jobs: { first: () -> A ![E], second: () -> B ![E] }) -> (A, B) ![E] {
    return ((jobs.first)(), (jobs.second)());
}
public flow capture_fields<A, B, effect E>(jobs: { first: () -> A ![E], second: () -> B ![E] }) -> (() -> (A, B) ![E]) ![] {
    return () => ((jobs.first)(), (jobs.second)());
}
public flow relay_fields<A, B, effect E>(jobs: { first: () -> A ![E], second: () -> B ![E] }) -> { first: () -> A ![E], second: () -> B ![E] } ![] {
    return jobs;
}
flow prepare_returned_fields() -> (i32, bool) ![] {
    let jobs = relay_fields({ first= () => 7, second= () => true });
    return ((jobs.first)(), (jobs.second)());
}

flow prepare_fields() -> (i32, bool) ![] { return invoke_fields({ first= () => 7, second= () => true }); }
flow prepare_captured_fields() -> (i32, bool) ![] {
    let run = capture_fields({ first= () => 7, second= () => true });
    return run();
}
flow prepare_pair() -> (i32, bool) ![] { return invoke_pair((() => 7, () => true)); }
flow prepare_captured_pair() -> (i32, bool) ![] {
    let run = capture_pair((() => 7, () => true));
    return run();
}
public flow capture_results<T, X, effect E>(jobs: Array<() -> T ![Error<X>, E]>) -> unit ![E] {
    return within((scheduler: Scheduler) => {
    for job in jobs limit Iterations(18446744073709551615) {
        let task: Task<Result<T, X>, ![E]> = start(() => job()?);
    }
    return;
    });
}
public flow plus(value: i32) -> i32 ![] { return value + 1; }
public flow alias(value: i32) -> i32 ![] { let apply = plus; return apply(value); }
spec Reading { flow read(self) -> i32 ![]; }
impl i32 ~ Reading { public flow read(self) -> i32 ![] { return self; } }
flow generic_read<R ~ Reading>(value: R) -> i32 ![] { return value.read(); }
type Counter = { value: i32 };
impl Counter {
    flow update(var self) -> unit ![] { self.value = generic_read(self.value); }
    flow update_operation(var self) -> unit ![] {
        let operation: Operation<i32> = Operation.new();
        self.value = operation.apply(self.value);
    }
}
type Operation<T> = { run: (T) -> i32 ![] };
impl Operation<T> {
    flow new(run: (T) -> i32 ![]) -> Operation<T> ![] {
        return Operation { run= run };
    }
    flow apply(self, value: T) -> i32 ![] { return (self.run)(value); }
}
impl<R ~ Reading> Operation<R> {
    flow new() -> Operation<R> ![] {
        return Operation { run= (value: R) => value.read() };
    }
}
flow update_concrete() -> i32 ![] {
    var counter = Counter { value= 42 };
    counter.update();
    counter.update_operation();
    return counter.value;
}
flow even(value: i32) -> bool ![] { if value == 0 { return true; } return odd(value - 1); }
flow odd(value: i32) -> bool ![] { if value == 0 { return false; } return even(value - 1); }
flow callback(value: () -> i32 ![]) -> i32 ![] { return value(); }
enum CallbackLookup<T> { Available(T), Empty, }
alias NativeArray<T> = [T];
spec OffsetIndex { flow offset(self) -> Option<usize> ![]; }
impl usize ~ OffsetIndex {
    public flow offset(self) -> Option<usize> ![] { return Some(self); }
}
flow array_callback(values: NativeArray<() -> i32 ![]>) -> i32 ![Error<IndexError>] {
    let index: usize = 0;
    return match values.lookup_source(index) {
        CallbackLookup.Available(callback) => callback(), CallbackLookup.Empty => 0,
    };
}
flow prepare_deferred_callback() -> i32 ![] {
    let values: NativeArray<() -> i32 ![]> = [() => 7];
    let load = values.lookup_later();
    return match load() {
        CallbackLookup.Available(callback) => callback(), CallbackLookup.Empty => 0,
    };
}

impl NativeArray<T> {
    flow lookup_later(self) -> (() -> CallbackLookup<T> ![]) ![] {
        return () => {
            let index: usize = 0;
            return self.lookup_source(index);
        };
    }
    flow lookup_source<I ~ OffsetIndex>(self, index: I) -> CallbackLookup<T> ![] {
        return match index.offset() {
            Some(offset) => {
                return match self[offset]? { Ok(value) => Found(value), Err(_) => Missing };
            },
            None => Missing,
        };
    }
}
public flow observe() -> unit ![Console.stdout_write] { perform Console.stdout_write("ready"); }
flow relay() -> unit ![Console.stdout_write] { observe(); }
flow default_handled() -> unit ![] { println("ready")?; }
flow factory() -> (() -> unit ![Console.stdout_write]) ![] {
    return () => { perform Console.stdout_write("later"); };
}
"#;
    let checked = check(source);
    let mut body_bytes = Vec::new();
    ciborium::ser::into_writer(checked.effects.as_ref(), &mut body_bytes).unwrap();
    let replayed_facts: etas_effects::EffectFacts =
        ciborium::de::from_reader(body_bytes.as_slice()).unwrap();
    assert_eq!(
        replayed_facts.source_call_arguments,
        checked.effects.source_call_arguments
    );
    assert!(
        replayed_facts
            .source_call_arguments
            .values()
            .any(|arguments| arguments
                .keys()
                .any(|target| matches!(target, etas_effects::ParameterCallTarget::Projection(_))))
    );
    for (unit, summary) in &checked.effects.unit_effects {
        assert_eq!(
            replayed_facts.unit_effects[unit]
                .exits
                .as_ref()
                .map(|exits| &exits.value_projections),
            summary.exits.as_ref().map(|exits| &exits.value_projections)
        );
    }
    let summary = |name| {
        checked
            .checked_callable_suspension(CheckedCallableRoot::Item(item(&checked, name)))
            .unwrap()
    };
    for name in [
        "plus",
        "alias",
        "even",
        "odd",
        "factory",
        "update_concrete",
        "prepare_returned_fields",
        "prepare_deferred_callback",
    ] {
        assert_eq!(summary(name), S::IMMEDIATE, "{name}");
    }
    assert_eq!(summary("callback"), S::UNKNOWN);
    assert_eq!(summary("array_callback"), S::UNKNOWN);
    assert_eq!(summary("generic_read"), S::UNKNOWN);
    assert!(summary("observe").may_suspend);
    assert!(summary("relay").may_suspend);
    assert!(!summary("default_handled").proven_non_suspending());
    let lambda = checked
        .hir
        .exprs
        .iter()
        .find_map(|(id, expr)| {
            (matches!(expr, etas_hir::HirExpr::Lambda { .. })
                && checked.hir.index().expr_item.get(&id) == Some(&item(&checked, "factory")))
            .then_some(id)
        })
        .unwrap();
    assert!(
        checked
            .checked_callable_suspension(CheckedCallableRoot::Lambda(lambda))
            .unwrap()
            .may_suspend
    );
    let artifact = etas_frontend::build_package_metadata_artifact_from_checked(
        etas_frontend::PackageMetadataBuildInput {
            definition_origin: Some(PackageKey::from_name_and_version("app", "0.1.0").unwrap()),
            package_id: "app".into(),
            package_version: "0.1.0".into(),
            package_edition: "2026".into(),
            compiler_version: etas_frontend::FRONTEND_COMPILER_VERSION.into(),
            source_payload_hash: etas_package_metadata::blake3_hash(source.as_bytes()),
            manifest_hash: etas_package_metadata::blake3_hash(b"app manifest"),
            dependency_lock_hash: etas_package_metadata::blake3_hash(b"app lock"),
            bins: vec![],
            dependencies: vec![],
            tool_bindings: vec![],
        },
        &checked,
    )
    .unwrap();
    let (_, replayed) = etas_package_metadata::package_metadata_from_artifact(
        std::path::Path::new("projected-callbacks.etasmeta"),
        &artifact.bytes,
    )
    .unwrap();
    let graph = replayed
        .public_metadata
        .effect_summaries
        .iter()
        .find(|summary| {
            summary
                .item
                .last()
                .is_some_and(|name| name == "capture_pair")
        })
        .and_then(|summary| summary.returned_callables.as_ref())
        .unwrap();
    assert!(graph.parameter_projections.contains(&vec![
        etas_code::descriptor::ValueProjectionStep::TupleElement(0)
    ]));
    assert!(graph.parameter_projections.contains(&vec![
        etas_code::descriptor::ValueProjectionStep::TupleElement(1)
    ]));
    let (_, metadata) = etas_package_metadata::package_metadata_from_artifact(
        std::path::Path::new("app.etasmeta"),
        &artifact.bytes,
    )
    .unwrap();
    let headers = metadata
        .public_metadata
        .definition_descriptors
        .unwrap()
        .headers
        .unwrap();
    for name in ["plus", "alias", "observe"] {
        let header = headers
            .callables
            .iter()
            .find(|header| {
                header
                    .id
                    .as_declaration()
                    .is_some_and(|key| key.declaration() == [name.to_owned()])
            })
            .unwrap();
        let portable = header.analysis.suspension.unwrap();
        assert_eq!(portable.may_suspend, summary(name).may_suspend);
        assert_eq!(portable.unknown, summary(name).unknown);
    }

    // Exercise source cursor analysis directly, before SDK query publication.
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module std.collections;
public type Iter<Element> = { next: () -> Option<(Element, Iter<Element>)> ![] };
flow numbers<T>(value: T, start: i32) -> Iter<T> ![] {
    return Iter { next= () => match start == 0 {
        true => None,
        false => Some((value, numbers(value, start - 1))),
    } };
}
type Compared<T> = { value: T, compare: (T, T) -> i32 ![] };
impl Compared<T> {
    flow compare_self(self) -> i32 ![] { return (self.compare)(self.value, self.value); }
}
flow compared<T>(values: Compared<T>) -> Iter<T> ![] {
    return Iter { next= () => Some((values.value, numbers(values.value, 0))) };
}
flow total() -> i32 ![] {
    var result: i32 = 0;
    for value in numbers(7, 4) limit Iterations(4) { result = result + value; }
    let values = Compared { value= 3, compare= (left: i32, right: i32) => left - right };
    result = result + values.compare_self();
    for value in compared(values) limit Iterations(1) { result = result + value; }
    return result;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut program = etas_hir::lower_program(&parsed.value);
    program.source_standard_types = true;
    let hir = etas_hir::HirRevision::try_new(program).unwrap();
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let registry = etas_std::standard_registry();
    let effects = etas_effects::RunEffectPipeline::run(etas_effects::EffectPipelineInput {
        source_snapshots: &[],
        residual_identity: None,
        trace_monitor_projector: None,
        external_implementations: &[],
        frozen_calls: None,
        hir: &hir,
        types: &types,
        std_registry: &registry,
        dependency_metadata: None,
        tool_bindings: &[],
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .unwrap()
    .effects;
    assert!(effects.diagnostics.is_empty(), "{:?}", effects.diagnostics);
    let total = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow) if hir.symbols.get(flow.symbol).unwrap().name == "total" => {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    assert!(!effects.facts.iteration_call_sources.is_empty());
    assert_eq!(
        effects.facts.suspension[&etas_effects::EffectUnit::Item(total)],
        S::IMMEDIATE,
        "origins={:?}; units={:?}; calls={:?}",
        effects.facts.iteration_call_sources,
        effects
            .facts
            .unit_effects
            .iter()
            .map(|(unit, summary)| (unit, &summary.exits, effects.facts.suspension.get(unit)))
            .collect::<Vec<_>>(),
        effects.facts.native_call_targets,
    );
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

#[test]
fn nongeneric_applied_behavior_preserves_checked_residual_references() {
    use etas_code::residual::identity::{ObligationId, ResidualObligationRef};

    let mut checked = check("module app; flow source(value: bool) -> bool ![] { return value; }");
    let owner = item(&checked, "source");
    let reference = ResidualObligationRef::new(
        ObligationId::from_canonical_fingerprint(CodeFingerprint::from_bytes([31; 32])),
        Default::default(),
    )
    .unwrap();
    let data = Arc::make_mut(&mut checked);
    Arc::make_mut(&mut data.effects)
        .item_effects
        .get_mut(&owner)
        .unwrap()
        .residual_checks
        .insert(reference.clone());

    let application = checked
        .checked_callable_application(owner, &[], &[])
        .unwrap();
    let behavior = application
        .applied_behavior(DescriptorLimits::default(), None)
        .unwrap();
    assert_eq!(
        behavior
            .summary()
            .residual_checks
            .iter()
            .collect::<Vec<_>>(),
        vec![&reference]
    );
}

#[test]
fn generic_applied_behavior_requires_checked_residual_substitution() {
    use etas_code::residual::identity::{ObligationId, ResidualObligationRef};

    let mut checked = check(
        "module app; flow source<T>(value: T) -> T ![] { return value; }
         flow prepare() -> bool ![] { return source(true); }",
    );
    let owner = item(&checked, "source");
    let Some(etas_types::ItemSignature::Flow(prepare)) = checked
        .types
        .item_signatures
        .get(&item(&checked, "prepare"))
    else {
        panic!("prepare signature");
    };
    let boolean = prepare.output;
    let reference = ResidualObligationRef::new(
        ObligationId::from_canonical_fingerprint(CodeFingerprint::from_bytes([32; 32])),
        Default::default(),
    )
    .unwrap();
    let data = Arc::make_mut(&mut checked);
    Arc::make_mut(&mut data.effects)
        .item_effects
        .get_mut(&owner)
        .unwrap()
        .residual_checks
        .insert(reference);
    let application = checked
        .checked_callable_application(owner, &[("T".into(), boolean)], &[])
        .unwrap();
    assert!(matches!(
        application.applied_behavior(DescriptorLimits::default(), None),
        Err(etas_frontend::CallableBehaviorError::Substitution(
            etas_effects::substitution::SummarySubstitutionError::UnsupportedResidualSubstitution
        ))
    ));
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
