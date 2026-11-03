use std::collections::{BTreeMap, HashMap};

use etas_code::{CodeFingerprint, DefinitionKind, PackageKey, descriptor::*};
use etas_frontend::{
    CheckedProgram, Frontend, ProjectInput, SourceInput,
    code::{
        descriptor::{DefinitionDescriptorProjector, ProjectionError},
        identity::{DefinitionIdentityIndex, DefinitionOrigins},
    },
};
use etas_hir::{HirItem, HirItemId};
use etas_types::ItemSignature;

fn checked(source: &str) -> CheckedProgram {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = etas_frontend::EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.unwrap()
}

fn checked_with_stable_source(source: &str) -> CheckedProgram {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = etas_frontend::EntryPolicy::Optional;
    let package = PackageKey::from_name_and_version("app", "1.0.0").unwrap();
    let key = etas_code::StableSourceKey::new(
        etas_code::StableSourceOrigin::Package(package),
        &["src".into(), "app.es".into()],
    )
    .unwrap();
    input
        .environment
        .stable_source_keys
        .push(etas_frontend::ProjectStableSourceKeyInput {
            source: input.sources[0].id,
            key,
        });
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.unwrap()
}

#[test]
fn analyzed_trace_projection_rejects_changed_imported_spec_snapshot() {
    use etas_code::residual::{
        identity::ObligationIdentityLimits,
        monitor::{AlternativeLimits, ConcreteEvidenceLimits},
        pattern::ActionPatternLimits,
    };
    use etas_frontend::code::descriptor::{
        TraceSourceLinkError, TraceSourceLinkLimits, TraceTemplateProjectionError,
    };
    let app = SourceInput::anonymous(
        "module app; import lib.safe.Safe as S; flow main() -> unit ![] ~ S { return; }",
    );
    let mut lib =
        SourceInput::anonymous("module lib.safe; public spec Safe: trace = -Console.stdout_write;");
    lib.id = etas_core::SourceId(1);
    let mut input = ProjectInput::single_source(app);
    input.options.entry_policy = etas_frontend::EntryPolicy::Optional;
    input.sources.push(lib);
    let package = PackageKey::from_name_and_version("app", "1.0.0").unwrap();
    for (source, file) in [
        (etas_core::SourceId(0), "app.es"),
        (etas_core::SourceId(1), "safe.es"),
    ] {
        input
            .environment
            .stable_source_keys
            .push(etas_frontend::ProjectStableSourceKeyInput {
                source,
                key: etas_code::StableSourceKey::new(
                    etas_code::StableSourceOrigin::Package(package),
                    &["src".into(), file.into()],
                )
                .unwrap(),
            });
    }
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let snapshots = checked
        .sources
        .sources
        .iter()
        .map(|file| etas_effects::EffectSourceSnapshot {
            source: file.id,
            key: checked.sources.stable_source_key(file.id).unwrap(),
            text: &file.text,
        })
        .collect::<Vec<_>>();
    let types = etas_types::TypeOutput {
        facts: checked.types.clone(),
        store: checked.type_store.clone(),
        diagnostics: vec![],
    };
    let mut effects = etas_effects::EffectOutput::default();
    let models = etas_effects::trace_spec::materialize_trace_spec_models(
        &checked.hir,
        &types,
        &checked.effect_registry,
        &[],
        &snapshots,
        &mut effects,
    )
    .unwrap();
    let trace_specs = std::mem::take(&mut effects.facts.trace_specs);
    effects.facts = checked.effects.as_ref().clone();
    effects.facts.trace_specs = trace_specs;
    let units = checked
        .effects
        .unit_effects
        .keys()
        .copied()
        .collect::<Vec<_>>();
    let analysis = etas_effects::trace_spec::analyze_trace_spec_monitors(
        etas_effects::trace_spec::TraceSpecAnalysisInput {
            source_snapshots: &snapshots,
            external_trace_specs: &[],
            hir: &checked.hir,
            types: &types,
            registry: &checked.effect_registry,
            models: &models,
            units: &units,
            external_summaries: &[],
        },
        &mut effects,
    )
    .unwrap();
    let item = flow(&checked, "main");
    let index = models.referenced_fact_indices[&item][0];
    let dispositions =
        etas_effects::trace_spec::collect_trace_static_dispositions(&analysis, None, None).unwrap();
    let limits = AlternativeLimits {
        evidence: ConcreteEvidenceLimits {
            pattern: ActionPatternLimits {
                max_selectors: 4,
                max_identity_segments: 16,
                max_identity_bytes: 128,
                max_literal_bytes: 128,
            },
            max_bindings: 4,
            max_value_bytes: 128,
        },
        max_alternatives: 4,
        max_clauses: 12,
        max_patterns: 4,
        max_checks_per_event: 32,
        max_product_work_per_event: 128,
    };
    let input = etas_effects::trace_spec::AnalyzedTraceLoweringInput {
        analysis: &analysis,
        dispositions: &dispositions,
        type_fact_index: index,
        item,
        limits,
        control: None,
    };
    let source_limits = TraceSourceLinkLimits {
        max_sources: 8,
        max_sites: 16,
        max_clauses: 16,
        max_snapshot_bytes: 4096,
        identity: ObligationIdentityLimits {
            max_generic_arguments: 16,
            max_structural_site_depth: 64,
        },
    };
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "locked:app@1")).unwrap();
    let HirItem::Flow(decl) = &checked.hir.items[item] else {
        panic!("flow")
    };
    let owner = GenericOwner::Declaration(identities.symbol(decl.symbol).unwrap().clone());
    let project = |program: &etas_frontend::CheckedProgramData, limits| {
        let mut projector =
            DefinitionDescriptorProjector::new(program, &identities, DescriptorLimits::default());
        project_flow(&mut projector, &checked, &identities, item);
        projector.finish_analyzed_trace_template(&owner, input, limits)
    };
    let template_limits = etas_code::residual::pattern::PatternTemplateLimits {
        pattern: limits.evidence.pattern,
        types: etas_code::residual::type_template::TypeTemplateLimits::default(),
        descriptor: DescriptorLimits::default(),
        max_binding_steps: 128,
    };
    let project_symbolic = |program: &etas_frontend::CheckedProgramData, limits| {
        let mut projector =
            DefinitionDescriptorProjector::new(program, &identities, DescriptorLimits::default());
        project_flow(&mut projector, &checked, &identities, item);
        projector.finish_analyzed_symbolic_trace_template(&owner, input, limits, template_limits)
    };
    let concrete = project(&checked, source_limits).unwrap();
    let symbolic = project_symbolic(&checked, source_limits).unwrap();
    assert_eq!(
        symbolic
            .template
            .instantiate(&BTreeMap::new(), &BTreeMap::new(), template_limits, limits)
            .unwrap()
            .template_id(),
        concrete.monitor().template_id()
    );
    let mut altered = (*checked).clone();
    altered
        .sources
        .sources
        .iter_mut()
        .find(|file| file.id == etas_core::SourceId(1))
        .unwrap()
        .text =
        std::sync::Arc::from("module lib.safe; public spec Safe: trace = +Console.stdout_write;");
    assert!(matches!(
        project(&altered, source_limits),
        Err(TraceTemplateProjectionError::SourceLink(
            TraceSourceLinkError::MismatchedClauseSite(_)
        ))
    ));
    assert!(matches!(
        project_symbolic(&altered, source_limits),
        Err(TraceTemplateProjectionError::SourceLink(
            TraceSourceLinkError::MismatchedClauseSite(_)
        ))
    ));
    let mut altered_facts = (*checked).clone();
    std::sync::Arc::make_mut(&mut altered_facts.effects)
        .trace_specs
        .conformances
        .get_mut(&item)
        .unwrap()[0]
        .alternatives
        .clear();
    assert!(matches!(
        project(&altered_facts, source_limits),
        Err(TraceTemplateProjectionError::AnalyzedSourceMismatch)
    ));
    assert!(matches!(
        project_symbolic(&altered_facts, source_limits),
        Err(TraceTemplateProjectionError::AnalyzedSourceMismatch)
    ));
    let max_bytes = checked
        .sources
        .sources
        .iter()
        .map(|s| s.text.len())
        .sum::<usize>()
        - 1;
    assert!(matches!(
        project(
            &checked,
            TraceSourceLinkLimits {
                max_snapshot_bytes: max_bytes,
                ..source_limits
            }
        ),
        Err(TraceTemplateProjectionError::SourceLink(
            TraceSourceLinkError::SnapshotBytesLimit
        ))
    ));
    assert!(matches!(
        project_symbolic(
            &checked,
            TraceSourceLinkLimits {
                max_snapshot_bytes: max_bytes,
                ..source_limits
            }
        ),
        Err(TraceTemplateProjectionError::SourceLink(
            TraceSourceLinkError::SnapshotBytesLimit
        ))
    ));
}

fn origins(checked: &CheckedProgram, source: &str) -> DefinitionOrigins {
    let key = |value: &str| {
        PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
            "etas.test.package-origin.v1",
            value.as_bytes(),
        ))
    };
    DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(checked, key(source)).unwrap(),
        standard_package: key("locked:etas.std:phase1"),
        external_definitions: BTreeMap::new(),
    }
}

