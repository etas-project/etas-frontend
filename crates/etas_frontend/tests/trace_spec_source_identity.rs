use etas_code::{CodeFingerprint, StableSourceKey, StableSourceOrigin};
use etas_core::SourceId;
use etas_frontend::{
    CheckRequest, EntryPolicy, EnvironmentChange, Frontend, FrontendSession, ProjectChangeSet,
    ProjectInput, ProjectRevision, ProjectStableSourceKeyInput, SourceInput,
};

fn checked(
    source_id: SourceId,
    text: &str,
    key: Option<StableSourceKey>,
) -> etas_frontend::ProjectOutput {
    let mut source = SourceInput::anonymous(text);
    source.id = source_id;
    let mut input = ProjectInput::single_source(source);
    input.options.entry_policy = EntryPolicy::Optional;
    if let Some(key) = key {
        input
            .environment
            .stable_source_keys
            .push(ProjectStableSourceKeyInput {
                source: source_id,
                key,
            });
    }
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output
}

fn clauses(
    source_id: SourceId,
    text: &str,
    key: Option<StableSourceKey>,
) -> Vec<Vec<Option<etas_code::residual::identity::ClauseId>>> {
    let output = checked(source_id, text, key);
    output
        .effects
        .unwrap()
        .facts
        .trace_specs
        .conformances
        .values()
        .flatten()
        .next()
        .unwrap()
        .alternatives
        .iter()
        .map(|alternative| {
            alternative
                .iter()
                .map(|clause| clause.source_clause())
                .collect()
        })
        .collect()
}

