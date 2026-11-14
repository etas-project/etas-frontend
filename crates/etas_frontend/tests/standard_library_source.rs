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
import std.text.char_count;
public flow decorate(value: string) -> string { return "[" + value + "]"; }
public flow decorated_width(value: string) -> usize { return char_count(decorate(value)); }
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
    for flow in ["join", "decorate", "decorated_width"] {
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
        package.write(
            "generic.es",
            r#"
module std.text;
flow private_identity<T>(value: T) -> T { return value; }
public flow generic_echo<T>(value: T) -> T { return private_identity<T>(value); }
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
    let bytes = first.encode().unwrap();
    assert_eq!(bytes, make_image().encode().unwrap());
    let restored = LibraryHirImage::decode(&bytes).unwrap();
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
}
