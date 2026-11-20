//! Installation units retain the source declarations that own resolved dependencies,
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
    pub fn module_compilations(
        &self,
        root: &Path,
        source_root: &Path,
    ) -> Vec<LibraryModuleCompilation> {
        use etas_hir::{HirExpr, HirItem, HirType};
        let files = self
            .sources
            .iter()
            .map(|source| (source.id.0, source))
            .collect::<BTreeMap<_, _>>();
        let parsed = files
            .iter()
            .map(|(id, source)| {
                let file = SourceFile::new(source.id, None, source.text.clone());
                let tokens = etas_syntax::lex(&file);
                let mut parse = etas_syntax::parse_program(file);
                // Declaration spans start at the declaration keyword; retain
                // a preceding visibility modifier when copying source text.
                for item in &mut parse.value.items {
                    if let Some(token) = tokens
                        .significant_tokens()
                        .take_while(|token| token.span.range.end <= item.span.range.start)
                        .last()
                        && matches!(
                            token.kind,
                            etas_syntax::TokenKind::Keyword(
                                etas_syntax::Keyword::Public | etas_syntax::Keyword::Private
                            )
                        )
                    {
                        item.span = token.span.cover(item.span);
                    }
                }
                (*id, parse.value)
            })
            .collect::<BTreeMap<_, _>>();
        let node_at = |span: etas_core::Span| {
            parsed
                .get(&span.source.0)?
                .items
                .iter()
                .enumerate()
                .find(|(_, item)| {
                    item.span.range.start <= span.range.start
                        && span.range.end <= item.span.range.end
                })
                .map(|(index, _)| (span.source.0, index))
        };
        let prelude = self
            .module_index
            .by_path
            .get(&crate::ModulePath {
                segments: vec!["std".into(), "prelude".into()],
            })
            .copied();
        let mut names = BTreeMap::<String, BTreeSet<(u32, usize)>>::new();
        let target_nodes = |target: &crate::ImportTarget| -> Vec<(u32, usize)> {
            match target {
                crate::ImportTarget::SourceItem { item, .. } => vec![(item.source.0, item.index)],
                crate::ImportTarget::Module(crate::ResolvedModuleTarget::Source {
                    module, ..
                }) => self.module_index.modules[*module]
                    .parts
                    .iter()
                    .flat_map(|part| &self.module_index.parts[*part].items)
                    .map(|item| (item.source.0, item.index))
                    .take(1)
                    .collect(),
                _ => Vec::new(),
            }
        };
        for import in &self.resolved_imports.imports {
            if Some(import.from) == prelude {
                names
                    .entry(import.local_name.clone())
                    .or_default()
                    .extend(target_nodes(&import.target));
            }
        }
        for import in &self.resolved_imports.wildcard_imports {
            if Some(import.from) == prelude {
                for (name, target) in &import.targets {
                    names
                        .entry(name.clone())
                        .or_default()
                        .extend(target_nodes(target));
                }
            }
        }
        let base = parsed
            .iter()
            .filter(|(id, _)| {
                let module = self.module_index.by_source[&files[id].id];
                self.module_index.modules[module].path.segments == ["std", "core"]
            })
            .flat_map(|(id, source)| (0..source.items.len()).map(move |index| (*id, index)))
            .collect::<BTreeSet<_>>();
        let mut local_names = BTreeMap::<u32, BTreeMap<String, Vec<(u32, usize)>>>::new();
        for import in &self.resolved_imports.imports {
            local_names
                .entry(import.import.source.0)
                .or_default()
                .insert(import.local_name.clone(), target_nodes(&import.target));
        }
        for import in &self.resolved_imports.wildcard_imports {
            for (name, target) in &import.targets {
                local_names
                    .entry(import.import.source.0)
                    .or_default()
                    .insert(name.clone(), target_nodes(target));
            }
        }
        // Effect type arguments also use static-path syntax, so they do not
        // necessarily appear in the HIR type arena. Retain their declarations
        // through the checked module names and qualified source paths.
        let mut qualified = BTreeMap::<Vec<String>, BTreeSet<(u32, usize)>>::new();
        let mut module_names =
            BTreeMap::<Vec<String>, BTreeMap<String, BTreeSet<(u32, usize)>>>::new();
        for (source, item) in &self.item_targets {
            let Some(symbol) = self.program.items[*item].symbol() else {
                continue;
            };
            let Some(path) = self.program.declaration_path(symbol) else {
                continue;
            };
            let node = (source.source.0, source.index);
            qualified.entry(path.clone()).or_default().insert(node);
            let module = &self.module_index.modules[self.module_index.by_source[&source.source]]
                .path
                .segments;
            if path.len() == module.len() + 1 && path.starts_with(module) {
                module_names
                    .entry(module.clone())
                    .or_default()
                    .entry(path.last().unwrap().clone())
                    .or_default()
                    .insert(node);
            }
        }
        let mut identifiers = BTreeMap::new();
        let mut dependencies = BTreeMap::new();
        for (id, source) in &files {
            let file = SourceFile::new(source.id, None, source.text.clone());
            let tokens = etas_syntax::lex(&file);
            let tokens = tokens.significant_tokens().collect::<Vec<_>>();
            let module = &self.module_index.modules[self.module_index.by_source[&source.id]]
                .path
                .segments;
            for (index, item) in parsed[id].items.iter().enumerate() {
                let mut referenced = base.clone();
                let mut used = BTreeSet::new();
                for (position, token) in tokens.iter().enumerate().filter(|(_, token)| {
                    item.span.range.start <= token.span.range.start
                        && token.span.range.end <= item.span.range.end
                }) {
                    if !token.kind.is_ident_like() {
                        continue;
                    }
                    let span = token.span.range;
                    let name = &source.text[span.start.to_usize()..span.end.to_usize()];
                    used.insert(name.to_owned());
                    if let Some(targets) = names.get(name) {
                        referenced.extend(targets);
                    }
                    if let Some(targets) = local_names.get(id).and_then(|names| names.get(name)) {
                        referenced.extend(targets);
                    }
                    if let Some(targets) =
                        module_names.get(module).and_then(|names| names.get(name))
                    {
                        referenced.extend(targets);
                    }
                    let mut path = vec![name.to_owned()];
                    let mut cursor = position + 1;
                    while cursor + 1 < tokens.len()
                        && tokens[cursor].kind
                            == etas_syntax::TokenKind::Punct(etas_syntax::Punct::Dot)
                        && tokens[cursor + 1].kind.is_ident_like()
                    {
                        let span = tokens[cursor + 1].span.range;
                        path.push(
                            source.text[span.start.to_usize()..span.end.to_usize()].to_owned(),
                        );
                        if let Some(targets) = qualified.get(&path) {
                            referenced.extend(targets);
                        }
                        cursor += 2;
                    }
                }
                identifiers.insert((*id, index), used);
                dependencies.insert((*id, index), referenced);
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
                .and_then(|item| node_at(self.program.items[item].span()))
        };
        let mut reference = |span: etas_core::Span, resolution: &etas_hir::ResolveResult| {
            if let etas_hir::ResolveResult::Resolved(symbol) = resolution
                && let Some(target) = source_of(*symbol)
                && let Some(owner) = node_at(span)
            {
                dependencies
                    .get_mut(&owner)
                    .expect("checked reference owner")
                    .insert(target);
            }
        };
        for symbol in self.program.symbols.iter() {
            if let etas_hir::SymbolDef::TypeParam { bounds, .. } = &symbol.def {
                for bound in bounds {
                    reference(bound.path.span, &bound.path.resolution);
                }
            }
        }
        for (_, ty) in self.program.types.iter() {
            match ty {
                HirType::Path { path, .. } => reference(path.span, &path.resolution),
                HirType::Existential { bounds, .. } => {
                    for bound in bounds {
                        reference(bound.path.span, &bound.path.resolution);
                    }
                }
                _ => {}
            }
        }
        for (_, expr) in self.program.exprs.iter() {
            match expr {
                HirExpr::Path(path) => reference(path.span, &path.resolution),
                HirExpr::Record(record) => {
                    if let Some(path) = &record.path {
                        reference(path.span, &path.resolution);
                    }
                }
                HirExpr::SpecMethodCall { spec_path, .. } => {
                    reference(spec_path.span, &spec_path.resolution)
                }
                HirExpr::Perform { action, .. } | HirExpr::Infer { action, .. } => {
                    reference(action.span, &action.effect.path.resolution)
                }
                _ => {}
            }
        }
        for (_, pattern) in self.program.pats.iter() {
            match pattern {
                etas_hir::HirPat::Record {
                    path: Some(path), ..
                }
                | etas_hir::HirPat::Variant { path, .. } => reference(path.span, &path.resolution),
                _ => {}
            }
        }
        for (_, item) in self.program.items.iter() {
            match item {
                HirItem::Impl(implementation) => match &implementation.target {
                    etas_hir::HirImplTarget::Inherent { target, .. } => {
                        reference(target.span, &target.resolution)
                    }
                    etas_hir::HirImplTarget::Agent { target, specs } => {
                        reference(target.span, &target.resolution);
                        for spec in specs {
                            reference(spec.spec_path.span, &spec.spec_path.resolution);
                        }
                    }
                    etas_hir::HirImplTarget::SpecSatisfaction { specs, .. } => {
                        for spec in specs {
                            reference(spec.spec_path.span, &spec.spec_path.resolution);
                        }
                    }
                    _ => {}
                },
                HirItem::Spec(spec) => {
                    for bound in &spec.bounds {
                        reference(bound.path.span, &bound.path.resolution);
                    }
                }
                HirItem::Effect(effect) => {
                    if let Some(parent) = &effect.extends {
                        reference(parent.path.span, &parent.path.resolution);
                    }
                }
                _ => {}
            }
        }
        for (expr, symbol) in &self.types.source_flow_calls {
            reference(
                self.program.exprs[*expr].span(&self.program.blocks),
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
                    self.program.exprs[expr].span(&self.program.blocks),
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
                            .insert(
                                node_at(implementation.span).expect("implementation declaration"),
                            );
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
            let source =
                node_at(self.program.items[*item].span()).expect("continuation declaration");
            for (_, arm) in self.program.handler_arms.iter() {
                dependencies
                    .get_mut(&node_at(arm.span).expect("handler declaration"))
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
            referenced.retain(|(id, _)| !prelude_sources.contains(id));
        }
        let roots = self
            .program
            .items
            .iter()
            .filter_map(|(_, item)| match item {
                HirItem::Flow(_)
                | HirItem::Tool(_)
                | HirItem::Agent(_)
                | HirItem::TopLevelLet(_) => node_at(item.span()),
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
            if !unique.insert(selected.clone()) {
                continue;
            }
            let prelude = self.selected_prelude(root, &selected);
            let mut selected_sources = selected.iter().map(|(id, _)| *id).collect::<BTreeSet<_>>();
            if let Some(source) = &prelude {
                selected_sources.insert(source.id.0);
            }
            let modules = selected_sources
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
                .retain(|key| selected_sources.contains(&key.source.0));
            environment
                .standalone_source_packages
                .retain(|package| selected_sources.contains(&package.source.0));
            environment.environment_fingerprint = None;
            environment.external_modules_fingerprint = None;
            let mut input = crate::ProjectInput {
                project_root: root.to_owned(),
                source_root: Some(source_root.to_owned()),
                options: self.compile_options.clone(),
                environment,
                sources: self
                    .sources
                    .iter()
                    .filter(|source| selected_sources.contains(&source.id.0))
                    .map(|source| crate::SourceInput {
                        id: source.id,
                        path: Some(root.join(&source.relative_path)),
                        text: {
                            let id = source.id.0;
                            let used = selected
                                .iter()
                                .filter(|(owner, _)| *owner == id)
                                .flat_map(|node| identifiers[node].iter().cloned())
                                .collect::<BTreeSet<_>>();
                            let mut text = format!(
                                "module {};\n",
                                self.module_index.modules[self.module_index.by_source[&source.id]]
                                    .path
                                    .segments
                                    .join(".")
                            );
                            for import in &self.resolved_imports.imports {
                                if import.import.source == source.id
                                    && used.contains(&import.local_name)
                                {
                                    text.push_str(&self.selected_import(
                                        &import.local_name,
                                        &import.target,
                                        import.visibility,
                                    ));
                                }
                            }
                            for import in &self.resolved_imports.wildcard_imports {
                                if import.import.source == source.id {
                                    for (name, target) in &import.targets {
                                        if used.contains(name) {
                                            text.push_str(&self.selected_import(
                                                name,
                                                target,
                                                import.visibility,
                                            ));
                                        }
                                    }
                                }
                            }
                            for (index, item) in parsed[&id].items.iter().enumerate() {
                                if selected.contains(&(id, index)) {
                                    text.push_str(
                                        &source.text[item.span.range.start.to_usize()
                                            ..item.span.range.end.to_usize()],
                                    );
                                    text.push('\n');
                                }
                            }
                            text
                        },
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

    fn selected_import(
        &self,
        name: &str,
        target: &crate::ImportTarget,
        visibility: etas_hir::Visibility,
    ) -> String {
        let path = match target {
            crate::ImportTarget::SourceItem { item, member, .. } => {
                let (_, declaration) = self
                    .item_targets
                    .iter()
                    .find(|(source, _)| source == item)
                    .expect("checked import target");
                let symbol = self.program.items[*declaration]
                    .symbol()
                    .expect("named imported declaration");
                let mut path = self
                    .program
                    .declaration_path(symbol)
                    .expect("import declaration path");
                if let Some(member) = member {
                    path.push(member.clone());
                }
                path
            }
            crate::ImportTarget::StdItem {
                module_path, name, ..
            } => {
                let mut path = module_path.segments.clone();
                path.push(name.clone());
                path
            }
            crate::ImportTarget::Module(crate::ResolvedModuleTarget::Source { module, .. }) => {
                self.module_index.modules[*module].path.segments.clone()
            }
            _ => unreachable!("source std imports source or primitive declarations"),
        };
        let public = if visibility == etas_hir::Visibility::Public {
            "public "
        } else {
            ""
        };
        if path.last().map(String::as_str) == Some(name) {
            format!("{public}import {};\n", path.join("."))
        } else {
            format!("{public}import {} as {name};\n", path.join("."))
        }
    }

    fn selected_prelude(
        &self,
        root: &Path,
        selected: &BTreeSet<(u32, usize)>,
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
        let mut lines = BTreeSet::new();
        let mut import = |name: &str, target: &crate::ImportTarget| {
            let path = match target {
                crate::ImportTarget::SourceItem {
                    module: _,
                    item,
                    member,
                    ..
                } => {
                    if !selected.contains(&(item.source.0, item.index)) {
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
                crate::ImportTarget::StdItem {
                    module_path, name, ..
                } => {
                    let mut path = module_path.segments.clone();
                    path.push(name.clone());
                    path
                }
                crate::ImportTarget::Module(crate::ResolvedModuleTarget::Source {
                    module, ..
                }) => {
                    let path = &self.module_index.modules[*module].path.segments;
                    if !self.module_index.modules[*module].parts.iter().any(|part| {
                        selected
                            .iter()
                            .any(|(id, _)| *id == self.module_index.parts[*part].source.0)
                    }) {
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