#[test]
fn materialized_clauses_retain_revalidatable_source_sites() {
    use etas_code::residual::identity::{
        CheckedSourceSnapshot, ObligationIdentityLimits, ResidualRuleKind,
    };

    let key = StableSourceKey::new(
        StableSourceOrigin::Project(CodeFingerprint::from_bytes([82; 32])),
        &["src".into(), "main.es".into()],
    )
    .unwrap();
    let text = "module app; effect Gate { action a() -> unit; action b() -> unit; action c() -> unit; } spec Safe: trace = (+Gate.a | +Gate.b) & -Gate.c; flow main() -> unit ![] ~ Safe { return; }";
    let output = checked(SourceId(0), text, Some(key));
    let mut effects = etas_effects::EffectOutput::default();
    let snapshots = [etas_effects::EffectSourceSnapshot {
        source: SourceId(0),
        key,
        text,
    }];
    let models = etas_effects::trace_spec::materialize_trace_spec_models(
        &output.hir.as_ref().unwrap().hir,
        output.types.as_ref().unwrap(),
        &output.effect_pipeline_artifacts.as_ref().unwrap().registry,
        &[],
        &snapshots,
        &mut effects,
    )
    .unwrap();
    assert_eq!(models.source_clause_sites.len(), 3);
    let emitted_sites = &output
        .checked
        .as_ref()
        .unwrap()
        .effects
        .trace_specs
        .source_clause_sites;
    assert_eq!(emitted_sites, &models.source_clause_sites);
    assert_eq!(
        &output
            .effects
            .as_ref()
            .unwrap()
            .facts
            .trace_specs
            .source_clause_sites,
        emitted_sites
    );
    let facts = &output.checked.as_ref().unwrap().effects.trace_specs;
    let json = serde_json::to_value(facts).unwrap();
    let decoded: etas_effects::TraceSpecFacts = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(&decoded, facts);
    let mut duplicate = json;
    let sites = duplicate["source_clause_sites"].as_array_mut().unwrap();
    sites.push(sites[0].clone());
    assert!(serde_json::from_value::<etas_effects::TraceSpecFacts>(duplicate).is_err());
    let mut inconsistent = facts.clone();
    inconsistent.source_clause_sites.insert(
        etas_code::residual::identity::ClauseId::from_canonical_fingerprint(
            CodeFingerprint::from_bytes([99; 32]),
        ),
        facts.source_clause_sites.values().next().unwrap().clone(),
    );
    assert!(serde_json::to_value(&inconsistent).is_err());
    let config = bincode::config::standard();
    let bytes = bincode::serde::encode_to_vec(facts, config).unwrap();
    let (decoded, consumed): (etas_effects::TraceSpecFacts, usize) =
        bincode::serde::decode_from_slice(&bytes, config).unwrap();
    assert_eq!(consumed, bytes.len());
    assert_eq!(&decoded, facts);
    let hir = &output.hir.as_ref().unwrap().hir;
    let types = output.types.as_ref().unwrap();
    let registry = &output.effect_pipeline_artifacts.as_ref().unwrap().registry;
    assert!(
        models
            .validate_source_sites(hir, types, registry, &[], &snapshots)
            .is_ok()
    );
    let analysis = etas_effects::trace_spec::analyze_trace_spec_monitors(
        etas_effects::trace_spec::TraceSpecAnalysisInput {
            source_snapshots: &snapshots,
            external_trace_specs: &[],
            hir,
            types,
            registry,
            models: &models,
            units: &[],
            external_summaries: &[],
        },
        &mut effects,
    )
    .unwrap();
    assert!(!models.referenced_fact_indices.is_empty());
    for (item, indices) in &models.referenced_fact_indices {
        for (index, alternatives) in indices.iter().zip(&models.referenced_by_item[item]) {
            let source = analysis.source_conformance(*index).unwrap();
            assert_eq!(source.item, *item);
            assert!(source.source_instance.is_some());
            let owner = source.source_owner.as_ref().unwrap();
            let declaration = &hir.items[*item];
            assert_eq!(
                owner.declaration().byte_start,
                u64::from(declaration.span().range.start.0)
            );
            assert_eq!(
                owner.declaration().byte_end,
                u64::from(declaration.span().range.end.0)
            );
            let expected = alternatives
                .iter()
                .map(|group| {
                    group
                        .iter()
                        .map(|clause| clause.source_clause())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            assert_eq!(source.clause_ids, expected);
        }
    }
    let mut changed_models = models.clone();
    changed_models.referenced_fact_indices.clear();
    let dispositions =
        etas_effects::trace_spec::collect_trace_static_dispositions(&analysis, None, None).unwrap();
    assert!(matches!(
        changed_models.validate_source_sites(hir, types, registry, &[], &snapshots),
        Err(etas_effects::EffectPipelineError::InvalidTraceSpecSourceSites { .. })
    ));
    assert!(matches!(
        etas_effects::trace_spec::validate_trace_spec_facts(
            &mut effects,
            etas_effects::trace_spec::TraceSpecValidationInput {
                external_summaries: &[],
                external_implementations: &[],
                hir,
                types,
                registry,
                models: &changed_models,
                analysis: &analysis,
                dispositions: &dispositions,
                residual_plans: &Default::default(),
                reachable_items: None,
                control: None,
            },
        ),
        Err(etas_effects::EffectPipelineError::InvalidTraceSpecSourceSites { .. })
    ));
    let mut missing = models.clone();
    missing.source_clause_sites.clear();
    assert!(
        missing
            .validate_source_sites(hir, types, registry, &[], &snapshots)
            .is_err()
    );
    let strip_identity = |clause: &mut etas_effects::trace_spec::TraceSpecClause| match clause {
        etas_effects::trace_spec::TraceSpecClause::Allow {
            source_path,
            source_clause,
            ..
        }
        | etas_effects::trace_spec::TraceSpecClause::Deny {
            source_path,
            source_clause,
            ..
        }
        | etas_effects::trace_spec::TraceSpecClause::RequireBefore {
            source_path,
            source_clause,
            ..
        }
        | etas_effects::trace_spec::TraceSpecClause::RequireAfter {
            source_path,
            source_clause,
            ..
        } => {
            *source_path = None;
            *source_clause = None;
        }
        _ => panic!("expected source clause"),
    };
    let mut stripped = models.clone();
    strip_identity(
        stripped
            .clauses_by_item
            .values_mut()
            .flatten()
            .flatten()
            .next()
            .unwrap(),
    );
    assert!(
        stripped
            .validate_source_sites(hir, types, registry, &[], &snapshots)
            .is_err()
    );
    let mut stripped_reference = models.clone();
    strip_identity(
        stripped_reference
            .referenced_by_item
            .values_mut()
            .flatten()
            .flatten()
            .flatten()
            .next()
            .unwrap(),
    );
    assert!(
        stripped_reference
            .validate_source_sites(hir, types, registry, &[], &snapshots)
            .is_err()
    );
    let mut changed_pattern = models.clone();
    let clause = changed_pattern
        .referenced_by_item
        .values_mut()
        .flatten()
        .flatten()
        .flatten()
        .next()
        .unwrap();
    match clause {
        etas_effects::trace_spec::TraceSpecClause::Allow { pattern, .. }
        | etas_effects::trace_spec::TraceSpecClause::Deny { pattern, .. } => {
            pattern.row.effects = etas_effects::EffectSet::default();
        }
        _ => panic!("expected allow or deny"),
    }
    assert!(
        changed_pattern
            .validate_source_sites(hir, types, registry, &[], &snapshots)
            .is_err()
    );
    let mut both_changed = models.clone();
    let target = both_changed
        .clauses_by_item
        .values()
        .flatten()
        .flatten()
        .next()
        .unwrap()
        .source_clause()
        .unwrap();
    for clause in both_changed
        .clauses_by_item
        .values_mut()
        .flatten()
        .flatten()
        .chain(
            both_changed
                .referenced_by_item
                .values_mut()
                .flatten()
                .flatten()
                .flatten(),
        )
    {
        if clause.source_clause() == Some(target) {
            match clause {
                etas_effects::trace_spec::TraceSpecClause::Allow { pattern, .. }
                | etas_effects::trace_spec::TraceSpecClause::Deny { pattern, .. } => {
                    pattern.row.effects = etas_effects::EffectSet::default();
                }
                _ => panic!("expected allow or deny"),
            }
        }
    }
    assert!(
        both_changed
            .validate_source_sites(hir, types, registry, &[], &snapshots)
            .is_err()
    );
    let mut wrong_fact = models.clone();
    *wrong_fact
        .referenced_fact_indices
        .values_mut()
        .flatten()
        .next()
        .unwrap() = usize::MAX;
    assert!(
        wrong_fact
            .validate_source_sites(hir, types, registry, &[], &snapshots)
            .is_err()
    );
    let analysis_input = etas_effects::trace_spec::TraceSpecAnalysisInput {
        source_snapshots: &snapshots,
        external_trace_specs: &[],
        hir: &output.hir.as_ref().unwrap().hir,
        types: output.types.as_ref().unwrap(),
        registry: &output.effect_pipeline_artifacts.as_ref().unwrap().registry,
        models: &missing,
        units: &[],
        external_summaries: &[],
    };
    assert!(matches!(
        etas_effects::trace_spec::analyze_trace_spec_monitors(analysis_input, &mut effects),
        Err(etas_effects::EffectPipelineError::InvalidSourceIdentity { .. })
    ));
    let changed_snapshots = [etas_effects::EffectSourceSnapshot {
        source: SourceId(0),
        key,
        text: "changed source",
    }];
    assert!(
        models
            .validate_source_sites(hir, types, registry, &[], &changed_snapshots)
            .is_err()
    );
    let mut extra = models.clone();
    let forged = etas_code::residual::identity::ClauseId::from_canonical_fingerprint(
        CodeFingerprint::from_bytes([99; 32]),
    );
    extra.source_clause_sites.insert(
        forged,
        models.source_clause_sites.values().next().unwrap().clone(),
    );
    assert!(
        extra
            .validate_source_sites(hir, types, registry, &[], &snapshots)
            .is_err()
    );
    let snapshot = CheckedSourceSnapshot::new(key, text.as_bytes());
    let limits = ObligationIdentityLimits {
        max_generic_arguments: 1024,
        max_structural_site_depth: 64,
    };
    for (id, site) in &models.source_clause_sites {
        assert_eq!(*id, site.id());
        assert_eq!(site.validate_snapshot(&snapshot, limits), Ok(()));
        assert!(
            site.rule(&snapshot, ResidualRuleKind::ActionPattern, &[], limits)
                .is_ok()
        );
    }
    for clause in models.clauses_by_item.values().flatten().flatten() {
        assert!(
            models
                .source_clause_sites
                .contains_key(&clause.source_clause().unwrap())
        );
    }
}

#[test]
fn trace_source_index_preserves_missing_source_identity() {
    let text = "module app; effect Gate { action a() -> unit; } spec Safe: trace = -Gate.a; flow main() -> unit ![] ~ Safe { return; }";
    let output = checked(SourceId(0), text, None);
    let hir = &output.hir.as_ref().unwrap().hir;
    let types = output.types.as_ref().unwrap();
    let registry = &output.effect_pipeline_artifacts.as_ref().unwrap().registry;
    let mut effects = etas_effects::EffectOutput::default();
    let models = etas_effects::trace_spec::materialize_trace_spec_models(
        hir,
        types,
        registry,
        &[],
        &[],
        &mut effects,
    )
    .unwrap();
    let analysis = etas_effects::trace_spec::analyze_trace_spec_monitors(
        etas_effects::trace_spec::TraceSpecAnalysisInput {
            source_snapshots: &[],
            external_trace_specs: &[],
            hir,
            types,
            registry,
            models: &models,
            units: &[],
            external_summaries: &[],
        },
        &mut effects,
    )
    .unwrap();
    assert!(!models.referenced_fact_indices.is_empty());
    for indices in models.referenced_fact_indices.values() {
        for index in indices {
            let source = analysis.source_conformance(*index).unwrap();
            assert!(source.source_instance.is_none());
            assert!(source.source_owner.is_none());
            assert!(source.clause_ids.iter().flatten().all(Option::is_none));
            assert!(analysis.checked_clause_fact(*index, 0, 0).is_err());
        }
    }
}

#[test]
fn checked_trace_owner_is_stable_across_sessions_and_bound_to_source_revision() {
    fn owner(
        source_id: SourceId,
        key: StableSourceKey,
        text: &str,
    ) -> etas_effects::trace_spec::CheckedTraceSourceOwner {
        let output = checked(source_id, text, Some(key));
        let hir = &output.hir.as_ref().unwrap().hir;
        let types = output.types.as_ref().unwrap();
        let registry = &output.effect_pipeline_artifacts.as_ref().unwrap().registry;
        let snapshots = [etas_effects::EffectSourceSnapshot {
            source: source_id,
            key,
            text,
        }];
        let mut effects = etas_effects::EffectOutput::default();
        let models = etas_effects::trace_spec::materialize_trace_spec_models(
            hir,
            types,
            registry,
            &[],
            &snapshots,
            &mut effects,
        )
        .unwrap();
        let analysis = etas_effects::trace_spec::analyze_trace_spec_monitors(
            etas_effects::trace_spec::TraceSpecAnalysisInput {
                source_snapshots: &snapshots,
                external_trace_specs: &[],
                hir,
                types,
                registry,
                models: &models,
                units: &[],
                external_summaries: &[],
            },
            &mut effects,
        )
        .unwrap();
        let index = *models
            .referenced_fact_indices
            .values()
            .next()
            .unwrap()
            .first()
            .unwrap();
        analysis
            .source_conformance(index)
            .unwrap()
            .source_owner
            .clone()
            .unwrap()
    }
    let key = StableSourceKey::new(
        StableSourceOrigin::Project(CodeFingerprint::from_bytes([45; 32])),
        &["src".into(), "main.es".into()],
    )
    .unwrap();
    let other_key = StableSourceKey::new(
        StableSourceOrigin::Project(CodeFingerprint::from_bytes([45; 32])),
        &["src".into(), "other.es".into()],
    )
    .unwrap();
    let source = "module app; spec Quiet: trace = -Console.stdout_write; flow f() -> i32 ![] ~ Quiet { return 1; }";
    let edited = source.replace("return 1", "return 2");
    let first = owner(SourceId(0), key, source);
    let same = owner(SourceId(37), key, source);
    let changed = owner(SourceId(0), key, &edited);
    let other = owner(SourceId(0), other_key, source);
    assert_eq!(first, same);
    assert_eq!(first.id(), changed.id());
    assert_ne!(first.declaration().revision, changed.declaration().revision);
    assert_ne!(first.id(), other.id());
}

#[test]
fn checked_clause_facts_are_bound_to_real_analysis_and_source_alternatives() {
    use etas_effects::trace_spec::{CheckedTraceClauseDisposition, TraceSpecClause};

    let key = StableSourceKey::new(
        StableSourceOrigin::Project(CodeFingerprint::from_bytes([42; 32])),
        &["src".into(), "main.es".into()],
    )
    .unwrap();
    let text = "module app; effect Gate { action a() -> unit; action b() -> unit; action c() -> unit; } spec Safe: trace = ((+Gate.a & +Gate.a) | +Gate.b) & -Gate.c & (Gate.a >> Gate.b) & +Gate.a; flow main() -> unit ![] ~ Safe { return; }";
    let output = checked(SourceId(0), text, Some(key));
    let hir = &output.hir.as_ref().unwrap().hir;
    let types = output.types.as_ref().unwrap();
    let artifacts = output.effect_pipeline_artifacts.as_ref().unwrap();
    let registry = &artifacts.registry;
    let snapshots = [etas_effects::EffectSourceSnapshot {
        source: SourceId(0),
        key,
        text,
    }];
    let mut effects = etas_effects::EffectOutput::default();
    let models = etas_effects::trace_spec::materialize_trace_spec_models(
        hir,
        types,
        registry,
        &[],
        &snapshots,
        &mut effects,
    )
    .unwrap();
    let checked_effects = output.effects.as_ref().unwrap();
    effects.facts.item_effects = checked_effects.facts.item_effects.clone();
    effects.facts.unit_effects = checked_effects.facts.unit_effects.clone();
    let analysis = etas_effects::trace_spec::analyze_trace_spec_monitors(
        etas_effects::trace_spec::TraceSpecAnalysisInput {
            source_snapshots: &snapshots,
            external_trace_specs: &[],
            hir,
            types,
            registry,
            models: &models,
            units: &artifacts.units,
            external_summaries: &[],
        },
        &mut effects,
    )
    .unwrap();
    let mut permissions = 0;
    let mut denials = 0;
    let mut temporal = 0;
    for (item, indices) in &models.referenced_fact_indices {
        for (index, alternatives) in indices.iter().zip(&models.referenced_by_item[item]) {
            for (alternative_index, clauses) in alternatives.iter().enumerate() {
                let mut allow_ids = std::collections::BTreeSet::new();
                for (clause_index, clause) in clauses.iter().enumerate() {
                    let fact = analysis
                        .checked_clause_fact(*index, alternative_index, clause_index)
                        .unwrap();
                    assert_eq!(fact.source_clause, clause.source_clause());
                    assert!(fact.source_clause.is_some());
                    match clause {
                        TraceSpecClause::Allow { .. } => {
                            assert!(allow_ids.insert(fact.source_clause.unwrap()));
                            assert_eq!(
                                fact.disposition,
                                CheckedTraceClauseDisposition::AlternativePermission
                            );
                            permissions += 1;
                        }
                        TraceSpecClause::Deny { .. } => {
                            assert_eq!(fact.disposition, CheckedTraceClauseDisposition::Proven);
                            denials += 1;
                        }
                        TraceSpecClause::RequireBefore { .. } => {
                            assert_eq!(fact.disposition, CheckedTraceClauseDisposition::Proven);
                            temporal += 1;
                        }
                        _ => panic!("unexpected test clause"),
                    }
                }
            }
            assert!(
                analysis
                    .checked_clause_fact(*index, alternatives.len(), 0)
                    .is_err()
            );
            let mut changed = models.clone();
            changed
                .referenced_fact_indices
                .get_mut(item)
                .unwrap()
                .clear();
            let dispositions =
                etas_effects::trace_spec::collect_trace_static_dispositions(&analysis, None, None)
                    .unwrap();
            assert!(matches!(
                etas_effects::trace_spec::validate_trace_spec_facts(
                    &mut effects,
                    etas_effects::trace_spec::TraceSpecValidationInput {
                        external_summaries: &[],
                        external_implementations: &[],
                        hir,
                        types,
                        registry,
                        models: &changed,
                        analysis: &analysis,
                        dispositions: &dispositions,
                        residual_plans: &Default::default(),
                        reachable_items: None,
                        control: None,
                    },
                ),
                Err(etas_effects::EffectPipelineError::InvalidTraceSpecSourceSites { .. })
            ));
        }
    }
    assert_eq!((permissions, denials, temporal), (5, 2, 2));

    // This exercises source linkage only; scope, monitor coverage and runtime
    // installation remain separate checks.
    use etas_code::residual::{
        identity::{
            ActivationSiteId, BehaviorContractRef, BindingLayoutId, CheckedSourceSnapshot,
            MonitorTemplateId, ObligationIdentityInputs, ObligationIdentityLimits, ResidualOwnerId,
            ResidualRuleKind, SemanticScopeId, SourceStructuralSite, SpecInstanceId,
        },
        model::{ResidualKind, ResidualObligation},
    };
    use etas_effects::trace_spec::{
        TraceObligationSourceError, TraceObligationSourceInput, TraceObligationSourceLimits,
        validate_trace_obligation_source,
    };
    let (item, indices) = models.referenced_fact_indices.iter().next().unwrap();
    let index = indices[0];
    let clauses = &models.referenced_by_item[item][0][0];
    let clause_index = clauses
        .iter()
        .position(|clause| matches!(clause, TraceSpecClause::Deny { .. }))
        .unwrap();
    let source = analysis.source_conformance(index).unwrap();
    let owner = source.source_owner.as_ref().unwrap();
    let spec = source.source_instance.as_ref().unwrap();
    let clause_id = clauses[clause_index].source_clause().unwrap();
    let site = &models.source_clause_sites[&clause_id];
    let checked_snapshot = CheckedSourceSnapshot::new(key, text.as_bytes());
    let identity_limits = ObligationIdentityLimits {
        max_generic_arguments: 16,
        max_structural_site_depth: 32,
    };
    let scope_site = SourceStructuralSite {
        location: owner.declaration(),
        source_bytes: text.as_bytes(),
        structural_path: &[],
    };
    let inputs = ObligationIdentityInputs {
        origin: site.location().clone(),
        owner: owner.id(),
        contract: BehaviorContractRef::SourceConstraint,
        spec: spec.id(),
        clause: clause_id,
        rule: site
            .rule(
                &checked_snapshot,
                ResidualRuleKind::TraceContract,
                &[],
                identity_limits,
            )
            .unwrap(),
        scope: SemanticScopeId::from_source_site(owner.id(), &scope_site, identity_limits).unwrap(),
        activation: ActivationSiteId::from_source_site(owner.id(), &scope_site, identity_limits)
            .unwrap(),
        static_bindings: Default::default(),
    };
    let kind = ResidualKind::TraceContract {
        monitor: MonitorTemplateId::from_canonical_fingerprint(CodeFingerprint::from_bytes(
            [50; 32],
        )),
    };
    let layout = BindingLayoutId::from_canonical_fingerprint(CodeFingerprint::from_bytes([51; 32]));
    let obligation =
        ResidualObligation::new(inputs.clone(), kind.clone(), layout, identity_limits).unwrap();
    let limits = TraceObligationSourceLimits {
        max_snapshots: 1,
        max_snapshot_bytes: text.len(),
        identity: identity_limits,
    };
    let validate = |obligation: &ResidualObligation,
                    snapshots: &[etas_effects::EffectSourceSnapshot<'_>],
                    suffix: &[u32],
                    limits| {
        validate_trace_obligation_source(TraceObligationSourceInput {
            analysis: &analysis,
            type_fact_index: index,
            alternative_index: 0,
            clause_index,
            obligation,
            clause_sites: &models.source_clause_sites,
            snapshots,
            rule_suffix: suffix,
            limits,
            control: None,
        })
    };
    assert_eq!(validate(&obligation, &snapshots, &[], limits), Ok(()));
    assert_eq!(
        validate(&obligation, &[], &[], limits),
        Err(TraceObligationSourceError::MissingSnapshot)
    );
    assert_eq!(
        validate(&obligation, &snapshots, &[0], limits),
        Err(TraceObligationSourceError::RuleMismatch)
    );
    assert_eq!(
        validate(
            &obligation,
            &snapshots,
            &[],
            TraceObligationSourceLimits {
                max_snapshot_bytes: text.len() - 1,
                ..limits
            }
        ),
        Err(TraceObligationSourceError::SnapshotBytesLimit)
    );
    let mut wrong = inputs.clone();
    wrong.origin = owner.declaration().clone();
    let wrong = ResidualObligation::new(wrong, kind.clone(), layout, identity_limits).unwrap();
    assert_eq!(
        validate(&wrong, &snapshots, &[], limits),
        Err(TraceObligationSourceError::OriginMismatch)
    );
    let mut wrong = inputs.clone();
    wrong.owner =
        ResidualOwnerId::from_canonical_fingerprint(CodeFingerprint::from_bytes([52; 32]));
    let wrong = ResidualObligation::new(wrong, kind.clone(), layout, identity_limits).unwrap();
    assert_eq!(
        validate(&wrong, &snapshots, &[], limits),
        Err(TraceObligationSourceError::OwnerMismatch)
    );
    let mut wrong = inputs.clone();
    wrong.spec = SpecInstanceId::from_canonical_fingerprint(CodeFingerprint::from_bytes([53; 32]));
    let wrong = ResidualObligation::new(wrong, kind.clone(), layout, identity_limits).unwrap();
    assert_eq!(
        validate(&wrong, &snapshots, &[], limits),
        Err(TraceObligationSourceError::SpecInstanceMismatch)
    );
    let mut wrong = inputs;
    wrong.clause = clauses
        .iter()
        .find_map(|clause| match clause {
            TraceSpecClause::Allow { source_clause, .. } => *source_clause,
            _ => None,
        })
        .unwrap();
    let wrong = ResidualObligation::new(wrong, kind, layout, identity_limits).unwrap();
    assert_eq!(
        validate(&wrong, &snapshots, &[], limits),
        Err(TraceObligationSourceError::ClauseMismatch)
    );
    let changed_text = text.replace("return;", "return ;");
    let changed = [etas_effects::EffectSourceSnapshot {
        source: SourceId(0),
        key,
        text: &changed_text,
    }];
    assert!(matches!(
        validate(
            &obligation,
            &changed,
            &[],
            TraceObligationSourceLimits {
                max_snapshot_bytes: changed_text.len(),
                ..limits
            }
        ),
        Err(TraceObligationSourceError::InvalidIdentity(_))
    ));
    let duplicate = [
        etas_effects::EffectSourceSnapshot {
            source: SourceId(0),
            key,
            text,
        },
        etas_effects::EffectSourceSnapshot {
            source: SourceId(1),
            key,
            text,
        },
    ];
    assert_eq!(
        validate(
            &obligation,
            &duplicate,
            &[],
            TraceObligationSourceLimits {
                max_snapshots: 2,
                max_snapshot_bytes: text.len() * 2,
                ..limits
            }
        ),
        Err(TraceObligationSourceError::DuplicateSnapshot)
    );
}

#[test]
fn checked_clause_fact_preserves_widened_trace_uncertainty() {
    use etas_effects::trace_spec::{CheckedTraceClauseDisposition, CheckedTraceUncertainty};

    let key = StableSourceKey::new(
        StableSourceOrigin::Project(CodeFingerprint::from_bytes([43; 32])),
        &["src".into(), "main.es".into()],
    )
    .unwrap();
    let text = "module app; effect Gate { action approve() -> unit; action request() -> unit; } spec Safe: trace = Gate.approve >> Gate.request; flow main(ready: bool) -> unit ![Gate.approve, Gate.request] ~ Safe { while ready limit Iterations(2) { perform Gate.approve(); perform Gate.request(); } }";
    let mut source = SourceInput::anonymous(text);
    source.id = SourceId(0);
    let mut input = ProjectInput::single_source(source);
    input.options.entry_policy = EntryPolicy::Optional;
    input
        .environment
        .stable_source_keys
        .push(ProjectStableSourceKeyInput {
            source: SourceId(0),
            key,
        });
    let output = Frontend.check_project(input);
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Effect(
                etas_core::EffectDiagnosticCode::IncompleteEffectFacts
            )),
        "{:?}",
        output.diagnostics
    );
    let hir = &output.hir.as_ref().unwrap().hir;
    let types = output.types.as_ref().unwrap();
    let artifacts = output.effect_pipeline_artifacts.as_ref().unwrap();
    let registry = &artifacts.registry;
    let snapshots = [etas_effects::EffectSourceSnapshot {
        source: SourceId(0),
        key,
        text,
    }];
    let mut effects = etas_effects::EffectOutput::default();
    let models = etas_effects::trace_spec::materialize_trace_spec_models(
        hir,
        types,
        registry,
        &[],
        &snapshots,
        &mut effects,
    )
    .unwrap();
    let checked_effects = output.effects.as_ref().unwrap();
    let trace_specs = std::mem::take(&mut effects.facts.trace_specs);
    effects.facts = checked_effects.facts.as_ref().clone();
    effects.facts.trace_specs = trace_specs;
    let analysis = etas_effects::trace_spec::analyze_trace_spec_monitors(
        etas_effects::trace_spec::TraceSpecAnalysisInput {
            source_snapshots: &snapshots,
            external_trace_specs: &[],
            hir,
            types,
            registry,
            models: &models,
            units: &artifacts.units,
            external_summaries: &[],
        },
        &mut effects,
    )
    .unwrap();
    let indices = models
        .referenced_fact_indices
        .values()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(indices.len(), 1);
    let fact = analysis.checked_clause_fact(indices[0], 0, 0).unwrap();
    assert!(fact.source_clause.is_some());
    assert_eq!(
        fact.disposition,
        CheckedTraceClauseDisposition::Unsupported(CheckedTraceUncertainty::OrderedActions)
    );
}

#[test]
fn checked_permission_facts_keep_or_alternatives_separate() {
    use etas_effects::trace_spec::{
        CheckedTracePermissionDisposition, CheckedTraceStaticDisposition,
    };

    let key = StableSourceKey::new(
        StableSourceOrigin::Project(CodeFingerprint::from_bytes([48; 32])),
        &["src".into(), "main.es".into()],
    )
    .unwrap();
    let text = "module app; effect Gate { action a() -> unit; action b() -> unit; } spec Safe: trace = +Gate.a | +Gate.b; flow main() -> unit ![Gate.a] ~ Safe { perform Gate.a(); }";
    let output = checked(SourceId(0), text, Some(key));
    let hir = &output.hir.as_ref().unwrap().hir;
    let types = output.types.as_ref().unwrap();
    let artifacts = output.effect_pipeline_artifacts.as_ref().unwrap();
    let snapshots = [etas_effects::EffectSourceSnapshot {
        source: SourceId(0),
        key,
        text,
    }];
    let mut effects = etas_effects::EffectOutput::default();
    let models = etas_effects::trace_spec::materialize_trace_spec_models(
        hir,
        types,
        &artifacts.registry,
        &[],
        &snapshots,
        &mut effects,
    )
    .unwrap();
    let trace_specs = std::mem::take(&mut effects.facts.trace_specs);
    effects.facts = output.effects.as_ref().unwrap().facts.as_ref().clone();
    effects.facts.trace_specs = trace_specs;
    let analysis = etas_effects::trace_spec::analyze_trace_spec_monitors(
        etas_effects::trace_spec::TraceSpecAnalysisInput {
            source_snapshots: &snapshots,
            external_trace_specs: &[],
            hir,
            types,
            registry: &artifacts.registry,
            models: &models,
            units: &artifacts.units,
            external_summaries: &[],
        },
        &mut effects,
    )
    .unwrap();
    let indices = models
        .referenced_fact_indices
        .values()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(indices.len(), 1);
    let first = analysis.checked_permission_fact(indices[0], 0).unwrap();
    let second = analysis.checked_permission_fact(indices[0], 1).unwrap();
    assert_eq!(
        first.disposition(),
        CheckedTracePermissionDisposition::Proven
    );
    assert_eq!(first.uncovered().len(), 0);
    assert_eq!(
        second.disposition(),
        CheckedTracePermissionDisposition::Violated
    );
    assert_eq!(second.uncovered().len(), 1);
    assert_eq!(
        second.uncovered().next().unwrap().coverage,
        etas_effects::trace_spec::CheckedPermissionCoverage::Uncovered
    );
    assert_eq!(
        analysis.checked_alternative_disposition(indices[0], 0, None),
        Ok(CheckedTraceStaticDisposition::Proven)
    );
    assert_eq!(
        analysis.checked_alternative_disposition(indices[0], 1, None),
        Ok(CheckedTraceStaticDisposition::Violated)
    );
    assert_eq!(
        analysis.checked_conformance_disposition(indices[0], None),
        Ok(CheckedTraceStaticDisposition::Proven)
    );
    assert!(analysis.checked_permission_fact(indices[0], 2).is_err());
    assert!(
        analysis
            .checked_alternative_disposition(indices[0], 2, None)
            .is_err()
    );
    struct Cancel;
    impl etas_utils::PipelineExecutionControl for Cancel {
        fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
            Err(etas_utils::PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::Cancelled,
                reason: "test cancellation".into(),
            })
        }
    }
    assert!(matches!(
        analysis.checked_conformance_disposition(indices[0], Some(&Cancel)),
        Err(etas_effects::EffectPipelineError::Interrupted(
            etas_utils::PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::Cancelled,
                ..
            }
        ))
    ));
}

