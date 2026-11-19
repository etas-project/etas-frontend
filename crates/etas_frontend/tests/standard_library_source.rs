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
        let package = Self(root);
        package.write("mod.es", "module std.text;");
        package.write(
            "assembly.es",
            r#"
module std.text;
public flow join(parts: Array<string>, separator: string) -> string {
    var result = "";
    var first = true;
    for part in parts limit Iterations(18446744073709551615) {
        if !first { result = result + separator; }
        result = result + part;
        first = false;
    }
    return result;
}
"#,
        );
        package
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
    assert_eq!(package.load().input.sources.len(), 2);
    package.write(
        "decorate.es",
        r#"
module std.text;
public flow decorate(value: string) -> string { return "[" + value + "]"; }
public flow decorate_twice(value: string) -> string { return decorate(decorate(value)); }
"#,
    );
    let loaded = package.load();
    assert_eq!(loaded.input.sources.len(), 3);
    package.write(
        "consumer.es",
        r#"
module std.text.consumer;
import std.text.decorate;
public flow render(value: string) -> string { return decorate(value); }
"#,
    );
    let loaded = package.load();
    assert_eq!(loaded.input.sources.len(), 4);
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
        let maintained = etas_std::library::source_checkout_manifest();
        let library = maintained.parent().unwrap().join("library/std");
        fs::create_dir_all(package.0.join("library/std/math")).unwrap();
        fs::copy(
            library.join("math/complete.es"),
            package.0.join("library/std/math/mod.es"),
        )
        .unwrap();
        let message = fs::read_to_string(library.join("agent/message/complete.es")).unwrap();
        let start = message
            .find("@std.role(\"std.agent.message.Message\")")
            .unwrap();
        let end = message[start..].find(";\n").unwrap() + start + 2;
        let directory = package.0.join("library/std/agent/message");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("mod.es"),
            format!(
                "module std.agent.message;\n{}\n{}",
                &message[start..end],
                r#"
impl Message<Source> {
    public flow with_body<Target>(self, value: Target) -> Message<Target> ![] {
        return Message {
            body = value,
            content = value,
            id = self.id,
            from = self.from,
            role = self.role,
            session = self.session,
            created_at = self.created_at,
            provenance = self.provenance,
        };
    }
    public flow payload(self) -> Source ![] { return self.content; }
}
public flow replaced_message(value: Message<string>) -> Message<i32> ![] {
    return value.with_body(7);
}
"#
            ),
        )
        .unwrap();
        fs::create_dir_all(package.0.join("library/std/result")).unwrap();
        fs::copy(
            library.join("result/mod.es"),
            package.0.join("library/std/result/mod.es"),
        )
        .unwrap();
        let fs_source = fs::read_to_string(library.join("fs/mod.es")).unwrap();
        let fs_header = &fs_source[..fs_source.find("public flow path<").unwrap()];
        let fs_flows = [
            "public flow root<",
            "public flow relative_name<",
            "public flow read_bytes<",
            "public flow read_bytes_limited<",
            "public flow write_bytes<",
            "public flow list<",
            "public flow list_limited<",
            "public flow stat<",
            "public flow atomic_replace<",
        ]
        .map(|start| {
            let source = &fs_source[fs_source.find(start).unwrap()..];
            &source[..source.find("\n}\n").unwrap() + 3]
        });
        fs::create_dir_all(package.0.join("library/std/fs")).unwrap();
        fs::write(
            package.0.join("library/std/fs/mod.es"),
            format!("{fs_header}\n{}", fs_flows.join("\n")),
        )
        .unwrap();
        let effects = fs::read_to_string(library.join("effects/mod.es")).unwrap();
        let file_io = &effects[effects.find("@std.role(\"std.effects.FileIO\")").unwrap()..];
        let file_io = &file_io[..file_io.find(';').unwrap() + 1];
        let fs_effect = &effects[effects.find("@std.role(\"std.effects.Fs\")").unwrap()..];
        fs::create_dir_all(package.0.join("library/std/effects")).unwrap();
        fs::write(package.0.join("library/std/effects/mod.es"), format!("module std.effects;\nimport std.fs.{{Region, WorkspacePath, FsStat}};\n{file_io}\n{fs_effect}")).unwrap();
        let collections = fs::read_to_string(library.join("collections/mod.es")).unwrap();
        let range = &collections[collections
            .find("@std.role(\"std.collections.Range\")")
            .unwrap()..];
        let range = &range[..range.find("\n};").unwrap() + 3];
        let algorithms = fs::read_to_string(library.join("collections/algorithms.es")).unwrap();
        let iteration = fs::read_to_string(library.join("collections/iteration.es")).unwrap();
        let iterable = &iteration[iteration.find("public spec Iterable<T>").unwrap()..];
        let iterable = &iterable[..iterable.find("\n}\n").unwrap() + 3];
        let reverse = &algorithms[algorithms
            .find("impl List<T> {\n    public flow reverse")
            .unwrap()..];
        let reverse = &reverse[..reverse.find("\n}\n").unwrap() + 3];
        let constructors = &algorithms[algorithms.find("impl Range<T> {").unwrap()..];
        let constructors = &constructors[..constructors.find("\n}\n").unwrap() + 3];
        let target = package.0.join("library/std/collections/range.es");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let sequence_roles = &collections[..collections
            .find("@std.role(\"std.collections.Range\")")
            .unwrap()];
        let slice_role = &collections[collections
            .find("@std.role(\"std.collections.Slice\")")
            .unwrap()
            ..collections
                .find("@std.role(\"std.collections.Iter\")")
                .unwrap()];
        fs::write(
            target.parent().unwrap().join("primitives.es"),
            format!("{sequence_roles}\n{slice_role}\n{iterable}\n{reverse}"),
        )
        .unwrap();
        for relative in ["collections/order.es", "collections/ordered.es"] {
            fs::copy(
                library.join(relative),
                package.0.join("library/std").join(relative),
            )
            .unwrap();
        }
        fs::write(
            target,
            format!("module std.collections;\n{range}\n{constructors}\n"),
        )
        .unwrap();
        for relative in [
            "runtime/error/complete.es",
            "runtime/budget/mod.es",
            "runtime/mod.es",
            "runtime/checkpoint/mod.es",
        ] {
            let target = package.0.join("library/std").join(relative);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::copy(library.join(relative), target).unwrap();
        }
        let data = fs::read_to_string(library.join("runtime/value/mod.es")).unwrap();
        let data = &data[..data.find("    public flow tagged(").unwrap()];
        let target = package.0.join("library/std/runtime/value/mod.es");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, format!("{data}\n}}\n")).unwrap();
        let json = fs::read_to_string(library.join("json/complete.es")).unwrap();
        let json = &json[..json.find("public flow parse(").unwrap()];
        fs::create_dir_all(package.0.join("library/std/json")).unwrap();
        fs::write(package.0.join("library/std/json/mod.es"), json).unwrap();
        for relative in ["memory/context.es", "stream/clock.es"] {
            let target = package.0.join("library/std").join(relative);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::copy(library.join(relative), target).unwrap();
        }
        let memory = fs::read_to_string(library.join("memory/complete.es")).unwrap();
        let error = memory
            .lines()
            .find(|line| line.starts_with("public type StorageError ="))
            .unwrap();
        fs::write(
            package.0.join("library/std/memory/error.es"),
            format!("module std.memory;\n{error}\n"),
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
    public flow count(self) -> usize ![] { return self.value.length; }
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
        fs::create_dir_all(package.0.join("library/std/core")).unwrap();
        let core = fs::read_to_string(library.join("core/mod.es")).unwrap();
        let termination = &core[..core.find("@std.role(\"std.core.Index\")").unwrap()];
        fs::write(
            package.0.join("library/std/core/termination.es"),
            termination,
        )
        .unwrap();
        fs::write(
            package.0.join("library/std/core/mod.es"),
            r#"
module std.core;
@std.role("std.core.Index")
public spec Index { flow to_usize(self) -> Option<usize> ![]; }
impl usize ~ Index {
    public flow to_usize(self) -> Option<usize> ![] { return Some(self); }
}
impl bytes {
    public flow size(self) -> usize ![] { return self.length; }
}
public flow byte_count(value: bytes) -> usize ![] { return value.size(); }
public flow checked_index<I ~ Index>(value: I) -> Option<usize> ![] {
    return value.to_usize();
}
public flow builtin_index() -> Option<usize> ![] {
    let value: usize = 3;
    return std.core.checked_index(std.text.generic_echo(value));
}
"#,
        )
        .unwrap();
        fs::create_dir_all(package.0.join("library/std/collections")).unwrap();
        fs::write(
            package.0.join("library/std/collections/mod.es"),
            r#"
module std.collections;
@std.role("std.collections.Iter")
public type Iter<Element> = { next: () -> Option<(Element, Iter<Element>)> ![] };
public flow numbers(start: i32, end: i32) -> Iter<i32> ![] {
    return Iter { next = () => {
        if start >= end { return None; }
        return Some((start, numbers(start + 1, end)));
    } };
}
public flow sum() -> i32 ![] {
    var total: i32 = 0;
    for value in numbers(1, 5) limit Iterations(4) { total = total + value; }
    return total;
}
"#,
        )
        .unwrap();
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
        .find(|method| method.name == "replace")
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
        .find(|method| method.name == "update")
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
    let money = linked
        .types
        .known_std_types
        .runtime_type(etas_std::RuntimeTypeRole::Money)
        .unwrap();
    assert!(
        matches!(linked.type_store.get(money), Some(etas_types::Type::Nominal(record)) if record.representation.is_some())
    );
    let transform = linked
        .types
        .type_impl_methods
        .values()
        .find(|method| method.name == "apply")
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
        .find(|method| method.name == "capture")
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
    let mut primitives = 0;
    for item in first.declarations.values() {
        if first.types.item_signatures.contains_key(item) {
            if let etas_hir::HirItem::Flow(etas_hir::HirFlowDecl {
                body: etas_hir::HirFlowBody::Runtime { role, span },
                ..
            }) = &first.program.items[*item]
            {
                primitives += 1;
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
            } else {
                assert!(linked.definition_sources.body(*item).is_some());
            }
            assert!(linked.effects.item_effects.contains_key(item));
        }
    }
    assert_eq!(primitives, 2);
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
                declaration.runtime_role.unwrap(),
                checked
                    .type_store
                    .get(etas_types::TypeId(constructor.0))
                    .unwrap()
            ),
            (etas_std::RuntimeTypeRole::Task, etas_types::Type::Task(_))
                | (
                    etas_std::RuntimeTypeRole::Scheduler,
                    etas_types::Type::SchedulerHandle
                )
                | (
                    etas_std::RuntimeTypeRole::Continuation,
                    etas_types::Type::Continuation(_)
                )
        ));
    }
}

