use std::{
    collections::{HashMap, HashSet, VecDeque},
    io,
    path::{Path, PathBuf},
};

use etas_core::{SourceFile as CoreSourceFile, SourceId};
use etas_syntax::ast;

use crate::{
    ModulePath, ProjectEntry, ProjectEnvironmentInput, ProjectInput, SourceInput, SourceKind,
};

#[derive(Clone, Debug)]
pub struct ProjectSourceLoadOptions {
    pub project_root: PathBuf,
    pub roots: Vec<PathBuf>,
    pub entry: ProjectEntry,
    pub scope: ProjectSourceLoadScope,
    pub source_kind: Option<SourceKind>,
    pub import_search_roots: Vec<PathBuf>,
    pub external_module_roots: Vec<ModulePath>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProjectSourceLoadScope {
    #[default]
    FullSourceTree,
    ImportClosureFromEntry,
}

#[derive(Clone, Debug)]
pub struct LoadedProjectInput {
    pub input: ProjectInput,
    pub sources: Vec<(PathBuf, CoreSourceFile)>,
}

pub trait SourceLoader {
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf>;
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    fn is_file(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FsSourceLoader;

impl SourceLoader for FsSourceLoader {
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        path.canonicalize()
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        std::fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct ProjectSourceLoader<L = FsSourceLoader> {
    source_loader: L,
}

impl Default for ProjectSourceLoader<FsSourceLoader> {
    fn default() -> Self {
        Self {
            source_loader: FsSourceLoader,
        }
    }
}

impl<L: SourceLoader> ProjectSourceLoader<L> {
    pub fn new(source_loader: L) -> Self {
        Self { source_loader }
    }

    pub fn load(&self, options: ProjectSourceLoadOptions) -> io::Result<LoadedProjectInput> {
        let plan = self.load_plan(&options)?;
        if plan.collect_source_files && options.scope == ProjectSourceLoadScope::FullSourceTree {
            let paths = self.collect_source_files(&plan.source_root)?;
            return self.load_project_paths(
                plan.project_root,
                Some(plan.source_root),
                paths,
                plan.entry,
                SourceKind::SourceProjectFile,
            );
        }

        let source_root = plan.source_root.clone();
        let allowed_roots = self.allowed_roots(
            &plan.project_root,
            &source_root,
            &options.roots,
            &options.import_search_roots,
        )?;
        let mut loaded_paths = HashSet::new();
        let mut modules = HashMap::new();
        let mut queue = VecDeque::new();
        let mut discovered_paths = Vec::new();

        for root in seed_roots_for_plan(&options, &plan, self)? {
            queue.push_back(root);
        }

        while let Some(path) = queue.pop_front() {
            let path = self.source_loader.canonicalize(&path)?;
            if !loaded_paths.insert(path.clone()) {
                continue;
            }

            let text = self.source_loader.read_to_string(&path)?;
            let source = CoreSourceFile::new(SourceId(0), Some(path.clone()), text.clone());
            let parsed = etas_syntax::parse_program(source.clone());
            if let Some(module) = parsed
                .value
                .module
                .as_ref()
                .map(|module| ModulePath::from_ast(&module.path))
            {
                for part in self.module_part_files(&source_root, &module, &allowed_roots)? {
                    queue.push_back(part);
                }
                modules.insert(module, path.clone());
            }

            for import in &parsed.value.imports {
                for candidate in import_module_candidates(&import.tree) {
                    if is_external_module(&candidate, &options.external_module_roots)
                        || modules.contains_key(&candidate)
                    {
                        continue;
                    }
                    for path in self.resolve_module_files(
                        &source_root,
                        &options.import_search_roots,
                        &allowed_roots,
                        &candidate,
                    )? {
                        queue.push_back(path);
                    }
                }
            }

            discovered_paths.push(path);
        }

        discovered_paths.sort();
        discovered_paths.dedup();
        let source_kind = options.source_kind.unwrap_or({
            if discovered_paths.len() == 1 {
                SourceKind::SingleFileInput
            } else {
                SourceKind::SourceProjectFile
            }
        });
        self.load_project_paths(
            plan.project_root,
            Some(plan.source_root),
            discovered_paths,
            plan.entry,
            source_kind,
        )
    }

    fn load_project_paths(
        &self,
        project_root: PathBuf,
        source_root: Option<PathBuf>,
        paths: Vec<PathBuf>,
        entry: ProjectEntry,
        source_kind: SourceKind,
    ) -> io::Result<LoadedProjectInput> {
        let mut sources = Vec::new();
        for (index, path) in paths.into_iter().enumerate() {
            let path = self.source_loader.canonicalize(&path)?;
            let text = self.source_loader.read_to_string(&path)?;
            let id = SourceId(index.min(u32::MAX as usize) as u32 + 1);
            let source = CoreSourceFile::new(id, Some(path.clone()), text);
            sources.push((path, source));
        }

        let input_sources = sources
            .iter()
            .map(|(_, source)| SourceInput {
                id: source.id,
                path: source.path.clone(),
                text: source.text().to_owned(),
                kind: source_kind.clone(),
            })
            .collect();

        Ok(LoadedProjectInput {
            input: ProjectInput {
                project_root,
                source_root,
                options: Default::default(),
                environment: ProjectEnvironmentInput::default(),
                sources: input_sources,
                entry,
            },
            sources,
        })
    }

    fn load_plan(&self, options: &ProjectSourceLoadOptions) -> io::Result<ProjectLoadPlan> {
        let input_root = self.input_package_root(options)?;
        let source_root = self.source_root(options, &input_root)?;
        let collect_source_files = options.roots.is_empty()
            || options
                .roots
                .iter()
                .any(|root| self.source_loader.is_dir(root));

        Ok(ProjectLoadPlan {
            project_root: input_root,
            source_root,
            entry: options.entry.clone(),
            collect_source_files,
        })
    }

    fn input_package_root(&self, options: &ProjectSourceLoadOptions) -> io::Result<PathBuf> {
        self.source_loader.canonicalize(&options.project_root)
    }

    fn source_root(
        &self,
        options: &ProjectSourceLoadOptions,
        project_root: &Path,
    ) -> io::Result<PathBuf> {
        if options.roots.is_empty() {
            return Ok(project_root.to_path_buf());
        }

        if options.roots.len() == 1 {
            let root = &options.roots[0];
            if self.source_loader.is_dir(root) {
                return self.source_loader.canonicalize(root);
            }
            if self.source_loader.is_file(root) {
                return self.infer_source_root_from_file(root);
            }
        }

        Ok(project_root.to_path_buf())
    }

    fn infer_source_root_from_file(&self, path: &Path) -> io::Result<PathBuf> {
        let path = self.source_loader.canonicalize(path)?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let text = self.source_loader.read_to_string(&path)?;
        let source = CoreSourceFile::new(SourceId(0), Some(path.clone()), text);
        let parsed = etas_syntax::parse_program(source);
        let Some(module) = parsed
            .value
            .module
            .as_ref()
            .map(|module| ModulePath::from_ast(&module.path))
        else {
            return self.source_loader.canonicalize(parent);
        };

        let mut root = parent.to_path_buf();
        let pop_count = if path.file_name().and_then(|name| name.to_str()) == Some("mod.es") {
            module.segments.len()
        } else {
            module.segments.len().saturating_sub(1)
        };
        for _ in 0..pop_count {
            if !root.pop() {
                return self.source_loader.canonicalize(parent);
            }
        }
        self.source_loader.canonicalize(&root)
    }

    fn collect_source_files(&self, source_root: &Path) -> io::Result<Vec<PathBuf>> {
        let mut paths = Vec::new();
        self.collect_source_files_inner(source_root, &mut paths)?;
        let mut canonical = paths
            .into_iter()
            .map(|path| self.source_loader.canonicalize(&path))
            .collect::<io::Result<Vec<_>>>()?;
        canonical.sort();
        canonical.dedup();
        Ok(canonical)
    }

    fn collect_source_files_inner(
        &self,
        path: &Path,
        sources: &mut Vec<PathBuf>,
    ) -> io::Result<()> {
        for entry in self.source_loader.read_dir(path)? {
            if self.source_loader.is_dir(&entry) {
                self.collect_source_files_inner(&entry, sources)?;
            } else if entry.extension().and_then(|ext| ext.to_str()) == Some("es") {
                sources.push(entry);
            }
        }
        Ok(())
    }

    fn resolve_module_files(
        &self,
        source_root: &Path,
        import_search_roots: &[PathBuf],
        allowed_roots: &[PathBuf],
        path: &ModulePath,
    ) -> io::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        let mut relative = PathBuf::new();
        for segment in &path.segments {
            relative.push(segment);
        }
        let file = source_root.join(relative.with_extension("es"));
        if self.is_allowed_file(&file, allowed_roots)? {
            files.push(file);
        }

        let mut mod_file = source_root.to_path_buf();
        for segment in &path.segments {
            mod_file.push(segment);
        }
        mod_file.push("mod.es");
        if self.is_allowed_file(&mod_file, allowed_roots)? {
            files.push(mod_file);
        }

        files.extend(self.module_part_files(source_root, path, allowed_roots)?);
        for search_root in import_search_roots {
            files.extend(self.resolve_module_files_from_search_root(
                search_root,
                path,
                allowed_roots,
            )?);
        }
        Ok(files)
    }

    fn resolve_module_files_from_search_root(
        &self,
        search_root: &Path,
        path: &ModulePath,
        allowed_roots: &[PathBuf],
    ) -> io::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        for start in 0..path.segments.len() {
            let mut relative = PathBuf::new();
            for segment in &path.segments[start..] {
                relative.push(segment);
            }

            let file = search_root.join(relative.with_extension("es"));
            if self.is_allowed_file(&file, allowed_roots)? {
                files.push(file);
            }

            let mut mod_file = search_root.to_path_buf();
            for segment in &path.segments[start..] {
                mod_file.push(segment);
            }
            mod_file.push("mod.es");
            if self.is_allowed_file(&mod_file, allowed_roots)? {
                files.push(mod_file);
            }
        }
        Ok(files)
    }

    fn module_part_files(
        &self,
        source_root: &Path,
        path: &ModulePath,
        allowed_roots: &[PathBuf],
    ) -> io::Result<Vec<PathBuf>> {
        let mut dir = source_root.to_path_buf();
        for segment in &path.segments {
            dir.push(segment);
        }
        let entries = match self.source_loader.read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        let mut files = entries
            .into_iter()
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("es"))
            .filter(|path| path.file_name().and_then(|name| name.to_str()) != Some("mod.es"))
            .filter_map(|path| match self.is_allowed_file(&path, allowed_roots) {
                Ok(true) => Some(Ok(path)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<io::Result<Vec<_>>>()?;
        files.sort();
        Ok(files)
    }

    fn allowed_roots(
        &self,
        project_root: &Path,
        source_root: &Path,
        roots: &[PathBuf],
        import_search_roots: &[PathBuf],
    ) -> io::Result<Vec<PathBuf>> {
        let mut allowed = Vec::new();
        self.push_allowed_root(&mut allowed, project_root)?;
        self.push_allowed_root(&mut allowed, source_root)?;
        for root in roots {
            let allowed_root = if self.source_loader.is_file(root) {
                root.parent().unwrap_or_else(|| Path::new("."))
            } else {
                root.as_path()
            };
            self.push_allowed_root(&mut allowed, allowed_root)?;
        }
        for root in import_search_roots {
            self.push_allowed_root(&mut allowed, root)?;
        }
        Ok(allowed)
    }

    fn push_allowed_root(&self, allowed: &mut Vec<PathBuf>, root: &Path) -> io::Result<()> {
        let root = match self.source_loader.canonicalize(root) {
            Ok(root) => root,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if !allowed.iter().any(|existing| existing == &root) {
            allowed.push(root);
        }
        Ok(())
    }

    fn is_allowed_file(&self, path: &Path, allowed_roots: &[PathBuf]) -> io::Result<bool> {
        if !self.source_loader.is_file(path) {
            return Ok(false);
        }
        let path = self.source_loader.canonicalize(path)?;
        Ok(allowed_roots.iter().any(|root| path.starts_with(root)))
    }
}

#[derive(Clone, Debug)]
struct ProjectLoadPlan {
    project_root: PathBuf,
    source_root: PathBuf,
    entry: ProjectEntry,
    collect_source_files: bool,
}

fn seed_roots_for_plan<L: SourceLoader>(
    options: &ProjectSourceLoadOptions,
    plan: &ProjectLoadPlan,
    loader: &ProjectSourceLoader<L>,
) -> io::Result<Vec<PathBuf>> {
    if options.scope != ProjectSourceLoadScope::ImportClosureFromEntry {
        return Ok(options.roots.clone());
    }
    let Some(module) = options.entry.module.as_ref() else {
        return Ok(options.roots.clone());
    };
    let candidates = loader.resolve_module_files(
        &plan.source_root,
        &options.import_search_roots,
        &loader.allowed_roots(
            &plan.project_root,
            &plan.source_root,
            &options.roots,
            &options.import_search_roots,
        )?,
        module,
    )?;
    if candidates.is_empty() {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "entry module {} could not be resolved from source root {}",
                module.segments.join("."),
                plan.source_root.display()
            ),
        ))
    } else {
        Ok(candidates)
    }
}

fn import_module_candidates(tree: &ast::ImportTree) -> Vec<ModulePath> {
    match tree {
        ast::ImportTree::Single { path, .. } => {
            let segments = ModulePath::from_ast(path).segments;
            (1..=segments.len())
                .rev()
                .map(|len| ModulePath {
                    segments: segments.iter().take(len).cloned().collect(),
                })
                .collect()
        }
        ast::ImportTree::Group { prefix, .. } | ast::ImportTree::Wildcard { prefix, .. } => {
            vec![ModulePath::from_ast(prefix)]
        }
        ast::ImportTree::Error { .. } => Vec::new(),
    }
}

fn is_external_module(path: &ModulePath, external_roots: &[ModulePath]) -> bool {
    external_roots
        .iter()
        .any(|root| path.segments.starts_with(&root.segments))
}