#[test]
fn checked_permission_fact_retains_unknown_selector() {
    use etas_effects::trace_spec::{
        CheckedTracePermissionDisposition, CheckedTraceStaticDisposition,
    };

    let key = StableSourceKey::new(
        StableSourceOrigin::Project(CodeFingerprint::from_bytes([49; 32])),
        &["src".into(), "main.es".into()],
    )
    .unwrap();
    let text = "module app; type Restricted; effect Gate { action request<T>() -> unit; action other() -> unit; } spec Safe: trace = +Gate.request<Restricted> | +Gate.other; flow main() -> unit ![Gate.request<_>] ~ Safe { perform Gate.request(); }";
    let mut source = SourceInput::anonymous(text);
    source.id = SourceId(0);
    let mut input = ProjectInput::single_source(source);
    input.options.entry_policy = EntryPolicy::Optional;
    input
        .environment
        .stable_source_keys
        .push(ProjectStableSourceKeyInput {
            source: SourceId(0),
            key,
        });
    let output = Frontend.check_project(input);
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Effect(
                etas_core::EffectDiagnosticCode::IncompleteEffectFacts
            )),
        "{:?}",
        output.diagnostics
    );
    let hir = &output.hir.as_ref().unwrap().hir;
    let types = output.types.as_ref().unwrap();
    let artifacts = output.effect_pipeline_artifacts.as_ref().unwrap();
    let snapshots = [etas_effects::EffectSourceSnapshot {
        source: SourceId(0),
        key,
        text,
    }];
    let mut effects = etas_effects::EffectOutput::default();
    let models = etas_effects::trace_spec::materialize_trace_spec_models(
        hir,
        types,
        &artifacts.registry,
        &[],
        &snapshots,
        &mut effects,
    )
    .unwrap();
    let trace_specs = std::mem::take(&mut effects.facts.trace_specs);
    effects.facts = output.effects.as_ref().unwrap().facts.as_ref().clone();
    effects.facts.trace_specs = trace_specs;
    let analysis = etas_effects::trace_spec::analyze_trace_spec_monitors(
        etas_effects::trace_spec::TraceSpecAnalysisInput {
            source_snapshots: &snapshots,
            external_trace_specs: &[],
            hir,
            types,
            registry: &artifacts.registry,
            models: &models,
            units: &artifacts.units,
            external_summaries: &[],
        },
        &mut effects,
    )
    .unwrap();
    let indices = models
        .referenced_fact_indices
        .values()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(indices.len(), 1);
    let permission = analysis.checked_permission_fact(indices[0], 0).unwrap();
    assert_eq!(
        permission.disposition(),
        CheckedTracePermissionDisposition::Unsupported
    );
    assert_eq!(permission.uncovered().len(), 1);
    assert_eq!(
        permission.uncovered().next().unwrap().coverage,
        etas_effects::trace_spec::CheckedPermissionCoverage::Undetermined
    );
    assert_eq!(
        analysis.checked_conformance_disposition(indices[0], None),
        Ok(CheckedTraceStaticDisposition::Unsupported)
    );
    assert_eq!(
        analysis.checked_alternative_disposition(indices[0], 1, None),
        Ok(CheckedTraceStaticDisposition::Violated)
    );
}