fn flow(checked: &CheckedProgram, name: &str) -> HirItemId {
    checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow) if checked.hir.symbols.get(flow.symbol).unwrap().name == name => {
                Some(id)
            }
            _ => None,
        })
        .unwrap()
}

fn project_flow(
    projector: &mut DefinitionDescriptorProjector<'_>,
    checked: &CheckedProgram,
    identities: &DefinitionIdentityIndex,
    item: HirItemId,
) -> TypeInfoRef {
    let HirItem::Flow(decl) = &checked.hir.items[item] else {
        panic!("flow")
    };
    let ItemSignature::Flow(signature) = &checked.types.item_signatures[&item] else {
        panic!("signature")
    };
    let names = decl
        .params
        .iter()
        .map(|symbol| Some(checked.hir.symbols.get(*symbol).unwrap().name.clone()))
        .collect::<Vec<_>>();
    projector
        .project_signature(
            GenericOwner::Declaration(identities.symbol(decl.symbol).unwrap().clone()),
            signature,
            &names,
            decl.return_type.map(|ty| checked.types.type_refs[&ty]),
        )
        .unwrap()
        .ty
}

fn signature(graph: &DescriptorGraph, reference: TypeInfoRef) -> &SignatureInfo {
    let TypeInfo::Function(signature) = graph.type_info(reference).unwrap() else {
        panic!("function")
    };
    signature
}

