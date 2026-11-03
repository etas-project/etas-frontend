use etas_code::{definition::DefinitionRef, descriptor::DescriptorLimits};

use super::{NativeDefinitionError, NativeDefinitionStore, SourceReflectionKey};
use crate::CheckedCallableApplication;

pub(super) struct LexicalBindings {
    pub types: std::collections::HashMap<String, etas_types::TypeId>,
    pub source_parameters: std::collections::HashMap<etas_hir::SymbolId, etas_types::TypeId>,
    pub rows: std::collections::HashMap<String, etas_types::EffectRowRef>,
}

impl NativeDefinitionStore {
    pub fn reflect_callable_application(
        &self,
        expression: etas_hir::HirExprId,
        application: &CheckedCallableApplication,
        limits: DescriptorLimits,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        self.reflect_callable_application_in_context(expression, application, None, limits)
    }

    pub fn reflect_callable_application_in_context(
        &self,
        expression: etas_hir::HirExprId,
        application: &CheckedCallableApplication,
        lexical: Option<&CheckedCallableApplication>,
        limits: DescriptorLimits,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let application =
            self.checked_callable_reflection_application(expression, application, lexical, limits)?;
        let fact = self
            .catalog
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
        self.reflect_checked_application(&application, fact.access_module, limits)
    }

    /// Validate the checked callable binding and its presented effect interface.
    pub fn checked_callable_reflection_application(
        &self,
        expression: etas_hir::HirExprId,
        application: &CheckedCallableApplication,
        lexical: Option<&CheckedCallableApplication>,
        limits: DescriptorLimits,
    ) -> Result<CheckedCallableApplication, NativeDefinitionError> {
        let interface = self.checked_reflection_interface(expression, application, lexical)?;
        application
            .with_checked_interface(interface, limits)
            .map_err(|_| NativeDefinitionError::CandidateInputMismatch)
    }

    /// Checks the reflection target and interface shape. Effect compatibility
    /// must be checked against the selected source application's behavior.
    pub(crate) fn checked_reflection_interface(
        &self,
        expression: etas_hir::HirExprId,
        application: &CheckedCallableApplication,
        lexical: Option<&CheckedCallableApplication>,
    ) -> Result<etas_types::TypeId, NativeDefinitionError> {
        if !std::sync::Arc::ptr_eq(application.program(), &self.catalog.checked) {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        }
        let fact = self
            .catalog
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
        let bindings = self.lexical_bindings(expression, lexical)?;
        let mut substitution = etas_types::TypeSubstitutionEngine::existing(
            &self.catalog.checked.type_store,
            &bindings.types,
        )
        .with_source_parameters(&bindings.source_parameters)
        .with_effect_arguments(&bindings.rows);
        let descriptor_type = substitution
            .substitute(fact.descriptor_type)
            .map_err(|_| NativeDefinitionError::CandidateInputMismatch)?;
        let Some(etas_types::Type::Applied { constructor, args }) =
            self.catalog.checked.type_store.get(descriptor_type)
        else {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        };
        if Some(etas_types::TypeId(constructor.0))
            != self
                .catalog
                .checked
                .types
                .known_std_types
                .code
                .callable_definition
        {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        }
        let [signature] = args.as_slice() else {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        };
        match &fact.target {
            etas_types::CheckedReflectionTarget::CallableValue {
                signature: bound, ..
            } if substitution.substitute(*bound).ok().as_ref() == Some(signature) => {}
            etas_types::CheckedReflectionTarget::Callable { .. }
            | etas_types::CheckedReflectionTarget::ExternalCallable { .. } => {
                let (_, owner) = self
                    .catalog
                    .editable_flow_reflection(expression)
                    .map_err(NativeDefinitionError::Projection)?;
                if owner != application.owner() {
                    return Err(NativeDefinitionError::CandidateInputMismatch);
                }
                let bindings = self
                    .catalog
                    .checked
                    .types
                    .generic_instantiations
                    .get(&expression)
                    .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
                if !bindings.deferred_effect_row_obligations.is_empty() {
                    return Err(NativeDefinitionError::CandidateInputMismatch);
                }
                let Some(etas_types::ItemSignature::Flow(template)) =
                    self.catalog.checked.types.item_signatures.get(&owner)
                else {
                    return Err(NativeDefinitionError::CandidateInputMismatch);
                };
                if template.generic_params.len() != application.arguments().len()
                    || template
                        .generic_params
                        .iter()
                        .zip(application.arguments())
                        .any(|(param, arg)| match arg {
                            etas_types::CallableGenericArg::Type(ty) => {
                                !bindings.type_bindings.iter().any(|(name, expected)| {
                                    name == &param.name
                                        && substitution.substitute(*expected).ok().as_ref()
                                            == Some(ty)
                                })
                            }
                            etas_types::CallableGenericArg::EffectRow(row) => {
                                !bindings.effect_row_bindings.iter().any(|(name, expected)| {
                                    name == &param.name
                                        && substitution
                                            .substitute_effect_row(expected.clone())
                                            .ok()
                                            .is_some_and(|expected| {
                                                crate::project::instantiation::normalized_row(
                                                    &expected,
                                                ) == crate::project::instantiation::normalized_row(
                                                    row,
                                                )
                                            })
                                })
                            }
                        })
                {
                    return Err(NativeDefinitionError::CandidateInputMismatch);
                }
            }
            _ => return Err(NativeDefinitionError::CandidateInputMismatch),
        }
        let Some(etas_types::Type::Function(expected)) =
            self.catalog.checked.type_store.get(*signature)
        else {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        };
        let actual = application.signature();
        if expected.input != actual.params || expected.output != actual.output {
            return Err(NativeDefinitionError::CandidateInputMismatch);
        }
        Ok(*signature)
    }

