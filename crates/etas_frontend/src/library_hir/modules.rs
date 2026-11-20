//! Installation units follow authored module dependencies. Lexing records
//! qualified references in bodies, signatures, imports and annotations while
//! excluding comments and string literals. Ordinary checking validates each
//! selected closure before it can be installed.

use super::LibraryHirImage;
use etas_core::SourceFile;
use etas_syntax::{Punct, TokenKind};
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
        let mut files = BTreeMap::<Vec<String>, Vec<_>>::new();
        for source in &self.sources {
            let module = self.module_index.by_source[&source.id];
            files
                .entry(self.module_index.modules[module].path.segments.clone())
                .or_default()
                .push(source);
        }
        let mut dependencies = BTreeMap::<Vec<String>, BTreeSet<Vec<String>>>::new();
        for (module, sources) in &files {
            let mut referenced = BTreeSet::new();
            // Source std's core namespace supplies its implicit prelude.
            if files.contains_key(&vec!["std".into(), "core".into()]) {
                referenced.insert(vec!["std".into(), "core".into()]);
            }
            for length in 1..module.len() {
                if files.contains_key(&module[..length]) {
                    referenced.insert(module[..length].to_vec());
                }
            }
            for source in sources {
                let file = SourceFile::new(source.id, None, source.text.clone());
                let stream = etas_syntax::lex(&file);
                let tokens = stream.significant_tokens().collect::<Vec<_>>();
                let text = |index: usize| {
                    let span = tokens[index].span.range;
                    &source.text[span.start.to_usize()..span.end.to_usize()]
                };
                let mut cursor = 0;
                while cursor < tokens.len() {
                    if tokens[cursor].kind != TokenKind::Ident || text(cursor) != "std" {
                        cursor += 1;
                        continue;
                    }
                    let mut path = vec!["std".to_owned()];
                    cursor += 1;
                    while cursor + 1 < tokens.len()
                        && tokens[cursor].kind == TokenKind::Punct(Punct::Dot)
                        && tokens[cursor + 1].kind.is_ident_like()
                    {
                        path.push(text(cursor + 1).to_owned());
                        cursor += 2;
                    }
                    while !path.is_empty() && !files.contains_key(&path) {
                        path.pop();
                    }
                    if !path.is_empty() {
                        referenced.insert(path);
                    }
                }
            }
            dependencies.insert(module.clone(), referenced);
        }
        // Implicit prelude names and generic bounds need their resolved owner,
        // even when no qualified path appears in the authored source.
        let mut reference = |source: etas_core::SourceId, resolution: &etas_hir::ResolveResult| {
            let Some(from) = self.module_index.by_source.get(&source) else {
                return;
            };
            let etas_hir::ResolveResult::Resolved(symbol) = resolution else {
                return;
            };
            let Some(symbol) = self.program.canonical_source_symbol(*symbol) else {
                return;
            };
            let mut path = match &self
                .program
                .symbols
                .get(symbol)
                .expect("checked dependency symbol")
                .def
            {
                etas_hir::SymbolDef::ImportAlias { path, .. } => path.clone(),
                _ => self.program.declaration_path(symbol).unwrap_or_default(),
            };
            while !path.is_empty() && !files.contains_key(&path) {
                path.pop();
            }
            if !path.is_empty() {
                let module = &self.module_index.modules[*from].path.segments;
                dependencies
                    .get_mut(module)
                    .expect("source module dependency set")
                    .insert(path);
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
                etas_hir::HirType::Path { path, .. } => {
                    reference(path.span.source, &path.resolution)
                }
                etas_hir::HirType::Existential { bounds, .. } => {
                    for bound in bounds {
                        reference(bound.path.span.source, &bound.path.resolution);
                    }
                }
                _ => {}
            }
        }
        for (_, expr) in self.program.exprs.iter() {
            if let etas_hir::HirExpr::Path(path) = expr {
                reference(path.span.source, &path.resolution);
            }
        }
        let mut unique = BTreeSet::new();
        let mut compilations = Vec::new();
        for module in files.keys() {
            let mut modules = BTreeSet::new();
            let mut pending = vec![module.clone()];
            while let Some(next) = pending.pop() {
                if modules.insert(next.clone()) {
                    pending.extend(dependencies[&next].iter().cloned());
                }
            }
            if modules.len() == files.len() || !unique.insert(modules.clone()) {
                continue;
            }
            let selected = modules
                .iter()
                .flat_map(|module| &files[module])
                .map(|source| source.id)
                .collect::<std::collections::HashSet<_>>();
            let mut environment = self.environment.clone();
            environment
                .stable_source_keys
                .retain(|key| selected.contains(&key.source));
            environment
                .standalone_source_packages
                .retain(|package| selected.contains(&package.source));
            environment.environment_fingerprint = None;
            environment.external_modules_fingerprint = None;
            let input = crate::ProjectInput {
                project_root: root.to_owned(),
                source_root: Some(root.to_owned()),
                options: self.compile_options.clone(),
                environment,
                sources: self
                    .sources
                    .iter()
                    .filter(|source| selected.contains(&source.id))
                    .map(|source| crate::SourceInput {
                        id: source.id,
                        path: Some(root.join(&source.relative_path)),
                        text: source.text.clone(),
                        kind: source.kind.clone(),
                    })
                    .collect(),
                entry: self.entry.requested.clone(),
            };
            compilations.push(LibraryModuleCompilation { modules, input });
        }
        compilations
    }
}
