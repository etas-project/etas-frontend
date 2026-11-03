use std::{collections::HashMap, sync::Arc};

use etas_code::descriptor::DescriptorLimits;
use etas_effects::{EffectSummary, substitution::EffectSummarySubstitution};
use etas_types::{CallableGenericArg, TypeSubstitutionEngine};

use super::{CheckedCallableApplication, signature};

/// Applied analysis of the original checked implementation. The application is
/// retained with source return/capture provenance; this is never S_new or an
/// emitted candidate, and cannot authorize compilation or execution by itself.
#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct AppliedCallableBehavior {
    application: CheckedCallableApplication,
    summary: Arc<EffectSummary>,
}

#[derive(Debug)]
pub enum CallableBehaviorError {
    MissingSummary(etas_hir::HirItemId),
    InvalidApplication(String),
    Substitution(etas_effects::substitution::SummarySubstitutionError),
}

impl std::fmt::Display for CallableBehaviorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSummary(item) => write!(
                f,
                "native application has no checked source summary: {item:?}"
            ),
            Self::InvalidApplication(reason) => {
                write!(f, "invalid checked application behavior: {reason}")
            }
            Self::Substitution(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for CallableBehaviorError {}

impl AppliedCallableBehavior {
    pub fn application(&self) -> &CheckedCallableApplication {
        &self.application
    }
    pub fn summary(&self) -> &EffectSummary {
        &self.summary
    }
}

impl CheckedCallableApplication {
    pub fn applied_behavior(
        &self,
        limits: DescriptorLimits,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<AppliedCallableBehavior, CallableBehaviorError> {
        self.applied_behavior_with_residual_bindings(limits, control, None)
    }

    pub(crate) fn applied_behavior_with_residual_bindings(
        &self,
        limits: DescriptorLimits,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
        residual_bindings: Option<
            &std::collections::BTreeMap<
                etas_code::residual::identity::GenericParameterId,
                etas_code::residual::identity::GenericArgumentIdentity,
            >,
        >,
    ) -> Result<AppliedCallableBehavior, CallableBehaviorError> {
        let checked = self.program();
        let source = checked
            .effects
            .item_effects
            .get(&self.owner())
            .ok_or(CallableBehaviorError::MissingSummary(self.owner()))?;
        let template = signature(&checked.types, self.owner()).ok_or_else(|| {
            CallableBehaviorError::InvalidApplication("checked signature is missing".into())
        })?;
        let parameters = match checked.items.get(self.owner()) {
            Some(etas_hir::HirItem::Flow(flow)) => &flow.type_params,
            Some(etas_hir::HirItem::Tool(tool)) => &tool.type_params,
            _ => {
                return Err(CallableBehaviorError::InvalidApplication(
                    "checked declaration is missing".into(),
                ));
            }
        };
        let mut types = HashMap::new();
        for (symbol, argument) in parameters.iter().zip(self.arguments()) {
            if let CallableGenericArg::Type(ty) = argument {
                types.insert(*symbol, *ty);
            }
        }
        let row_refs = if self.arguments().is_empty() {
            HashMap::new()
        } else {
            // Preserve prepared row spelling for readonly lookup of nested
            // callable/handler types, without allocating a new type universe.
            checked
                .native_instantiations
                .checked_instance(self.owner(), self.arguments())
                .map_err(CallableBehaviorError::InvalidApplication)?
                .ok_or_else(|| {
                    CallableBehaviorError::InvalidApplication("prepared instance is missing".into())
                })?
                .bindings(Some(template))
                .map_err(CallableBehaviorError::InvalidApplication)?
                .1
        };
        let rows = row_refs
            .iter()
            .map(|(name, row)| {
                etas_effects::validate::resolve_checked_type_row(
                    &checked.type_store,
                    &checked.effect_registry,
                    row,
                )
                .map(|row| (name.clone(), row))
                .map_err(CallableBehaviorError::InvalidApplication)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let names = HashMap::new();
        let types = TypeSubstitutionEngine::existing(&checked.type_store, &names)
            .with_source_parameters(&types)
            .with_effect_arguments(&row_refs)
            .with_expansion_depth(limits.max_expansion_depth);
        let mut substitution = EffectSummarySubstitution::new(
            types,
            &checked.effect_registry,
            &rows,
            limits.max_nodes,
            control,
        );
        if let Some(bindings) = residual_bindings {
            substitution = substitution.with_residual_bindings(bindings);
        }
        let summary = substitution
            .apply(source)
            .map_err(CallableBehaviorError::Substitution)?;
        if residual_bindings.is_some()
            && summary.residual_checks.iter().any(|reference| {
                reference.substitutions().values().any(|argument| {
                    matches!(argument,
                etas_code::residual::identity::GenericArgumentIdentity::TypeParameter(_)
                | etas_code::residual::identity::GenericArgumentIdentity::EffectRowParameter(_)
            )
                })
            })
        {
            return Err(CallableBehaviorError::InvalidApplication(
                "applied residual obligation retains an unbound generic parameter".into(),
            ));
        }
        Ok(AppliedCallableBehavior {
            application: self.clone(),
            summary: Arc::new(summary),
        })
    }
}