#[test]
fn agent_trace_conformance_preserves_declared_and_callable_owners() {
    let key = StableSourceKey::new(
        StableSourceOrigin::Project(CodeFingerprint::from_bytes([44; 32])),
        &["src".into(), "main.es".into()],
    )
    .unwrap();
    for declaration in [
        "agent Worker() -> i32 ~ Quiet { return 1; }",
        "agent Worker; impl agent Worker ~ Quiet { flow run() -> i32 { return 1; } }",
        "agent Worker() -> i32 ~ Quiet { return 1; } impl agent Worker { flow helper() -> unit { return; } }",
    ] {
        let text = format!(
            "module app; spec Quiet: trace = -Console.stdout_write; {declaration} flow main() -> unit {{ return; }}"
        );
        let output = checked(SourceId(0), &text, Some(key));
        let hir = &output.hir.as_ref().unwrap().hir;
        let types = output.types.as_ref().unwrap();
        let artifacts = output.effect_pipeline_artifacts.as_ref().unwrap();
        let snapshots = [etas_effects::EffectSourceSnapshot {
            source: SourceId(0),
            key,
            text: &text,
        }];
        let mut effects = etas_effects::EffectOutput::default();
        let models = etas_effects::trace_spec::materialize_trace_spec_models(
            hir,
            types,
            &artifacts.registry,
            &[],
            &snapshots,
            &mut effects,
        )
        .unwrap();
        assert!(!models.referenced_fact_indices.is_empty(), "{declaration}");
        let checked_effects = output.effects.as_ref().unwrap();
        let trace_specs = std::mem::take(&mut effects.facts.trace_specs);
        effects.facts = checked_effects.facts.as_ref().clone();
        effects.facts.trace_specs = trace_specs;
        let analysis = etas_effects::trace_spec::analyze_trace_spec_monitors(
            etas_effects::trace_spec::TraceSpecAnalysisInput {
                source_snapshots: &snapshots,
                external_trace_specs: &[],
                hir,
                types,
                registry: &artifacts.registry,
                models: &models,
                units: &artifacts.units,
                external_summaries: &[],
            },
            &mut effects,
        )
        .unwrap();
        for (owner, indices) in &models.referenced_fact_indices {
            for index in indices {
                let conformance = analysis.source_conformance(*index).unwrap();
                assert_eq!(conformance.item, *owner);
                assert!(conformance.source_instance.is_some());
                assert!(conformance.source_owner.is_some());
                assert_eq!(
                    analysis
                        .checked_clause_fact(*index, 0, 0)
                        .unwrap()
                        .disposition,
                    etas_effects::trace_spec::CheckedTraceClauseDisposition::Proven
                );
            }
        }
    }
}

