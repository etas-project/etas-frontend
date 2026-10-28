use etas_code::{CallableId, descriptor::*};
use etas_effects::{CallableExitSummary, CallableOutcome, ReturnObservation};
use etas_hir::{HirItem, SymbolDef};

use super::{DefinitionDescriptorProjector, ProjectionError};

impl DefinitionDescriptorProjector<'_> {
    pub(super) fn project_outcomes(
        &mut self,
        exits: &CallableExitSummary,
        callable: &CallableId,
        signature: TypeInfoRef,
        parameters: &[etas_hir::SymbolId],
        scope: usize,
    ) -> Result<TraceInfoRef, ProjectionError> {
        self.ensure_nodes(exits.exits.len().saturating_mul(2).saturating_add(1))?;
        self.ensure_edges(exits.exits.len().saturating_mul(3))?;
        let root = self.reserve_trace()?;
        let mut branches = Vec::with_capacity(exits.exits.len());
        for exit in &exits.exits {
            let trace = self.analysis_trace(&exit.trace, callable, signature, parameters, scope)?;
            let outcome = match &exit.outcome {
                CallableOutcome::Returned { ty, observation } => {
                    let observation = match observation {
                        ReturnObservation::Any | ReturnObservation::CallableSources(_) => {
                            ReturnObservationInfo::Any
                        }
                        ReturnObservation::Unit => ReturnObservationInfo::Unit,
                        ReturnObservation::Bool(value) => ReturnObservationInfo::Bool(*value),
                        ReturnObservation::Variant(symbol) => {
                            let constructor = *symbol;
                            let symbol = self
                                .checked
                                .symbols
                                .get(*symbol)
                                .ok_or(ProjectionError::MissingSymbol(*symbol))?;
                            let SymbolDef::EnumVariant {
                                enum_item,
                                variant_index,
                            } = symbol.def
                            else {
                                return Err(ProjectionError::Invalid(
                                    "return variant has no checked constructor identity".into(),
                                ));
                            };
                            let Some(HirItem::Enum(decl)) = self.checked.items.get(enum_item)
                            else {
                                return Err(ProjectionError::Invalid(
                                    "return variant has no checked enum declaration".into(),
                                ));
                            };
                            let owner = self
                                .identities
                                .symbol(decl.symbol)
                                .ok_or(ProjectionError::MissingDeclaration(decl.symbol))?
                                .clone();
                            let variant =
                                decl.variants.get(variant_index as usize).ok_or_else(|| {
                                    ProjectionError::Invalid(
                                        "return constructor has no variant declaration".into(),
                                    )
                                })?;
                            if variant.symbol != constructor {
                                return Err(ProjectionError::Invalid(
                                    "return constructor differs from its declared enum slot".into(),
                                ));
                            }
                            let name = symbol.name.clone();
                            self.text(&name)?;
                            ReturnObservationInfo::Variant { owner, name }
                        }
                    };
                    CallableOutcomeInfo::Returned {
                        ty: self.type_ref(*ty, scope)?,
                        observation,
                    }
                }
                CallableOutcome::Raised { error } => CallableOutcomeInfo::Raised {
                    error: self.type_ref(*error, scope)?,
                },
                CallableOutcome::NonReturn => CallableOutcomeInfo::NonReturn,
            };
            let terminal = self.reserve_trace()?;
            self.traces[terminal.0 as usize] = Some(TraceInfo::Outcome(outcome));
            let branch = self.reserve_trace()?;
            self.traces[branch.0 as usize] = Some(TraceInfo::Seq(vec![trace, terminal]));
            branches.push(branch);
        }
        self.traces[root.0 as usize] = Some(TraceInfo::Choice(branches));
        Ok(root)
    }
}