fn check_trace_plan_source_links(
    checked: &CheckedProgram,
    item: HirItemId,
    projection: &etas_frontend::code::descriptor::TraceTemplateProjection,
    monitor_limits: etas_code::residual::monitor::AlternativeLimits,
) {
    use etas_code::residual::{
        binding::{BindingLayout, BindingLayoutLimits},
        identity::{
            ActivationSiteId, BehaviorContractRef, CheckedSourceSnapshot, GenericArgumentIdentity,
            GenericParameterId, ObligationId, ObligationIdentityInputs, ObligationIdentityLimits,
            ResidualObligationRef, ResidualRuleKind, SemanticScopeId, SourceStructuralSite,
        },
        model::{ResidualKind, ResidualObligation, ResidualObligationSet, ResidualSetLimits},
        monitor::{AlternativeRule, CompiledTraceComponent, TraceComponentLimits},
        plan::{
            SemanticPlanComponent, SemanticPlanLimits, SemanticPlanMapping, SemanticResidualPlan,
        },
    };
    use etas_effects::trace_spec::{
        TraceObligationSourceLimits, TracePlanSourceBinding, TracePlanSourceError,
        TracePlanSourceInput, TracePlanSourceLimits, validate_trace_plan_sources,
    };

    let file = &checked.sources.sources[0];
    let key = checked.sources.stable_source_key(file.id).unwrap();
    let snapshots = [etas_effects::EffectSourceSnapshot {
        source: file.id,
        key,
        text: &file.text,
    }];
    let types = etas_types::TypeOutput {
        facts: checked.types.clone(),
        store: checked.type_store.clone(),
        diagnostics: vec![],
    };
    let pipeline = etas_effects::RunEffectPipeline::run(etas_effects::EffectPipelineInput {
        source_snapshots: &snapshots,
        residual_identity: None,
        trace_monitor_projector: None,
        frozen_calls: None,
        std_registry: &checked.std_registry,
        dependency_metadata: None,
        tool_bindings: &[],
        external_trace_specs: &[],
        hir: &checked.hir,
        types: &types,
        external_summaries: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .unwrap();
    let models = &pipeline.effects.facts.trace_specs;
    let (analysis, dispositions) = pipeline.trace_specs.into_parts();
    let index = types
        .facts
        .trace_spec_conformances
        .iter()
        .position(|fact| fact.item == item)
        .unwrap();
    let identities =
        DefinitionIdentityIndex::build(checked, &origins(checked, "locked:app@1")).unwrap();
    let HirItem::Flow(decl) = &checked.hir.items[item] else {
        panic!("flow")
    };
    let generic_owner = GenericOwner::Declaration(identities.symbol(decl.symbol).unwrap().clone());
    let input = etas_effects::trace_spec::AnalyzedTraceLoweringInput {
        analysis: &analysis,
        dispositions: &dispositions,
        type_fact_index: index,
        item,
        limits: monitor_limits,
        control: None,
    };
    let source_limits = etas_frontend::code::descriptor::TraceSourceLinkLimits {
        max_sources: 8,
        max_sites: 16,
        max_clauses: 16,
        max_snapshot_bytes: 64 * 1024,
        identity: ObligationIdentityLimits {
            max_generic_arguments: 16,
            max_structural_site_depth: 64,
        },
    };
    let mut projector =
        DefinitionDescriptorProjector::new(checked, &identities, DescriptorLimits::default());
    project_flow(&mut projector, checked, &identities, item);
    let analyzed = projector
        .finish_analyzed_trace_template(&generic_owner, input, source_limits)
        .unwrap();
    assert_eq!(
        analyzed.monitor().template_id(),
        projection.monitor().template_id()
    );
    let other = flow(checked, "broad");
    let mut projector =
        DefinitionDescriptorProjector::new(checked, &identities, DescriptorLimits::default());
    project_flow(&mut projector, checked, &identities, item);
    assert!(matches!(
        projector.finish_analyzed_trace_template(
            &generic_owner,
            etas_effects::trace_spec::AnalyzedTraceLoweringInput {
                item: other,
                ..input
            },
            source_limits
        ),
        Err(etas_frontend::code::descriptor::TraceTemplateProjectionError::CallableOwnerMismatch)
    ));
    let HirItem::Flow(other_decl) = &checked.hir.items[other] else {
        panic!("flow")
    };
    let other_owner =
        GenericOwner::Declaration(identities.symbol(other_decl.symbol).unwrap().clone());
    let mut projector =
        DefinitionDescriptorProjector::new(checked, &identities, DescriptorLimits::default());
    project_flow(&mut projector, checked, &identities, item);
    assert!(matches!(
        projector.finish_analyzed_trace_template(&other_owner, input, source_limits),
        Err(etas_frontend::code::descriptor::TraceTemplateProjectionError::CallableOwnerMismatch)
    ));
    let mut altered = (**checked).clone();
    altered.sources.sources[0].text = std::sync::Arc::from("different source revision");
    let mut projector =
        DefinitionDescriptorProjector::new(&altered, &identities, DescriptorLimits::default());
    project_flow(&mut projector, checked, &identities, item);
    assert!(matches!(
        projector.finish_analyzed_trace_template(&generic_owner, input, source_limits),
        Err(etas_frontend::code::descriptor::TraceTemplateProjectionError::AnalyzedSourceMismatch)
    ));
    let mut projector =
        DefinitionDescriptorProjector::new(checked, &identities, DescriptorLimits::default());
    project_flow(&mut projector, checked, &identities, item);
    assert!(matches!(
        projector.finish_analyzed_trace_template(
            &generic_owner,
            input,
            etas_frontend::code::descriptor::TraceSourceLinkLimits {
                max_snapshot_bytes: 0,
                ..source_limits
            }
        ),
        Err(
            etas_frontend::code::descriptor::TraceTemplateProjectionError::SourceLink(
                etas_frontend::code::descriptor::TraceSourceLinkError::SnapshotBytesLimit
            )
        )
    ));
    struct CancelAnalyzed;
    impl etas_utils::PipelineExecutionControl for CancelAnalyzed {
        fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
            Err(etas_utils::PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::Cancelled,
                reason: "test cancellation".into(),
            })
        }
    }
    let mut projector =
        DefinitionDescriptorProjector::new(checked, &identities, DescriptorLimits::default());
    project_flow(&mut projector, checked, &identities, item);
    assert!(matches!(
        projector.finish_analyzed_trace_template(
            &generic_owner,
            etas_effects::trace_spec::AnalyzedTraceLoweringInput {
                control: Some(&CancelAnalyzed),
                ..input
            },
            source_limits
        ),
        Err(
            etas_frontend::code::descriptor::TraceTemplateProjectionError::Lower(
                etas_effects::trace_spec::TraceRulesLoweringError::Interrupted(_)
            )
        )
    ));
    let source = analysis.source_conformance(index).unwrap();
    let owner = source.source_owner.as_ref().unwrap();
    let spec = source.source_instance.as_ref().unwrap();
    let clause_id = projection.alternatives()[0][0].source_clause.unwrap();
    let site = &models.source_clause_sites[&clause_id];
    let identity_limits = ObligationIdentityLimits {
        max_generic_arguments: 16,
        max_structural_site_depth: 64,
    };
    let snapshot = CheckedSourceSnapshot::new(key, file.text.as_bytes());
    let owner_site = SourceStructuralSite {
        location: owner.declaration(),
        source_bytes: file.text.as_bytes(),
        structural_path: &[],
    };
    let scope_limits = etas_effects::trace_spec::TraceObligationSourceLimits {
        max_snapshots: 4,
        max_snapshot_bytes: 64 * 1024,
        identity: identity_limits,
    };
    let scope_input = |snapshots| etas_effects::trace_spec::TraceCallableScopeInput {
        analysis: &analysis,
        dispositions: &dispositions,
        type_fact_index: index,
        item,
        hir: &checked.hir,
        snapshots,
        limits: scope_limits,
        control: None,
    };
    let scope =
        etas_effects::trace_spec::checked_trace_callable_scope(scope_input(&snapshots)).unwrap();
    assert_eq!(scope.owner, owner.id());
    assert_eq!(
        scope.scope,
        SemanticScopeId::from_source_site(owner.id(), &owner_site, identity_limits).unwrap()
    );
    assert_eq!(
        scope.activation,
        ActivationSiteId::from_source_site(owner.id(), &owner_site, identity_limits).unwrap()
    );
    let stale_text = file.text.replace("Gate", "GATE");
    let stale = [etas_effects::EffectSourceSnapshot {
        text: &stale_text,
        ..snapshots[0]
    }];
    assert!(matches!(
        etas_effects::trace_spec::checked_trace_callable_scope(scope_input(&stale)),
        Err(etas_effects::trace_spec::TraceCallableScopeError::OwnerMismatch)
    ));
    let layout_limits = BindingLayoutLimits {
        max_bindings: 4,
        max_projections_per_binding: 4,
        max_total_projections: 4,
    };
    let layout = BindingLayout::new(BTreeMap::new(), layout_limits).unwrap();
    let expected_identity = ObligationIdentityInputs {
        origin: site.location().clone(),
        owner: owner.id(),
        contract: BehaviorContractRef::SourceConstraint,
        spec: spec.id(),
        clause: clause_id,
        rule: site
            .rule(
                &snapshot,
                ResidualRuleKind::TraceContract,
                &[],
                identity_limits,
            )
            .unwrap(),
        scope: scope.scope,
        activation: scope.activation,
        static_bindings: Default::default(),
    };
    let identity = etas_effects::trace_spec::trace_obligation_identity(
        etas_effects::trace_spec::TraceObligationIdentityInput {
            scope: scope_input(&snapshots),
            alternative_index: 0,
            clause_index: 0,
            clause_sites: &models.source_clause_sites,
            contract: BehaviorContractRef::SourceConstraint,
            rule_kind: ResidualRuleKind::TraceContract,
            rule_suffix: &[],
            max_identity_work: 1024,
        },
    )
    .unwrap();
    assert_eq!(identity.inputs(), &expected_identity);
    let frozen_contract = BehaviorContractRef::FrozenInput(
        etas_code::residual::identity::BehaviorContractId::from_canonical_fingerprint(
            CodeFingerprint::from_bytes([94; 32]),
        ),
    );
    let frozen_identity = etas_effects::trace_spec::trace_obligation_identity(
        etas_effects::trace_spec::TraceObligationIdentityInput {
            scope: scope_input(&snapshots),
            alternative_index: 0,
            clause_index: 0,
            clause_sites: &models.source_clause_sites,
            contract: frozen_contract,
            rule_kind: ResidualRuleKind::TraceContract,
            rule_suffix: &[],
            max_identity_work: 1024,
        },
    )
    .unwrap();
    let identity_id = identity.id();
    let obligation = ResidualObligation::new(
        identity.inputs().clone(),
        ResidualKind::TraceContract {
            monitor: projection.monitor().template_id(),
        },
        layout.id(),
        identity_limits,
    )
    .unwrap();
    assert_eq!(obligation.id(), identity_id);
    let mut patterns = BTreeMap::new();
    for clause in projection.alternatives().iter().flatten() {
        match &clause.rule {
            AlternativeRule::Allow(pattern) | AlternativeRule::Deny(pattern) => {
                patterns.insert(pattern.id(), pattern.clone());
            }
            AlternativeRule::Before { guard, target } => {
                patterns.insert(guard.id(), guard.clone());
                patterns.insert(target.id(), target.clone());
            }
        }
    }
    let component = CompiledTraceComponent::compile(
        obligation.clone(),
        projection.monitor().template(),
        &patterns,
        &layout,
        TraceComponentLimits {
            identity: identity_limits,
            layout: layout_limits,
            monitor: monitor_limits,
        },
    )
    .unwrap();
    let component_id = component.id();
    let frozen_obligation = ResidualObligation::new(
        frozen_identity.into_inputs(),
        ResidualKind::TraceContract {
            monitor: projection.monitor().template_id(),
        },
        layout.id(),
        identity_limits,
    )
    .unwrap();
    let frozen_component = CompiledTraceComponent::compile(
        frozen_obligation.clone(),
        projection.monitor().template(),
        &patterns,
        &layout,
        TraceComponentLimits {
            identity: identity_limits,
            layout: layout_limits,
            monitor: monitor_limits,
        },
    )
    .unwrap();
    let frozen_component_id = frozen_component.id();
    let mut frozen_obligations = ResidualObligationSet::new(ResidualSetLimits {
        identity: identity_limits,
        max_obligations: 2,
        max_generic_arguments: 16,
    });
    frozen_obligations
        .insert(frozen_obligation.clone())
        .unwrap();
    let frozen_plan = SemanticResidualPlan::build(
        frozen_obligations,
        vec![SemanticPlanComponent::Trace(frozen_component)],
        vec![layout.clone()],
        vec![SemanticPlanMapping {
            obligation: frozen_obligation.id(),
            component: frozen_component_id,
        }],
        SemanticPlanLimits {
            max_obligations: 2,
            max_components: 2,
            max_layouts: 2,
            max_mappings: 2,
            max_plan_nodes: 16,
            identity: identity_limits,
            layout: layout_limits,
        },
    )
    .unwrap();
    let mut forged_identity = identity.inputs().clone();
    forged_identity.scope =
        SemanticScopeId::from_canonical_fingerprint(CodeFingerprint::from_bytes([93; 32]));
    let forged_obligation = ResidualObligation::new(
        forged_identity,
        ResidualKind::TraceContract {
            monitor: projection.monitor().template_id(),
        },
        layout.id(),
        identity_limits,
    )
    .unwrap();
    let forged_component = CompiledTraceComponent::compile(
        forged_obligation.clone(),
        projection.monitor().template(),
        &patterns,
        &layout,
        TraceComponentLimits {
            identity: identity_limits,
            layout: layout_limits,
            monitor: monitor_limits,
        },
    )
    .unwrap();
    let forged_component_id = forged_component.id();
    let mut forged_obligations = ResidualObligationSet::new(ResidualSetLimits {
        identity: identity_limits,
        max_obligations: 2,
        max_generic_arguments: 16,
    });
    forged_obligations
        .insert(forged_obligation.clone())
        .unwrap();
    let forged_plan = SemanticResidualPlan::build(
        forged_obligations,
        vec![SemanticPlanComponent::Trace(forged_component)],
        vec![layout.clone()],
        vec![SemanticPlanMapping {
            obligation: forged_obligation.id(),
            component: forged_component_id,
        }],
        SemanticPlanLimits {
            max_obligations: 2,
            max_components: 2,
            max_layouts: 2,
            max_mappings: 2,
            max_plan_nodes: 16,
            identity: identity_limits,
            layout: layout_limits,
        },
    )
    .unwrap();
    let mut obligations = ResidualObligationSet::new(ResidualSetLimits {
        identity: identity_limits,
        max_obligations: 2,
        max_generic_arguments: 16,
    });
    obligations.insert(obligation.clone()).unwrap();
    let plan = SemanticResidualPlan::build(
        obligations,
        vec![SemanticPlanComponent::Trace(component)],
        vec![layout],
        vec![SemanticPlanMapping {
            obligation: obligation.id(),
            component: component_id,
        }],
        SemanticPlanLimits {
            max_obligations: 2,
            max_components: 2,
            max_layouts: 2,
            max_mappings: 2,
            max_plan_nodes: 16,
            identity: identity_limits,
            layout: layout_limits,
        },
    )
    .unwrap();
    let limits = TracePlanSourceLimits {
        max_obligations: 2,
        max_bindings: 2,
        max_clause_sites: 16,
        max_total_identity_work: 64,
        source: TraceObligationSourceLimits {
            max_snapshots: 1,
            max_snapshot_bytes: file.text.len(),
            identity: identity_limits,
        },
    };
    let reference = ResidualObligationRef::new(obligation.id(), Default::default()).unwrap();
    let expected = [reference.clone()].into_iter().collect();
    let binding = || TracePlanSourceBinding {
        reference: &reference,
        type_fact_index: index,
        alternative_index: 0,
        clause_index: 0,
        rule_suffix: &[],
    };
    let forged_reference =
        ResidualObligationRef::new(forged_obligation.id(), Default::default()).unwrap();
    let frozen_reference =
        ResidualObligationRef::new(frozen_obligation.id(), Default::default()).unwrap();
    let frozen_expected = [frozen_reference.clone()].into_iter().collect();
    let validate_frozen = |expected_contract| {
        validate_trace_plan_sources(TracePlanSourceInput {
            plan: &frozen_plan,
            analysis: &analysis,
            dispositions: &dispositions,
            hir: &checked.hir,
            expected_contract,
            clause_sites: &models.source_clause_sites,
            snapshots: &snapshots,
            expected: &frozen_expected,
            bindings: &[TracePlanSourceBinding {
                reference: &frozen_reference,
                ..binding()
            }],
            limits,
            control: None,
        })
    };
    assert_eq!(validate_frozen(frozen_contract), Ok(()));
    assert_eq!(
        validate_frozen(BehaviorContractRef::SourceConstraint),
        Err(TracePlanSourceError::SourceIdentityMismatch(
            frozen_obligation.id()
        ))
    );
    assert_eq!(
        validate_trace_plan_sources(TracePlanSourceInput {
            plan: &forged_plan,
            analysis: &analysis,
            dispositions: &dispositions,
            hir: &checked.hir,
            expected_contract: BehaviorContractRef::SourceConstraint,
            clause_sites: &models.source_clause_sites,
            snapshots: &snapshots,
            expected: &[forged_reference.clone()].into_iter().collect(),
            bindings: &[TracePlanSourceBinding {
                reference: &forged_reference,
                ..binding()
            }],
            limits,
            control: None,
        }),
        Err(TracePlanSourceError::SourceIdentityMismatch(
            forged_obligation.id()
        ))
    );
    let verify = |bindings: &[TracePlanSourceBinding<'_>], limits| {
        validate_trace_plan_sources(TracePlanSourceInput {
            plan: &plan,
            analysis: &analysis,
            dispositions: &dispositions,
            hir: &checked.hir,
            expected_contract: BehaviorContractRef::SourceConstraint,
            clause_sites: &models.source_clause_sites,
            snapshots: &snapshots,
            expected: &expected,
            bindings,
            limits,
            control: None,
        })
    };
    assert_eq!(verify(&[binding()], limits), Ok(()));
    let reference_binding = binding();
    let projection_input = |plan| TracePlanSourceInput {
        plan,
        analysis: &analysis,
        dispositions: &dispositions,
        hir: &checked.hir,
        expected_contract: BehaviorContractRef::SourceConstraint,
        clause_sites: &models.source_clause_sites,
        snapshots: &snapshots,
        expected: &expected,
        bindings: std::slice::from_ref(&reference_binding),
        limits,
        control: None,
    };
    assert!(analyzed.validate_plan(projection_input(&plan)).is_ok());
    assert!(matches!(
        projection.validate_plan(projection_input(&plan)),
        Err(etas_frontend::code::descriptor::TracePlanProjectionError::MissingAnalyzedSource)
    ));
    let wrong_binding = TracePlanSourceBinding {
        type_fact_index: index + 1,
        ..binding()
    };
    let mut wrong_input = projection_input(&plan);
    wrong_input.bindings = std::slice::from_ref(&wrong_binding);
    assert!(matches!(
        analyzed.validate_plan(wrong_input),
        Err(etas_frontend::code::descriptor::TracePlanProjectionError::SourceMismatch)
    ));
    let mut cancelled_input = projection_input(&plan);
    cancelled_input.control = Some(&CancelAnalyzed);
    assert!(matches!(
        analyzed.validate_plan(cancelled_input),
        Err(etas_frontend::code::descriptor::TracePlanProjectionError::Interrupted(_))
    ));
    let mut limited_input = projection_input(&plan);
    limited_input.limits.max_bindings = 0;
    assert!(matches!(
        analyzed.validate_plan(limited_input),
        Err(
            etas_frontend::code::descriptor::TracePlanProjectionError::Source(
                TracePlanSourceError::BindingLimit
            )
        )
    ));

    // A valid source identity alone does not prove that the plan implements
    // the analyzed trace rules.
    let pattern = patterns.values().next().unwrap().clone();
    let mut forged_rules = vec![vec![AlternativeRule::Allow(pattern.clone())]];
    let mut forged_monitor = etas_code::residual::monitor::CompiledTraceAlternatives::compile(
        &forged_rules,
        monitor_limits,
    )
    .unwrap();
    if forged_monitor.template_id() == projection.monitor().template_id() {
        forged_rules[0][0] = AlternativeRule::Deny(pattern);
        forged_monitor = etas_code::residual::monitor::CompiledTraceAlternatives::compile(
            &forged_rules,
            monitor_limits,
        )
        .unwrap();
    }
    assert_ne!(
        forged_monitor.template_id(),
        projection.monitor().template_id()
    );
    let bad_obligation = ResidualObligation::new(
        identity.inputs().clone(),
        ResidualKind::TraceContract {
            monitor: forged_monitor.template_id(),
        },
        obligation.bindings(),
        identity_limits,
    )
    .unwrap();
    assert_eq!(bad_obligation.id(), obligation.id());
    let layout = plan.layout(obligation.bindings()).unwrap();
    let bad_component = CompiledTraceComponent::compile(
        bad_obligation.clone(),
        forged_monitor.template(),
        &patterns,
        layout,
        TraceComponentLimits {
            identity: identity_limits,
            layout: layout_limits,
            monitor: monitor_limits,
        },
    )
    .unwrap();
    let bad_component_id = bad_component.id();
    let mut bad_obligations = ResidualObligationSet::new(ResidualSetLimits {
        identity: identity_limits,
        max_obligations: 2,
        max_generic_arguments: 16,
    });
    bad_obligations.insert(bad_obligation.clone()).unwrap();
    let bad_plan = SemanticResidualPlan::build(
        bad_obligations,
        vec![SemanticPlanComponent::Trace(bad_component)],
        vec![layout.clone()],
        vec![SemanticPlanMapping {
            obligation: bad_obligation.id(),
            component: bad_component_id,
        }],
        SemanticPlanLimits {
            max_obligations: 2,
            max_components: 2,
            max_layouts: 2,
            max_mappings: 2,
            max_plan_nodes: 16,
            identity: identity_limits,
            layout: layout_limits,
        },
    )
    .unwrap();
    assert!(validate_trace_plan_sources(projection_input(&bad_plan)).is_ok());
    assert!(matches!(
        analyzed.validate_plan(projection_input(&bad_plan)),
        Err(etas_frontend::code::descriptor::TracePlanProjectionError::MonitorMismatch(id)) if id == obligation.id()
    ));
    let other_program =
        checked_with_stable_source("module app; flow different() -> unit ![] { return; }");
    assert!(matches!(
        validate_trace_plan_sources(TracePlanSourceInput {
            plan: &plan,
            analysis: &analysis,
            dispositions: &dispositions,
            hir: &other_program.hir,
            expected_contract: BehaviorContractRef::SourceConstraint,
            clause_sites: &models.source_clause_sites,
            snapshots: &snapshots,
            expected: &expected,
            bindings: &[binding()],
            limits,
            control: None,
        }),
        Err(TracePlanSourceError::SourceIdentity { id, .. }) if id == obligation.id()
    ));
    assert_eq!(
        verify(&[], limits),
        Err(TracePlanSourceError::MissingBinding(
            reference.fingerprint()
        ))
    );
    assert_eq!(
        verify(&[binding(), binding()], limits),
        Err(TracePlanSourceError::DuplicateBinding(
            reference.fingerprint()
        ))
    );
    let unexpected =
        ObligationId::from_canonical_fingerprint(CodeFingerprint::from_bytes([90; 32]));
    let unexpected = ResidualObligationRef::new(unexpected, Default::default()).unwrap();
    assert_eq!(
        verify(
            &[TracePlanSourceBinding {
                reference: &unexpected,
                ..binding()
            }],
            limits
        ),
        Err(TracePlanSourceError::UnexpectedBinding(
            unexpected.fingerprint()
        ))
    );
    let unexpected_expected = [unexpected.clone()].into_iter().collect();
    assert_eq!(
        validate_trace_plan_sources(TracePlanSourceInput {
            plan: &plan,
            analysis: &analysis,
            dispositions: &dispositions,
            hir: &checked.hir,
            expected_contract: BehaviorContractRef::SourceConstraint,
            clause_sites: &models.source_clause_sites,
            snapshots: &snapshots,
            expected: &unexpected_expected,
            bindings: &[TracePlanSourceBinding {
                reference: &unexpected,
                ..binding()
            }],
            limits,
            control: None,
        }),
        Err(TracePlanSourceError::UnexpectedObligation(
            unexpected.obligation()
        ))
    );
    assert_eq!(
        validate_trace_plan_sources(TracePlanSourceInput {
            plan: &plan,
            analysis: &analysis,
            dispositions: &dispositions,
            hir: &checked.hir,
            expected_contract: BehaviorContractRef::SourceConstraint,
            clause_sites: &models.source_clause_sites,
            snapshots: &snapshots,
            expected: &Default::default(),
            bindings: &[],
            limits,
            control: None,
        }),
        Err(TracePlanSourceError::MissingObligation(obligation.id()))
    );
    let generic = ResidualObligationRef::new(
        obligation.id(),
        BTreeMap::from([(
            GenericParameterId::from_canonical_fingerprint(CodeFingerprint::from_bytes([91; 32])),
            GenericArgumentIdentity::Type(CodeFingerprint::from_bytes([92; 32])),
        )]),
    )
    .unwrap();
    let generic_expected = [reference.clone(), generic.clone()].into_iter().collect();
    let generic_binding = TracePlanSourceBinding {
        reference: &generic,
        ..binding()
    };
    assert_eq!(
        validate_trace_plan_sources(TracePlanSourceInput {
            plan: &plan,
            analysis: &analysis,
            dispositions: &dispositions,
            hir: &checked.hir,
            expected_contract: BehaviorContractRef::SourceConstraint,
            clause_sites: &models.source_clause_sites,
            snapshots: &snapshots,
            expected: &generic_expected,
            bindings: &[binding(), generic_binding],
            limits,
            control: None,
        }),
        Err(TracePlanSourceError::Reference {
            id: obligation.id(),
            reference: generic.fingerprint(),
            reason: etas_effects::trace_spec::TraceObligationReferenceError::BindingSetMismatch,
        })
    );
    assert_eq!(
        validate_trace_plan_sources(TracePlanSourceInput {
            plan: &plan,
            analysis: &analysis,
            dispositions: &dispositions,
            hir: &checked.hir,
            expected_contract: BehaviorContractRef::SourceConstraint,
            clause_sites: &models.source_clause_sites,
            snapshots: &snapshots,
            expected: &generic_expected,
            bindings: &[binding()],
            limits,
            control: None,
        }),
        Err(TracePlanSourceError::MissingBinding(generic.fingerprint()))
    );
    assert!(matches!(
        validate_trace_plan_sources(TracePlanSourceInput {
            plan: &plan,
            analysis: &analysis,
            dispositions: &dispositions,
            hir: &checked.hir,
            expected_contract: BehaviorContractRef::SourceConstraint,
            clause_sites: &models.source_clause_sites,
            snapshots: &snapshots,
            expected: &generic_expected,
            bindings: &[
                binding(),
                TracePlanSourceBinding {
                    reference: &generic,
                    alternative_index: 1,
                    ..binding()
                },
            ],
            limits,
            control: None,
        }),
        Err(TracePlanSourceError::Obligation {
            id,
            reference: failed_reference,
            reason: etas_effects::trace_spec::TraceObligationSourceError::ClauseMismatch,
        }) if id == obligation.id() && failed_reference == generic.fingerprint()
    ));
    assert_eq!(
        verify(
            &[binding()],
            TracePlanSourceLimits {
                max_bindings: 0,
                ..limits
            }
        ),
        Err(TracePlanSourceError::BindingLimit)
    );
    assert!(matches!(
        verify(&[TracePlanSourceBinding { alternative_index: 1, ..binding() }], limits),
        Err(TracePlanSourceError::Obligation { id, reason: etas_effects::trace_spec::TraceObligationSourceError::ClauseMismatch, .. }) if id == obligation.id()
    ));
    assert_eq!(
        verify(
            &[TracePlanSourceBinding {
                rule_suffix: &[0],
                ..binding()
            }],
            TracePlanSourceLimits {
                max_total_identity_work: 0,
                ..limits
            }
        ),
        Err(TracePlanSourceError::IdentityWorkLimit)
    );
    assert_eq!(
        verify(
            &[binding()],
            TracePlanSourceLimits {
                max_clause_sites: 0,
                ..limits
            }
        ),
        Err(TracePlanSourceError::ClauseSiteLimit)
    );
    struct Cancel;
    impl etas_utils::PipelineExecutionControl for Cancel {
        fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
            Err(etas_utils::PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::Cancelled,
                reason: "cancelled by test".into(),
            })
        }
    }
    assert!(matches!(
        validate_trace_plan_sources(TracePlanSourceInput {
            plan: &plan,
            analysis: &analysis,
            dispositions: &dispositions,
            hir: &checked.hir,
            expected_contract: BehaviorContractRef::SourceConstraint,
            clause_sites: &models.source_clause_sites,
            snapshots: &snapshots,
            expected: &expected,
            bindings: &[binding()],
            limits,
            control: Some(&Cancel),
        }),
        Err(TracePlanSourceError::Interrupted(
            etas_utils::PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::Cancelled,
                ..
            }
        ))
    ));
    struct CancelAfter(std::sync::atomic::AtomicUsize);
    impl etas_utils::PipelineExecutionControl for CancelAfter {
        fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
            if self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 3 {
                return Err(etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    reason: "cancelled during binding validation".into(),
                });
            }
            Ok(())
        }
    }
    let cancel = CancelAfter(std::sync::atomic::AtomicUsize::new(0));
    assert!(matches!(
        validate_trace_plan_sources(TracePlanSourceInput {
            plan: &plan,
            analysis: &analysis,
            dispositions: &dispositions,
            hir: &checked.hir,
            expected_contract: BehaviorContractRef::SourceConstraint,
            clause_sites: &models.source_clause_sites,
            snapshots: &snapshots,
            expected: &expected,
            bindings: &[binding()],
            limits,
            control: Some(&cancel),
        }),
        Err(TracePlanSourceError::Interrupted(
            etas_utils::PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::Cancelled,
                ..
            }
        ))
    ));
    assert!(cancel.0.load(std::sync::atomic::Ordering::Relaxed) >= 4);
}