#[test]
fn inline_conformance_cannot_discard_its_source_identity() {
    let key = StableSourceKey::new(
        StableSourceOrigin::Project(CodeFingerprint::from_bytes([82; 32])),
        &["src".into(), "main.es".into()],
    )
    .unwrap();
    let text = "module app; effect Gate { action a() -> unit; } flow main() -> unit ![] ~ (-Gate.a) { return; }";
    let output = checked(SourceId(0), text, Some(key));
    let snapshots = [etas_effects::EffectSourceSnapshot {
        source: SourceId(0),
        key,
        text,
    }];
    let hir = &output.hir.as_ref().unwrap().hir;
    let types = output.types.as_ref().unwrap();
    let registry = &output.effect_pipeline_artifacts.as_ref().unwrap().registry;
    let mut effects = etas_effects::EffectOutput::default();
    let mut models = etas_effects::trace_spec::materialize_trace_spec_models(
        hir,
        output.types.as_ref().unwrap(),
        &output.effect_pipeline_artifacts.as_ref().unwrap().registry,
        &[],
        &snapshots,
        &mut effects,
    )
    .unwrap();
    assert!(
        models
            .validate_source_sites(hir, types, registry, &[], &snapshots)
            .is_ok()
    );
    let mut changed = models.clone();
    let clause = changed
        .referenced_by_item
        .values_mut()
        .flatten()
        .flatten()
        .flatten()
        .next()
        .unwrap();
    let etas_effects::trace_spec::TraceSpecClause::Deny { pattern, .. } = clause else {
        panic!("expected inline denial")
    };
    pattern.row.effects = etas_effects::EffectSet::default();
    assert!(
        changed
            .validate_source_sites(hir, types, registry, &[], &snapshots)
            .is_err()
    );
    let clause = models
        .referenced_by_item
        .values_mut()
        .flatten()
        .flatten()
        .flatten()
        .next()
        .unwrap();
    let etas_effects::trace_spec::TraceSpecClause::Deny {
        source_path,
        source_clause,
        ..
    } = clause
    else {
        panic!("expected inline denial")
    };
    *source_path = None;
    *source_clause = None;
    models.source_clause_sites.clear();
    assert!(
        models
            .validate_source_sites(hir, types, registry, &[], &snapshots)
            .is_err()
    );
}

#[test]
fn source_spec_instances_are_stable_and_require_source_identity() {
    let key = StableSourceKey::new(
        StableSourceOrigin::Project(CodeFingerprint::from_bytes([82; 32])),
        &["src".into(), "main.es".into()],
    )
    .unwrap();
    let text = "module app; effect Gate { action a() -> unit; } spec Safe: trace = +Gate.a; flow main() -> unit ![] ~ Safe { return; }";
    let instance = |source, text: &str, key| {
        checked(source, text, key)
            .effects
            .unwrap()
            .facts
            .trace_specs
            .conformances
            .values()
            .flatten()
            .next()
            .unwrap()
            .source_spec_instance
            .as_ref()
            .map(|instance| instance.id())
    };
    let first = instance(SourceId(0), text, Some(key));
    assert!(first.is_some());
    assert_eq!(first, instance(SourceId(17), text, Some(key)));
    assert_ne!(
        first,
        instance(SourceId(0), &format!("{text}\n"), Some(key))
    );
    assert_eq!(instance(SourceId(0), text, None), None);
    let inline = "module app; effect Gate { action a() -> unit; } flow main() -> unit ![] ~ (-Gate.a) { return; }";
    assert!(instance(SourceId(0), inline, Some(key)).is_some());
    assert_eq!(
        instance(SourceId(0), inline, Some(key)),
        instance(SourceId(17), inline, Some(key))
    );
    let generic = "module app; effect Probe { action read<T>(value: T) -> unit; } spec NoProbe<R>: trace = -Probe.read<R>; flow main() -> unit ![] ~ NoProbe<i32> { return; }";
    let generic_id = instance(SourceId(0), generic, Some(key));
    assert!(generic_id.is_some());
    assert_eq!(generic_id, instance(SourceId(17), generic, Some(key)));
    let symbolic = "module app; effect Probe { action read<T>(value: T) -> unit; } spec NoProbe<R>: trace = -Probe.read<R>; flow check<T>() -> unit ![] ~ NoProbe<T> { return; } flow main() -> unit { return; }";
    let symbolic_id = instance(SourceId(0), symbolic, Some(key));
    assert!(symbolic_id.is_some());
    assert_eq!(symbolic_id, instance(SourceId(17), symbolic, Some(key)));
    let agent_impl = "module app; effect Probe { action read<T>(value: T) -> unit; } spec NoProbe<R>: trace = -Probe.read<R>; agent Worker; impl agent Worker ~ NoProbe<i32> { flow run() -> i32 { return 1; } } flow main() -> unit { return; }";
    let impl_id = instance(SourceId(0), agent_impl, Some(key));
    assert!(impl_id.is_some());
    assert_eq!(impl_id, instance(SourceId(17), agent_impl, Some(key)));
    assert_eq!(instance(SourceId(0), agent_impl, None), None);
}

