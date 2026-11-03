use std::collections::BTreeMap;

use etas_code::{
    CodeFingerprint,
    contract::CanonicalEffect,
    descriptor::GenericParameterKind,
    residual::{
        identity::{GenericArgumentIdentity, GenericParameterId},
        type_binding::TypeArgumentDescriptors,
    },
};
use etas_types::CallableGenericArg;

use super::{CheckedResidualPlans, DefinitionDescriptorProjector, ResidualActionError, item_owner};

/// One application proved by this checked program. Preparation can share it;
/// each invocation still owns its monitor state.
#[derive(Clone, Debug)]
pub struct CheckedResidualApplication {
    pub fingerprint: CodeFingerprint,
    pub arguments: Vec<CallableGenericArg>,
    pub type_bindings: Vec<(String, etas_types::TypeId)>,
    pub effect_row_bindings: Vec<(String, etas_types::EffectRowRef)>,
    pub bindings: BTreeMap<GenericParameterId, GenericArgumentIdentity>,
    pub type_descriptors: TypeArgumentDescriptors,
    pub actions: Vec<CanonicalEffect>,
}

impl CheckedResidualPlans {
    pub fn applications(
        &self,
        program: &crate::CheckedProgram,
        item: etas_hir::HirItemId,
    ) -> Result<Vec<CheckedResidualApplication>, ResidualActionError> {
        let fail = |error: String| ResidualActionError::Projection(error);
        let identities = self
            .identities
            .as_deref()
            .ok_or(ResidualActionError::MissingIdentityIndex)?;
        let owner = item_owner(identities, &program.hir, item)
            .map_err(|_| ResidualActionError::UnknownOwner(item))?;
        let source_owner = self
            .item_plans(item)
            .next()
            .and_then(|plan| plan.plan().obligations().iter().next())
            .ok_or(ResidualActionError::UnknownOwner(item))?
            .identity()
            .owner;
        let applications = match program.hir.items.get(item) {
            Some(etas_hir::HirItem::Flow(flow)) if flow.type_params.is_empty() => vec![None],
            Some(etas_hir::HirItem::Tool(tool)) if tool.type_params.is_empty() => vec![None],
            Some(etas_hir::HirItem::Flow(_) | etas_hir::HirItem::Tool(_)) => program
                .checked_callable_applications(item)
                .map_err(|error| fail(error.to_string()))?
                .into_iter()
                .map(Some)
                .collect(),
            Some(etas_hir::HirItem::Agent(_)) => vec![None],
            _ => return Err(ResidualActionError::UnknownOwner(item)),
        };
        let signature = crate::project::instantiation::signature(&program.types, item);
        let mut result = Vec::with_capacity(applications.len());
        for application in applications {
            let arguments = application
                .as_ref()
                .map_or(&[][..], |value| value.arguments());
            let mut projector = DefinitionDescriptorProjector::from_input(
                self.descriptor_input(program),
                identities,
                self.limits,
            );
            let mut roots = Vec::with_capacity(arguments.len());
            for argument in arguments {
                roots.push(match argument {
                    CallableGenericArg::Type(ty) => etas_code::descriptor::DescriptorNode::Type(
                        projector
                            .project_closed_types(owner.clone(), &[*ty])
                            .map_err(|error| fail(error.to_string()))?[0],
                    ),
                    CallableGenericArg::EffectRow(row) => {
                        etas_code::descriptor::DescriptorNode::Row(
                            projector
                                .project_closed_rows(owner.clone(), std::slice::from_ref(row))
                                .map_err(|error| fail(error.to_string()))?[0],
                        )
                    }
                });
            }
            let graph = projector
                .finish()
                .map_err(|error| fail(error.to_string()))?;
            let mut bindings = BTreeMap::new();
            let mut type_roots = BTreeMap::new();
            let mut parts = Vec::new();
            for (position, root) in roots.into_iter().enumerate() {
                let (kind, identity) = match root {
                    etas_code::descriptor::DescriptorNode::Type(ty) => (
                        GenericParameterKind::Type,
                        GenericArgumentIdentity::Type(
                            graph
                                .type_fingerprint(ty)
                                .map_err(|error| fail(error.to_string()))?,
                        ),
                    ),
                    etas_code::descriptor::DescriptorNode::Row(row) => (
                        GenericParameterKind::EffectRow,
                        GenericArgumentIdentity::EffectRow(
                            graph
                                .row_fingerprint(row)
                                .map_err(|error| fail(error.to_string()))?,
                        ),
                    ),
                    _ => unreachable!(),
                };
                let id = GenericParameterId::from_source_owner(source_owner, position as u32, kind);
                let fingerprint = match identity {
                    GenericArgumentIdentity::Type(fp) | GenericArgumentIdentity::EffectRow(fp) => {
                        fp
                    }
                    _ => unreachable!(),
                };
                parts.push(id.fingerprint().as_bytes().to_vec());
                parts.push(fingerprint.as_bytes().to_vec());
                bindings.insert(id, identity);
                if let etas_code::descriptor::DescriptorNode::Type(root) = root {
                    type_roots.insert(id, root);
                }
            }
            let type_descriptors = TypeArgumentDescriptors::new(graph, type_roots, self.limits)
                .map_err(|error| fail(error.to_string()))?;
            let behavior = application
                .as_ref()
                .map(|application| {
                    application
                        .applied_behavior_with_residual_bindings(self.limits, None, Some(&bindings))
                        .map_err(|error| fail(error.to_string()))
                })
                .transpose()?;
            let summary = match &behavior {
                Some(behavior) => behavior.summary(),
                None => program
                    .effects
                    .item_effects
                    .get(&item)
                    .ok_or_else(|| fail("residual owner has no effect summary".into()))?,
            };
            let mut actions = Vec::new();
            for effect in summary
                .requested_actions
                .effects
                .iter()
                .chain(summary.default_actions.effects.iter())
                .filter(|effect| effect.is_action())
            {
                let action = self.canonical_action(program, item, effect)?;
                if !actions.contains(&action) {
                    actions.push(action);
                }
            }
            let mut type_bindings = Vec::new();
            let mut effect_row_bindings = Vec::new();
            if let Some(signature) = signature {
                for (parameter, argument) in signature.generic_params.iter().zip(arguments) {
                    match argument {
                        CallableGenericArg::Type(ty) => {
                            type_bindings.push((parameter.name.clone(), *ty))
                        }
                        CallableGenericArg::EffectRow(row) => {
                            effect_row_bindings.push((parameter.name.clone(), row.clone()))
                        }
                    }
                }
            }
            result.push(CheckedResidualApplication {
                fingerprint: CodeFingerprint::of_parts(
                    "etas.frontend.residual.application.v1",
                    parts.iter().map(Vec::as_slice),
                ),
                arguments: arguments.to_vec(),
                type_bindings,
                effect_row_bindings,
                bindings,
                type_descriptors,
                actions,
            });
        }
        Ok(result)
    }
}