    pub(super) fn reflect_checked_application(
        &self,
        application: &CheckedCallableApplication,
        access: Option<etas_hir::HirModuleId>,
        limits: DescriptorLimits,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let fingerprint = self
            .catalog
            .callable_application_fingerprint(application, limits)
            .map_err(NativeDefinitionError::Projection)?;
        let key = SourceReflectionKey::Applied {
            item: application.owner(),
            application: fingerprint,
            access,
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
            .retained_body_at_access(id, access, &self.syntax)
            .map_err(NativeDefinitionError::Projection)?;
        self.remember_source_view(key, self.reflect_frozen(environment, body, access)?)
            .map_err(Into::into)
    }
}

impl NativeDefinitionStore {
    pub(super) fn lexical_bindings(
        &self,
        expression: etas_hir::HirExprId,
        lexical: Option<&CheckedCallableApplication>,
    ) -> Result<LexicalBindings, NativeDefinitionError> {
        let mut types = std::collections::HashMap::new();
        let mut source_parameters = std::collections::HashMap::new();
        let mut rows = std::collections::HashMap::new();
        if let Some(lexical) = lexical {
            let owner = self
                .catalog
                .checked
                .hir
                .index()
                .expr_item
                .get(&expression)
                .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
            if !std::sync::Arc::ptr_eq(lexical.program(), &self.catalog.checked)
                || lexical.owner() != *owner
            {
                return Err(NativeDefinitionError::CandidateInputMismatch);
            }
            let template =
                crate::project::instantiation::signature(&self.catalog.checked.types, *owner)
                    .ok_or(NativeDefinitionError::CandidateInputMismatch)?;
            if template.generic_params.len() != lexical.arguments().len() {
                return Err(NativeDefinitionError::CandidateInputMismatch);
            }
            for (parameter, argument) in template.generic_params.iter().zip(lexical.arguments()) {
                match argument {
                    etas_types::CallableGenericArg::Type(ty)
                        if parameter.kind == etas_types::CallableGenericParamKind::Type =>
                    {
                        let Some(etas_types::Type::Named(name)) =
                            self.catalog.checked.type_store.get(parameter.subject)
                        else {
                            return Err(NativeDefinitionError::CandidateInputMismatch);
                        };
                        let Some(symbol) = name.source_parameter else {
                            return Err(NativeDefinitionError::CandidateInputMismatch);
                        };
                        if source_parameters.insert(symbol, *ty).is_some() {
                            return Err(NativeDefinitionError::CandidateInputMismatch);
                        }
                        types.insert(parameter.name.clone(), *ty);
                    }
                    etas_types::CallableGenericArg::EffectRow(row)
                        if parameter.kind == etas_types::CallableGenericParamKind::Effect =>
                    {
                        rows.insert(parameter.name.clone(), row.clone());
                    }
                    _ => return Err(NativeDefinitionError::CandidateInputMismatch),
                }
            }
        }
        Ok(LexicalBindings {
            types,
            source_parameters,
            rows,
        })
    }
}
