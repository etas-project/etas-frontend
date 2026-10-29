use etas_code::{definition::DefinitionRef, descriptor::DescriptorLimits};

use super::{NativeDefinitionError, NativeDefinitionStore, SourceReflectionKey};
use crate::CheckedCallableApplication;

impl NativeDefinitionStore {
    pub fn reflect_callable_application(
        &self,
        expression: etas_hir::HirExprId,
        application: &CheckedCallableApplication,
        limits: DescriptorLimits,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let fingerprint = self
            .catalog
            .callable_application_fingerprint(application, limits)
            .map_err(NativeDefinitionError::Projection)?;
        let fact = self
            .catalog
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
        let etas_types::CheckedReflectionTarget::CallableValue { signature, .. } = fact.target
        else {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        };
        if !matches!(self.catalog.checked.type_store.get(fact.descriptor_type),
            Some(etas_types::Type::Applied { constructor, args })
                if Some(etas_types::TypeId(constructor.0)) == self.catalog.checked.types.known_std_types.code.callable_definition
                    && args.as_slice() == [signature])
        {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        }
        let Some(etas_types::Type::Function(expected)) =
            self.catalog.checked.type_store.get(signature)
        else {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        };
        let actual = application.signature();
        if expected.input != actual.params
            || expected.output != actual.output
            || expected
                .effects
                .as_ref()
                .map(crate::project::instantiation::normalized_row)
                != actual
                    .effects
                    .as_ref()
                    .map(crate::project::instantiation::normalized_row)
        {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        }
        let key = SourceReflectionKey::Applied {
            item: application.owner(),
            application: fingerprint,
            access: fact.access_module,
        };
        if let Some(reference) = self.source_view(&key)? {
            let view = self.resolve(&reference)?;
            let cached = view
                .environment()?
                .callable_application()
                .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
            if super::store::same_checked_application(cached, application) {
                return Ok(reference);
            }
        }
        let environment = self
            .catalog
            .freeze_callable_application(application, limits)
            .map_err(NativeDefinitionError::Projection)?;
        let id = environment
            .contract()
            .entry()
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
        let body = self
            .catalog
            .retained_body_at_access(id, fact.access_module, &self.syntax)
            .map_err(NativeDefinitionError::Projection)?;
        self.remember_source_view(
            key,
            self.reflect_frozen(environment, body, fact.access_module)?,
        )
        .map_err(Into::into)
    }
}
