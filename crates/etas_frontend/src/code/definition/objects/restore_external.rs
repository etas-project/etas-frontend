use std::sync::Arc;

use etas_code::{
    definition::{BaseRevisionInput, DefinitionAnalysisState, DefinitionRef, DefinitionSnapshot},
    descriptor::DescriptorLimits,
};
use etas_types::{
    CallableGenericArg, CallableGenericParamKind, TypeInterner, TypeOutput, TypeSubstitutionEngine,
};
use etas_utils::PipelineExecutionControl;

use super::{
    DefinitionEnvironment, DefinitionProvenance, NativeDefinition, NativeDefinitionError,
    NativeDefinitionStore,
};
use crate::{
    code::{
        definition::{DefinitionCatalog, NativeTypeCorrespondence},
        descriptor::ProjectionError,
    },
    project::instantiation::{
        self, InstantiationError, NativeGenericArgument, NativeInstantiation,
    },
};

impl NativeDefinitionStore {
    /// Rebind a metadata-backed draft only after independently checking the
    /// producer and proving its original frozen snapshot and type arguments.
    pub fn restore_external_application_controlled(
        &self,
        definition: &DefinitionRef,
        producer: &NativeDefinitionStore,
        limits: DescriptorLimits,
        control: &dyn PipelineExecutionControl,
    ) -> Result<DefinitionRef, NativeDefinitionError> {
        let invalid = || NativeDefinitionError::CandidateInputMismatch;
        checkpoint(control)?;
        let original = self.resolve(definition)?;
        let evidence = original
            .0
            .external_application
            .as_ref()
            .ok_or_else(invalid)?;
        let application = &evidence.application;
        if !Arc::ptr_eq(evidence.catalog.checked_program(), application.program()) {
            return Err(invalid());
        }
        let external = self
            .catalog
            .external_callables
            .get(application.callable())
            .ok_or_else(invalid)?;
        let portable = external.snapshot.as_ref().ok_or_else(invalid)?;
        let owner = producer
            .catalog
            .item_owner(application.callable())
            .ok_or_else(invalid)?;
        let header = &producer
            .catalog
            .callables
            .get(application.callable())
            .ok_or_else(invalid)?
            .header;
        let template = producer
            .catalog
            .freeze_flow_view(header, owner)
            .map_err(NativeDefinitionError::Projection)?;
        let revision = producer
            .catalog
            .source_revision(&template, &self.syntax)
            .map_err(NativeDefinitionError::Projection)?;
        if revision != portable.revision
            || template
                .dependency_info()
                .map_err(NativeDefinitionError::Projection)?
                != portable.dependencies
            || template.generic_environment_context() != portable.generic_environment_context
        {
            return Err(invalid());
        }
        checkpoint(control)?;
        let producer_program = producer.catalog.checked_program();
        let signature =
            instantiation::signature(&producer_program.types, owner).ok_or_else(invalid)?;
        if signature.generic_params.len() != application.arguments().len() {
            return Err(invalid());
        }
        let correspondence = NativeTypeCorrespondence::build(&evidence.catalog, &producer.catalog)
            .map_err(NativeDefinitionError::Projection)?;
        let interner = TypeInterner::from_store((*producer_program.type_store).clone());
        let budget = etas_types::TypeSubstitutionBudget::new(
            limits.max_nodes,
            limits.max_edges,
            Some(control),
        );
        if application.arguments().len() > limits.max_nodes {
            return Err(NativeDefinitionError::Projection(ProjectionError::Limit {
                resource: "generic application arguments",
                limit: limits.max_nodes,
            }));
        }
        let mut type_roots = Vec::new();
        let mut row_roots = Vec::new();
        for argument in application.arguments() {
            checkpoint(control)?;
            match argument {
                CallableGenericArg::Type(ty) => type_roots.push(*ty),
                CallableGenericArg::EffectRow(row) => row_roots.push(row),
            }
        }
        let (reachable, spec_plan) = crate::code::definition::checked_type_and_spec_plan(
            crate::code::definition::CheckedTypeAndSpecPlanInput {
                source: application.program(),
                target: &producer_program.std_registry,
                roots: &type_roots,
                rows: &row_roots,
                declarations: correspondence.declarations(),
                identity: &|spec| evidence.catalog.native_spec_identity(spec).cloned(),
                max_depth: limits.max_expansion_depth,
                control,
                budget: &budget,
            },
        )
        .map_err(NativeDefinitionError::Projection)?;
        let mut identities = std::collections::HashMap::new();
        for ty in &reachable {
            if matches!(
                application.program().type_store.get(*ty),
                Some(etas_types::Type::Nominal(_) | etas_types::Type::Enum(_))
            ) {
                let identity = evidence.catalog.native_type_declaration(*ty).ok_or(
                    NativeDefinitionError::TypeRelocation(
                        etas_types::TypeSubstitutionError::MissingTypeIdentity(*ty),
                    ),
                )?;
                identities.insert(*ty, identity.clone());
            }
        }
        let foreign = etas_types::ty::relocate_foreign_type_graph(
            interner,
            etas_types::ty::ForeignTypeGraphInput {
                source: &application.program().type_store,
                facts: &application.program().types,
                target_facts: &producer_program.types,
                reachable: &reachable,
                identities: &identities,
                declarations: correspondence.declarations(),
                effect_names: correspondence.effect_names(),
                specs: &spec_plan.mapping,
                max_depth: limits.max_expansion_depth,
                budget: &budget,
            },
        )
        .map_err(super::external_application::substitution_error)?;
        let mut interner = foreign.interner;
        let declarations = foreign.declarations;
        let mut foreign_type_bindings = producer_program
            .foreign_type_bindings
            .extend_checked(
                &evidence.catalog,
                &identities,
                &declarations,
                interner.store(),
                control,
            )
            .map_err(NativeDefinitionError::Projection)?;
        foreign_type_bindings
            .extend_specs_checked(&evidence.catalog, &spec_plan.mapping, control)
            .map_err(NativeDefinitionError::Projection)?;
        let mut relocation = TypeSubstitutionEngine::relocating_materialized(
            etas_types::ty::TypeRelocationMaterializationInput {
                source: &application.program().type_store,
                target: &mut interner,
                declaration_types: &declarations,
                effect_names: correspondence.effect_names(),
            },
        )
        .with_expansion_depth(limits.max_expansion_depth)
        .with_budget(&budget);
        let mut types = Vec::new();
        let mut rows = Vec::new();
        let mut arguments = Vec::new();
        for (parameter, argument) in signature.generic_params.iter().zip(application.arguments()) {
            match (parameter.kind, argument) {
                (CallableGenericParamKind::Type, CallableGenericArg::Type(ty)) => {
                    let ty = relocation
                        .substitute(*ty)
                        .map_err(super::external_application::substitution_error)?;
                    types.push((parameter.name.clone(), ty));
                    arguments.push(NativeGenericArgument::Type(ty));
                }
                (CallableGenericParamKind::Effect, CallableGenericArg::EffectRow(row)) => {
                    let row = relocation
                        .substitute_effect_row(row.clone())
                        .map_err(super::external_application::substitution_error)?;
                    rows.push((parameter.name.clone(), row.clone()));
                    arguments.push(NativeGenericArgument::EffectRow(row));
                }
                _ => return Err(invalid()),
            }
        }
        drop(relocation);
        let spec_facts = spec_plan
            .materialize(crate::code::definition::SpecMaterializationInput {
                source: &application.program().type_store,
                target: &mut interner,
                declarations: &declarations,
                effect_names: correspondence.effect_names(),
                reachable: &reachable,
                max_depth: limits.max_expansion_depth,
                budget: &budget,
            })
            .map_err(super::external_application::substitution_error)?;
        for identity in spec_facts.canonical.signatures.keys() {
            if !foreign_type_bindings.verifies_spec(identity) {
                return Err(NativeDefinitionError::Projection(
                    ProjectionError::MissingIdentity("canonical spec origin proof".into()),
                ));
            }
        }
        let (target_facts, reflection_targets) = producer_program
            .reflection_targets
            .extend_checked_types(
                &producer_program.types,
                interner.store(),
                foreign.type_generics,
                foreign.enum_layouts,
                spec_facts,
                Some(control),
            )
            .map_err(|error| match error {
                crate::code::reflection::ReflectionTargetError::Invalid(reason) => {
                    NativeDefinitionError::Projection(ProjectionError::Invalid(reason.into()))
                }
                crate::code::reflection::ReflectionTargetError::Interrupted(error) => {
                    interrupted(error)
                }
            })?;
        let types_output = TypeOutput {
            facts: target_facts.clone(),
            store: producer_program.type_store.clone(),
            diagnostics: Vec::new(),
        };
        let mut seeds = producer_program.native_instantiations.instances().to_vec();
        seeds.push(NativeInstantiation { owner, arguments });
        let prepared = instantiation::prepare(
            instantiation::PreparationInput {
                hir: &producer_program.hir,
                types: &types_output,
                effects: &producer_program.effects,
                limits,
                control: Some(control),
            },
            &seeds,
            Some(interner),
        )
        .map_err(|error| match error {
            InstantiationError::Interrupted(error) => interrupted(error),
            InstantiationError::Resource { .. } => {
                NativeDefinitionError::Projection(ProjectionError::Limit {
                    resource: "native generic instantiation",
                    limit: limits.max_nodes,
                })
            }
            InstantiationError::Invalid(reason) => {
                NativeDefinitionError::Projection(ProjectionError::Invalid(reason))
            }
        })?;
        let mut rebound = (**producer_program).clone();
        rebound.types = target_facts;
        rebound.reflection_targets = reflection_targets;
        rebound.foreign_type_bindings = Arc::new(foreign_type_bindings);
        rebound.type_store = prepared.store;
        rebound.native_instantiations = prepared.plan;
        let rebound = Arc::new(rebound);
        let catalog = Arc::new(
            DefinitionCatalog::build(rebound.clone(), &producer.catalog.origins, limits)
                .map_err(NativeDefinitionError::Projection)?,
        );
        let derived = Arc::new(self.with_catalog(catalog));
        let checked = rebound
            .checked_callable_application(owner, &types, &rows)
            .map_err(|_| invalid())?;
        let native_base = derived.reflect_checked_application(&checked, None, limits)?;
        let expected_base = self.reflect_external_application_from_catalog(
            &evidence.catalog,
            application.clone(),
            original.0.reflection_access,
            limits,
        )?;
        let native = derived.resolve(&native_base)?;
        let expected = self.resolve(&expected_base)?;
        if native.revision() != expected.revision()
            || native
                .dependency_info()
                .map_err(NativeDefinitionError::Projection)?
                != expected
                    .dependency_info()
                    .map_err(NativeDefinitionError::Projection)?
        {
            return Err(invalid());
        }
        let body = original
            .body()
            .cloned()
            .ok_or(NativeDefinitionError::MissingBodySource)?;
        let inputs = original.revision().inputs();
        let data = DefinitionSnapshot::from_base(
            native.0.data.shared_base(),
            BaseRevisionInput {
                body: Some(body),
                content: inputs.content,
                parent: inputs.parent,
                analysis_configuration: native.0.analysis_configuration,
                summaries: None,
            },
        )
        .map_err(NativeDefinitionError::Definition)?;
        if data.revision() != original.revision() {
            return Err(invalid());
        }
        checkpoint(control)?;
        let draft = self.publish(NativeDefinition {
            data,
            environment: DefinitionEnvironment::Native(native.environment()?.clone()),
            auxiliary: DefinitionAnalysisState::Unavailable {
                reason: etas_code::definition::AnalysisUnavailableReason::NotAnalyzed,
            },
            method_snapshots: Default::default(),
            analysis_configuration: native.0.analysis_configuration,
            analysis_graph: native.0.analysis_graph.clone(),
            reflection_body_access: original.0.reflection_body_access,
            reflection_access: None,
            provenance: DefinitionProvenance::Draft,
            implementation: None,
            external_application: original.0.external_application.clone(),
            external_restoration: Some(Arc::new(super::ExternalRestorationReceipt {
                metadata_base: original.0.data.shared_base(),
                native_environment: native.environment()?.clone(),
            })),
        })?;
        Ok(draft)
    }
}

fn checkpoint(control: &dyn PipelineExecutionControl) -> Result<(), NativeDefinitionError> {
    control.checkpoint().map_err(interrupted)
}

fn interrupted(error: etas_utils::PipelineInterruption) -> NativeDefinitionError {
    NativeDefinitionError::Projection(ProjectionError::Syntax(
        etas_code::syntax::SyntaxOperationError::Interrupted(error),
    ))
}
