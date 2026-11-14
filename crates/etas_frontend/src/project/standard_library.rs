use std::sync::Arc;

use etas_hir::{HirExpr, ResolveResult, SymbolDef};
use etas_types::{
    CallableGenericParamKind, TypeInterner, TypeOutput, TypeSubstitutionEngine,
    ty::TypeRelocationMaterializationInput,
};

use super::instantiation::{self, InstantiationError, NativeGenericArgument, NativeInstantiation};
use crate::{
    CheckedProgramData, StandardLibrary,
    code::{
        definition::{DefinitionCatalog, NativeTypeCorrespondence},
        identity::{DefinitionOrigins, standard_package_key},
    },
};

impl StandardLibrary {
    /// Prepare the consumer's checked generic applications against the archived
    /// body graph. This materializes types and invocation proofs; it never
    /// lowers source or checks the library bodies again.
    pub(crate) fn for_consumer(
        self: &Arc<Self>,
        consumer: &CheckedProgramData,
    ) -> Result<Arc<Self>, String> {
        let mut calls = Vec::new();
        for (site, application) in &consumer.types.generic_instantiations {
            let value = match consumer.hir.exprs.get(*site) {
                Some(HirExpr::Call { callee, .. }) => *callee,
                Some(HirExpr::Path(_)) => *site,
                _ => continue,
            };
            let Some(HirExpr::Path(path)) = consumer.hir.exprs.get(value) else {
                continue;
            };
            let ResolveResult::Resolved(symbol) = path.resolution else {
                continue;
            };
            let Some(SymbolDef::ImportAlias { path, .. }) =
                consumer.symbols.get(symbol).map(|symbol| &symbol.def)
            else {
                continue;
            };
            let Some(binding) = consumer
                .std_registry
                .lookup_qualified(path)
                .and_then(|symbol| consumer.std_registry.body_binding(symbol.id))
            else {
                continue;
            };
            calls.push((*site, binding.callable.clone(), application));
        }
        if calls.is_empty() {
            return Ok(self.clone());
        }
        calls.sort_by_key(|(site, _, _)| *site);
        let source = catalog(Arc::new(consumer.clone()))?;
        let target = catalog(self.program.clone())?;
        let correspondence =
            NativeTypeCorrespondence::build(&source, &target).map_err(|e| e.to_string())?;
        let mut interner = TypeInterner::from_store(self.program.type_store.as_ref().clone());
        let mut seeds = self.program.native_instantiations.instances().to_vec();
        {
            let mut relocation = TypeSubstitutionEngine::relocating_materialized(
                TypeRelocationMaterializationInput {
                    source: &consumer.type_store,
                    target: &mut interner,
                    declaration_types: correspondence.declarations(),
                    effect_names: correspondence.effect_names(),
                },
            );
            for (_, callable, application) in calls {
                let owner = target
                    .item_owner(&callable)
                    .ok_or("std generic callable has no checked body")?;
                let signature = instantiation::signature(&self.program.types, owner)
                    .ok_or("std generic body has no checked signature")?;
                let arguments = signature
                    .generic_params
                    .iter()
                    .map(|parameter| {
                        Ok(match parameter.kind {
                            CallableGenericParamKind::Type => NativeGenericArgument::Type(
                                relocation
                                    .substitute(
                                        application
                                            .type_bindings
                                            .iter()
                                            .find(|(name, _)| name == &parameter.name)
                                            .ok_or("std generic call has no checked type argument")?
                                            .1,
                                    )
                                    .map_err(|e| e.to_string())?,
                            ),
                            CallableGenericParamKind::Effect => NativeGenericArgument::EffectRow(
                                relocation
                                    .substitute_effect_row(
                                        application
                                            .effect_row_bindings
                                            .iter()
                                            .find(|(name, _)| name == &parameter.name)
                                            .ok_or(
                                                "std generic call has no checked effect argument",
                                            )?
                                            .1
                                            .clone(),
                                    )
                                    .map_err(|e| e.to_string())?,
                            ),
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                seeds.push(NativeInstantiation {
                    owner,
                    arguments,
                    owner_bindings: vec![],
                });
            }
        }
        let types = TypeOutput {
            store: self.program.type_store.clone(),
            facts: self.program.types.clone(),
            diagnostics: vec![],
        };
        let prepared = instantiation::prepare(
            instantiation::PreparationInput {
                hir: &self.program.hir,
                types: &types,
                effects: &self.program.effects,
                limits: Default::default(),
                control: None,
            },
            &seeds,
            Some(interner),
        )
        .map_err(|error| match error {
            InstantiationError::Invalid(reason) | InstantiationError::Resource { reason, .. } => {
                reason
            }
            InstantiationError::Interrupted(error) => error.reason,
        })?;
        let mut program = self.program.as_ref().clone();
        program.type_store = prepared.store;
        program.native_instantiations = prepared.plan;
        let mut linked = self.as_ref().clone();
        linked.program = Arc::new(program);
        Ok(Arc::new(linked))
    }
}

fn catalog(program: crate::CheckedProgram) -> Result<DefinitionCatalog, String> {
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::checked_source_modules(&program)
            .map_err(|e| e.to_string())?,
        standard_package: standard_package_key(&program.std_registry).map_err(|e| e.to_string())?,
        external_definitions: DefinitionOrigins::checked_external_definitions(&program)
            .map_err(|e| e.to_string())?,
    };
    DefinitionCatalog::build(program, &origins, Default::default()).map_err(|e| e.to_string())
}