#[test]
fn record_descriptor_order_does_not_prove_selector_disjointness() {
    use etas_code::{
        contract::{CanonicalEffect, CanonicalSelector},
        residual::pattern::{ActionPattern, ActionPatternLimits, ActionPatternMatch},
    };
    use etas_types::{Assignable, TypeRelation};

    let checked = checked(
        "module app; flow f(a: { x: i32, y: bool }, b: { y: bool, x: i32 }) -> unit ![] { return; }",
    );
    let item = flow(&checked, "f");
    let HirItem::Flow(decl) = &checked.hir.items[item] else {
        panic!("flow")
    };
    let ItemSignature::Flow(signature) = &checked.types.item_signatures[&item] else {
        panic!("signature")
    };
    TypeRelation::new(&checked.type_store)
        .assignable(signature.params[0], signature.params[1])
        .unwrap();
    let identities = DefinitionIdentityIndex::build(&checked, &origins(&checked, "app@1")).unwrap();
    let owner = GenericOwner::Declaration(identities.symbol(decl.symbol).unwrap().clone());
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    let roots = projector
        .project_closed_types(owner, &signature.params)
        .unwrap();
    let graph = projector.finish().unwrap();
    let first = graph.type_fingerprint(roots[0]).unwrap();
    let second = graph.type_fingerprint(roots[1]).unwrap();
    assert_ne!(first, second);
    let effect = etas_code::DefinitionKey::new(
        PackageKey::from_canonical_fingerprint(CodeFingerprint::from_bytes([1; 32])),
        vec!["app".into()],
        vec!["Gate".into()],
        DefinitionKind::Effect,
    )
    .unwrap();
    let action = |ty| CanonicalEffect {
        target: EffectTargetInfo::Action {
            effect: effect.clone(),
            name: "send".into(),
        },
        arguments: vec![CanonicalSelector::Type(ty)],
    };
    let limits = ActionPatternLimits {
        max_selectors: 4,
        max_identity_segments: 16,
        max_identity_bytes: 128,
        max_literal_bytes: 128,
    };
    let pattern = ActionPattern::new(action(first), limits).unwrap();
    assert_eq!(
        pattern.match_action(&action(second), limits),
        Ok(ActionPatternMatch::NeedsConcreteSelector)
    );
}

