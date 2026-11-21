use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use etas_frontend::{
    Frontend, ModulePath, ProjectEntry, ProjectSourceLoadOptions, ProjectSourceLoadScope,
    ProjectSourceLoader, StandardLibraryMode,
};

struct SourcePackage(PathBuf);

impl SourcePackage {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "etas-std-source-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("library/std/text")).unwrap();
        fs::create_dir_all(root.join("library/std/runtime/budget")).unwrap();
        fs::write(root.join("library/std/runtime/budget/mod.es"),
            "module std.runtime.budget; public type Money = { private amount: f64, private currency: string };").unwrap();
        fs::create_dir_all(root.join("library/std/runtime/time")).unwrap();
        fs::write(
            root.join("library/std/runtime/time/mod.es"),
            "module std.runtime.time; public type Duration = { millis: u128 };",
        )
        .unwrap();
        let package = Self(root);
        package.write("mod.es", "module std.text;");
        package.write(
            "assembly.es",
            r#"
module std.text;
public flow join(parts: (string, string), separator: string) -> string {
    let (first, second) = parts;
    return first + separator + second;
}
"#,
        );
        package
    }

    fn copy_maintained_library(&self) {
        fn copy_tree(source: &std::path::Path, target: &std::path::Path) {
            fs::create_dir_all(target).unwrap();
            for entry in fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                let destination = target.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy_tree(&entry.path(), &destination);
                } else {
                    fs::copy(entry.path(), destination).unwrap();
                }
            }
        }
        let manifest = etas_std::library::source_checkout_manifest();
        copy_tree(
            &manifest.parent().unwrap().join("library/std"),
            &self.0.join("library/std"),
        );
    }

    fn write(&self, file: &str, source: &str) {
        fs::write(self.0.join("library/std/text").join(file), source).unwrap();
    }

    fn load(&self) -> etas_frontend::LoadedProjectInput {
        ProjectSourceLoader::default()
            .load(ProjectSourceLoadOptions {
                project_root: self.0.clone(),
                roots: vec![self.0.join("library")],
                entry: ProjectEntry {
                    module: None,
                    flow: "main".into(),
                },
                scope: ProjectSourceLoadScope::FullSourceTree,
                source_kind: None,
                import_search_roots: vec![],
                external_module_roots: vec![],
            })
            .unwrap()
    }
}

