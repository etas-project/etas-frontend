use std::{collections::HashMap, sync::Arc};

use etas_hir::{HirItem, HirItemId, SymbolDef};
use etas_types::{
    CallableGenericArg, CallableGenericParamKind, CallableSignature, EffectRowRef, Type, TypeId,
    TypeSubstitutionEngine,
};

use super::instantiation::{normalized_row, signature};
use crate::{CheckedProgram, CheckedProgramData};

mod behavior;
pub use behavior::{AppliedCallableBehavior, CallableBehaviorError};

/// Native instantiation evidence, pinned to its checked type universe. This is
/// neither a portable descriptor nor a candidate-compilation permit.
#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct CheckedCallableApplication {
    program: CheckedProgram,
    owner: HirItemId,
    arguments: Vec<CallableGenericArg>,
    signature: CallableSignature,
    presented_type: Option<TypeId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallableApplicationError {
    MissingCallable(HirItemId),
    InvalidBinding(String),
    InvalidPreparedPlan(String),
    UnpreparedApplication(HirItemId),
    Type(etas_types::TypeSubstitutionError),
    Interrupted(etas_utils::PipelineInterruption),
}

impl std::fmt::Display for CallableApplicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingCallable(item) => {
                write!(f, "native application has no checked callable: {item:?}")
            }
            Self::InvalidBinding(reason) => {
                write!(f, "native application has invalid bindings: {reason}")
            }
            Self::InvalidPreparedPlan(reason) => {
                write!(
                    f,
                    "native application has inconsistent checked evidence: {reason}"
                )
            }
            Self::UnpreparedApplication(item) => write!(
                f,
                "native application was not prepared by this checked program: {item:?}"
            ),
            Self::Type(error) => error.fmt(f),
            Self::Interrupted(interruption) => write!(
                f,
                "native application enumeration interrupted: {}",
                interruption.reason
            ),
        }
    }
}

impl std::error::Error for CallableApplicationError {}

impl From<etas_types::TypeSubstitutionError> for CallableApplicationError {
    fn from(error: etas_types::TypeSubstitutionError) -> Self {
        Self::Type(error)
    }
}

impl CheckedCallableApplication {
    pub fn program(&self) -> &CheckedProgram {
        &self.program
    }
    pub fn owner(&self) -> HirItemId {
        self.owner
    }
    pub fn arguments(&self) -> &[CallableGenericArg] {
        &self.arguments
    }
    pub fn signature(&self) -> &CallableSignature {
        &self.signature
    }

    pub fn presented_type(&self) -> Option<TypeId> {
        self.presented_type
    }

    /// The interface comes from a checked reflection fact in this program;
    /// the existing solved behavior must fit its effect bound.
    pub(crate) fn with_checked_interface(
        &self,
        ty: TypeId,
        limits: etas_code::descriptor::DescriptorLimits,
    ) -> Result<Self, CallableApplicationError> {
        let Some(Type::Function(interface)) = self.program.type_store.get(ty) else {
            return Err(CallableApplicationError::InvalidBinding(
                "checked callable interface is not a function type".into(),
            ));
        };
        if self.signature.params != interface.input || self.signature.output != interface.output {
            return Err(CallableApplicationError::InvalidBinding(
                "checked callable interface has different parameter or result types".into(),
            ));
        }
        if self.signature.effects.as_ref().map(normalized_row)
            == interface.effects.as_ref().map(normalized_row)
        {
            return Ok(self.clone());
        }
        if let Some(row) = &interface.effects {
            let allowed = etas_effects::validate::resolve_checked_type_row(
                &self.program.type_store,
                &self.program.effect_registry,
                row,
            )
            .map_err(CallableApplicationError::InvalidBinding)?;
            let behavior = self.applied_behavior(limits, None).map_err(|error| {
                CallableApplicationError::InvalidPreparedPlan(error.to_string())
            })?;
            if !(etas_effects::EffectCoverage {
                registry: &self.program.effect_registry,
                types: &self.program.type_store,
            })
            .row_covers(&allowed, &behavior.summary().escaping_effects)
            {
                return Err(CallableApplicationError::InvalidBinding(
                    "checked callable behavior exceeds its presented effect interface".into(),
                ));
            }
        }
        let mut result = self.clone();
        result.signature.effects = interface.effects.clone();
        result.presented_type = Some(ty);
        Ok(result)
    }
}