#[test]
fn agent_impl_trace_instances_preserve_distinct_checked_generic_arguments() {
    let key = StableSourceKey::new(
        StableSourceOrigin::Project(CodeFingerprint::from_bytes([83; 32])),
        &["src".into(), "main.es".into()],
    )
    .unwrap();
    let source = "module app; effect Probe { action read<T>(value: T) -> unit; } spec NoProbe<R>: trace = -Probe.read<R>; agent First; impl agent First ~ NoProbe<i32> { flow run() -> i32 { return 1; } } agent Second; impl agent Second ~ NoProbe<bool> { flow run() -> i32 { return 2; } } agent Third; impl agent Third ~ NoProbe<Array<i32>> { flow run() -> i32 { return 3; } } flow main() -> unit { return; }";
    let identities = |source_id| {
        checked(source_id, source, Some(key))
            .effects
            .unwrap()
            .facts
            .trace_specs
            .conformances
            .values()
            .flatten()
            .map(|fact| {
                fact.source_spec_instance
                    .as_ref()
                    .expect("checked source spec identity")
                    .id()
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    let first = identities(SourceId(0));
    assert_eq!(first.len(), 3);
    assert_eq!(first, identities(SourceId(17)));
}

#[test]
fn local_clause_ids_follow_source_identity_not_session_ids_or_expansion_order() {
    let key = StableSourceKey::new(
        StableSourceOrigin::Project(CodeFingerprint::from_bytes([82; 32])),
        &["src".into(), "main.es".into()],
    )
    .unwrap();
    let text = "module app; effect Gate { action a() -> unit; action b() -> unit; action c() -> unit; } spec Safe: trace = (+Gate.a | +Gate.b) & -Gate.c; flow main() -> unit ![] ~ Safe { return; }";
    let first = clauses(SourceId(0), text, Some(key));
    let second = clauses(SourceId(17), text, Some(key));
    assert_eq!(first, second);
    assert!(first.iter().flatten().all(Option::is_some));
    assert_ne!(first[0][0], first[1][0]);
    assert_eq!(first[0][1], first[1][1]);
    let edited = clauses(SourceId(0), &format!("{text}\n"), Some(key));
    assert_ne!(first[0][0], edited[0][0]);
    assert!(
        clauses(SourceId(0), text, None)
            .iter()
            .flatten()
            .all(Option::is_none)
    );
}

#[test]
fn incremental_check_recomputes_clause_ids_when_source_identity_changes() {
    let mut source = SourceInput::anonymous(
        "module app; effect Gate { action a() -> unit; } spec Safe: trace = +Gate.a; flow main() -> unit ![] ~ Safe { return; }",
    );
    source.id = SourceId(0);
    let mut input = ProjectInput::single_source(source);
    input.options.entry_policy = EntryPolicy::Optional;
    let make_key = |byte| {
        StableSourceKey::new(
            StableSourceOrigin::Project(CodeFingerprint::from_bytes([byte; 32])),
            &["src".into(), "main.es".into()],
        )
        .unwrap()
    };
    input
        .environment
        .stable_source_keys
        .push(ProjectStableSourceKeyInput {
            source: SourceId(0),
            key: make_key(82),
        });
    let mut environment = input.environment.clone();
    let mut session = FrontendSession::new();
    let project = session.open_project(input);
    let first = session
        .check(project, CheckRequest::default())
        .unwrap()
        .output;
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let clause_id = |output: &etas_frontend::ProjectOutput| {
        output
            .effects
            .as_ref()
            .unwrap()
            .facts
            .trace_specs
            .conformances
            .values()
            .flatten()
            .next()
            .unwrap()
            .alternatives[0][0]
            .source_clause()
            .unwrap()
    };
    let first_id = clause_id(&first);
    environment.stable_source_keys[0].key = make_key(83);
    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![],
                dependency_overlay_changes: vec![],
                environment_change: Some(EnvironmentChange::Replace(Box::new(environment))),
                option_changes: vec![],
            },
        )
        .unwrap();
    let second = session
        .check(project, CheckRequest::incremental())
        .unwrap()
        .output;
    assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    let second_id = clause_id(&second);
    assert_ne!(first_id, second_id);
    let sites = &second
        .checked
        .as_ref()
        .unwrap()
        .effects
        .trace_specs
        .source_clause_sites;
    assert!(sites.contains_key(&second_id));
    assert!(!sites.contains_key(&first_id));
}

#[test]
fn generic_spec_instances_preserve_canonical_arguments_and_symbolic_owners() {
    let key = StableSourceKey::new(
        StableSourceOrigin::Project(CodeFingerprint::from_bytes([82; 32])),
        &["src".into(), "main.es".into()],
    )
    .unwrap();
    let text = "module app; effect Probe { action read<T>(value: T) -> unit; } spec NoProbe<R>: trace = -Probe.read<R>; flow first() -> unit ![] ~ NoProbe<i32> { return; } flow same() -> unit ![] ~ NoProbe<i32> { return; } flow different() -> unit ![] ~ NoProbe<bool> { return; } flow left<T>() -> unit ![] ~ NoProbe<T> { return; } flow right<T>() -> unit ![] ~ NoProbe<T> { return; } flow main() -> unit { return; }";
    let project = checked(SourceId(0), text, Some(key));
    let hir = &project.hir.as_ref().unwrap().hir;
    let facts = &project
        .effects
        .as_ref()
        .unwrap()
        .facts
        .trace_specs
        .conformances;
    let ids = facts
        .iter()
        .map(|(item, conformances)| {
            let symbol = hir.items.get(*item).unwrap().symbol().unwrap();
            (
                hir.symbols.get(symbol).unwrap().name.clone(),
                conformances[0].source_spec_instance.as_ref().unwrap().id(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(ids["first"], ids["same"]);
    assert_ne!(ids["first"], ids["different"]);
    assert_ne!(ids["left"], ids["right"]);
    assert_ne!(ids["first"], ids["left"]);
    use etas_code::{
        descriptor::{
            DescriptorGraph, DescriptorGraphInput, DescriptorLimits, GenericParameterKind,
            PrimitiveInfo, TypeInfo, TypeInfoRef,
        },
        residual::identity::{
            CheckedSourceSnapshot, CheckedSourceStructuralSite, GenericParameterId,
            ObligationIdentityLimits, ResidualOwnerId, ResidualOwnerKind, SpecInstanceId,
            StaticBindingRecipe,
        },
    };
    let declaration = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            etas_hir::HirItem::Spec(spec) => Some(spec),
            _ => None,
        })
        .unwrap();
    let snapshot = CheckedSourceSnapshot::new(key, text.as_bytes());
    let location = snapshot
        .location(
            u64::from(declaration.span.range.start.0),
            u64::from(declaration.span.range.end.0),
        )
        .unwrap();
    let site = CheckedSourceStructuralSite {
        location: &location,
        snapshot: &snapshot,
        structural_path: &[],
    };
    let limits = ObligationIdentityLimits {
        max_generic_arguments: 1024,
        max_structural_site_depth: 64,
    };
    let graph = DescriptorGraph::new(
        DescriptorGraphInput {
            types: vec![TypeInfo::Primitive(PrimitiveInfo::I32)],
            ..Default::default()
        },
        DescriptorLimits::default(),
    )
    .unwrap();
    let primitive =
        StaticBindingRecipe::ExplicitType(graph.type_fingerprint(TypeInfoRef(0)).unwrap());
    assert_eq!(
        ids["first"],
        SpecInstanceId::from_checked_source_declaration(
            &site,
            std::slice::from_ref(&primitive),
            limits
        )
        .unwrap()
    );
    let first_instance = facts
        .iter()
        .find_map(|(item, values)| {
            let symbol = hir.items.get(*item)?.symbol()?;
            (hir.symbols.get(symbol)?.name == "first")
                .then_some(values[0].source_spec_instance.as_ref()?)
        })
        .unwrap();
    assert_eq!(first_instance.arguments(), &[primitive]);
    assert_eq!(first_instance.declaration(), &location);
    assert!(first_instance.validate_snapshot(&snapshot, limits).is_ok());
    let left = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if hir
                    .symbols
                    .get(flow.symbol)
                    .is_some_and(|symbol| symbol.name == "left") =>
            {
                Some(flow)
            }
            _ => None,
        })
        .unwrap();
    let owner_location = snapshot
        .location(
            u64::from(left.span.range.start.0),
            u64::from(left.span.range.end.0),
        )
        .unwrap();
    let owner = ResidualOwnerId::from_source_declaration(
        &owner_location,
        text.as_bytes(),
        ResidualOwnerKind::Flow,
    )
    .unwrap();
    let symbolic = StaticBindingRecipe::TypeParameter(GenericParameterId::from_source_owner(
        owner,
        0,
        GenericParameterKind::Type,
    ));
    assert_eq!(
        ids["left"],
        SpecInstanceId::from_checked_source_declaration(
            &site,
            std::slice::from_ref(&symbolic),
            limits
        )
        .unwrap()
    );
    let symbolic_instance = facts
        .iter()
        .find_map(|(item, values)| {
            let symbol = hir.items.get(*item)?.symbol()?;
            (hir.symbols.get(symbol)?.name == "left")
                .then_some(values[0].source_spec_instance.as_ref()?)
        })
        .unwrap();
    assert_eq!(symbolic_instance.arguments(), &[symbolic]);
}

#[test]
fn imported_source_spec_alias_uses_the_declaration_identity() {
    let app_text = "module app; import lib.safe.Safe as S; import lib.safe.Safe as Direct; flow first() -> unit ![] ~ S { return; } flow main() -> unit ![] ~ Direct { return; }";
    let lib_text = "module lib.safe; public spec Safe: trace = -Console.stdout_write;";
    let app = SourceInput::anonymous(app_text);
    let mut lib = SourceInput::anonymous(lib_text);
    lib.id = SourceId(1);
    let mut input = ProjectInput::single_source(app);
    input.options.entry_policy = EntryPolicy::Optional;
    input.sources.push(lib);
    let origin = StableSourceOrigin::Project(CodeFingerprint::from_bytes([82; 32]));
    for (source, name) in [(SourceId(0), "main.es"), (SourceId(1), "safe.es")] {
        input
            .environment
            .stable_source_keys
            .push(ProjectStableSourceKeyInput {
                source,
                key: StableSourceKey::new(origin, &["src".into(), name.into()]).unwrap(),
            });
    }
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ids = output
        .effects
        .as_ref()
        .unwrap()
        .facts
        .trace_specs
        .conformances
        .values()
        .flatten()
        .map(|fact| fact.source_spec_instance.as_ref().unwrap().id())
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], ids[1]);

    use etas_code::residual::{
        identity::{
            ActivationSiteId, BehaviorContractRef, BindingLayoutId, CheckedSourceSnapshot,
            MonitorTemplateId, ObligationIdentityInputs, ObligationIdentityLimits,
            ResidualRuleKind, SemanticScopeId, SourceStructuralSite,
        },
        model::{ResidualKind, ResidualObligation},
    };
    use etas_effects::trace_spec::{
        TraceObligationSourceError, TraceObligationSourceInput, TraceObligationSourceLimits,
        validate_trace_obligation_source,
    };
    let app_key = StableSourceKey::new(origin, &["src".into(), "main.es".into()]).unwrap();
    let lib_key = StableSourceKey::new(origin, &["src".into(), "safe.es".into()]).unwrap();
    let snapshots = [
        etas_effects::EffectSourceSnapshot {
            source: SourceId(0),
            key: app_key,
            text: app_text,
        },
        etas_effects::EffectSourceSnapshot {
            source: SourceId(1),
            key: lib_key,
            text: lib_text,
        },
    ];
    let hir = &output.hir.as_ref().unwrap().hir;
    let types = output.types.as_ref().unwrap();
    let artifacts = output.effect_pipeline_artifacts.as_ref().unwrap();
    let mut effects = etas_effects::EffectOutput::default();
    let models = etas_effects::trace_spec::materialize_trace_spec_models(
        hir,
        types,
        &artifacts.registry,
        &[],
        &snapshots,
        &mut effects,
    )
    .unwrap();
    let trace_specs = std::mem::take(&mut effects.facts.trace_specs);
    effects.facts = output.effects.as_ref().unwrap().facts.as_ref().clone();
    effects.facts.trace_specs = trace_specs;
    let analysis = etas_effects::trace_spec::analyze_trace_spec_monitors(
        etas_effects::trace_spec::TraceSpecAnalysisInput {
            source_snapshots: &snapshots,
            external_trace_specs: &[],
            hir,
            types,
            registry: &artifacts.registry,
            models: &models,
            units: &artifacts.units,
            external_summaries: &[],
        },
        &mut effects,
    )
    .unwrap();
    let index = *models
        .referenced_fact_indices
        .values()
        .next()
        .unwrap()
        .first()
        .unwrap();
    let source = analysis.source_conformance(index).unwrap();
    let owner = source.source_owner.as_ref().unwrap();
    let spec = source.source_instance.as_ref().unwrap();
    let clause = source.clause_ids[0][0].unwrap();
    let site = &models.source_clause_sites[&clause];
    assert_eq!(owner.declaration().source, app_key);
    assert_eq!(spec.declaration().source, lib_key);
    assert_eq!(site.location().source, lib_key);
    let identity_limits = ObligationIdentityLimits {
        max_generic_arguments: 4,
        max_structural_site_depth: 16,
    };
    let owner_site = SourceStructuralSite {
        location: owner.declaration(),
        source_bytes: app_text.as_bytes(),
        structural_path: &[],
    };
    let lib_snapshot = CheckedSourceSnapshot::new(lib_key, lib_text.as_bytes());
    let obligation = ResidualObligation::new(
        ObligationIdentityInputs {
            origin: site.location().clone(),
            owner: owner.id(),
            contract: BehaviorContractRef::SourceConstraint,
            spec: spec.id(),
            clause,
            rule: site
                .rule(
                    &lib_snapshot,
                    ResidualRuleKind::TraceContract,
                    &[],
                    identity_limits,
                )
                .unwrap(),
            scope: SemanticScopeId::from_source_site(owner.id(), &owner_site, identity_limits)
                .unwrap(),
            activation: ActivationSiteId::from_source_site(
                owner.id(),
                &owner_site,
                identity_limits,
            )
            .unwrap(),
            static_bindings: Default::default(),
        },
        ResidualKind::TraceContract {
            monitor: MonitorTemplateId::from_canonical_fingerprint(CodeFingerprint::from_bytes(
                [54; 32],
            )),
        },
        BindingLayoutId::from_canonical_fingerprint(CodeFingerprint::from_bytes([55; 32])),
        identity_limits,
    )
    .unwrap();
    let limits = TraceObligationSourceLimits {
        max_snapshots: 2,
        max_snapshot_bytes: app_text.len() + lib_text.len(),
        identity: identity_limits,
    };
    let validate = |snapshots: &[etas_effects::EffectSourceSnapshot<'_>]| {
        validate_trace_obligation_source(TraceObligationSourceInput {
            analysis: &analysis,
            type_fact_index: index,
            alternative_index: 0,
            clause_index: 0,
            obligation: &obligation,
            clause_sites: &models.source_clause_sites,
            snapshots,
            rule_suffix: &[],
            limits,
            control: None,
        })
    };
    assert_eq!(validate(&snapshots), Ok(()));
    assert_eq!(
        validate(&snapshots[..1]),
        Err(TraceObligationSourceError::MissingSnapshot)
    );
    assert_eq!(
        validate(&snapshots[1..]),
        Err(TraceObligationSourceError::MissingSnapshot)
    );
}

