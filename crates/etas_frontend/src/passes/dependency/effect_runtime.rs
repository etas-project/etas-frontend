use std::collections::BTreeSet;

use crate::passes::artifacts::{
    EFFECT_OUTPUT, MODULE_INDEX, REACHABILITY_FACTS, RUNTIME_SOURCE_REQUIREMENTS,
    VALIDATED_EXTERNAL_ENVIRONMENT,
};
use crate::{
    ModulePath, ProjectContext, RuntimeSourceReason, RuntimeSourceReasonKind,
    RuntimeSourceRequirement,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

pub struct CompleteEffectRuntimeReachabilityPass;

impl Pass<ProjectContext> for CompleteEffectRuntimeReachabilityPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("CompleteEffectRuntimeReachabilityPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                EFFECT_OUTPUT,
                MODULE_INDEX,
                REACHABILITY_FACTS,
                RUNTIME_SOURCE_REQUIREMENTS,
                VALIDATED_EXTERNAL_ENVIRONMENT,
            ]))
            .produces(ArtifactSet::from([
                REACHABILITY_FACTS,
                RUNTIME_SOURCE_REQUIREMENTS,
            ]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        _: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        match complete(context) {
            Ok(()) => PassResult::changed(
                PreservedArtifacts::All,
                ArtifactSet::from([REACHABILITY_FACTS, RUNTIME_SOURCE_REQUIREMENTS]),
            ),
            Err(reason) => PassResult::failed(reason),
        }
    }
}

fn complete(context: &mut ProjectContext) -> Result<(), String> {
    let environment = context
        .validated_external_environment
        .as_ref()
        .ok_or("effect runtime linkage requires validated metadata")?
        .environment();
    let effects = context
        .effects
        .as_ref()
        .ok_or("effect runtime linkage requires solved effects")?;
    let artifacts = context
        .effect_pipeline_artifacts
        .as_ref()
        .ok_or("effect runtime linkage requires pipeline artifacts")?;
    let modules = context
        .modules
        .as_ref()
        .ok_or("effect runtime linkage requires module namespaces")?;
    let mut requirements = context
        .runtime_source_requirements
        .clone()
        .ok_or("effect runtime linkage requires runtime source requirements")?;
    let mut linked = BTreeSet::new();
    for method in &artifacts.external_implementations {
        if !effects
            .facts
            .metadata_callable_graphs
            .contains_key(&method.graph_key)
        {
            continue;
        }
        let key = method.identity.implementation();
        let mut origins = environment
            .external_definition_origins
            .iter()
            .filter(|origin| origin.origins.package == key.package());
        let (Some(origin), None) = (origins.next(), origins.next()) else {
            return Err("selected implementation has no unique checked package origin".into());
        };
        let namespace = origin
            .origins
            .modules
            .iter()
            .find(|module| module.canonical_path == key.module())
            .ok_or("selected implementation has no checked module origin")?;
        if !linked.insert((origin.package, namespace.path.clone())) {
            continue;
        }
        let package = environment
            .external_packages
            .iter()
            .find(|package| package.id == origin.package)
            .ok_or("selected implementation package is not in the validated environment")?;
        let module = ModulePath {
            segments: namespace.path.clone(),
        };
        let source = modules
            .dependency_namespaces
            .source_for_namespace(package.id, &module)?;
        let requirement = requirements
            .dependencies
            .entry(package.id)
            .or_insert_with(|| RuntimeSourceRequirement {
                package: package.id,
                import_root: package.import_root.clone(),
                seed_modules: BTreeSet::new(),
                required_modules: BTreeSet::new(),
                reasons: Vec::new(),
            });
        requirement.seed_modules.insert(source.clone());
        requirement.required_modules.insert(source);
        let reason = RuntimeSourceReason {
            item_path: namespace.path.clone(),
            module_path: module,
            kind: RuntimeSourceReasonKind::ExternalRuntimeModule,
        };
        if !requirement.reasons.contains(&reason) {
            requirement.reasons.push(reason);
        }
    }
    let reachability = context
        .reachability
        .as_mut()
        .ok_or("effect runtime linkage requires reachability facts")?;
    reachability.runtime_source_requirements = requirements.clone();
    context.runtime_source_requirements = Some(requirements);
    Ok(())
}