impl CheckedProgramData {
    /// Enumerate the applications already proved and materialized for this
    /// program. These are local checked facts, not portable type identities.
    pub fn checked_callable_applications(
        self: &Arc<Self>,
        owner: HirItemId,
    ) -> Result<Vec<CheckedCallableApplication>, CallableApplicationError> {
        self.enumerate_callable_applications(owner, None)
    }

    pub fn checked_callable_applications_controlled(
        self: &Arc<Self>,
        owner: HirItemId,
        control: &dyn etas_utils::PipelineExecutionControl,
    ) -> Result<Vec<CheckedCallableApplication>, CallableApplicationError> {
        self.enumerate_callable_applications(owner, Some(control))
    }

    fn enumerate_callable_applications(
        self: &Arc<Self>,
        owner: HirItemId,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<Vec<CheckedCallableApplication>, CallableApplicationError> {
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(CallableApplicationError::Interrupted)?;
        }
        let template = signature(&self.types, owner)
            .ok_or(CallableApplicationError::MissingCallable(owner))?;
        let mut applications = Vec::new();
        for instance in self.native_instantiations.instances().iter() {
            if let Some(control) = control {
                control
                    .checkpoint()
                    .map_err(CallableApplicationError::Interrupted)?;
            }
            if instance.owner != owner {
                continue;
            }
            let (types, rows) = instance
                .bindings(Some(template))
                .map_err(CallableApplicationError::InvalidPreparedPlan)?;
            applications.push(self.checked_callable_application(
                owner,
                &types.into_iter().collect::<Vec<_>>(),
                &rows.into_iter().collect::<Vec<_>>(),
            )?);
        }
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(CallableApplicationError::Interrupted)?;
        }
        Ok(applications)
    }

    pub fn checked_callable_application(
        self: &Arc<Self>,
        owner: HirItemId,
        type_bindings: &[(String, TypeId)],
        effect_row_bindings: &[(String, EffectRowRef)],
    ) -> Result<CheckedCallableApplication, CallableApplicationError> {
        let invalid = |reason: &str| CallableApplicationError::InvalidBinding(reason.into());
        let inconsistent =
            |reason: &str| CallableApplicationError::InvalidPreparedPlan(reason.into());
        let template = signature(&self.types, owner)
            .ok_or(CallableApplicationError::MissingCallable(owner))?;
        let (parameters, value_parameter_count) = match self.items.get(owner) {
            Some(HirItem::Flow(flow)) => (&flow.type_params, flow.params.len()),
            Some(HirItem::Tool(tool)) => (&tool.type_params, tool.params.len()),
            _ => return Err(CallableApplicationError::MissingCallable(owner)),
        };
        if template.generic_params.len() != parameters.len()
            || template.params.len() != value_parameter_count
        {
            return Err(inconsistent("signature binder count disagrees with HIR"));
        }
        if parameters.len() != type_bindings.len() + effect_row_bindings.len() {
            return Err(invalid(
                "generic argument count disagrees with the declaration",
            ));
        }
        let mut types = HashMap::new();
        for (name, ty) in type_bindings {
            if types.insert(name.clone(), *ty).is_some() {
                return Err(invalid("duplicate type argument"));
            }
        }
        let mut rows = HashMap::new();
        for (name, row) in effect_row_bindings {
            if rows.insert(name.clone(), normalized_row(row)).is_some() {
                return Err(invalid("duplicate effect-row argument"));
            }
        }
        let names = HashMap::new();
        let validation = TypeSubstitutionEngine::existing(&self.type_store, &names);
        let mut source_parameters = HashMap::new();
        let mut arguments = Vec::with_capacity(parameters.len());
        for (parameter, symbol) in template.generic_params.iter().zip(parameters) {
            let source = self
                .symbols
                .get(*symbol)
                .ok_or_else(|| inconsistent("generic binder symbol is missing"))?;
            if source.name != parameter.name
                || !matches!(self.type_store.get(parameter.subject), Some(Type::Named(name)) if name.source_parameter == Some(*symbol))
            {
                return Err(inconsistent(
                    "generic binder identity disagrees with the declaration",
                ));
            }
            match (parameter.kind, &source.def) {
                (CallableGenericParamKind::Type, SymbolDef::TypeParam { .. }) => {
                    let ty = *types
                        .get(&parameter.name)
                        .ok_or_else(|| invalid("missing or wrong-kind type argument"))?;
                    validation.validate_closed_argument(ty)?;
                    source_parameters.insert(*symbol, ty);
                    arguments.push(CallableGenericArg::Type(ty));
                }
                (CallableGenericParamKind::Effect, SymbolDef::EffectParam { .. }) => {
                    let row = rows
                        .get(&parameter.name)
                        .ok_or_else(|| invalid("missing or wrong-kind effect-row argument"))?;
                    validation.validate_closed_effect_argument(row)?;
                    arguments.push(CallableGenericArg::EffectRow(row.clone()));
                }
                _ => {
                    return Err(inconsistent(
                        "generic binder kind disagrees with the declaration",
                    ));
                }
            }
        }
        if !arguments.is_empty() {
            let prepared = self
                .native_instantiations
                .checked_instance(owner, &arguments)
                .map_err(CallableApplicationError::InvalidPreparedPlan)?
                .ok_or(CallableApplicationError::UnpreparedApplication(owner))?;
            // The row set has been proved equal. Preserve the prepared spelling
            // for readonly type lookup: it owns the already-materialized TypeIds.
            rows = prepared
                .bindings(Some(template))
                .map_err(CallableApplicationError::InvalidPreparedPlan)?
                .1;
        }
        let mut engine = TypeSubstitutionEngine::existing(&self.type_store, &names)
            .with_source_parameters(&source_parameters)
            .with_effect_arguments(&rows);
        let signature = CallableSignature {
            generic_params: Vec::new(),
            params: template
                .params
                .iter()
                .map(|ty| engine.substitute(*ty))
                .collect::<Result<_, _>>()?,
            output: engine.substitute(template.output)?,
            effects: template
                .effects
                .clone()
                .map(|row| {
                    engine
                        .substitute_effect_row(row)
                        .map(|row| normalized_row(&row))
                })
                .transpose()?,
            requested_actions: template
                .requested_actions
                .clone()
                .map(|row| {
                    engine
                        .substitute_effect_row(row)
                        .map(|row| normalized_row(&row))
                })
                .transpose()?,
        };
        Ok(CheckedCallableApplication {
            program: self.clone(),
            owner,
            arguments,
            signature,
            presented_type: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntryPolicy, Frontend, ProjectInput, SourceInput};

    #[test]
    fn presented_interface_rejects_effects_outside_the_checked_bound() {
        let mut input = ProjectInput::single_source(SourceInput::anonymous(
            r#"
module app;
effect Audit { action ping() -> unit; }
flow effectful<T>(value: T) -> T { perform Audit.ping(); return value; }
flow pure(value: bool) -> bool ![] { return value; }
flow prepare() -> unit { let p: (bool) -> bool ![] = pure; effectful<bool>(true); return; }
"#,
        ));
        input.options.entry_policy = EntryPolicy::Optional;
        let output = Frontend.check_project(input);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let checked = output.checked.unwrap();
        let owner = checked
            .items
            .iter()
            .find_map(|(item, declaration)| match declaration {
                HirItem::Flow(flow)
                    if checked.symbols.get(flow.symbol).unwrap().name == "effectful" =>
                {
                    Some(item)
                }
                _ => None,
            })
            .unwrap();
        let boolean = checked
            .type_store
            .iter()
            .find_map(|(id, ty)| {
                (ty == &Type::Primitive(etas_types::PrimitiveType::Bool)).then_some(id)
            })
            .unwrap();
        let interface = checked
            .type_store
            .iter()
            .find_map(|(id, ty)| match ty {
                Type::Function(flow)
                    if flow.input == [boolean]
                        && flow.output == boolean
                        && flow
                            .effects
                            .as_ref()
                            .is_some_and(|row| row.effects.is_empty() && row.tail.is_none()) =>
                {
                    Some(id)
                }
                _ => None,
            })
            .unwrap();
        let application = checked
            .checked_callable_application(owner, &[("T".into(), boolean)], &[])
            .unwrap();
        assert!(
            !application
                .applied_behavior(Default::default(), None)
                .unwrap()
                .summary()
                .escaping_effects
                .is_empty()
        );
        assert!(
            application
                .with_checked_interface(interface, Default::default())
                .is_err()
        );
    }
}