#[test]
fn imported_generic_spec_identity_is_shared_by_flow_and_agent_impl_aliases() {
    let app_text = "module app; import lib.safe.Safe as S; import lib.safe.Safe as Direct; import lib.bridge.Exported as E; flow first() -> unit ![] ~ S<i32> { return; } agent Worker; impl agent Worker ~ Direct<i32> { flow run() -> i32 { return 1; } } flow through() -> unit ![] ~ E<i32> { return; } flow different() -> unit ![] ~ S<bool> { return; } flow main() -> unit { return; }";
    let lib_text = "module lib.safe; public spec Safe<R>: trace = -Console.stdout_write;";
    let bridge_text = "module lib.bridge; public import lib.safe.Safe as Exported;";
    let app = SourceInput::anonymous(app_text);
    let mut lib = SourceInput::anonymous(lib_text);
    lib.id = SourceId(1);
    let mut bridge = SourceInput::anonymous(bridge_text);
    bridge.id = SourceId(2);
    let mut input = ProjectInput::single_source(app);
    input.options.entry_policy = EntryPolicy::Optional;
    input.sources.push(lib);
    input.sources.push(bridge);
    let origin = StableSourceOrigin::Project(CodeFingerprint::from_bytes([82; 32]));
    for (source, name) in [
        (SourceId(0), "main.es"),
        (SourceId(1), "safe.es"),
        (SourceId(2), "bridge.es"),
    ] {
        input
            .environment
            .stable_source_keys
            .push(ProjectStableSourceKeyInput {
                source,
                key: StableSourceKey::new(origin, &["src".into(), name.into()]).unwrap(),
            });
    }
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let hir = &output.hir.as_ref().unwrap().hir;
    let identities = output
        .effects
        .as_ref()
        .unwrap()
        .facts
        .trace_specs
        .conformances
        .iter()
        .map(|(item, facts)| {
            let symbol = hir.items.get(*item).unwrap().symbol().unwrap();
            let name = hir.symbols.get(symbol).unwrap().name.clone();
            let instance = facts[0]
                .source_spec_instance
                .as_ref()
                .unwrap_or_else(|| panic!("missing spec identity for {name}"));
            (name, instance.id())
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(identities["first"], identities["run"]);
    assert_eq!(identities["first"], identities["through"]);
    assert_ne!(identities["first"], identities["different"]);
    let (first_item, first) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow) if hir.symbols.get(flow.symbol)?.name == "first" => {
                Some((id, flow))
            }
            _ => None,
        })
        .unwrap();
    let etas_hir::HirDeclarationConformanceTarget::Path(reference) = &first.conformances[0].target
    else {
        panic!("expected named spec")
    };
    let etas_hir::ResolveResult::Resolved(alias) = reference.spec_path.resolution else {
        panic!("expected resolved import")
    };
    let snapshots = [
        (SourceId(0), "main.es", app_text),
        (SourceId(1), "safe.es", lib_text),
        (SourceId(2), "bridge.es", bridge_text),
    ]
    .map(|(source, name, text)| etas_effects::EffectSourceSnapshot {
        source,
        key: StableSourceKey::new(origin, &["src".into(), name.into()]).unwrap(),
        text,
    });
    for wrong_target in [alias, first.symbol] {
        let mut altered = hir.program().clone();
        altered.source_import_targets.insert(alias, wrong_target);
        let mut effects = etas_effects::EffectOutput::default();
        let models = etas_effects::trace_spec::materialize_trace_spec_models(
            &altered,
            output.types.as_ref().unwrap(),
            &output.effect_pipeline_artifacts.as_ref().unwrap().registry,
            &[],
            &snapshots,
            &mut effects,
        )
        .unwrap();
        assert!(
            effects.facts.trace_specs.conformances[&first_item][0]
                .source_spec_instance
                .is_none()
        );
        let revision = etas_hir::HirRevision::try_new(altered).unwrap();
        assert!(matches!(
            etas_effects::trace_spec::analyze_trace_spec_monitors(
                etas_effects::trace_spec::TraceSpecAnalysisInput {
                    source_snapshots: &snapshots, external_trace_specs: &[],
                    hir: &revision, types: output.types.as_ref().unwrap(),
                    registry: &output.effect_pipeline_artifacts.as_ref().unwrap().registry,
                    models: &models, units: &[], external_summaries: &[],
                }, &mut effects,
            ),
            Err(etas_effects::EffectPipelineError::InvalidTraceSpecSourceSites { reason })
                if reason.contains("resolved source reference")
        ));
    }
}

