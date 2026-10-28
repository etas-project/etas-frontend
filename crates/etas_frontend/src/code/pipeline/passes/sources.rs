use crate::passes::artifacts::{PARSED_SOURCE_SET, SOURCE_SET};
use crate::{ProjectContext, SourceFile, SourceKind, SourceSet};
use etas_utils::{
    ArtifactKey, ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};
use std::{collections::HashSet, sync::Arc};

pub(crate) const FROZEN_DEFINITION_ENVIRONMENT: ArtifactKey =
    ArtifactKey::new("code", "frozen_definition_environment");
pub(crate) const CANDIDATE_BODY_OVERLAYS: ArtifactKey =
    ArtifactKey::new("code", "candidate_body_overlays");

pub(crate) struct FreezeDefinitionEnvironmentPass;

impl Pass<ProjectContext> for FreezeDefinitionEnvironmentPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("FreezeDefinitionEnvironmentPass", PassKind::Transform).produces(
            ArtifactSet::from([
                SOURCE_SET,
                PARSED_SOURCE_SET,
                FROZEN_DEFINITION_ENVIRONMENT,
                CANDIDATE_BODY_OVERLAYS,
            ]),
        )
    }
    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        _: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let Some(candidate) = &mut context.candidate else {
            return PassResult::failed(
                "candidate checking requires a frozen definition environment",
            );
        };
        if !context.input.sources.is_empty() {
            return PassResult::failed(
                "candidate checking cannot inject new project source inputs",
            );
        }
        let checked = candidate.overlay.frozen().checked();
        let archive = &checked.definition_sources;
        let original = archive.source_set();
        let parsed = archive.parsed_sources();
        let original_ids = parsed
            .iter()
            .map(|source| source.source)
            .collect::<HashSet<_>>();
        let mut files = original
            .files
            .iter()
            .filter(|source| original_ids.contains(&source.id))
            .cloned()
            .collect::<Vec<_>>();
        let mut ids = files.iter().map(|source| source.id).collect::<HashSet<_>>();
        let mut revisions = candidate.revisions.values().collect::<Vec<_>>();
        revisions.sort_by_key(|revision| revision.relocated.source().id.0);
        for revision in revisions {
            let source = revision.relocated.source();
            if !ids.insert(source.id) {
                return PassResult::failed(
                    "candidate body source identity collides with the frozen source archive",
                );
            }
            files.push(SourceFile {
                id: source.id,
                path: None,
                text: source.text.clone(),
                line_index: source.line_index.clone(),
                kind: SourceKind::VirtualGenerated,
            });
        }
        let mut paths = files
            .iter()
            .filter_map(|source| source.path.clone())
            .collect::<HashSet<_>>();
        let Some(mut next) = files.iter().map(|source| source.id.0).max() else {
            return PassResult::failed("candidate source archive has no source identity");
        };
        candidate.dependency_source_ids.clear();
        for source in &candidate.dependency_sources {
            let SourceKind::DependencySourceOverlay {
                package,
                import_root,
            } = &source.kind
            else {
                return PassResult::failed(
                    "candidate source completion cannot replace project sources",
                );
            };
            if !checked
                .definition_environment
                .environment()
                .external_packages
                .iter()
                .any(|input| input.id == *package && input.import_root == *import_root)
            {
                return PassResult::failed(
                    "candidate dependency source is outside its frozen package environment",
                );
            }
            let Some(path) = source.path.as_ref().filter(|path| path.is_absolute()) else {
                return PassResult::failed(
                    "candidate dependency source requires an absolute file identity",
                );
            };
            if !paths.insert(path.clone()) {
                return PassResult::failed(
                    "candidate dependency source cannot replace an archived file",
                );
            }
            let Some(id) = next.checked_add(1) else {
                return PassResult::failed(
                    "candidate dependency source identity space is exhausted",
                );
            };
            next = id;
            let id = etas_core::SourceId(id);
            candidate.dependency_source_ids.push(id);
            files.push(SourceFile {
                id,
                path: Some(path.clone()),
                text: Arc::from(source.text.as_str()),
                line_index: etas_core::LineIndex::new(&source.text),
                kind: source.kind.clone(),
            });
        }
        context.sources = Some(Arc::new(SourceSet {
            files,
            project_root: original.project_root.clone(),
            source_root: original.source_root.clone(),
            environment_fingerprint: original.environment_fingerprint.clone(),
            external_modules_fingerprint: original.external_modules_fingerprint.clone(),
        }));
        context.parsed_sources = parsed.to_vec();
        context.reused_parsed_sources = parsed.len();
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}

pub(crate) struct ParseCandidateDependencySourcesPass;

impl Pass<ProjectContext> for ParseCandidateDependencySourcesPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ParseCandidateDependencySourcesPass", PassKind::Transform)
            .requires(ArtifactSet::from([
                SOURCE_SET,
                PARSED_SOURCE_SET,
                FROZEN_DEFINITION_ENVIRONMENT,
            ]))
            .produces(ArtifactSet::one(PARSED_SOURCE_SET))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let Some(candidate) = context.candidate.as_ref() else {
            return PassResult::failed(
                "candidate dependency parsing requires its frozen environment",
            );
        };
        let sources = candidate.dependency_source_ids.clone();
        let mut pipeline = etas_utils::Pipeline::new("frontend.candidate_dependency_source")
            .pass(crate::passes::ParseSourceFilePass);
        for source in sources {
            let control = manager.run_nested_pipeline(
                &mut pipeline,
                context,
                Some(etas_utils::UnitKey::new(
                    crate::SOURCE_FILE_UNIT_KIND,
                    source.0.into(),
                )),
            );
            if control != etas_utils::PassControl::Continue {
                return PassResult {
                    control,
                    ..PassResult::unchanged()
                };
            }
        }
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(PARSED_SOURCE_SET))
    }
}
