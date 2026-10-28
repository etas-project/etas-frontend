use std::collections::HashMap;

use etas_code::descriptor::{AnnotationInfo, AnnotationReferenceInfo, DescriptorLimits};
use etas_core::SourceId;
use etas_hir::{HirItemId, SymbolDef, SymbolId};

use crate::{
    CheckedProgramData, ImportTarget, ModuleId, ResolvedModuleTarget,
    code::{
        descriptor::ProjectionError,
        identity::{DefinitionIdentityIndex, DefinitionOrigins, SourceModuleOrigin},
    },
    static_annotation::AnnotationProjection,
};

pub(super) struct AnnotationBindings<'a> {
    checked: &'a CheckedProgramData,
    identities: &'a DefinitionIdentityIndex,
    origins: &'a DefinitionOrigins,
    imports: HashMap<SourceId, HashMap<String, ResolvedModuleTarget>>,
    sources: HashMap<ModuleId, &'a SourceModuleOrigin>,
    external_modules: HashMap<
        crate::ExternalPackageId,
        std::collections::BTreeMap<Vec<String>, AnnotationReferenceInfo>,
    >,
}

impl<'a> AnnotationBindings<'a> {
    pub fn new(
        checked: &'a CheckedProgramData,
        identities: &'a DefinitionIdentityIndex,
        origins: &'a DefinitionOrigins,
    ) -> Result<Self, ProjectionError> {
        let mut imports: HashMap<_, HashMap<_, _>> = HashMap::new();
        for import in &checked.resolved_imports.imports {
            let ImportTarget::Module(target) = &import.target else {
                continue;
            };
            let part = checked
                .module_index
                .parts
                .get(import.from_part)
                .ok_or_else(|| {
                    ProjectionError::Invalid(
                        "resolved annotation module import has no source part".into(),
                    )
                })?;
            if imports
                .entry(part.source)
                .or_default()
                .insert(import.local_name.clone(), target.clone())
                .is_some()
            {
                return Err(ProjectionError::Invalid(
                    "resolved annotation module import has duplicate bindings".into(),
                ));
            }
        }
        let mut sources = HashMap::new();
        for module in &checked.hir.modules {
            let data = checked
                .hir
                .modules_arena
                .get(*module)
                .ok_or(ProjectionError::MissingModule(*module))?;
            let source = checked
                .module_index
                .by_source
                .get(&data.span.source)
                .ok_or_else(|| {
                    ProjectionError::Invalid(
                        "checked annotation source module has no project module identity".into(),
                    )
                })?;
            let origin = origins
                .source_modules
                .get(module)
                .ok_or(ProjectionError::MissingPackage(*module))?;
            if sources.insert(*source, origin).is_some() {
                return Err(ProjectionError::Invalid(
                    "annotation source module has multiple canonical origins".into(),
                ));
            }
        }
        let mut external_modules: HashMap<_, std::collections::BTreeMap<_, _>> = HashMap::new();
        for input in checked.external_definition_origins() {
            for module in &input.origins.modules {
                let reference = AnnotationReferenceInfo::Module {
                    package: input.origins.package,
                    path: module.canonical_path.clone(),
                };
                if external_modules
                    .entry(input.package)
                    .or_default()
                    .insert(module.path.clone(), reference)
                    .is_some()
                {
                    return Err(ProjectionError::Invalid(
                        "duplicate checked external module annotation origin".into(),
                    ));
                }
            }
        }
        Ok(Self {
            checked,
            identities,
            origins,
            imports,
            sources,
            external_modules,
        })
    }

    pub fn project(
        &self,
        item: HirItemId,
        limits: DescriptorLimits,
    ) -> Result<Vec<AnnotationInfo>, ProjectionError> {
        let projector =
            AnnotationProjection::new(self.checked, |symbol| self.reference(symbol), limits);
        self.checked
            .hir
            .item_annotations
            .get(&item)
            .into_iter()
            .flatten()
            .map(|annotation| {
                projector
                    .project(annotation)
                    .map_err(|error| match error.resource_limit {
                        Some((resource, limit)) => ProjectionError::Limit { resource, limit },
                        None => ProjectionError::InvalidAnnotation {
                            span: error.span,
                            reason: error.reason,
                        },
                    })
            })
            .collect()
    }

    fn reference(&self, symbol: SymbolId) -> Result<AnnotationReferenceInfo, String> {
        let data = self
            .checked
            .hir
            .symbols
            .get(symbol)
            .ok_or("annotation symbol is missing from checked HIR")?;
        if let SymbolDef::Module { module } = data.def {
            let origin = self
                .origins
                .source_modules
                .get(&module)
                .ok_or("annotation module has no canonical package origin")?;
            return Ok(module_reference(origin));
        }
        if matches!(data.def, SymbolDef::ImportAlias { .. })
            && let Some(target) = self
                .imports
                .get(&data.definition_span.source)
                .and_then(|bindings| bindings.get(&data.name))
        {
            return self.imported_module(target);
        }
        self.identities
            .symbol(symbol)
            .cloned()
            .map(AnnotationReferenceInfo::Declaration)
            .ok_or_else(|| "annotation reference has no stable declaration identity".into())
    }

    fn imported_module(
        &self,
        target: &ResolvedModuleTarget,
    ) -> Result<AnnotationReferenceInfo, String> {
        match target {
            ResolvedModuleTarget::Source { module, .. } => self
                .sources
                .get(module)
                .map(|origin| module_reference(origin))
                .ok_or_else(|| "resolved source module has no canonical annotation origin".into()),
            ResolvedModuleTarget::Std { module, .. } => {
                let module = self
                    .checked
                    .std_registry
                    .module(*module)
                    .ok_or("resolved standard module is absent from the checked registry")?;
                Ok(AnnotationReferenceInfo::Module {
                    package: self.origins.standard_package,
                    path: module.path.clone(),
                })
            }
            ResolvedModuleTarget::External { package: Some(package), path, .. } => self.external_modules
                .get(package).and_then(|modules| modules.get(path.segments.as_slice())).cloned()
                .ok_or_else(|| "resolved external module requires a checked canonical package/module annotation origin".into()),
            ResolvedModuleTarget::External { package: None, .. } => Err("resolved external module has no checked package owner".into()),
        }
    }
}

fn module_reference(origin: &SourceModuleOrigin) -> AnnotationReferenceInfo {
    AnnotationReferenceInfo::Module {
        package: origin.package(),
        path: origin.module().to_vec(),
    }
}
