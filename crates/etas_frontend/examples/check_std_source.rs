use std::{error::Error, path::PathBuf};

use etas_frontend::{
    Frontend, ProjectEntry, ProjectSourceLoadOptions, ProjectSourceLoadScope, ProjectSourceLoader,
};

fn main() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: check_std_source <source-root>")?,
    )
    .canonicalize()?;
    let loaded = ProjectSourceLoader::default().load(ProjectSourceLoadOptions {
        project_root: root.clone(),
        roots: vec![root],
        entry: ProjectEntry {
            module: None,
            flow: "main".into(),
        },
        scope: ProjectSourceLoadScope::FullSourceTree,
        source_kind: None,
        import_search_roots: vec![],
        external_module_roots: vec![],
    })?;
    let output = Frontend.check_standard_library_source(loaded.input);
    for diagnostic in &output.diagnostics {
        eprintln!("{diagnostic:?}");
    }
    if output.checked.is_none() {
        return Err("std source library did not pass checking".into());
    }
    println!(
        "Checked {} source files in {} modules",
        output.sources.as_ref().unwrap().files.len(),
        output.modules.as_ref().unwrap().modules.len()
    );
    Ok(())
}