#[test]
fn checked_trace_template_uses_canonical_actions_and_complete_alternatives() {
    use etas_code::{
        contract::{CanonicalEffect, CanonicalSelector},
        residual::{
            monitor::{
                AlternativeLimits, AlternativeRule, CompiledTraceAlternatives,
                ConcreteEvidenceLimits,
            },
            pattern::{ActionPattern, ActionPatternLimits},
        },
    };
    use etas_frontend::code::descriptor::TraceTemplateProjectionError;
    let checked = checked_with_stable_source(
        r#"
module app;
effect Gate { action a<T>(value: T) -> unit; action b<T>(value: T) -> unit; }
effect Child extends Gate { action c(value: i32) -> unit; }
spec Rule<R>: trace = (+Gate.a<R> | -Gate.b<R>) & (Gate.a<R> >> Gate.b<R>);
spec Family: trace = -Gate;
spec AnyA: trace = -Gate.a;
spec Reverse: trace = Gate.b<i32> << Gate.a<i32>;
flow main() -> unit ![] ~ Rule<i32> { return; }
flow broad() -> unit ![] ~ Family { return; }
flow any_a() -> unit ![] ~ AnyA { return; }
flow generic<T>() -> unit ![] ~ Rule<T> { return; }
flow reversed() -> unit ![] ~ Reverse { return; }
"#,
    );
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "locked:app@1")).unwrap();
    let main = flow(&checked, "main");
    let HirItem::Flow(decl) = &checked.hir.items[main] else {
        panic!("flow")
    };
    let owner = GenericOwner::Declaration(identities.symbol(decl.symbol).unwrap().clone());
    let alternatives = &checked.effects.trace_specs.conformances[&main][0].alternatives;
    let pattern_limits = ActionPatternLimits {
        max_selectors: 4,
        max_identity_segments: 16,
        max_identity_bytes: 128,
        max_literal_bytes: 128,
    };
    let limits = AlternativeLimits {
        evidence: ConcreteEvidenceLimits {
            pattern: pattern_limits,
            max_bindings: 4,
            max_value_bytes: 128,
        },
        max_alternatives: 4,
        max_clauses: 12,
        max_patterns: 4,
        max_checks_per_event: 32,
        max_product_work_per_event: 128,
    };
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    project_flow(&mut projector, &checked, &identities, main);
    let compiled = projector
        .finish_trace_template(&owner, alternatives, limits)
        .unwrap();
    let link_limits = etas_frontend::code::descriptor::TraceSourceLinkLimits {
        max_sources: 8,
        max_sites: 16,
        max_clauses: 16,
        max_snapshot_bytes: 64 * 1024,
        identity: etas_code::residual::identity::ObligationIdentityLimits {
            max_generic_arguments: 16,
            max_structural_site_depth: 64,
        },
    };
    let sites = &checked.effects.trace_specs.source_clause_sites;
    compiled
        .validate_snapshot_links(&checked.sources, sites, link_limits)
        .unwrap();
    check_trace_plan_source_links(&checked, main, &compiled, limits);
    compiled
        .validate_snapshot_links(
            &checked.sources,
            sites,
            etas_frontend::code::descriptor::TraceSourceLinkLimits {
                max_snapshot_bytes: checked.sources.sources[0].text.len(),
                ..link_limits
            },
        )
        .expect("a source shared by multiple clauses is hashed and charged once");
    let mut missing = sites.clone();
    let clause = compiled.alternatives()[0][0].source_clause.unwrap();
    missing.remove(&clause);
    assert!(matches!(
        compiled.validate_snapshot_links(&checked.sources, &missing, link_limits),
        Err(etas_frontend::code::descriptor::TraceSourceLinkError::MissingClauseSite(id)) if id == clause
    ));
    let mut stale = checked.sources.clone();
    stale.sources[0].text = std::sync::Arc::from("changed source");
    assert!(matches!(
        compiled.validate_snapshot_links(&stale, sites, link_limits),
        Err(etas_frontend::code::descriptor::TraceSourceLinkError::MismatchedClauseSite(_))
    ));
    assert!(matches!(
        compiled.validate_snapshot_links(
            &checked.sources,
            sites,
            etas_frontend::code::descriptor::TraceSourceLinkLimits {
                max_snapshot_bytes: 0,
                ..link_limits
            },
        ),
        Err(etas_frontend::code::descriptor::TraceSourceLinkError::SnapshotBytesLimit)
    ));
    assert_eq!(compiled.alternatives().len(), alternatives.len());
    for (projected, checked_clauses) in compiled.alternatives().iter().zip(alternatives) {
        assert_eq!(projected.len(), checked_clauses.len());
        for (projected, checked_clause) in projected.iter().zip(checked_clauses) {
            assert_eq!(projected.source_clause, checked_clause.source_clause());
            assert!(projected.source_clause.is_some());
            let span = match checked_clause {
                etas_effects::trace_spec::TraceSpecClause::Allow { span, .. }
                | etas_effects::trace_spec::TraceSpecClause::Deny { span, .. }
                | etas_effects::trace_spec::TraceSpecClause::RequireBefore { span, .. }
                | etas_effects::trace_spec::TraceSpecClause::RequireAfter { span, .. } => *span,
                etas_effects::trace_spec::TraceSpecClause::Limit { .. } => panic!("limit"),
            };
            assert_eq!(projected.span, span);
        }
    }
    assert_eq!(
        compiled.alternatives()[0][1].source_clause,
        compiled.alternatives()[1][1].source_clause
    );
    assert!(matches!(
        compiled.alternatives()[0][1].rule,
        AlternativeRule::Before { .. }
    ));

    let gate = checked
        .hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Effect(effect) if checked.hir.symbols.get(effect.symbol)?.name == "Gate" => {
                Some(effect)
            }
            _ => None,
        })
        .unwrap();
    let gate = identities.symbol(gate.symbol).unwrap().clone();
    let graph = DescriptorGraph::new(
        DescriptorGraphInput {
            types: vec![TypeInfo::Primitive(PrimitiveInfo::I32)],
            ..Default::default()
        },
        DescriptorLimits::default(),
    )
    .unwrap();
    let ty = graph.type_fingerprint(TypeInfoRef(0)).unwrap();
    let action = |name: &str| {
        ActionPattern::new(
            CanonicalEffect {
                target: EffectTargetInfo::Action {
                    effect: gate.clone(),
                    name: name.into(),
                },
                arguments: vec![CanonicalSelector::Type(ty)],
            },
            pattern_limits,
        )
        .unwrap()
    };
    let (a, b) = (action("a"), action("b"));
    let AlternativeRule::Before { guard, target } = &compiled.alternatives()[0][1].rule else {
        panic!("temporal rule")
    };
    assert_eq!(guard.id(), a.id());
    assert_eq!(target.id(), b.id());
    let expected = CompiledTraceAlternatives::compile(
        &[
            vec![
                AlternativeRule::Allow(a.clone()),
                AlternativeRule::Before {
                    guard: a.clone(),
                    target: b.clone(),
                },
            ],
            vec![
                AlternativeRule::Deny(b.clone()),
                AlternativeRule::Before {
                    guard: a,
                    target: b,
                },
            ],
        ],
        limits,
    )
    .unwrap();
    assert_eq!(compiled.monitor().template_id(), expected.template_id());
    let reversed = flow(&checked, "reversed");
    let HirItem::Flow(reversed_decl) = &checked.hir.items[reversed] else {
        panic!("flow")
    };
    let reversed_owner =
        GenericOwner::Declaration(identities.symbol(reversed_decl.symbol).unwrap().clone());
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    project_flow(&mut projector, &checked, &identities, reversed);
    let reversed_alternatives =
        &checked.effects.trace_specs.conformances[&reversed][0].alternatives;
    let actual = projector
        .finish_trace_template(&reversed_owner, reversed_alternatives, limits)
        .unwrap();
    let AlternativeRule::Before { guard, target } = &actual.alternatives()[0][0].rule else {
        panic!("reverse temporal rule")
    };
    assert_eq!(guard.id(), action("a").id());
    assert_eq!(target.id(), action("b").id());
    let expected = CompiledTraceAlternatives::compile(
        &[vec![AlternativeRule::Before {
            guard: action("a"),
            target: action("b"),
        }]],
        limits,
    )
    .unwrap();
    assert_eq!(actual.monitor().template_id(), expected.template_id());
    let projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    assert!(matches!(
        projector.finish_trace_template(&owner, alternatives, limits),
        Err(TraceTemplateProjectionError::MissingCallableScope)
    ));
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    project_flow(&mut projector, &checked, &identities, main);
    assert!(matches!(
        projector.finish_trace_template(
            &owner,
            alternatives,
            AlternativeLimits {
                max_alternatives: 1,
                ..limits
            }
        ),
        Err(TraceTemplateProjectionError::AlternativeLimit)
    ));
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    project_flow(&mut projector, &checked, &identities, main);
    assert!(matches!(
        projector.finish_trace_template(
            &owner,
            alternatives,
            AlternativeLimits {
                max_clauses: 1,
                ..limits
            }
        ),
        Err(TraceTemplateProjectionError::ClauseLimit)
    ));
    let mut unsupported = alternatives.clone();
    unsupported[0].push(etas_effects::trace_spec::TraceSpecClause::Limit {
        fact: etas_effects::TraceSpecClauseFact::Limit { count: 1 },
    });
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    project_flow(&mut projector, &checked, &identities, main);
    assert!(matches!(
        projector.finish_trace_template(&owner, &unsupported, limits),
        Err(TraceTemplateProjectionError::UnsupportedLimit)
    ));

    let broad = flow(&checked, "broad");
    let HirItem::Flow(broad_decl) = &checked.hir.items[broad] else {
        panic!("flow")
    };
    let broad_owner =
        GenericOwner::Declaration(identities.symbol(broad_decl.symbol).unwrap().clone());
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    project_flow(&mut projector, &checked, &identities, broad);
    let broad_alternatives = &checked.effects.trace_specs.conformances[&broad][0].alternatives;
    let actual = projector
        .finish_trace_template(&broad_owner, broad_alternatives, limits)
        .unwrap();
    let child = checked
        .hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Effect(effect) if checked.hir.symbols.get(effect.symbol)?.name == "Child" => {
                Some(effect)
            }
            _ => None,
        })
        .unwrap();
    let child = identities.symbol(child.symbol).unwrap().clone();
    let family = ActionPattern::from_effect_family(
        CanonicalEffect {
            target: EffectTargetInfo::Effect(gate.clone()),
            arguments: vec![],
        },
        vec![child.clone()],
        pattern_limits,
    )
    .unwrap();
    let expected =
        CompiledTraceAlternatives::compile(&[vec![AlternativeRule::Deny(family.clone())]], limits)
            .unwrap();
    assert_eq!(actual.monitor().template_id(), expected.template_id());
    let child_action = CanonicalEffect {
        target: EffectTargetInfo::Action {
            effect: child,
            name: "c".into(),
        },
        arguments: vec![],
    };
    assert_eq!(
        family.match_action(&child_action, pattern_limits),
        Ok(etas_code::residual::pattern::ActionPatternMatch::Matches)
    );

    let any = flow(&checked, "any_a");
    let HirItem::Flow(any_decl) = &checked.hir.items[any] else {
        panic!("flow")
    };
    let any_owner = GenericOwner::Declaration(identities.symbol(any_decl.symbol).unwrap().clone());
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    project_flow(&mut projector, &checked, &identities, any);
    let actual = projector
        .finish_trace_template(
            &any_owner,
            &checked.effects.trace_specs.conformances[&any][0].alternatives,
            limits,
        )
        .unwrap();
    let wildcard = ActionPattern::new(
        CanonicalEffect {
            target: EffectTargetInfo::Action {
                effect: gate.clone(),
                name: "a".into(),
            },
            arguments: vec![CanonicalSelector::Wildcard],
        },
        pattern_limits,
    )
    .unwrap();
    let expected =
        CompiledTraceAlternatives::compile(&[vec![AlternativeRule::Deny(wildcard)]], limits)
            .unwrap();
    assert_eq!(actual.monitor().template_id(), expected.template_id());

    let generic = flow(&checked, "generic");
    let HirItem::Flow(generic_decl) = &checked.hir.items[generic] else {
        panic!("flow")
    };
    let generic_owner =
        GenericOwner::Declaration(identities.symbol(generic_decl.symbol).unwrap().clone());
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    project_flow(&mut projector, &checked, &identities, generic);
    assert!(matches!(
        projector.finish_trace_template(
            &generic_owner,
            &checked.effects.trace_specs.conformances[&generic][0].alternatives,
            limits
        ),
        Err(TraceTemplateProjectionError::TypeSelector(
            etas_types::ty::TypeSubstitutionError::UnresolvedType(_)
        ))
    ));
    let mut bounded_alternatives = alternatives.clone();
    let etas_effects::trace_spec::TraceSpecClause::Allow { pattern, .. } =
        &mut bounded_alternatives[0][0]
    else {
        panic!("allow")
    };
    let etas_effects::Effect::AppliedAction(action) = pattern.row.effects.iter().next().unwrap()
    else {
        panic!("action")
    };
    pattern
        .arg_bounds
        .push(etas_effects::trace_spec::TraceSpecArgBound {
            target: etas_effects::trace_spec::TraceSpecArgBoundTarget::Action(
                action.action.clone(),
            ),
            arg_index: 0,
            name: "Index".into(),
            spec_symbol: etas_hir::SymbolId(0),
            spec_args: vec![],
        });
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    project_flow(&mut projector, &checked, &identities, main);
    assert!(matches!(
        projector.finish_trace_template(&owner, &bounded_alternatives, limits),
        Err(TraceTemplateProjectionError::BoundedPattern)
    ));
    let memory_read = checked
        .effect_registry
        .action_by_name("Memory.read")
        .unwrap();
    let i32_type = checked
        .type_store
        .iter()
        .find_map(|(id, ty)| {
            matches!(
                ty,
                etas_types::Type::Primitive(etas_types::PrimitiveType::I32)
            )
            .then_some(id)
        })
        .unwrap();
    let mut memory_alternatives = alternatives.clone();
    let etas_effects::trace_spec::TraceSpecClause::Allow { pattern, .. } =
        &mut memory_alternatives[0][0]
    else {
        panic!("allow")
    };
    pattern.row = etas_effects::EffectRow::closed(etas_effects::EffectSet::one(
        etas_effects::Effect::AppliedAction(etas_effects::ActionInstanceRef {
            action: memory_read.clone(),
            args: vec![etas_types::EffectArgRef::Type(i32_type)],
        }),
    ));
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    project_flow(&mut projector, &checked, &identities, main);
    assert!(matches!(
        projector.finish_trace_template(&owner, &memory_alternatives, limits),
        Err(TraceTemplateProjectionError::ContainmentSelector)
    ));
    let etas_effects::trace_spec::TraceSpecClause::Allow { pattern, .. } =
        &mut memory_alternatives[0][0]
    else {
        panic!("allow")
    };
    pattern.row = etas_effects::EffectRow::closed(etas_effects::EffectSet::one(
        etas_effects::Effect::AppliedAction(etas_effects::ActionInstanceRef {
            action: memory_read,
            args: vec![etas_types::EffectArgRef::Wildcard],
        }),
    ));
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    project_flow(&mut projector, &checked, &identities, main);
    assert!(
        projector
            .finish_trace_template(&owner, &memory_alternatives, limits)
            .is_ok()
    );
}