#[test]
fn generic_spec_accepts_closed_structural_type_binding_identity() {
    let key = StableSourceKey::new(
        StableSourceOrigin::Project(CodeFingerprint::from_bytes([82; 32])),
        &["src".into(), "main.es".into()],
    )
    .unwrap();
    let text = "module app; effect Probe { action read<T>(value: T) -> unit; } spec NoProbe<R>: trace = -Probe.read<R>; flow first() -> unit ![] ~ NoProbe<Array<i32>> { return; } flow main() -> unit { return; }";
    let output = checked(SourceId(0), text, Some(key));
    let hir = &output.hir.as_ref().unwrap().hir;
    let source_arg = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            etas_hir::HirItem::Flow(flow) if !flow.conformances.is_empty() => {
                match &flow.conformances[0].target {
                    etas_hir::HirDeclarationConformanceTarget::Path(path) => {
                        path.spec_args.first().copied()
                    }
                    _ => None,
                }
            }
            _ => None,
        })
        .unwrap();
    assert!(
        output
            .types
            .as_ref()
            .unwrap()
            .facts
            .type_refs
            .contains_key(&source_arg)
    );
    let actual = output
        .effects
        .as_ref()
        .unwrap()
        .facts
        .trace_specs
        .conformances
        .values()
        .flatten()
        .next()
        .unwrap()
        .source_spec_instance
        .as_ref()
        .unwrap()
        .id();
    use etas_code::{
        descriptor::{
            BuiltinTypeInfo, DescriptorGraph, DescriptorGraphInput, DescriptorLimits,
            PrimitiveInfo, TypeInfo, TypeInfoRef,
        },
        residual::identity::{
            CheckedSourceSnapshot, CheckedSourceStructuralSite, ObligationIdentityLimits,
            SpecInstanceId, StaticBindingRecipe,
        },
    };
    let graph = DescriptorGraph::new(
        DescriptorGraphInput {
            types: vec![
                TypeInfo::Builtin {
                    constructor: BuiltinTypeInfo::Array,
                    arguments: vec![TypeInfoRef(1)],
                },
                TypeInfo::Primitive(PrimitiveInfo::I32),
            ],
            ..Default::default()
        },
        DescriptorLimits::default(),
    )
    .unwrap();
    let spec = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            etas_hir::HirItem::Spec(spec) => Some(spec),
            _ => None,
        })
        .unwrap();
    let snapshot = CheckedSourceSnapshot::new(key, text.as_bytes());
    let location = snapshot
        .location(
            u64::from(spec.span.range.start.0),
            u64::from(spec.span.range.end.0),
        )
        .unwrap();
    let site = CheckedSourceStructuralSite {
        location: &location,
        snapshot: &snapshot,
        structural_path: &[],
    };
    let expected = SpecInstanceId::from_checked_source_declaration(
        &site,
        &[StaticBindingRecipe::ExplicitType(
            graph.type_fingerprint(TypeInfoRef(0)).unwrap(),
        )],
        ObligationIdentityLimits {
            max_generic_arguments: 1024,
            max_structural_site_depth: 64,
        },
    )
    .unwrap();
    assert_eq!(actual, expected);
    let unresolved = "module app; effect Probe { action read<T>(value: T) -> unit; } spec NoProbe<R>: trace = -Probe.read<R>; flow check<T>() -> unit ![] ~ NoProbe<Array<T>> { return; } flow main() -> unit { return; }";
    let output = checked(SourceId(0), unresolved, Some(key));
    let effects = output.effects.unwrap();
    let instance = effects
        .facts
        .trace_specs
        .conformances
        .values()
        .flatten()
        .next()
        .unwrap()
        .source_spec_instance
        .as_ref()
        .unwrap();
    let relocated = checked(SourceId(17), unresolved, Some(key));
    let relocated_id = relocated
        .effects
        .unwrap()
        .facts
        .trace_specs
        .conformances
        .values()
        .flatten()
        .next()
        .unwrap()
        .source_spec_instance
        .as_ref()
        .unwrap()
        .id();
    assert_eq!(instance.id(), relocated_id);
    let [StaticBindingRecipe::TypeTemplate(template)] = instance.arguments() else {
        panic!("expected symbolic structural recipe")
    };
    use etas_code::residual::type_template::{
        StaticTypeNode, StaticTypeTemplate, TypeTemplateLimits,
    };
    let [
        StaticTypeNode::Parameter(binder),
        StaticTypeNode::Builtin {
            constructor: BuiltinTypeInfo::Array,
            arguments,
        },
    ] = template.nodes()
    else {
        panic!("expected Array<T> recipe")
    };
    assert_eq!(arguments, &[0]);
    let integer = StaticTypeTemplate::new(
        vec![StaticTypeNode::Primitive(PrimitiveInfo::I32)],
        0,
        TypeTemplateLimits::default(),
    )
    .unwrap();
    let resolved = template
        .substitute(
            &[(*binder, integer)].into_iter().collect(),
            TypeTemplateLimits::default(),
        )
        .unwrap();
    assert_eq!(
        resolved
            .closed_descriptor_fingerprint(DescriptorLimits::default())
            .unwrap(),
        Some(graph.type_fingerprint(TypeInfoRef(0)).unwrap())
    );
    let snapshot = CheckedSourceSnapshot::new(key, unresolved.as_bytes());
    assert!(
        instance
            .validate_snapshot(
                &snapshot,
                ObligationIdentityLimits {
                    max_generic_arguments: 1024,
                    max_structural_site_depth: 64
                }
            )
            .is_ok()
    );
    let decoded: etas_code::residual::identity::SourceSpecInstance =
        serde_json::from_str(&serde_json::to_string(instance).unwrap()).unwrap();
    assert_eq!(&decoded, instance);
    assert!(
        decoded
            .validate_snapshot(
                &snapshot,
                ObligationIdentityLimits {
                    max_generic_arguments: 1024,
                    max_structural_site_depth: 64
                }
            )
            .is_ok()
    );
}
