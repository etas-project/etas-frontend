use std::collections::BTreeMap;

use etas_code::{
    CallableId, CodeFingerprint, RevisionInputs,
    descriptor::{
        AgentHeaderInfo, BodyAccess, CallableBodyInfo, CallableHeaderInfo, DefinitionBodies,
        DefinitionSnapshotInfo, DefinitionSnapshots, DescriptorLimits,
    },
};

use super::{DefinitionCatalog, FrozenDefinitionEnvironment};
use crate::{
    CheckedProgram,
    code::{
        descriptor::ProjectionError, identity::DefinitionOrigins,
        service::NativeCodeSyntaxOperations,
    },
};

pub(super) fn export(
    checked: CheckedProgram,
    origins: &DefinitionOrigins,
    limits: DescriptorLimits,
    headers: &[AgentHeaderInfo],
    flow_headers: &[CallableHeaderInfo],
) -> Result<(DefinitionSnapshots, DefinitionBodies), ProjectionError> {
    if headers.is_empty()
        && flow_headers.is_empty()
        && !checked.definition_sources.has_retained_bodies()
    {
        return Ok((
            DefinitionSnapshots {
                agents: Vec::new(),
                flows: Vec::new(),
                methods: Vec::new(),
            },
            DefinitionBodies {
                entries: Vec::new(),
            },
        ));
    }
    // Public projection is deliberately incomplete: revisions must come from
    // the producer's full native definition, including private surroundings.
    let catalog = DefinitionCatalog::build(checked, origins, limits)?;
    let items = catalog
        .checked
        .items
        .iter()
        .filter_map(|(item, declaration)| {
            let symbol = declaration.symbol()?;
            catalog
                .agent_bindings
                .get(&symbol)
                .map(|identity| (identity, item))
        })
        .collect::<BTreeMap<_, _>>();
    let syntax = NativeCodeSyntaxOperations::default();
    let mut agents = Vec::with_capacity(headers.len());
    let mut methods = Vec::new();
    for header in headers {
        let item = items.get(&header.identity).copied().ok_or_else(|| {
            ProjectionError::MissingIdentity("published agent native declaration".into())
        })?;
        let frozen = catalog.freeze_agent_view(header.clone(), item)?;
        let visible = header
            .default_entry
            .iter()
            .chain(&header.methods)
            .collect::<std::collections::BTreeSet<_>>();
        methods.extend(
            catalog
                .source_method_snapshots(&frozen, &syntax)?
                .into_iter()
                .filter(|snapshot| visible.contains(&snapshot.revision.definition)),
        );
        agents.push(DefinitionSnapshotInfo {
            identity: header.identity.clone(),
            revision: catalog.source_revision(&frozen, &syntax)?,
            dependencies: frozen.dependency_info()?,
        });
    }
    let mut flows = Vec::with_capacity(flow_headers.len());
    for header in flow_headers {
        let item = catalog.item_owner(&header.id).ok_or_else(|| {
            ProjectionError::MissingIdentity("published flow native declaration".into())
        })?;
        let frozen = catalog.freeze_flow_view(header, item)?;
        flows.push(DefinitionSnapshotInfo {
            identity: frozen.identity().clone(),
            revision: catalog.source_revision(&frozen, &syntax)?,
            dependencies: frozen.dependency_info()?,
        });
    }
    let mut entries = Vec::new();
    for (callable, item) in catalog.native_callables() {
        let exposure = catalog.checked.definition_sources.exposure(item);
        if !exposure.retained {
            continue;
        }
        let slot = catalog
            .checked
            .definition_sources
            .body(item)
            .ok_or_else(|| {
                ProjectionError::MissingIdentity("retained body native source slot".into())
            })?;
        let source = catalog
            .checked
            .definition_sources
            .source(slot.source)
            .ok_or_else(|| {
                ProjectionError::MissingIdentity("retained body immutable source".into())
            })?;
        entries.push(CallableBodyInfo {
            callable: callable.clone(),
            access: if exposure.exported {
                BodyAccess::Exported
            } else {
                BodyAccess::Package
            },
            source: syntax
                .export_source_body(
                    source.file.clone(),
                    source.program.clone(),
                    etas_syntax::ProgramBodySelection {
                        item: slot.item.index,
                        impl_member: slot.impl_member,
                    },
                )
                .map_err(ProjectionError::Syntax)?,
        });
    }
    Ok((
        DefinitionSnapshots {
            agents,
            flows,
            methods,
        },
        DefinitionBodies { entries },
    ))
}

impl DefinitionCatalog {
    pub(super) fn source_revision(
        &self,
        environment: &FrozenDefinitionEnvironment,
        syntax: &NativeCodeSyntaxOperations,
    ) -> Result<RevisionInputs, ProjectionError> {
        let contract = environment.contract();
        let (definition, content) = match contract.entry() {
            Some(callable) => (callable.clone(), self.body_content(callable, syntax)?),
            None => (
                CallableId::declaration(contract.identity().clone())?,
                CodeFingerprint::of_bytes("etas.code.absent-default-body.v1", b""),
            ),
        };
        Ok(RevisionInputs {
            definition,
            content,
            environment: environment.fingerprint(),
            contract: environment.contract().fingerprint(),
            compilation: environment.compilation_fingerprint(),
            parent: None,
        })
    }
}