#[test]
fn checked_source_projection_keeps_generic_binder_ownership_and_bounds() {
    let checked = checked(
        r#"
module app.types;
spec First;
spec Second;
flow first<T ~ First>(value: T) -> T { return value; }
flow second<T ~ Second>(value: T) -> T { return value; }
"#,
    );
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "locked:app@1")).unwrap();
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    let first = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "first"),
    );
    let second = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "second"),
    );
    let graph = projector.finish().unwrap();
    let first = signature(&graph, first);
    let second = signature(&graph, second);
    assert_ne!(first.generics, second.generics);
    assert_ne!(first.output, second.output);
    for signature in [first, second] {
        let TypeInfo::Parameter(binder) = graph.type_info(signature.output).unwrap() else {
            panic!("type parameter")
        };
        assert_eq!(signature.generics, [*binder]);
        assert_eq!(signature.parameters[0].ty, signature.output);
        assert_eq!(signature.parameters[0].name.as_deref(), Some("value"));
        assert_eq!(graph.binder(*binder).unwrap().bounds.len(), 1);
    }
    assert!(graph.data().types.len() < checked.type_store.len());
}

#[test]
fn recursive_adt_descriptors_project_finite_shared_graphs() {
    let checked = checked(
        r#"
module app.types;
enum Tree<T> { Leaf(T), Branch { left: Tree<T>, right: Tree<T> }, }
type Directory = { entries: Array<Entry> };
enum Entry { Folder(Directory), File(string), }
flow tree(value: Tree<i32>) -> Tree<i32> { return value; }
flow directory(value: Directory) -> Directory { return value; }
"#,
    );
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "locked:app@1")).unwrap();
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    let tree = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "tree"),
    );
    let directory = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "directory"),
    );
    let graph = projector.finish().unwrap();
    assert!(graph.data().types.len() < 32);
    assert!(graph.type_fingerprint(tree).is_ok());
    assert!(graph.type_fingerprint(directory).is_ok());
    let tree_enum = graph
        .data()
        .types
        .iter()
        .find_map(|ty| match ty {
            TypeInfo::Enum {
                definition,
                variants,
                ..
            } if definition.declaration() == ["Tree"] => Some(variants),
            _ => None,
        })
        .unwrap();
    assert!(
        matches!(&tree_enum[1].payload, VariantPayloadInfo::Named(fields) if fields.len() == 2 && fields[0].name == "left")
    );
}