#[test]
fn native_enum_payload_requirement_is_checked_at_the_source_variant() {
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
    let file = loaded
        .input
        .sources
        .iter()
        .find(|file| file.text == source)
        .unwrap()
        .id;
    let output = Frontend.check_standard_library_source(loaded.input);
    assert!(output.checked.is_none());
    let diagnostic = output
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic
                .message
                .contains("native enum ABI requires unit variant `ParseFailure`")
        })
        .unwrap();
    assert_eq!(diagnostic.primary.span.source, file);
    let range = diagnostic.primary.span.range;
    assert!(source[range.start.0 as usize..range.end.0 as usize].contains("ParseFailure(string)"));
}

#[test]
fn source_filesystem_types_keep_region_bounds_and_native_read_actions() {
    let package = SourcePackage::new();
    fs::create_dir_all(package.0.join("library/std/fs")).unwrap();
    let maintained = etas_std::library::source_checkout_manifest()
        .parent()
        .unwrap();
    fs::create_dir_all(package.0.join("library/std/effects")).unwrap();
    let effects = fs::read_to_string(maintained.join("library/std/effects/mod.es")).unwrap();
    let file_io = &effects[effects.find("@std.role(\"std.effects.FileIO\")").unwrap()..];
    let file_io = &file_io[..file_io.find(';').unwrap() + 1];
    let fs_effect = &effects[effects.find("@std.role(\"std.effects.Fs\")").unwrap()..];
    fs::write(package.0.join("library/std/effects/mod.es"),
        format!("module std.effects;\nimport std.fs.{{Region, WorkspacePath, FsStat}};\n{file_io}\n{fs_effect}")
            .replace("<R ~ Region>", "<Scope ~ Region>")
            .replace("WorkspacePath<R>", "WorkspacePath<Scope>"),
    ).unwrap();
    let source = fs::read_to_string(maintained.join("library/std/fs/mod.es")).unwrap();
    let header = &source[..source.find("public flow path<").unwrap()];
    let methods = [
        "root",
        "relative_name",
        "read_bytes",
        "read_bytes_limited",
        "write_bytes",
        "list",
        "list_limited",
        "stat",
        "atomic_replace",
    ]
    .map(|name| {
        let source = &source[source.find(&format!("public flow {name}<")).unwrap()..];
        &source[..source.find("\n}\n").unwrap() + 3]
    });
    let source = format!("{header}\n{}", methods.join("\n"));
    let library = maintained.join("library/std");
    fs::create_dir_all(package.0.join("library/std/collections")).unwrap();
    let collections = fs::read_to_string(library.join("collections/mod.es")).unwrap();
    let sequences = &collections[..collections
        .find("@std.role(\"std.collections.Range\")")
        .unwrap()];
    let iterator = &collections[collections
        .rfind("@std.role(\"std.collections.Iter\")")
        .unwrap()..];
    let algorithms = fs::read_to_string(library.join("collections/algorithms.es")).unwrap();
    let reverse = &algorithms[algorithms
        .find("impl List<T> {\n    public flow reverse")
        .unwrap()..];
    let reverse = &reverse[..reverse.find("\n}\n").unwrap() + 3];
    let iteration = fs::read_to_string(library.join("collections/iteration.es")).unwrap();
    let iterable = &iteration[iteration.find("public spec Iterable<T>").unwrap()..];
    let iterable = &iterable[..iterable.find("\n}\n").unwrap() + 3];
    fs::write(
        package.0.join("library/std/collections/mod.es"),
        format!("{sequences}\n{iterator}\n{iterable}\n{reverse}\n"),
    )
    .unwrap();
    for relative in [
        "collections/order.es",
        "collections/ordered.es",
        "result/mod.es",
        "option/mod.es",
    ] {
        let target = package.0.join("library/std").join(relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(library.join(relative), target).unwrap();
    }
    let core = fs::read_to_string(library.join("core/mod.es")).unwrap();
    let termination = &core[..core.find("@std.role(\"std.core.Index\")").unwrap()];
    fs::create_dir_all(package.0.join("library/std/core")).unwrap();
    fs::write(package.0.join("library/std/core/mod.es"), termination).unwrap();

    fs::write(
        package.0.join("library/std/fs/mod.es"),
        source
            .replace(
                "public type WorkspacePath<R ~ Region>",
                "public type WorkspacePath<Authority ~ Region>",
            )
            .replace(
                "public enum ReadLimitError {",
                "public enum ReadLimitError { SdkOnly,",
            ),
    )
    .unwrap();
    let output = Frontend.check_standard_library_source(package.load().input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let path = checked
        .std_registry
        .lookup_qualified(&["std", "fs", "WorkspacePath"])
        .unwrap();
    let etas_std::StdDecl::Type(declaration) = &path.decl else {
        unreachable!()
    };
    assert_eq!(declaration.params[0].name, "Authority");
    assert_eq!(
        declaration.params[0].bounds[0].path,
        ["std", "fs", "Region"]
    );
    // IOError is an ordinary source nominal, without a language role or copied declaration.
    assert!(
        checked
            .std_registry
            .lookup_qualified(&["std", "fs", "IOError"])
            .is_none()
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
        "Scope"
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
    assert!(matches!(
        checked.type_store.get(signature.params[1]),
        Some(etas_types::Type::Option(_))
    ));
    assert!(!checked.types.std_intrinsic_calls.values().any(|call| {
        call.qualified_path
            .starts_with(&["std".into(), "fs".into()])
    }));
}
