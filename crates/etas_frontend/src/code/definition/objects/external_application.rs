use std::{collections::HashMap, sync::Arc};

use etas_code::{CallableId, DefinitionKind};
use etas_hir::{HirExpr, HirExprId, HirReflectTarget, ResolveResult};
use etas_types::{
    CallableGenericArg, CallableGenericParamKind, CallableSignature, EffectRowRef, SymbolTypeFact,
    Type, TypeId, TypeSubstitutionEngine,
};

use super::{NativeDefinitionError, NativeDefinitionStore};
use crate::{CheckedCallableApplication, CheckedProgram, code::definition::StaticReflectionView};

pub(super) fn substitution_error(
    error: etas_types::TypeSubstitutionError,
) -> NativeDefinitionError {
    use crate::code::descriptor::ProjectionError;
    use etas_types::TypeSubstitutionError;
    if let TypeSubstitutionError::Interrupted(interruption) = error {
        return NativeDefinitionError::Projection(ProjectionError::Syntax(
            etas_code::syntax::SyntaxOperationError::Interrupted(interruption),
        ));
    }
    let limit = match error {
        TypeSubstitutionError::WorkLimit { resource, limit } => Some((resource, limit)),
        TypeSubstitutionError::ExpansionDepth(limit) => Some(("type expansion depth", limit)),
        TypeSubstitutionError::RepresentationLimit(limit) => Some(("type representation", limit)),
        _ => None,
    };
    match limit {
        Some((resource, limit)) => {
            NativeDefinitionError::Projection(ProjectionError::Limit { resource, limit })
        }
        None => NativeDefinitionError::TypeRelocation(error),
    }
}

/// A checked application of a metadata-backed declaration. Its native type
/// IDs remain scoped to `program`; the stable callable identity comes from the
/// validated external descriptor, never from the spelling of an import.
#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct CheckedExternalCallableApplication {
    program: CheckedProgram,
    expression: HirExprId,
    callable: CallableId,
    package: Option<crate::ExternalPackageId>,
    arguments: Vec<CallableGenericArg>,
    signature: CallableSignature,
}

impl CheckedExternalCallableApplication {
    pub fn program(&self) -> &CheckedProgram {
        &self.program
    }
    pub fn expression(&self) -> HirExprId {
        self.expression
    }
    pub fn callable(&self) -> &CallableId {
        &self.callable
    }
    pub fn package(&self) -> Option<crate::ExternalPackageId> {
        self.package
    }
    pub fn arguments(&self) -> &[CallableGenericArg] {
        &self.arguments
    }
    pub fn signature(&self) -> &CallableSignature {
        &self.signature
    }
}