impl Drop for SourcePackage {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn adding_a_source_part_adds_public_std_flows_without_rust_registration_or_main() {
    let package = SourcePackage::new();
    assert_eq!(package.load().input.sources.len(), 4);
    package.write(
        "decorate.es",
        r#"
module std.text;
public flow decorate(value: string) -> string { return "[" + value + "]"; }
public flow decorate_twice(value: string) -> string { return decorate(decorate(value)); }
"#,
    );
    let loaded = package.load();
    assert_eq!(loaded.input.sources.len(), 5);
    package.write(
        "consumer.es",
        r#"
module std.text.consumer;
import std.text.decorate;
public flow render(value: string) -> string { return decorate(value); }
"#,
    );
    let loaded = package.load();
    assert_eq!(loaded.input.sources.len(), 6);
    let consumer_fingerprint = loaded.input.options.canonical_options_fingerprint();
    let output = Frontend.check_standard_library_source(loaded.input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.as_ref().expect("checked source std");
    assert_eq!(
        checked.compile_options.standard_library_mode,
        StandardLibraryMode::SourceBootstrap
    );
    assert_ne!(
        consumer_fingerprint,
        checked.compile_options.canonical_options_fingerprint()
    );
    assert!(output.entry.as_ref().unwrap().resolved.is_none());
    let modules = output.modules.as_ref().unwrap();
    let id = modules.by_path[&ModulePath {
        segments: vec!["std".into(), "text".into()],
    }];
    let text = modules.modules.get(id).unwrap();
    assert_eq!(text.parts.len(), 3);
    for flow in ["join", "decorate", "decorate_twice"] {
        assert_eq!(
            text.visibility_exports.items[flow].visibility,
            etas_hir::Visibility::Public
        );
    }
    assert!(
        output
            .sources
            .as_ref()
            .unwrap()
            .files
            .iter()
            .all(|source| !matches!(source.kind, etas_frontend::SourceKind::VirtualStd { .. }))
    );
}

#[test]
fn std_source_build_checks_an_unreferenced_private_body_at_its_source_location() {
    let package = SourcePackage::new();
    package.write(
        "private.es",
        "module std.text; flow unused() -> string { return 42; }",
    );
    let loaded = package.load();
    let source = loaded
        .input
        .sources
        .iter()
        .find(|source| source.path.as_ref().unwrap().ends_with("private.es"))
        .unwrap()
        .id;
    let output = Frontend.check_standard_library_source(loaded.input);
    assert!(output.checked.is_none());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.primary.span.source == source),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn checked_library_hir_round_trips_private_generic_bodies_with_portable_identities() {
    use etas_frontend::library_hir::LibraryHirImage;
    let owner = etas_code::PackageKey::from_name_and_version("std", "0.2.2").unwrap();
    let make_image = || {
        let package = SourcePackage::new();
        package.copy_maintained_library();
        fs::write(
            package.0.join("library/std/agent/message/fixture.es"),
            r#"
module std.agent.message;
impl Message<Source> {
    public flow with_body<Target>(self, value: Target) -> Message<Target> ![] {
        return Message {
            body= value, content= value, id= self.id, from= self.from,
            role= self.role, session= self.session, created_at= self.created_at,
            provenance= self.provenance,
        };
    }
}
public flow replaced_message(value: Message<string>) -> Message<i32> ![] {
    return value.with_body(7);
}
"#,
        )
        .unwrap();
        package.write(
            "generic.es",
            r#"
module std.text;
public import std.text.Choice.{First as Begin, Second as End};
public enum Choice { First, Second, }
public flow classify(value: Choice) -> i32 ![] {
    return match value { Begin => 1, End => 2 };
}
flow private_identity<T>(value: T) -> T { return value; }
public flow generic_echo<T>(value: T) -> T { return private_identity<T>(value); }
public spec Read<T> { flow read(self) -> T ![]; }
public spec Update { flow update(var self, value: i32) -> unit ![]; }
public type Container<T> = { value: T };
impl Container<T> {
    public flow replace(var self, value: T) -> T ![] {
        let previous = self.value;
        self = Container { value };
        return previous;
    }
}
public flow replaced_value() -> i32 ![] {
    var value = Container { value = 3 };
    let previous = value.replace(7);
    return previous + value.value;
}
impl Container<T> ~ Read<T> {
    public flow read(self) -> T ![] { return self.value; }
}
impl Container<List<T>> {
    public flow first(self) -> Option<T> ![] {
        return match self.value { [] => None, value :: _ => Some(value) };
    }
    public flow count(self) -> usize ![] { return self.value.len(); }
}
public flow literal_label() -> string ![] {
    let face = char { bits = 128578 };
    return std.text.generic_echo(string { scalars = ['A', face] });
}
public flow interval_total() -> usize ![] {
    let interval = std.collections.Range<usize> { start = 1, end = 3, include_start = true, include_end = true };
    let closed = std.collections.Range.closed(interval.start, interval.end);
    return closed.start + closed.end;
}
public flow read_string<S ~ Read<string>>(value: S) -> string ![] { return value.read(); }
public type Transform<T> = { callback: (T) -> T ![] };
impl Transform<T> {
    public flow apply(self, value: T) -> T ![] { return (self.callback)(value); }
}
public flow numeric_value<T ~ std.math.Number>(left: T, right: T) -> T ![] {
    return std.math.abs(std.math.min(left + right, right));
}
public flow numeric_result() -> i32 ![] { return numeric_value(-7, 4); }
type Files;
impl Files ~ std.fs.Region;
public flow portable_path() -> string ![] {
    return std.fs.relative_name(std.fs.root<Files>());
}
public flow portable_checkpoint() -> unit ![] {
    let checkpoint: (i32) -> unit ![] = std.runtime.checkpoint;
    checkpoint(42);
}
public flow transformed_value() -> i32 ![] {
    let transform = Transform { callback = (value: i32) => value + 1 };
    let wide = Transform { callback = (value: u128) => value + 1 };
    let increment = wide.apply(0);
    return transform.apply(i32 { bits = increment.bits + 40 });
}
public flow describe() -> unit { let definition = ^generic_echo; return; }
"#,
        );
        let mut loaded = package.load();
        loaded.input.environment.local_source_package = Some(owner);
        loaded.input.environment.stable_source_keys = loaded
            .input
            .sources
            .iter()
            .map(|source| {
                let path = source
                    .path
                    .as_ref()
                    .unwrap()
                    .strip_prefix(&package.0)
                    .unwrap();
                let segments = path
                    .components()
                    .map(|part| part.as_os_str().to_str().unwrap().to_owned())
                    .collect::<Vec<_>>();
                etas_frontend::ProjectStableSourceKeyInput {
                    source: source.id,
                    key: etas_code::StableSourceKey::new(
                        etas_code::StableSourceOrigin::Package(owner),
                        &segments,
                    )
                    .unwrap(),
                }
            })
            .collect();
        let output = Frontend.check_standard_library_source(loaded.input);
        assert!(output.checked.is_some(), "{:?}", output.diagnostics);
        LibraryHirImage::from_checked(output.checked.as_ref().unwrap(), owner, &package.0).unwrap()
    };
    // Independent sessions and checkouts exercise map ordering and path removal.
    let first = make_image();
    assert!(!first.methods.is_empty());
    assert!(!first.types.reflections.is_empty());
    let bytes = first.encode().unwrap();
    let second = make_image();
    let mut changed = Vec::new();
    macro_rules! compare_part {
        ($field:ident) => {
            if bincode::serde::encode_to_vec(&first.$field, bincode::config::standard()).unwrap()
                != bincode::serde::encode_to_vec(&second.$field, bincode::config::standard())
                    .unwrap()
            {
                changed.push(stringify!($field));
            }
        };
    }
    compare_part!(program);
    compare_part!(type_store);
    compare_part!(types);
    compare_part!(effects);
    compare_part!(effect_registry);
    compare_part!(concurrency);
    compare_part!(cpu_parallel);
    assert!(
        bytes == second.encode().unwrap(),
        "noncanonical image components: {changed:?}"
    );
    let restored = LibraryHirImage::decode(&bytes).unwrap();
    assert!(
        restored
            .types
            .spec_signatures
            .values()
            .any(|spec| { spec.name == "Index" && spec.methods.len() == 1 })
    );
    assert!(
        restored
            .program
            .pats
            .iter()
            .any(|(_, pat)| matches!(pat, etas_hir::HirPat::EmptyList { .. }))
    );
    assert!(
        restored
            .program
            .pats
            .iter()
            .any(|(_, pat)| matches!(pat, etas_hir::HirPat::ListCons { .. }))
    );
    assert!(
        restored
            .types
            .std_spec_aliases
            .values()
            .any(|path| path == &["std", "math", "Number"])
    );
    assert!(
        restored
            .types
            .spec_signatures
            .values()
            .any(|spec| spec.name == "Number" && spec.methods.is_empty())
    );
    assert!(restored.encode().unwrap() == bytes);
    assert_eq!(
        restored.types.item_signatures.len(),
        first.types.item_signatures.len()
    );
    assert!(!restored.types.item_signatures.is_empty());
    assert_eq!(
        restored.effects.item_effects.len(),
        first.effects.item_effects.len()
    );
    assert_eq!(restored.package, owner);
    assert!(
        restored
            .sources
            .iter()
            .all(|source| !source.relative_path.is_absolute())
    );
    for name in ["join", "private_identity", "generic_echo"] {
        assert!(
            restored
                .declarations
                .keys()
                .any(|key| key.package() == owner && key.declaration() == [name.to_owned()])
        );
    }
    let original = etas_hir::dump_hir(&first.program, Default::default());
    let revision = restored.into_revision().unwrap();
    assert_eq!(original, etas_hir::dump_hir(&revision, Default::default()));
    let installed = SourcePackage::new();
    let linked = LibraryHirImage::decode(&bytes)
        .unwrap()
        .into_checked_from_image(&installed.0)
        .unwrap();
    assert_eq!(
        original,
        etas_hir::dump_hir(&linked.hir, Default::default())
    );
    assert!(linked.uses_environment(&first.environment));
    assert!(linked.entry.is_none());
    let replace = linked
        .types
        .methods
        .entries()
        .find(|method| {
            method.name == "replace"
                && etas_types::ty::display_type(&linked.type_store, method.receiver_type)
                    .starts_with("std.text.Container<")
        })
        .unwrap();
    assert!(replace.receiver.is_updating());
    let etas_types::method::MethodTarget::Source { callable, .. } = replace.target else {
        panic!("restored source method");
    };
    assert!(
        linked
            .checked_callable_suspension(etas_frontend::CheckedCallableRoot::Item(callable))
            .unwrap()
            .proven_non_suspending()
    );
    let update = linked
        .types
        .methods
        .entries()
        .find(|method| method.name == "update" && matches!(
            &method.target,
            etas_types::method::MethodTarget::BoundSpec { spec: etas_types::CheckedSpecRef::Source(spec), .. }
                if linked.types.spec_signatures[spec].name == "Update"
        ))
        .unwrap();
    assert_eq!(
        update.receiver,
        etas_types::method::MethodReceiver::Updating { slot: 0 }
    );
    assert!(
        linked
            .types
            .spec_signatures
            .values()
            .any(|spec| { spec.name == "Update" && spec.methods[0].updating_receiver })
    );
    assert_eq!(linked.effects.suspension, first.effects.suspension);
    for name in [
        "generic_echo",
        "interval_total",
        "replaced_message",
        "portable_path",
    ] {
        let item = linked
            .hir
            .items
            .iter()
            .find_map(|(item, declaration)| {
                let etas_hir::HirItem::Flow(flow) = declaration else {
                    return None;
                };
                (linked.symbols.get(flow.symbol).unwrap().name == name).then_some(item)
            })
            .unwrap();
        assert!(
            linked
                .checked_callable_suspension(etas_frontend::CheckedCallableRoot::Item(item))
                .unwrap()
                .proven_non_suspending(),
            "{name}"
        );
    }
    assert!(linked.types.known_std_types.index_error.is_some());
    let money = linked.type_store.iter().find_map(|(ty, value)| {
        matches!(value, etas_types::Type::Nominal(record) if record.name == "std.runtime.budget.Money").then_some(ty)
    }).expect("source-owned Money declaration");
    assert!(
        matches!(linked.type_store.get(money), Some(etas_types::Type::Nominal(record)) if record.representation.is_some())
    );
    let transform = linked
        .types
        .type_impl_methods
        .values()
        .find(|method| {
            method.name == "apply"
                && etas_types::ty::display_type(&linked.type_store, method.self_type)
                    .starts_with("std.text.Transform<")
        })
        .unwrap();
    let applications = linked
        .checked_callable_applications(transform.body)
        .unwrap();
    assert_eq!(applications.len(), 2);
    let mut argument_types = applications
        .iter()
        .map(|application| {
            assert_eq!(application.owner_bindings().len(), 1);
            assert_eq!(
                application.signature().params[1],
                application.signature().output
            );
            etas_types::ty::display_type(&linked.type_store, application.signature().output)
        })
        .collect::<Vec<_>>();
    argument_types.sort();
    assert_eq!(argument_types, ["i32", "u128"]);
    assert_eq!(
        linked.checked_reflections().len(),
        first.types.reflections.len()
    );
    let capture = linked
        .types
        .type_impl_methods
        .values()
        .find(|method| {
            method.name == "capture"
                && etas_types::ty::display_type(&linked.type_store, method.self_type)
                    == "std.runtime.value.Data"
        })
        .unwrap();
    assert!(matches!(
        &linked.hir.items[capture.body],
        etas_hir::HirItem::Flow(etas_hir::HirFlowDecl {
            body: etas_hir::HirFlowBody::Runtime {
                role: etas_std::StdExecutionRole::CaptureData,
                ..
            },
            ..
        })
    ));
    assert!(
        linked.effects.suspension[&etas_effects::EffectUnit::Item(capture.body)]
            .proven_non_suspending()
    );
    for role in [
        etas_std::StdExecutionRole::StorageContext,
        etas_std::StdExecutionRole::StorageNonce,
        etas_std::StdExecutionRole::StreamClock,
    ] {
        let item = linked
            .hir
            .items
            .iter()
            .find_map(|(item, declaration)| match declaration {
                etas_hir::HirItem::Flow(etas_hir::HirFlowDecl {
                    body: etas_hir::HirFlowBody::Runtime { role: actual, .. },
                    ..
                }) if *actual == role => Some(item),
                _ => None,
            })
            .unwrap();
        assert!(
            linked.effects.suspension[&etas_effects::EffectUnit::Item(item)]
                .proven_non_suspending()
        );
        assert_eq!(
            linked.effects.item_effects[&item].determinism,
            etas_effects::Determinism::NonDeterministic
        );
        assert_eq!(
            linked.effects.item_effects[&item].independent_determinism,
            Some(etas_effects::Determinism::NonDeterministic)
        );
    }
    for item in first.declarations.values() {
        if first.types.item_signatures.contains_key(item) {
            if let etas_hir::HirItem::Flow(etas_hir::HirFlowDecl {
                body: etas_hir::HirFlowBody::Runtime { role, span },
                ..
            }) = &first.program.items[*item]
            {
                assert!(
                    matches!(&linked.hir.items[*item], etas_hir::HirItem::Flow(etas_hir::HirFlowDecl {
                    body: etas_hir::HirFlowBody::Runtime { role: restored, .. }, ..
                }) if role == restored)
                );
                assert!(linked.definition_sources.source(span.source).is_some());
                assert!(linked.definition_sources.body(*item).is_none());
                let suspension = linked.effects.suspension[&etas_effects::EffectUnit::Item(*item)];
                match role {
                    etas_std::StdExecutionRole::Abort | etas_std::StdExecutionRole::CaptureData => {
                        assert!(suspension.proven_non_suspending())
                    }
                    etas_std::StdExecutionRole::StorageContext
                    | etas_std::StdExecutionRole::StorageNonce
                    | etas_std::StdExecutionRole::StreamClock
                    | etas_std::StdExecutionRole::TaskStart
                    | etas_std::StdExecutionRole::TaskCancel
                    | etas_std::StdExecutionRole::ParseBody
                    | etas_std::StdExecutionRole::AgentWithBody
                    | etas_std::StdExecutionRole::CallableWithBody
                    | etas_std::StdExecutionRole::ToolReference
                    | etas_std::StdExecutionRole::CurrentSession
                    | etas_std::StdExecutionRole::MessageContextCapture
                    | etas_std::StdExecutionRole::MessageContextRecord
                    | etas_std::StdExecutionRole::PayloadConvert => {
                        assert!(suspension.proven_non_suspending());
                    }
                    etas_std::StdExecutionRole::Checkpoint => {
                        assert_eq!(
                            suspension,
                            etas_effects::suspension::SuspensionSummary::SUSPENDING
                        );
                        assert_eq!(
                            linked.effects.item_effects[item].support,
                            etas_effects::InterpreterSupport::from(
                                etas_effects::RuntimeRequirementReason::Checkpoint
                            )
                        );
                    }
                    etas_std::StdExecutionRole::Requeue
                    | etas_std::StdExecutionRole::Park
                    | etas_std::StdExecutionRole::ScopeEnter
                    | etas_std::StdExecutionRole::TaskNextCompletion => {
                        assert_eq!(
                            suspension,
                            etas_effects::suspension::SuspensionSummary::SUSPENDING
                        );
                    }
                }
            } else if matches!(
                &first.program.items[*item],
                etas_hir::HirItem::Flow(etas_hir::HirFlowDecl {
                    body: etas_hir::HirFlowBody::Decl { .. },
                    ..
                })
            ) {
                assert!(linked.definition_sources.body(*item).is_none());
            } else {
                assert!(
                    linked.definition_sources.body(*item).is_some(),
                    "missing body for {:?}",
                    first.program.items[*item]
                );
            }
            assert!(linked.effects.item_effects.contains_key(item));
        }
    }
}

#[test]
fn artifact_registry_exposes_source_flow_signature_and_canonical_owner() {
    use etas_code::{CallableId, DefinitionKey, DefinitionKind, PackageKey};
    use etas_frontend::{CheckRequest, FrontendSession, ProjectInput, SourceInput};
    use etas_std::{FlowDecl, StdRegistryBuilder, StdRegistryImage};
    let native = etas_std::standard_registry().native_bootstrap();
    let module = native
        .modules()
        .find(|module| module.path == ["std", "text"])
        .unwrap()
        .id;
    let definition = DefinitionKey::new(
        PackageKey::from_name_and_version("std", "0.2.4").unwrap(),
        vec!["std".into(), "text".into()],
        vec!["from_bundle".into()],
        DefinitionKind::Flow,
    )
    .unwrap();
    let mut builder = StdRegistryBuilder::from_registry(native);
    let symbol = builder.source_flow(
        module,
        "from_bundle",
        FlowDecl::pure("from_bundle", &["string"], "string"),
        CallableId::declaration(definition.clone()).unwrap(),
        "Artifact source flow",
    );
    builder.prelude("from_bundle", symbol);
    let bytes = serde_json::to_vec(&StdRegistryImage::capture(&builder.finish()).unwrap()).unwrap();
    let image: StdRegistryImage = serde_json::from_slice(&bytes).unwrap();
    let registry = std::sync::Arc::new(image.into_registry().unwrap());
    let key = etas_frontend::code::identity::standard_definition_key(
        &registry,
        etas_frontend::code::identity::standard_package_key(&registry).unwrap(),
        registry.symbol(symbol).unwrap(),
    )
    .unwrap();
    assert_eq!(key, Some(definition));
    let mut session = FrontendSession::with_std_registry(registry);
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(
        "module app; import std.text.{from_bundle as echo}; flow main() -> string { return echo(from_bundle(\"loaded\")); }",
    )));
    let response = session.check(project, CheckRequest::one_shot()).unwrap();
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.output.diagnostics
    );
}

