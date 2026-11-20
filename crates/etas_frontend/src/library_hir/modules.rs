//! Installation units retain the source files that own resolved declarations,
//! calls and methods. Prelude names come from the checked source declarations;
//! each unit receives the corresponding source imports. Ordinary checking
//! validates every selected closure before installation.

use super::LibraryHirImage;
use etas_core::SourceFile;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Clone, Debug)]
pub struct LibraryModuleCompilation {
    pub modules: BTreeSet<Vec<String>>,
    pub input: crate::ProjectInput,
}

impl LibraryHirImage {
    pub fn module_compilations(&self, root: &Path) -> Vec<LibraryModuleCompilation> {
        use etas_hir::{HirExpr, HirItem, HirType};
        let files = self
            .sources
            .iter()
            .map(|source| (source.id.0, source))
            .collect::<BTreeMap<_, _>>();
        let prelude = self
            .module_index
            .by_path
            .get(&crate::ModulePath {
                segments: vec!["std".into(), "prelude".into()],
            })
            .copied();
        let mut names = BTreeMap::<String, BTreeSet<u32>>::new();
        let target_files = |target: &crate::ImportTarget| -> Vec<u32> {
            match target {
                crate::ImportTarget::SourceItem { item, .. } => vec![item.source.0],
                crate::ImportTarget::Module(crate::ResolvedModuleTarget::Source {
                    module, ..
                }) => self.module_index.modules[*module]
                    .parts
                    .iter()
                    .map(|part| self.module_index.parts[*part].source.0)
                    .min_by_key(|id| files[id].text.len())
                    .into_iter()
                    .collect(),
                _ => Vec::new(),
            }
        };
        for import in &self.resolved_imports.imports {
            if Some(import.from) == prelude {
                names
                    .entry(import.local_name.clone())
                    .or_default()
                    .extend(target_files(&import.target));
            }
        }
        for import in &self.resolved_imports.wildcard_imports {
            if Some(import.from) == prelude {
                for (name, target) in &import.targets {
                    names
                        .entry(name.clone())
                        .or_default()
                        .extend(target_files(target));
                }
            }
        }
        let base = self
            .sources
            .iter()
            .filter(|source| {
                let module = self.module_index.by_source[&source.id];
                self.module_index.modules[module].path.segments == ["std", "core"]
            })
            .map(|source| source.id.0)
            .collect::<BTreeSet<_>>();
        let mut dependencies = BTreeMap::<u32, BTreeSet<u32>>::new();
        for (id, source) in &files {
            let mut referenced = base.clone();
            let file = SourceFile::new(source.id, None, source.text.clone());
            for token in etas_syntax::lex(&file).significant_tokens() {
                if token.kind.is_ident_like() {
                    let span = token.span.range;
                    let name = &source.text[span.start.to_usize()..span.end.to_usize()];
                    if let Some(targets) = names.get(name) {
                        referenced.extend(targets);
                    }
                }
            }
            dependencies.insert(*id, referenced);
        }
        for import in &self.resolved_imports.imports {
            dependencies
                .get_mut(&import.import.source.0)
                .expect("source import owner")
                .extend(target_files(&import.target));
        }
        for import in &self.resolved_imports.wildcard_imports {
            for target in import.targets.values() {
                dependencies
                    .get_mut(&import.import.source.0)
                    .expect("source import owner")
                    .extend(target_files(target));
            }
        }
        let source_of = |symbol: etas_hir::SymbolId| {
            let symbol = self
                .program
                .canonical_source_symbol(symbol)
                .unwrap_or(symbol);
            self.program
                .symbols
                .get(symbol)
                .and_then(|symbol| symbol.defining_item)
                .map(|item| self.program.items[item].span().source.0)
                .filter(|id| files.contains_key(id))
        };
        let mut reference = |source: etas_core::SourceId, resolution: &etas_hir::ResolveResult| {
            if let etas_hir::ResolveResult::Resolved(symbol) = resolution
                && let Some(target) = source_of(*symbol)
            {
                dependencies
                    .get_mut(&source.0)
                    .expect("checked reference owner")
                    .insert(target);
            }
        };
        for symbol in self.program.symbols.iter() {
            if let etas_hir::SymbolDef::TypeParam { bounds, .. } = &symbol.def {
                for bound in bounds {
                    reference(bound.path.span.source, &bound.path.resolution);
                }
            }
        }
        for (_, ty) in self.program.types.iter() {
            match ty {
                HirType::Path { path, .. } => reference(path.span.source, &path.resolution),
                HirType::Existential { bounds, .. } => {
                    for bound in bounds {
                        reference(bound.path.span.source, &bound.path.resolution);
                    }
                }
                _ => {}
            }
        }
        for (_, expr) in self.program.exprs.iter() {
            match expr {
                HirExpr::Path(path) => reference(path.span.source, &path.resolution),
                HirExpr::Perform { action, .. } | HirExpr::Infer { action, .. } => {
                    reference(action.span.source, &action.effect.path.resolution)
                }
                _ => {}
            }
        }
        for (_, item) in self.program.items.iter() {
            match item {
                HirItem::Impl(implementation) => match &implementation.target {
                    etas_hir::HirImplTarget::Inherent { target, .. } => {
                        reference(target.span.source, &target.resolution)
                    }
                    etas_hir::HirImplTarget::Agent { target, specs } => {
                        reference(target.span.source, &target.resolution);
                        for spec in specs {
                            reference(spec.spec_path.span.source, &spec.spec_path.resolution);
                        }
                    }
                    etas_hir::HirImplTarget::SpecSatisfaction { specs, .. } => {
                        for spec in specs {
                            reference(spec.spec_path.span.source, &spec.spec_path.resolution);
                        }
                    }
                    _ => {}
                },
                HirItem::Spec(spec) => {
                    for bound in &spec.bounds {
                        reference(bound.path.span.source, &bound.path.resolution);
                    }
                }
                HirItem::Effect(effect) => {
                    if let Some(parent) = &effect.extends {
                        reference(parent.path.span.source, &parent.path.resolution);
                    }
                }
                _ => {}
            }
        }
        for (expr, symbol) in &self.types.source_flow_calls {
            reference(
                self.program.exprs[*expr].span(&self.program.blocks).source,
                &etas_hir::ResolveResult::Resolved(*symbol),
            );
        }
        for (expr, method) in self
            .types
            .checked_method_calls
            .iter()
            .map(|(expr, method)| (*expr, method))
            .chain(
                self.types
                    .bound_method_values
                    .iter()
                    .map(|(expr, method)| (*expr, &method.method)),
            )
        {
            if let etas_types::method::MethodTarget::Source { symbol, .. }
            | etas_types::method::MethodTarget::Agent { symbol, .. } = &method.target
            {
                reference(
                    self.program.exprs[expr].span(&self.program.blocks).source,
                    &etas_hir::ResolveResult::Resolved(*symbol),
                );
            }
        }
        // Keep the checked implementation environment of every source spec.
        for (_, item) in self.program.items.iter() {
            if let HirItem::Impl(implementation) = item {
                let specs = match &implementation.target {
                    etas_hir::HirImplTarget::SpecSatisfaction { specs, .. }
                    | etas_hir::HirImplTarget::Agent { specs, .. } => specs,
                    _ => continue,
                };
                for spec in specs {
                    if let etas_hir::ResolveResult::Resolved(symbol) = &spec.spec_path.resolution
                        && let Some(owner) = source_of(*symbol)
                    {
                        dependencies
                            .get_mut(&owner)
                            .expect("source spec owner")
                            .insert(implementation.span.source.0);
                    }
                }
            }
        }
        // Overload ordinals are part of canonical callable identities.
        for symbol in self.program.symbols.iter() {
            if let Some(group) = self.program.flow_overload_group(symbol.id) {
                let sources = group
                    .iter()
                    .filter_map(|symbol| source_of(*symbol))
                    .collect::<BTreeSet<_>>();
                for source in &sources {
                    dependencies
                        .get_mut(source)
                        .expect("overload owner")
                        .extend(&sources);
                }
            }
        }
        if let Some(ty) = self.types.known_std_types.execution.continuation
            && let Some(key) = self.type_store.nominal_identity(ty)
            && let Some(item) = self.declarations.get(key)
        {
            let source = self.program.items[*item].span().source.0;
            for (_, arm) in self.program.handler_arms.iter() {
                dependencies
                    .get_mut(&arm.span.source.0)
                    .expect("handler owner")
                    .insert(source);
            }
        }
        let prelude_sources = self
            .sources
            .iter()
            .filter(|source| Some(self.module_index.by_source[&source.id]) == prelude)
            .map(|source| source.id.0)
            .collect::<BTreeSet<_>>();
        for referenced in dependencies.values_mut() {
            referenced.retain(|id| !prelude_sources.contains(id));
        }
        let roots = self
            .program
            .items
            .iter()
            .filter_map(|(_, item)| match item {
                HirItem::Flow(_)
                | HirItem::Tool(_)
                | HirItem::Agent(_)
                | HirItem::TopLevelLet(_) => Some(item.span().source.0),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let mut unique = BTreeSet::new();
        let mut compilations = Vec::new();
        for source in roots {
            let mut selected = BTreeSet::new();
            let mut pending = vec![source];
            while let Some(next) = pending.pop() {
                if selected.insert(next) {
                    pending.extend(&dependencies[&next]);
                }
            }
            if selected.len() == files.len() || !unique.insert(selected.clone()) {
                continue;
            }
            let prelude = self.selected_prelude(root, &selected);
            if let Some(source) = &prelude {
                selected.insert(source.id.0);
            }
            let modules = selected
                .iter()
                .map(|id| {
                    self.module_index.modules[self.module_index.by_source[&files[id].id]]
                        .path
                        .segments
                        .clone()
                })
                .collect();
            let mut environment = self.environment.clone();
            environment
                .stable_source_keys
                .retain(|key| selected.contains(&key.source.0));
            environment
                .standalone_source_packages
                .retain(|package| selected.contains(&package.source.0));
            environment.environment_fingerprint = None;
            environment.external_modules_fingerprint = None;
            let mut input = crate::ProjectInput {
                project_root: root.to_owned(),
                source_root: Some(root.to_owned()),
                options: self.compile_options.clone(),
                environment,
                sources: self
                    .sources
                    .iter()
                    .filter(|source| selected.contains(&source.id.0))
                    .map(|source| crate::SourceInput {
                        id: source.id,
                        path: Some(root.join(&source.relative_path)),
                        text: source.text.clone(),
                        kind: source.kind.clone(),
                    })
                    .collect(),
                entry: self.entry.requested.clone(),
            };
            if let Some(prelude) = prelude {
                let id = prelude.id;
                *input
                    .sources
                    .iter_mut()
                    .find(|source| source.id == id)
                    .expect("selected prelude unit") = prelude;
            }
            compilations.push(LibraryModuleCompilation { modules, input });
        }
        compilations
    }

    fn selected_prelude(
        &self,
        root: &Path,
        selected: &BTreeSet<u32>,
    ) -> Option<crate::SourceInput> {
        let (prelude, module) = self
            .module_index
            .modules
            .iter()
            .find(|(_, module)| module.path.segments == ["std", "prelude"])?;
        let source_id = self.module_index.parts[*module.parts.first()?].source;
        let source = self
            .sources
            .iter()
            .find(|source| source.id == source_id)
            .expect("checked prelude source");
        let native = self
            .native_registry
            .clone()
            .into_registry()
            .expect("checked native contracts");
        let mut lines = BTreeSet::new();
        let mut import = |name: &str, target: &crate::ImportTarget| {
            let path = match target {
                crate::ImportTarget::SourceItem {
                    module: _,
                    item,
                    member,
                    ..
                } => {
                    if !selected.contains(&item.source.0) {
                        return;
                    }
                    let item = self
                        .item_targets
                        .iter()
                        .find(|(source, _)| source == item)
                        .map(|(_, item)| *item)
                        .expect("checked prelude target");
                    let symbol = self.program.items[item]
                        .symbol()
                        .expect("named prelude target");
                    let mut path = self
                        .program
                        .declaration_path(symbol)
                        .expect("prelude declaration path");
                    if let Some(member) = member {
                        path.push(member.clone());
                    }
                    path
                }
                crate::ImportTarget::StdItem { symbol, .. } => native
                    .symbol(*symbol)
                    .expect("primitive prelude target")
                    .qualified_path
                    .clone(),
                crate::ImportTarget::Module(crate::ResolvedModuleTarget::Source {
                    module, ..
                }) => {
                    let path = &self.module_index.modules[*module].path.segments;
                    if !self.module_index.modules[*module]
                        .parts
                        .iter()
                        .any(|part| selected.contains(&self.module_index.parts[*part].source.0))
                    {
                        return;
                    }
                    path.clone()
                }
                _ => unreachable!("source std prelude has checked source or primitive targets"),
            };
            if path.last().map(String::as_str) == Some(name) {
                lines.insert(format!("public import {};", path.join(".")));
            } else {
                lines.insert(format!("public import {} as {name};", path.join(".")));
            }
        };
        for item in &self.resolved_imports.imports {
            if item.from == prelude && item.visibility == etas_hir::Visibility::Public {
                import(&item.local_name, &item.target);
            }
        }
        for item in &self.resolved_imports.wildcard_imports {
            if item.from == prelude && item.visibility == etas_hir::Visibility::Public {
                for (name, target) in &item.targets {
                    import(name, target);
                }
            }
        }
        Some(crate::SourceInput {
            id: source.id,
            path: Some(root.join(&source.relative_path)),
            text: format!(
                "module std.prelude;\n{}\n",
                lines.into_iter().collect::<Vec<_>>().join("\n")
            ),
            kind: source.kind.clone(),
        })
    }
}
