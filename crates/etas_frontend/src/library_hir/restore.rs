use std::{path::Path, sync::Arc};

use etas_hir::{HirItem, HirRevision};
use etas_types::{SignatureEnvironment, TypeOutput};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassControl, PassDescriptor, PassKind, PassManager, PassResult,
    Pipeline, PreservedArtifacts, UnitOrder, UnitSelector,
};

use super::{LibraryHirImage, LibraryHirImageError, error};
use crate::{
    HirBodyBindings, HirItemBindings, HirOutput, ProjectContext, ProjectInput,
    SOURCE_FILE_UNIT_KIND, SourceInput,
    passes::{self, artifacts::*},
};

pub(super) fn restore(
    image: LibraryHirImage,
    root: &Path,
    registry: Arc<etas_std::StdRegistry>,
) -> Result<crate::CheckedProgram, LibraryHirImageError> {
    if image.schema != super::SCHEMA || image.compiler_version != crate::FRONTEND_COMPILER_VERSION {
        return Err(error("incompatible checked library image"));
    }
    if image.std_contract != registry.version().compatibility_identity() {
        return Err(error(
            "library image and toolchain standard contracts differ",
        ));
    }
    let declarations = image.declarations.clone();
    let methods = image.methods.clone();
    let package = image.package;
    let input = ProjectInput {
        project_root: root.to_owned(),
        source_root: Some(root.to_owned()),
        options: image.compile_options.clone(),
        environment: image.environment.clone(),
        sources: image
            .sources
            .iter()
            .map(|source| SourceInput {
                id: source.id,
                path: Some(root.join(&source.relative_path)),
                text: source.text.clone(),
                kind: source.kind.clone(),
            })
            .collect(),
        entry: image.entry.requested.clone(),
    };
    let context = ProjectContext::new_with_std_registry(input, registry);
    let pipeline = Pipeline::new("frontend.restore_library")
        .pass(passes::BuildSourceSetPass)
        .for_each(
            UnitSelector::Kind(SOURCE_FILE_UNIT_KIND),
            UnitOrder::SourceOrder,
            Pipeline::new("frontend.library_source_archive").pass(passes::ParseSourceFilePass),
        )
        .pass(LoadLibraryFactsPass(Some(image)))
        .pass(passes::BuildModuleCatalogPass)
        .pass(passes::ValidateExternalEnvironmentPass)
        .pass(passes::ValidateItemAnnotationsPass)
        .pass(passes::RetainDefinitionSourcesPass)
        .pass(passes::PrepareEffectRegistryPass)
        .pass(passes::RunEffectPipelinePass)
        .pass(passes::ValidateResidualBackendSupportPass)
        .pass(passes::MaterializeGenericEffectRowsPass)
        .pass(passes::CheckStandardLibraryImplementationsPass)
        .pass(passes::ValidateHandlerInstantiationsPass)
        .pass(passes::AnalyzeLoopProgressPass)
        .pass(passes::AnalyzeConcurrencyPass)
        .pass(passes::AnalyzeCpuParallelEligibilityPass)
        .pass(passes::VerifyInterpreterSupportPass)
        .pass(passes::PrepareInvocationTypesPass)
        .pass(crate::code::pipeline::passes::ResolveReflectionTargetsPass)
        .pass(passes::BuildCheckedProjectPass);
    let run =
        crate::pipeline::run_controlled_context_pipeline(context, pipeline, false, None, None);
    if let PassControl::Failed(failure) = run.control {
        return Err(error(failure.message));
    }
    let checked = run.output.checked.ok_or_else(|| {
        error(format!(
            "library publication failed: {:?}",
            run.output.diagnostics,
        ))
    })?;
    let linked = LibraryHirImage::from_checked(&checked, package, root)?;
    if linked.declarations != declarations || linked.methods != methods {
        return Err(error(
            "library declarations changed while restoring native provenance",
        ));
    }
    Ok(checked)
}

/// Installs descriptive archive data. Executable proofs are rebuilt by the
/// ordinary effect, handler, reflection and checked-publication passes below.
struct LoadLibraryFactsPass(Option<LibraryHirImage>);

impl Pass<ProjectContext> for LoadLibraryFactsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("LoadLibraryFactsPass", PassKind::Transform)
            .requires(ArtifactSet::from([SOURCE_SET, PARSED_SOURCE_SET]))
            .produces(ArtifactSet::from([
                HIR_OUTPUT,
                MODULE_INDEX,
                RESOLVED_IMPORTS,
                RESOLVED_PATHS,
                TYPE_OUTPUT,
                SIGNATURE_FACTS,
                CALLABLE_OUTPUT_FACTS,
                TYPE_ENVIRONMENT,
                TOP_LEVEL_LET_FACTS,
                PROJECT_ENTRY,
                REACHABILITY_FACTS,
                ANALYSIS_REACHABILITY_FACTS,
                ADMITTED_STD_LIBRARY_SOURCES,
            ]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        _: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let image = self.0.take().expect("library facts load exactly once");
        let hir = match HirRevision::try_new(image.program) {
            Ok(hir) => hir,
            Err(reason) => return PassResult::failed(format!("invalid library HIR: {reason:?}")),
        };
        let mut bodies = HirBodyBindings::default();
        for (item, slot) in image.body_slots {
            let block = match hir.items.get(item) {
                Some(HirItem::Flow(flow)) => Some(flow.body.block()),
                Some(HirItem::Tool(tool)) => match &tool.body {
                    etas_hir::HirToolBody::Source(body) => Some(body.block()),
                    _ => None,
                },
                Some(HirItem::Agent(agent)) => agent.body.block(),
                _ => None,
            };
            let Some(block) = block else {
                return PassResult::failed("library body slot has no callable HIR body");
            };
            bodies.body_items.insert(slot.clone(), item);
            bodies.ast_to_root_block.insert(slot.clone(), block);
            bodies.root_block_to_ast.insert(block, slot);
        }
        context.hir_item_bindings = Some(HirItemBindings {
            ast_to_hir: image.item_targets.into_iter().collect(),
            hir_to_ast: image.item_sources.into_iter().collect(),
            external_import_members: image.external_import_members.into_iter().collect(),
        });
        context.hir_body_bindings = Some(bodies);
        let types = TypeOutput {
            facts: Arc::new(image.types),
            store: Arc::new(image.type_store),
            diagnostics: Vec::new(),
        };
        context.type_environment = Some(SignatureEnvironment::new(&hir, &types));
        context.signature_types = Some(types.clone());
        context.types = Some(types);
        context.hir = Some(HirOutput { hir });
        context.modules = Some(Arc::new(image.module_index));
        context.resolved_imports = Some(Arc::new(image.resolved_imports));
        context.resolved_paths = Some(Arc::new(image.reflection_paths));
        context.top_level_lets = Some(image.top_level_lets);
        context.entry = Some(image.entry);
        context.reachability = Some(image.reachability);
        context.optimization_report = image.optimization_report;
        context.admitted_std_library_sources = Some(Arc::new(Default::default()));
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}