#[test]
fn source_task_runtime_roles_keep_their_checked_opaque_abi() {
    let package = SourcePackage::new();
    fs::create_dir_all(package.0.join("library/std/task")).unwrap();
    fs::write(
        package.0.join("library/std/task/mod.es"),
        r#"
module std.task;
@std.role("std.task.Task")
public type Task<Output, effect Remaining>;
@std.role("std.task.Scheduler")
public type Scheduler;
@std.role("std.task.Continuation")
public type Continuation<ActionOutput>;
@std.binding("std.task.run")
public flow execute<T, effect E>(body: (Scheduler) -> T ![E]) -> T ![E] {}
"#,
    )
    .unwrap();
    let output = Frontend.check_standard_library_source(package.load().input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    assert_eq!(checked.types.source_type_roles.len(), 3);
    for (symbol, native) in &checked.types.source_type_roles {
        let etas_types::SymbolTypeFact::Type { constructor } = checked.types.symbol_types[symbol]
        else {
            panic!("opaque role is not a compiler type");
        };
        let etas_std::StdDecl::Type(declaration) =
            &checked.std_registry.symbol(*native).unwrap().decl
        else {
            unreachable!()
        };
        assert!(matches!(
            (
                declaration.name.as_str(),
                checked
                    .type_store
                    .get(etas_types::TypeId(constructor.0))
                    .unwrap()
            ),
            ("Task", etas_types::Type::Task(_))
                | ("Scheduler", etas_types::Type::SchedulerHandle)
                | ("Continuation", etas_types::Type::Continuation(_))
        ));
    }
}

#[test]
fn ordinary_source_enum_payloads_do_not_inherit_a_legacy_native_abi() {
    let package = SourcePackage::new();
    let maintained = etas_std::library::source_checkout_manifest()
        .parent()
        .unwrap()
        .join("library/std/text/errors.es");
    let source = fs::read_to_string(maintained)
        .unwrap()
        .replace("\n    ParseFailure,", "\n    ParseFailure(string),");
    package.write("errors.es", &source);
    let loaded = package.load();
    let output = Frontend.check_standard_library_source(loaded.input);
    let checked = output.checked.expect(&format!("{:?}", output.diagnostics));
    let variant = checked
        .types
        .enum_layouts
        .values()
        .flat_map(|layout| &layout.variants)
        .find(|variant| variant.name == "ParseFailure")
        .unwrap();
    assert_eq!(variant.fields.len(), 1);
    assert!(matches!(
        checked.type_store.get(variant.fields[0]),
        Some(etas_types::Type::Primitive(
            etas_types::PrimitiveType::String
        ))
    ));
}

#[test]
fn source_filesystem_types_keep_region_bounds_and_native_read_actions() {
    let package = SourcePackage::new();
    package.copy_maintained_library();
    let source = fs::read_to_string(package.0.join("library/std/fs/mod.es")).unwrap();
    fs::write(
        package.0.join("library/std/fs/mod.es"),
        source.replace(
            "public enum ReadLimitError {",
            "public enum ReadLimitError { SdkOnly,",
        ),
    )
    .unwrap();
    let output = Frontend.check_standard_library_source(package.load().input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let path = checked
        .symbols
        .iter()
        .find(|symbol| symbol.name == "WorkspacePath")
        .unwrap();
    let etas_types::SymbolTypeFact::NominalType { constructor, .. } =
        checked.types.symbol_types[&path.id]
    else {
        panic!("source-owned workspace path");
    };
    let parameter = &checked.types.type_generics[&etas_types::TypeId(constructor.0)][0];
    assert_eq!(parameter.name, "R");
    let [bound] = parameter.bounds.as_slice() else {
        panic!("region bound");
    };
    let etas_types::CheckedSpecRef::Source(region) = bound.spec else {
        panic!("source region");
    };
    assert_eq!(checked.types.spec_signatures[&region].name, "Region");
    // Its installed query is derived from the ordinary source declaration.
    assert!(
        checked
            .std_registry
            .lookup_qualified(&["std", "fs", "IOError"])
            .is_some_and(|symbol| matches!(&symbol.decl,
                etas_std::StdDecl::Type(declaration) if declaration.runtime_role.is_none()))
    );
    let error = checked.types.known_std_types.filesystem_io_error.unwrap();
    assert!(
        matches!(checked.type_store.get(error), Some(etas_types::Type::Nominal(nominal))
        if nominal.name == "std.fs.IOError" && nominal.representation.is_some())
    );
    let Some(etas_types::Type::Nominal(declaration)) = checked.type_store.get(error) else {
        unreachable!()
    };
    assert!(matches!(
        checked.type_store.get(declaration.representation.unwrap()),
        Some(etas_types::Type::Primitive(
            etas_types::PrimitiveType::String
        ))
    ));
    let limit = checked.types.known_std_types.file_read_limit_error.unwrap();
    let variants = &checked.types.enum_layouts[&limit].variants;
    assert!(variants.iter().any(|variant| variant.name == "SdkOnly"));
    let payload = &variants
        .iter()
        .find(|variant| variant.name == "LimitExceeded")
        .unwrap()
        .fields;
    assert!(matches!(
        checked.type_store.get(payload[0]),
        Some(etas_types::Type::Primitive(
            etas_types::PrimitiveType::USize
        ))
    ));
    let native_stat = checked
        .std_registry
        .lookup_qualified(&["std", "effects", "actions", "Fs", "stat"])
        .unwrap();
    let source_stat = checked
        .hir
        .standard_action_targets
        .iter()
        .find_map(|(symbol, target)| {
            (*target == native_stat.id
                && matches!(
                    checked.hir.symbols.get(*symbol).map(|s| &s.def),
                    Some(etas_hir::SymbolDef::EffectAction { .. })
                ))
            .then_some(*symbol)
        })
        .unwrap();
    assert_eq!(
        checked.types.action_signatures[&source_stat].generic_params[0].name,
        "R"
    );
    let stable = checked.effect_registry.action(source_stat).unwrap();
    assert_eq!(stable.id.0, 139);
    assert!(stable.has_default_handler);
    let stat = checked
        .effects
        .performed_actions
        .values()
        .find(|call| call.action_symbol == Some(source_stat))
        .unwrap();
    assert_eq!(
        checked.types.expr_types[&stat.expr],
        checked.types.action_signatures[&source_stat].output
    );
    assert_eq!(
        checked.types.expr_types[&stat.expr],
        checked.types.qualified_action_signatures["std.effects.actions.Fs.stat"].output
    );
    assert!(!checked.types.known_std_types.opaque_values.contains(&error));
    let read = checked
        .std_registry
        .lookup_qualified(&["std", "effects", "actions", "Fs", "read"])
        .unwrap();
    let source_read = checked
        .hir
        .standard_action_targets
        .iter()
        .find_map(|(symbol, target)| {
            (*target == read.id
                && matches!(
                    checked.hir.symbols.get(*symbol).map(|symbol| &symbol.def),
                    Some(etas_hir::SymbolDef::EffectAction { .. })
                ))
            .then_some(*symbol)
        })
        .unwrap();
    let call = checked
        .effects
        .performed_actions
        .values()
        .find(|call| call.action_symbol == Some(source_read))
        .unwrap();
    let signature = &checked.types.action_signatures[&source_read];
    assert_eq!(call.args.len(), 2);
    assert_eq!(signature.params.len(), 2);
    let limit = etas_types::value_representation::option_parameter(
        &checked.type_store,
        &checked.types.known_std_types,
        signature.params[1],
    )
    .expect("source Option read limit");
    assert!(matches!(
        checked.type_store.get(limit),
        Some(etas_types::Type::Primitive(
            etas_types::PrimitiveType::USize
        ))
    ));
    assert!(!checked.types.std_intrinsic_calls.values().any(|call| {
        call.qualified_path
            .starts_with(&["std".into(), "fs".into()])
    }));
}
