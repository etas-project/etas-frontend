use super::{
    NativeDefinitionError, NativeDefinitionStore, NativeMethodSnapshot, SourceReflectionKey,
};
use crate::code::definition::StaticReflectionView;

impl NativeDefinitionStore {
    pub fn reflect_agent_value_method_snapshot(
        &self,
        expression: etas_hir::HirExprId,
        item: etas_hir::HirItemId,
    ) -> Result<NativeMethodSnapshot, NativeDefinitionError> {
        let access = self
            .catalog
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?
            .access_module;
        let StaticReflectionView::Callable(header) = self
            .catalog
            .agent_value_reflection(expression, item)
            .map_err(NativeDefinitionError::Projection)?
        else {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        };
        self.method_snapshot_at_access(&header.id, access)
    }

    pub fn reflect_method_snapshot(
        &self,
        expression: etas_hir::HirExprId,
    ) -> Result<NativeMethodSnapshot, NativeDefinitionError> {
        let access = self
            .catalog
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?
            .access_module;
        let StaticReflectionView::Callable(header) = self
            .catalog
            .static_reflection(expression)
            .map_err(NativeDefinitionError::Projection)?
        else {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        };
        self.method_snapshot_at_access(&header.id, access)
    }

    pub(super) fn method_snapshot_at_access(
        &self,
        callable: &etas_code::CallableId,
        access: Option<etas_hir::HirModuleId>,
    ) -> Result<NativeMethodSnapshot, NativeDefinitionError> {
        if let Some(external) = self.catalog.external_callables.get(callable) {
            let method = external
                .canonical
                .as_method()
                .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
            let owner = self
                .catalog
                .external_agents
                .get(method.implementation().subject())
                .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
            let key = SourceReflectionKey::External {
                component: owner.component,
                access,
            };
            let reference = match self.source_view(&key)? {
                Some(reference) => reference,
                None => self.remember_source_view(key, self.reflect_external(owner, access)?)?,
            };
            let snapshot = self
                .resolve(&reference)?
                .method_snapshot(&external.canonical)?;
            if external
                .snapshot
                .as_ref()
                .is_none_or(|published| published != snapshot.coordinates.as_ref())
            {
                return Err(NativeDefinitionError::CandidateInputMismatch);
            }
            return Ok(snapshot);
        }
        let native = &self
            .catalog
            .callables
            .get(callable)
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?
            .native;
        super::super::store::check_access(native.private_to, access)
            .map_err(NativeDefinitionError::Projection)?;
        let method = native
            .canonical
            .as_method()
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
        let identity = method.implementation().subject();
        let agent = self
            .catalog
            .agents
            .get(identity)
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
        super::super::store::check_access(agent.private_to, access)
            .map_err(NativeDefinitionError::Projection)?;
        let mut declarations =
            self.catalog
                .checked
                .items
                .iter()
                .filter_map(|(item, declaration)| {
                    if !matches!(declaration, etas_hir::HirItem::Agent(_)) {
                        return None;
                    }
                    declaration
                        .symbol()
                        .and_then(|symbol| self.catalog.agent_bindings.get(&symbol))
                        .filter(|owner| *owner == identity)
                        .map(|_| item)
                });
        let item = declarations
            .next()
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
        if declarations.next().is_some() {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        }
        let key = SourceReflectionKey::Native { item, access };
        let reference = match self.source_view(&key)? {
            Some(reference) => reference,
            None => {
                let environment = self
                    .catalog
                    .freeze_agent_view(agent.header.clone(), item)
                    .map_err(NativeDefinitionError::Projection)?;
                let body = agent
                    .header
                    .default_entry
                    .as_ref()
                    .map(|id| {
                        self.catalog
                            .retained_body_at_access(id, access, &self.syntax)
                    })
                    .transpose()
                    .map_err(NativeDefinitionError::Projection)?
                    .flatten();
                self.remember_source_view(key, self.reflect_frozen(environment, body, access)?)?
            }
        };
        self.resolve(&reference)?.method_snapshot(&native.canonical)
    }
}
