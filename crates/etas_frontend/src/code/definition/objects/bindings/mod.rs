use std::{collections::HashMap, sync::Arc};

use super::{
    super::DefinitionCatalog, NativeDefinitionError, NativeDefinitionStore, SourceReflectionKey,
};
use crate::{
    CheckedProgram,
    code::{descriptor::ProjectionError, service::CheckedCandidate},
};
use etas_code::{
    CallableId,
    definition::AgentDefinitionRef,
    descriptor::{AgentHeaderInfo, CallableHeaderInfo},
    syntax::SyntaxBlockRef,
};
use etas_hir::{HirExprId, HirItemId, HirModuleId};

mod build;

pub enum NativeSourceReflection {
    Agent(AgentDefinitionRef),
    Callable(Box<NativeCallableReflection>),
}

pub struct NativeCallableReflection {
    pub header: CallableHeaderInfo,
    pub graph: etas_code::descriptor::DescriptorGraph,
    pub body: Option<SyntaxBlockRef>,
}

enum SourceTarget {
    Agent {
        header: AgentHeaderInfo,
        item: HirItemId,
    },
    Callable {
        header: CallableHeaderInfo,
        canonical: CallableId,
    },
    ExternalCallable {
        header: CallableHeaderInfo,
        canonical: CallableId,
        graph: etas_code::descriptor::DescriptorGraph,
    },
    ExternalAgent(Box<super::super::external::ExternalAgent>),
}

struct Binding {
    candidate_agent: Option<HirItemId>,
    access: Option<HirModuleId>,
    target: SourceTarget,
}

/// Native source bindings verified once against an emitted candidate. HIR IDs
/// remain local to their owning programs; reflection does not rebind a source
/// declaration to whichever implementation happens to be executing.
pub struct FrozenReflectionBindings {
    program: CheckedProgram,
    source: Arc<NativeDefinitionStore>,
    bindings: HashMap<HirExprId, Binding>,
}

impl FrozenReflectionBindings {
    pub fn build(
        source: Arc<NativeDefinitionStore>,
        candidate: &CheckedCandidate,
        catalog: &DefinitionCatalog,
    ) -> Result<Self, NativeDefinitionError> {
        build::build(source, candidate, catalog).map_err(NativeDefinitionError::Projection)
    }
    pub fn source(&self) -> &Arc<NativeDefinitionStore> {
        &self.source
    }

    pub fn reflect(
        &self,
        program: &CheckedProgram,
        expression: HirExprId,
        candidate_agent: Option<HirItemId>,
    ) -> Result<NativeSourceReflection, NativeDefinitionError> {
        let invalid = || NativeDefinitionError::CandidateInputMismatch;
        if !Arc::ptr_eq(program, &self.program) {
            return Err(invalid());
        }
        let binding = self.bindings.get(&expression).ok_or_else(invalid)?;
        if binding.candidate_agent != candidate_agent {
            return Err(invalid());
        }
        match &binding.target {
            SourceTarget::ExternalAgent(external) => {
                let key = SourceReflectionKey::External {
                    component: external.component,
                    access: binding.access,
                };
                if let Some(reference) = self.source.source_view(key)? {
                    return Ok(NativeSourceReflection::Agent(reference));
                }
                let reference = self.source.reflect_external(external, binding.access)?;
                self.source
                    .remember_source_view(key, reference)
                    .map(NativeSourceReflection::Agent)
                    .map_err(Into::into)
            }
            SourceTarget::Agent { header, item } => {
                let key = SourceReflectionKey::Native {
                    item: *item,
                    access: binding.access,
                };
                if let Some(reference) = self.source.source_view(key)? {
                    return Ok(NativeSourceReflection::Agent(reference));
                }
                let environment = self
                    .source
                    .catalog
                    .freeze_agent_view(header.clone(), *item)
                    .map_err(NativeDefinitionError::Projection)?;
                let body = header
                    .default_entry
                    .as_ref()
                    .map(|callable| {
                        self.source.catalog.retained_body_at_access(
                            callable,
                            binding.access,
                            &self.source.syntax,
                        )
                    })
                    .transpose()
                    .map_err(NativeDefinitionError::Projection)?
                    .flatten();
                let reference = self
                    .source
                    .reflect_frozen(environment, body, binding.access)?;
                self.source
                    .remember_source_view(key, reference)
                    .map(NativeSourceReflection::Agent)
                    .map_err(Into::into)
            }
            SourceTarget::Callable { header, canonical } => {
                let body = self
                    .source
                    .catalog
                    .retained_body_at_access(canonical, binding.access, &self.source.syntax)
                    .map_err(NativeDefinitionError::Projection)?;
                Ok(NativeSourceReflection::Callable(Box::new(
                    NativeCallableReflection {
                        header: header.clone(),
                        graph: self.source.catalog.descriptors().clone(),
                        body,
                    },
                )))
            }
            SourceTarget::ExternalCallable {
                header,
                canonical,
                graph,
            } => {
                let body = self
                    .source
                    .catalog
                    .retained_body_at_access(canonical, binding.access, &self.source.syntax)
                    .map_err(NativeDefinitionError::Projection)?;
                Ok(NativeSourceReflection::Callable(Box::new(
                    NativeCallableReflection {
                        header: header.clone(),
                        graph: graph.clone(),
                        body,
                    },
                )))
            }
        }
    }
}