#[test]
fn open_effect_rows_and_selectors_preserve_checked_binder_identity() {
    let checked = checked(
        r#"
module app.types;
flow apply<T, effect E>(value: T, callback: (T) -> T ![E]) -> T ![Error<T>, E] {
    return callback(value);
}
"#,
    );
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "locked:app@1")).unwrap();
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    let root = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "apply"),
    );
    let graph = projector.finish().unwrap();
    let signature = signature(&graph, root);
    let row = graph.effect_row(signature.effects.unwrap()).unwrap();
    let tail = row.tail.unwrap();
    assert!(signature.generics.contains(&tail));
    assert_eq!(
        graph.binder(tail).unwrap().kind,
        GenericParameterKind::EffectRow
    );
    assert_eq!(row.effects.len(), 1);
    assert_eq!(
        row.effects[0].arguments,
        [StaticEffectArgInfo::Type(signature.output)]
    );
    let TypeInfo::Function(callback) = graph.type_info(signature.parameters[1].ty).unwrap() else {
        panic!("callback")
    };
    assert_eq!(
        graph.effect_row(callback.effects.unwrap()).unwrap().tail,
        Some(tail)
    );
}

#[test]
fn package_identity_is_explicit_and_independent_of_hir_arena_positions() {
    let build = |source: &str, origin: &str| {
        let checked = checked(source);
        let identities =
            DefinitionIdentityIndex::build(&checked, &origins(&checked, origin)).unwrap();
        let mut projector =
            DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
        let root = project_flow(
            &mut projector,
            &checked,
            &identities,
            flow(&checked, "identity"),
        );
        projector.finish().unwrap().type_fingerprint(root).unwrap()
    };
    let a = build(
        "module app; flow identity<T>(value: T) -> T { return value; }",
        "origin:a",
    );
    let b = build(
        "module app; type Unrelated; flow identity<T>(value: T) -> T { return value; }",
        "origin:a",
    );
    let other_package = build(
        "module app; flow identity<T>(value: T) -> T { return value; }",
        "origin:b",
    );
    assert_eq!(a, b);
    assert_ne!(a, other_package);
}

#[test]
fn missing_ownership_and_small_projection_budgets_fail_closed() {
    let checked = checked("module app; flow identity<T>(value: T) -> T { return value; }");
    let mut owner = origins(&checked, "origin:a");
    owner.source_modules = HashMap::new();
    assert!(matches!(
        DefinitionIdentityIndex::build(&checked, &owner),
        Err(ProjectionError::MissingPackage(_))
    ));
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "origin:a")).unwrap();
    let mut projector = DefinitionDescriptorProjector::new(
        &checked,
        &identities,
        DescriptorLimits {
            max_nodes: 1,
            ..DescriptorLimits::default()
        },
    );
    let item = flow(&checked, "identity");
    let HirItem::Flow(decl) = &checked.hir.items[item] else {
        panic!("flow")
    };
    let ItemSignature::Flow(signature) = &checked.types.item_signatures[&item] else {
        panic!("signature")
    };
    assert!(matches!(
        projector.project_signature(
            GenericOwner::Declaration(identities.symbol(decl.symbol).unwrap().clone()),
            signature,
            &[Some("value".into())],
            Some(signature.output)
        ),
        Err(ProjectionError::Limit { .. })
    ));
}

