use std::collections::HashMap;

use etas_code::{CallableId, DefinitionKey, DefinitionKind, PackageKey};
use etas_core::SourceId;
use etas_hir::{HirItemId, HirProgram};

use crate::{ModuleOrigin, ProjectContext};

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub enum CpuCallableOrigin {
    Program,
    Package(CallableId),
}

pub(super) struct CallableOrigins(HashMap<SourceId, Option<ModuleOwner>>);

enum ModuleOwner {
    Program,
    Package {
        package: PackageKey,
        module: Vec<String>,
    },
}

impl CallableOrigins {
    pub(super) fn new(context: &ProjectContext) -> Option<Self> {
        let sources = context.sources.as_ref()?;
        let modules = context.modules.as_ref()?;
        let environment = context
            .validated_external_environment
            .as_ref()?
            .environment();
        Some(Self(
            sources
                .files
                .iter()
                .map(|source| {
                    let owner = (|| {
                        let module = modules.modules.get(*modules.by_source.get(&source.id)?)?;
                        Some(match module.origin {
                            ModuleOrigin::DependencySourceOverlay { package, .. } => {
                                let owner = environment
                                    .source_package_for(source.id, &source.kind)
                                    .ok()?;
                                let module = modules
                                    .dependency_namespaces
                                    .source_for_namespace(package, &module.path)
                                    .ok()?;
                                ModuleOwner::Package {
                                    package: owner,
                                    module: module.segments,
                                }
                            }
                            _ => ModuleOwner::Program,
                        })
                    })();
                    (source.id, owner)
                })
                .collect(),
        ))
    }

    pub(super) fn callable(
        &self,
        hir: &HirProgram,
        item: HirItemId,
        types: &etas_types::TypeFacts,
    ) -> Option<CpuCallableOrigin> {
        let item = hir.items.get(item)?;
        let symbol = item.symbol()?;
        // Methods have an implementation-owned CallableId and receiver contract.
        if types.type_impl_methods.contains_key(&symbol)
            || types.agent_methods.contains_key(&symbol)
        {
            return None;
        }
        match self.0.get(&item.span().source)?.as_ref()? {
            ModuleOwner::Program => Some(CpuCallableOrigin::Program),
            ModuleOwner::Package { package, module } => {
                let identity = DefinitionKey::new(
                    *package,
                    module.clone(),
                    vec![hir.symbols.get(symbol)?.name.clone()],
                    DefinitionKind::Flow,
                )
                .ok()?;
                Some(CpuCallableOrigin::Package(
                    CallableId::declaration(identity).ok()?,
                ))
            }
        }
    }
}
