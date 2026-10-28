use crate::{
    ExternalPackageId, ImportTarget, ProjectEnvironmentInput, ResolvedImports, ResolvedModuleTarget,
};
use etas_core::Span;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Debug, Default)]
pub(crate) struct ExternalImportAnchorIndex {
    package_spans: BTreeMap<ExternalPackageId, Span>,
    item_spans: BTreeMap<(ExternalPackageId, Vec<String>), Span>,
    module_spans: BTreeMap<(ExternalPackageId, Vec<String>), Span>,
    transitive_packages: BTreeSet<ExternalPackageId>,
}

impl ExternalImportAnchorIndex {
    pub(crate) fn build(imports: &ResolvedImports, environment: &ProjectEnvironmentInput) -> Self {
        let mut index = Self::default();
        for import in &imports.imports {
            index.insert_target(&import.target, import.span);
            if let ImportTarget::Module(module) = &import.target
                && let Some(members) = imports.module_members.get(&module.key())
            {
                for target in members.values() {
                    index.insert_target(target, import.span);
                }
            }
        }
        for import in &imports.wildcard_imports {
            let ResolvedModuleTarget::External {
                package: Some(package),
                path,
                ..
            } = &import.target_module
            else {
                continue;
            };
            index.insert_module(*package, path.segments.clone(), import.span);
            for target in import.targets.values() {
                index.insert_target(target, import.span);
            }
        }
        let mut dependencies = BTreeMap::<_, Vec<_>>::new();
        for edge in &environment.external_package_dependencies {
            dependencies
                .entry(edge.package)
                .or_default()
                .push(edge.dependency);
        }
        let mut pending = index.package_spans.keys().copied().collect::<VecDeque<_>>();
        while let Some(owner) = pending.pop_front() {
            let span = index.package_spans[&owner];
            for dependency in dependencies.get(&owner).into_iter().flatten() {
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    index.package_spans.entry(*dependency)
                {
                    entry.insert(span);
                    index.transitive_packages.insert(*dependency);
                    pending.push_back(*dependency);
                }
            }
        }
        index
    }

    fn insert_target(&mut self, target: &ImportTarget, span: Span) {
        match target {
            ImportTarget::ExternalItem {
                package: Some(package),
                module_path,
                name,
                ..
            } => {
                let mut item = module_path.segments.clone();
                item.push(name.clone());
                self.insert_item(*package, item, span);
                self.insert_module(*package, module_path.segments.clone(), span);
            }
            ImportTarget::Module(ResolvedModuleTarget::External {
                package: Some(package),
                path,
                ..
            }) => self.insert_module(*package, path.segments.clone(), span),
            _ => {}
        }
    }

    fn insert_item(&mut self, package: ExternalPackageId, item: Vec<String>, span: Span) {
        self.package_spans.entry(package).or_insert(span);
        self.item_spans.entry((package, item)).or_insert(span);
    }

    fn insert_module(&mut self, package: ExternalPackageId, module: Vec<String>, span: Span) {
        self.package_spans.entry(package).or_insert(span);
        self.module_spans.entry((package, module)).or_insert(span);
    }

    pub(crate) fn package_span(&self, package: ExternalPackageId) -> Option<Span> {
        self.package_spans.get(&package).copied()
    }

    pub(crate) fn item_span(&self, package: ExternalPackageId, item: &[String]) -> Option<Span> {
        self.item_spans
            .get(&(package, item.to_vec()))
            .copied()
            .or_else(|| {
                self.module_spans
                    .iter()
                    .filter(|((candidate_package, module), _)| {
                        *candidate_package == package && item.starts_with(module)
                    })
                    .max_by_key(|((_, module), _)| module.len())
                    .map(|(_, span)| *span)
            })
            .or_else(|| {
                self.transitive_packages
                    .contains(&package)
                    .then(|| self.package_span(package))
                    .flatten()
            })
    }
}

#[cfg(test)]
mod tests {
    use etas_core::{SourceId, TextSize};

    use super::*;

    fn module_import(package: u32, source: u32) -> crate::ResolvedImport {
        let span = Span::empty(SourceId(source), TextSize(7));
        crate::ResolvedImport {
            from: crate::ModuleId(0),
            from_part: crate::ModulePartId(0),
            import: crate::AstImportRef {
                source: SourceId(source),
                module_part: Some(crate::ModulePartId(0)),
                index: 0,
                span,
            },
            target: ImportTarget::Module(ResolvedModuleTarget::External {
                key: crate::ModuleKey(package),
                package: Some(ExternalPackageId(package)),
                module: crate::ExternalModuleId(package),
                path: crate::ModulePath {
                    segments: vec![format!("dep{package}"), "api".into()],
                },
            }),
            local_name: format!("dep{package}"),
            visibility: etas_hir::Visibility::Private,
            span,
        }
    }

    fn environment(edges: &[(u32, u32)]) -> ProjectEnvironmentInput {
        ProjectEnvironmentInput {
            external_package_dependencies: edges
                .iter()
                .map(
                    |(package, dependency)| crate::ProjectExternalPackageDependencyInput {
                        package: ExternalPackageId(*package),
                        dependency: ExternalPackageId(*dependency),
                    },
                )
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn transitive_anchors_follow_declared_dependency_edges_and_preserve_direct_imports() {
        let first = module_import(1, 10);
        let direct = module_import(3, 30);
        let imports = ResolvedImports {
            imports: vec![first.clone(), direct.clone()],
            ..Default::default()
        };
        let index = ExternalImportAnchorIndex::build(
            &imports,
            &environment(&[(1, 2), (2, 3), (2, 4), (3, 4), (5, 6)]),
        );
        assert_eq!(index.package_span(ExternalPackageId(2)), Some(first.span));
        assert_eq!(index.package_span(ExternalPackageId(3)), Some(direct.span));
        assert_eq!(index.package_span(ExternalPackageId(5)), None);
        assert_eq!(index.package_span(ExternalPackageId(6)), None);
        let indirect = vec!["dep2".into(), "api".into(), "Value".into()];
        assert_eq!(
            index.item_span(ExternalPackageId(2), &indirect),
            Some(first.span)
        );
        assert_eq!(
            index.item_span(
                ExternalPackageId(3),
                &["dep3".into(), "api".into(), "Value".into()]
            ),
            Some(direct.span)
        );
        assert_eq!(
            index.item_span(ExternalPackageId(3), &["unimported".into(), "Value".into()]),
            None
        );
        assert_eq!(index.item_span(ExternalPackageId(7), &indirect), None);
    }

    #[test]
    fn external_import_anchors_keep_package_identity_for_equal_item_paths() {
        let first_package = ExternalPackageId(1);
        let second_package = ExternalPackageId(2);
        let first_span = Span::empty(SourceId(10), TextSize(1));
        let second_span = Span::empty(SourceId(20), TextSize(2));
        let item = vec!["shared".to_owned(), "api".to_owned(), "run".to_owned()];
        let mut index = ExternalImportAnchorIndex::default();
        index.insert_item(first_package, item.clone(), first_span);
        index.insert_item(second_package, item.clone(), second_span);

        assert_eq!(index.item_span(first_package, &item), Some(first_span));
        assert_eq!(index.item_span(second_package, &item), Some(second_span));
        assert_eq!(index.item_span(ExternalPackageId(3), &item), None);
    }
}
