use super::*;
use crate::code::definition::{StaticReflectionView, store::check_access};
use etas_types::{CheckedReflectionTarget, ItemSignature};

pub(super) fn build(
    source: Arc<NativeDefinitionStore>,
    candidate: &CheckedCandidate,
    catalog: &DefinitionCatalog,
) -> Result<FrozenReflectionBindings, ProjectionError> {
    if !Arc::ptr_eq(
        source.catalog.checked_program(),
        candidate.frozen().checked(),
    ) || !Arc::ptr_eq(catalog.checked_program(), candidate.program())
        || *source.catalog.origins
            != candidate
                .frozen()
                .origins_for(candidate.frozen().checked())?
        || *catalog.origins != candidate.frozen().origins_for(candidate.program())?
    {
        return Err(invalid(
            "frozen reflection binding catalogs do not belong to the emitted candidate",
        ));
    }
    let mut access_modules = HashMap::new();
    for module in &source.catalog.checked.hir.modules {
        let package = source
            .catalog
            .origins
            .source_modules
            .get(module)
            .ok_or_else(|| invalid("source reflection module has no package origin"))?;
        if access_modules
            .insert((package.package(), package.module().to_vec()), *module)
            .is_some()
        {
            return Err(invalid("source reflection module identity is ambiguous"));
        }
    }
    let mut bindings = HashMap::new();
    for (expr, fact) in candidate.program().checked_reflections() {
        let access =
            fact.access_module
                .map(|module| {
                    let package = catalog.origins.source_modules.get(&module).ok_or_else(|| {
                        invalid("candidate reflection access has no package origin")
                    })?;
                    access_modules
                        .get(&(package.package(), package.module().to_vec()))
                        .copied()
                        .ok_or_else(|| {
                            invalid("candidate reflection access has no frozen source module")
                        })
                })
                .transpose()?;
        let candidate_agent = match fact.target {
            CheckedReflectionTarget::AgentValue { component, .. } => {
                let symbol = candidate
                    .program()
                    .types
                    .agent_components
                    .get(&component)
                    .ok_or_else(|| {
                        invalid("runtime reflection has no checked nominal agent binding")
                    })?;
                let Some(etas_hir::SymbolDef::Item { item }) = candidate
                    .program()
                    .symbols
                    .get(*symbol)
                    .map(|symbol| &symbol.def)
                else {
                    return Err(invalid(
                        "runtime reflection agent has no source declaration",
                    ));
                };
                Some(*item)
            }
            _ => None,
        };
        let view = match candidate_agent {
            Some(item) => catalog.agent_value_reflection(*expr, item)?,
            None => catalog.static_reflection(*expr)?,
        };
        let target = match view {
            StaticReflectionView::Agent(header) => {
                if let Some(external) = catalog.external_agents.get(&header.identity) {
                    let original = source
                        .catalog
                        .external_agents
                        .get(&header.identity)
                        .ok_or_else(|| {
                            invalid("external reflected agent has no frozen metadata binding")
                        })?;
                    if original.header != external.header
                        || original.graph != external.graph
                        || original.snapshot != external.snapshot
                        || original.callables != external.callables
                        || original.bodies != external.bodies
                    {
                        return Err(invalid(
                            "external reflected agent changed its frozen definition",
                        ));
                    }
                    bindings.insert(
                        *expr,
                        Binding {
                            access,
                            candidate_agent,
                            target: SourceTarget::ExternalAgent(Box::new(original.clone())),
                        },
                    );
                    continue;
                }
                let original = source.catalog.agents.get(&header.identity).ok_or_else(|| {
                    invalid("reflected nominal agent is not a frozen declaration")
                })?;
                check_access(original.private_to, access)?;
                let mut header = original.header.clone();
                let mut visible = Vec::new();
                for id in &header.methods {
                    let method = source
                        .catalog
                        .callables
                        .get(id)
                        .ok_or_else(|| invalid("frozen method has no native declaration"))?;
                    if check_access(method.native.private_to, access).is_ok() {
                        visible.push(id.clone());
                    }
                }
                let visible = visible
                    .into_iter()
                    .collect::<std::collections::BTreeSet<_>>();
                header.retain_additional_methods(|id| visible.contains(id));
                let mut items = source.catalog.checked.types.item_signatures.iter().filter_map(|(item, signature)| {
                    matches!(signature, ItemSignature::Agent(agent) if agent.component == original.component).then_some(*item)
                });
                let item = items
                    .next()
                    .ok_or_else(|| invalid("frozen nominal agent has no source item"))?;
                if items.next().is_some() {
                    return Err(invalid("frozen nominal agent has ambiguous source items"));
                }
                SourceTarget::Agent { header, item }
            }
            StaticReflectionView::Callable(header) => {
                if let Some(external) = catalog.external_callables.get(&header.id) {
                    let original = source
                        .catalog
                        .external_callables
                        .get(&external.canonical)
                        .ok_or_else(|| {
                            invalid("external reflected callable has no frozen metadata binding")
                        })?;
                    let mut selected_header = original.header.clone();
                    selected_header.id = header.id.clone();
                    if original.canonical != external.canonical
                        || original.graph != external.graph
                        || original.body != external.body
                        || selected_header != external.header
                    {
                        return Err(invalid(
                            "external reflected callable changed its frozen descriptors",
                        ));
                    }
                    bindings.insert(
                        *expr,
                        Binding {
                            access,
                            candidate_agent,
                            target: SourceTarget::ExternalCallable {
                                header: header.clone(),
                                canonical: original.canonical.clone(),
                                graph: original.graph.clone(),
                            },
                        },
                    );
                    continue;
                }
                let canonical = &catalog
                    .callables
                    .get(&header.id)
                    .ok_or_else(|| invalid("reflected callable has no canonical native binding"))?
                    .native
                    .canonical;
                let original = source
                    .catalog
                    .callables
                    .get(canonical)
                    .ok_or_else(|| invalid("reflected callable is not a frozen declaration"))?;
                check_access(original.native.private_to, access)?;
                let mut source_header = original.header.clone();
                source_header.id = header.id.clone();
                SourceTarget::Callable {
                    header: source_header,
                    canonical: canonical.clone(),
                }
            }
        };
        bindings.insert(
            *expr,
            Binding {
                access,
                candidate_agent,
                target,
            },
        );
    }
    Ok(FrozenReflectionBindings {
        program: candidate.program().clone(),
        source,
        bindings,
    })
}

fn invalid(reason: &str) -> ProjectionError {
    ProjectionError::Invalid(reason.into())
}