#[test]
fn nominal_agent_and_wide_integer_types_remain_distinct_descriptors() {
    let checked = checked(
        r#"
module app;
agent Worker;
flow value(worker: Worker, signed: i128, unsigned: u128) -> Worker { return worker; }
"#,
    );
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "origin:a")).unwrap();
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    let root = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "value"),
    );
    let graph = projector.finish().unwrap();
    let signature = signature(&graph, root);
    assert!(
        matches!(graph.type_info(signature.output), Some(TypeInfo::Nominal { definition, representation: None, .. }) if definition.kind() == DefinitionKind::Agent)
    );
    assert_eq!(
        graph.type_info(signature.parameters[1].ty),
        Some(&TypeInfo::Primitive(PrimitiveInfo::I128))
    );
    assert_eq!(
        graph.type_info(signature.parameters[2].ty),
        Some(&TypeInfo::Primitive(PrimitiveInfo::U128))
    );
}

#[test]
fn std_nominal_fields_and_enum_payloads_come_from_the_canonical_registry() {
    let checked = checked(
        r#"
module app;
import std.http.codec.HttpWireResponseHead;
import std.http.codec.HttpDecodeStep;
flow head(value: HttpWireResponseHead) -> HttpWireResponseHead { return value; }
flow step(value: HttpDecodeStep<HttpWireResponseHead>) -> HttpDecodeStep<HttpWireResponseHead> { return value; }
"#,
    );
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "origin:a")).unwrap();
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    let head = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "head"),
    );
    let step = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "step"),
    );
    let graph = projector.finish().unwrap();
    let TypeInfo::Nominal {
        representation: Some(record),
        definition,
        ..
    } = graph.type_info(signature(&graph, head).output).unwrap()
    else {
        panic!("canonical std nominal record");
    };
    assert_eq!(definition.module(), ["std", "http", "codec"]);
    let TypeInfo::Record(fields) = graph.type_info(*record).unwrap() else {
        panic!("record shape")
    };
    let symbol = checked
        .std_registry
        .lookup_qualified(&["std", "http", "codec", "HttpWireResponseHead"])
        .unwrap();
    let etas_std::StdDecl::Type(decl) = &symbol.decl else {
        panic!("std type")
    };
    let Some(etas_std::StdType::Record(declared)) = &decl.representation else {
        panic!("registry record")
    };
    assert_eq!(
        fields.iter().map(|field| &field.name).collect::<Vec<_>>(),
        declared.iter().map(|field| &field.name).collect::<Vec<_>>()
    );
    let TypeInfo::Applied { constructor, .. } =
        graph.type_info(signature(&graph, step).output).unwrap()
    else {
        panic!("generic std enum")
    };
    let TypeInfo::Enum { variants, .. } = graph.type_info(*constructor).unwrap() else {
        panic!("enum payloads")
    };
    assert_eq!(
        variants
            .iter()
            .map(|variant| variant.name.as_str())
            .collect::<Vec<_>>(),
        ["NeedMore", "Complete", "Malformed"]
    );
    assert!(graph.type_fingerprint(step).is_ok());
}

#[test]
fn alias_declaration_remains_descriptive_without_retagging_its_target() {
    let checked = checked(
        "module app; alias Values<T> = Array<T>; flow use_values(value: Values<i32>) -> Values<i32> { return value; }",
    );
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "origin:a")).unwrap();
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    let symbol = checked
        .hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::TypeAlias(decl) => Some(decl.symbol),
            _ => None,
        })
        .unwrap();
    let alias = projector.project_type_declaration(symbol).unwrap();
    let use_values = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "use_values"),
    );
    let graph = projector.finish().unwrap();
    assert!(
        matches!(graph.type_info(alias), Some(TypeInfo::Alias { definition, parameters, .. }) if definition.declaration() == ["Values"] && parameters.len() == 1)
    );
    assert!(matches!(
        graph.type_info(signature(&graph, use_values).output),
        Some(TypeInfo::Builtin {
            constructor: BuiltinTypeInfo::Array,
            ..
        })
    ));
}

#[test]
fn descriptor_bounds_follow_resolved_reexports_wildcards_and_std_prelude() {
    for import in ["import app.bridge.API as Selected;", "import app.bridge.*;"] {
        let bound = if import.contains("Selected") {
            "Selected"
        } else {
            "API"
        };
        let consumer = format!(
            "module app.consumer; {import} flow use_bound<T ~ {bound}, I ~ Index>(value: T, index: I) -> T {{ return value; }}"
        );
        let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
        input.options.entry_policy = etas_frontend::EntryPolicy::Optional;
        input.sources = [
            "module app.bounds; public spec Allowed;",
            "module app.bridge; public import app.bounds.Allowed as API;",
            &consumer,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, text)| SourceInput {
            id: etas_core::SourceId(index as u32 + 1),
            path: None,
            text: text.into(),
            kind: etas_frontend::SourceKind::SourceProjectFile,
        })
        .collect();
        let output = Frontend.check_project(input);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let checked = output.checked.unwrap();
        let identities =
            DefinitionIdentityIndex::build(&checked, &origins(&checked, "origin:a")).unwrap();
        let mut projector =
            DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
        let root = project_flow(
            &mut projector,
            &checked,
            &identities,
            flow(&checked, "use_bound"),
        );
        let graph = projector.finish().unwrap();
        let signature = signature(&graph, root);
        let allowed = &graph.binder(signature.generics[0]).unwrap().bounds[0].definition;
        assert_eq!(allowed.module(), ["app", "bounds"]);
        assert_eq!(allowed.declaration(), ["Allowed"]);
        let index = &graph.binder(signature.generics[1]).unwrap().bounds[0].definition;
        assert_eq!(index.module(), ["std", "core"]);
        assert_eq!(index.declaration(), ["Index"]);
    }
}

#[test]
fn missing_checked_bound_facts_are_not_projected_as_an_unconstrained_binder() {
    let mut checked = checked("module app; spec Allowed; type Container<T ~ Allowed>;");
    let owner = origins(&checked, "origin:a");
    let data = std::sync::Arc::make_mut(&mut checked);
    std::sync::Arc::make_mut(&mut data.types)
        .type_param_bounds
        .clear();
    assert!(matches!(
        DefinitionIdentityIndex::build(&checked, &owner),
        Err(ProjectionError::Invalid(_))
    ));
}

#[test]
fn unresolved_std_named_types_cannot_become_opaque_successful_descriptors() {
    let mut checked = checked("module app; flow value() -> unit { return; }");
    let data = std::sync::Arc::make_mut(&mut checked);
    let unknown = std::sync::Arc::make_mut(&mut data.type_store).intern(etas_types::Type::Named(
        etas_types::NamedTypeRef {
            source_parameter: None,
            name: "std.forged.Unknown".into(),
        },
    ));
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "origin:a")).unwrap();
    let item = flow(&checked, "value");
    let HirItem::Flow(decl) = &checked.hir.items[item] else {
        panic!("flow")
    };
    let ItemSignature::Flow(signature) = &checked.types.item_signatures[&item] else {
        panic!("signature")
    };
    let mut malformed = signature.clone();
    malformed.output = unknown;
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    projector
        .project_signature(
            GenericOwner::Declaration(identities.symbol(decl.symbol).unwrap().clone()),
            &malformed,
            &[],
            None,
        )
        .unwrap();
    assert!(matches!(
        projector.finish(),
        Err(ProjectionError::MissingIdentity(_))
    ));
}

#[test]
fn private_import_aliases_in_separate_module_parts_keep_distinct_bindings() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
    input.options.entry_policy = etas_frontend::EntryPolicy::Optional;
    input.sources = [
        "module app.first; public spec Allowed;",
        "module app.second; public spec Allowed;",
        "module app.shared; import app.first.Allowed as Choice; flow first<T ~ Choice>(value: T) -> T { return value; }",
        "module app.shared; import app.second.Allowed as Choice; flow second<T ~ Choice>(value: T) -> T { return value; }",
    ].into_iter().enumerate().map(|(index, text)| SourceInput {
        id: etas_core::SourceId(index as u32 + 1), path: None, text: text.into(), kind: etas_frontend::SourceKind::SourceProjectFile,
    }).collect();
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "origin:a")).unwrap();
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    let first = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "first"),
    );
    let second = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "second"),
    );
    let graph = projector.finish().unwrap();
    let first = &graph
        .binder(signature(&graph, first).generics[0])
        .unwrap()
        .bounds[0]
        .definition;
    let second = &graph
        .binder(signature(&graph, second).generics[0])
        .unwrap()
        .bounds[0]
        .definition;
    assert_eq!(first.module(), ["app", "first"]);
    assert_eq!(second.module(), ["app", "second"]);
    assert_ne!(first, second);
}
