use std::{error::Error, path::PathBuf};

use etas_frontend::{
    Frontend, ProjectEntry, ProjectSourceLoadOptions, ProjectSourceLoadScope, ProjectSourceLoader,
};

fn main() -> Result<(), Box<dyn Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if let [flag, path, root] = args.as_slice()
        && flag == "--load-library"
    {
        let image = etas_frontend::library_hir::LibraryHirImage::decode(&std::fs::read(path)?)?;
        let checked = image.into_checked_from_image(std::path::Path::new(root))?;
        println!(
            "Restored checked library: {} declarations, {} effect summaries, {} sources",
            checked.hir.items.len(),
            checked.effects.item_effects.len(),
            checked.sources.sources.len()
        );
        return Ok(());
    }
    if let [flag, path] = args.as_slice()
        && flag == "--read-hir"
    {
        let image = etas_frontend::library_hir::LibraryHirImage::decode(&std::fs::read(path)?)?;
        let declarations = image.declarations.len();
        let sources = image.sources.len();
        let signatures = image.types.item_signatures.len();
        let effects = image.effects.item_effects.len();
        let revision = image.into_revision()?;
        println!(
            "Loaded {declarations} declarations from {sources} sources; {} HIR items, {signatures} typed signatures, {effects} effect summaries",
            revision.items.len()
        );
        return Ok(());
    }
    let root = PathBuf::from(args.first().ok_or(
        "usage: check_std_source <source-root> [<package-name> <package-version> <hir-image>]",
    )?)
    .canonicalize()?;
    let mut loaded = ProjectSourceLoader::default().load(ProjectSourceLoadOptions {
        project_root: root.clone(),
        roots: vec![root.clone()],
        entry: ProjectEntry {
            module: None,
            flow: "main".into(),
        },
        scope: ProjectSourceLoadScope::FullSourceTree,
        source_kind: None,
        import_search_roots: vec![],
        external_module_roots: vec![],
    })?;
    let package_root = root.parent().expect("source root has a package parent");
    let image_output = if let [_, name, version, image] = args.as_slice() {
        let package = etas_code::PackageKey::from_name_and_version(name, version)?;
        loaded.input.environment.local_source_package = Some(package);
        loaded.input.environment.stable_source_keys = loaded
            .input
            .sources
            .iter()
            .map(|source| {
                let relative = source.path.as_ref().unwrap().strip_prefix(package_root)?;
                let path = relative
                    .components()
                    .map(|part| part.as_os_str().to_str().unwrap().to_owned())
                    .collect::<Vec<_>>();
                Ok(etas_frontend::ProjectStableSourceKeyInput {
                    source: source.id,
                    key: etas_code::StableSourceKey::new(
                        etas_code::StableSourceOrigin::Package(package),
                        &path,
                    )
                    .map_err(|e| format!("{e:?}"))?,
                })
            })
            .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
        Some((package, image))
    } else {
        None
    };
    let output = Frontend.check_standard_library_source(loaded.input);
    for diagnostic in &output.diagnostics {
        eprintln!("{diagnostic:?}");
    }
    if output.checked.is_none() {
        return Err("std source library did not pass checking".into());
    }
    if let Some((package, path)) = image_output {
        let image = etas_frontend::library_hir::LibraryHirImage::from_checked(
            output.checked.as_ref().unwrap(),
            package,
            package_root,
        )?;
        std::fs::write(path, image.encode()?)?;
    }
    println!(
        "Checked {} source files in {} modules",
        output.sources.as_ref().unwrap().files.len(),
        output.modules.as_ref().unwrap().modules.len()
    );
    Ok(())
}