impl NativeDefinitionStore {
    pub fn checked_external_callable_application(
        &self,
        expression: HirExprId,
        lexical: Option<&CheckedCallableApplication>,
        limits: etas_code::descriptor::DescriptorLimits,
    ) -> Result<CheckedExternalCallableApplication, NativeDefinitionError> {
        let invalid = || NativeDefinitionError::CandidateInputMismatch;
        let program = self.catalog.checked_program();
        let fact = program
            .checked_reflections()
            .get(&expression)
            .ok_or_else(invalid)?;
        if !matches!(
            fact.target,
            etas_types::CheckedReflectionTarget::Callable { .. }
                | etas_types::CheckedReflectionTarget::ExternalCallable { .. }
        ) {
            return Err(invalid());
        }
        let Some(HirExpr::Reflect(reflection)) = program.hir.exprs.get(expression) else {
            return Err(invalid());
        };
        let HirReflectTarget::Path(path) = &reflection.target else {
            return Err(invalid());
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return Err(invalid());
        };
        let Some(SymbolTypeFact::Flow {
            signature: template,
        }) = program.types.symbol_types.get(&symbol)
        else {
            return Err(invalid());
        };
        let StaticReflectionView::Callable(header) = self
            .catalog
            .static_reflection(expression)
            .map_err(NativeDefinitionError::Projection)?
        else {
            return Err(invalid());
        };
        let external = self
            .catalog
            .external_callables
            .get(&header.id)
            .ok_or_else(invalid)?;
        if header.id != external.canonical
            || header
                .id
                .as_declaration()
                .is_none_or(|key| key.kind() != DefinitionKind::Flow)
        {
            return Err(invalid());
        }
        let instantiation = program
            .types
            .generic_instantiations
            .get(&expression)
            .ok_or_else(invalid)?;
        let package = match program.reflection_resolution(expression) {
            Some(crate::HirPathResolution::ExplicitImport { target })
            | Some(crate::HirPathResolution::PartialImport { target, .. })
            | Some(crate::HirPathResolution::WildcardImport { item: target, .. }) => match target {
                crate::ImportTarget::ExternalItem { package, .. } => *package,
                crate::ImportTarget::Module(crate::ResolvedModuleTarget::External {
                    package,
                    ..
                }) => *package,
                _ => None,
            },
            _ => None,
        };
        if !instantiation.deferred_effect_row_obligations.is_empty() {
            return Err(invalid());
        }
        let budget =
            etas_types::TypeSubstitutionBudget::new(limits.max_nodes, limits.max_edges, None);
        let lexical = self.lexical_bindings(expression, lexical)?;
        let mut enclosing = TypeSubstitutionEngine::existing(&program.type_store, &lexical.types)
            .with_source_parameters(&lexical.source_parameters)
            .with_effect_arguments(&lexical.rows)
            .with_expansion_depth(limits.max_expansion_depth)
            .with_budget(&budget);
        let descriptor = enclosing
            .substitute(fact.descriptor_type)
            .map_err(substitution_error)?;
        let Some(Type::Applied { constructor, args }) = program.type_store.get(descriptor) else {
            return Err(invalid());
        };
        if Some(TypeId(constructor.0)) != program.types.known_std_types.code.callable_definition {
            return Err(invalid());
        }
        let [signature] = args.as_slice() else {
            return Err(invalid());
        };
        let Some(Type::Function(expected)) = program.type_store.get(*signature) else {
            return Err(invalid());
        };

        let mut types = HashMap::new();
        let mut rows = HashMap::new();
        for (name, ty) in &instantiation.type_bindings {
            let ty = enclosing.substitute(*ty).map_err(substitution_error)?;
            if types.insert(name.clone(), ty).is_some() {
                return Err(invalid());
            }
        }
        for (name, row) in &instantiation.effect_row_bindings {
            let row = enclosing
                .substitute_effect_row(row.clone())
                .map_err(substitution_error)?;
            if rows.insert(name.clone(), row).is_some() {
                return Err(invalid());
            }
        }
        if types.len() + rows.len() != template.generic_params.len() {
            return Err(invalid());
        }
        let empty = HashMap::new();
        let validator = TypeSubstitutionEngine::existing(&program.type_store, &empty)
            .with_expansion_depth(limits.max_expansion_depth)
            .with_budget(&budget);
        let mut arguments = Vec::with_capacity(template.generic_params.len());
        for param in &template.generic_params {
            match param.kind {
                CallableGenericParamKind::Type => {
                    let ty = *types.get(&param.name).ok_or_else(invalid)?;
                    validator
                        .validate_closed_argument(ty)
                        .map_err(substitution_error)?;
                    arguments.push(CallableGenericArg::Type(ty));
                }
                CallableGenericParamKind::Effect => {
                    let row = rows.get(&param.name).ok_or_else(invalid)?;
                    validator
                        .validate_closed_effect_argument(row)
                        .map_err(substitution_error)?;
                    arguments.push(CallableGenericArg::EffectRow(
                        crate::project::instantiation::normalized_row(row),
                    ));
                }
            }
        }
        let mut substitution = TypeSubstitutionEngine::existing(&program.type_store, &types)
            .with_effect_arguments(&rows)
            .with_expansion_depth(limits.max_expansion_depth)
            .with_budget(&budget);
        let params = template
            .params
            .iter()
            .map(|ty| substitution.substitute(*ty))
            .collect::<Result<Vec<_>, _>>()
            .map_err(substitution_error)?;
        let output = substitution
            .substitute(template.output)
            .map_err(substitution_error)?;
        let effects: Option<EffectRowRef> = template
            .effects
            .clone()
            .map(|row| substitution.substitute_effect_row(row))
            .transpose()
            .map_err(substitution_error)?;
        let requested_actions = template
            .requested_actions
            .clone()
            .map(|row| substitution.substitute_effect_row(row))
            .transpose()
            .map_err(substitution_error)?;
        if params != expected.input
            || output != expected.output
            || effects
                .as_ref()
                .map(crate::project::instantiation::normalized_row)
                != expected
                    .effects
                    .as_ref()
                    .map(crate::project::instantiation::normalized_row)
        {
            return Err(invalid());
        }
        Ok(CheckedExternalCallableApplication {
            program: Arc::clone(program),
            expression,
            callable: header.id.clone(),
            package,
            arguments,
            signature: CallableSignature {
                generic_params: Vec::new(),
                params,
                output,
                effects,
                requested_actions,
            },
        })
    }
}

#[cfg(test)]
mod relocation_error_tests {
    use super::*;

    #[test]
    fn missing_checked_type_identity_is_not_a_candidate_mismatch() {
        for error in [
            etas_types::TypeSubstitutionError::MissingTypeIdentity(etas_types::TypeId(17)),
            etas_types::TypeSubstitutionError::MissingEnumLayout(etas_types::TypeId(18)),
            etas_types::TypeSubstitutionError::MissingEffectIdentity("scope".into()),
        ] {
            let actual = substitution_error(error.clone());
            assert!(
                matches!(actual, NativeDefinitionError::TypeRelocation(found) if found == error)
            );
        }
    }
}
