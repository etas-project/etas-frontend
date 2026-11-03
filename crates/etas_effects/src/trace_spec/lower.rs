//! Lower checked trace clauses into the shared monitor representation.
//!
//! Pattern projection is supplied by the checked identity owner. This output
//! is descriptive: source linkage, coverage and installation remain required.

use etas_code::residual::{
    monitor::{
        AlternativeError, AlternativeLimits, AlternativeRule, CompiledTraceAlternatives,
        SymbolicTraceAlternatives, SymbolicTraceError, SymbolicTraceRule,
    },
    pattern::{ActionPattern, ActionPatternTemplate},
};

use super::{TraceSpecClause, TraceSpecClauseAlternatives, TraceSpecPattern};

#[derive(Debug)]
pub enum TraceRulesLoweringError {
    AlternativeLimit,
    ClauseLimit,
    PatternCount,
    PatternMismatch,
    UnsupportedLimit,
    Analysis(crate::EffectPipelineError),
    Monitor(AlternativeError),
    Symbolic(SymbolicTraceError),
    Interrupted(etas_utils::PipelineInterruption),
}

pub struct LoweredTraceRules {
    pub alternatives: Vec<Vec<AlternativeRule>>,
    pub monitor: CompiledTraceAlternatives,
}

#[derive(Clone)]
pub struct TracePatternProjection<'a> {
    pub source: &'a TraceSpecPattern,
    pub pattern: ActionPattern,
}

pub struct TracePatternTemplateProjection<'a> {
    pub source: &'a TraceSpecPattern,
    pub pattern: ActionPatternTemplate,
}

enum ProjectedRule<T> {
    Allow(T),
    Deny(T),
    Before { guard: T, target: T },
}

/// Checked analysis and its recorded classification are required before a
/// model can be used as an input to residual plan lowering. The action
/// projections still need independent canonical-identity verification.
#[derive(Clone, Copy)]
pub struct AnalyzedTraceLoweringInput<'a> {
    pub analysis: &'a super::TraceSpecAnalysisOutput,
    pub dispositions: &'a super::TraceStaticDispositions,
    pub type_fact_index: usize,
    pub item: etas_hir::HirItemId,
    pub limits: AlternativeLimits,
    pub control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
}

pub fn lower_analyzed_trace_rules(
    input: AnalyzedTraceLoweringInput<'_>,
    patterns: Vec<TracePatternProjection<'_>>,
) -> Result<LoweredTraceRules, TraceRulesLoweringError> {
    let source = input
        .dispositions
        .analyzed_lowering_source(
            input.type_fact_index,
            input.item,
            input.analysis,
            input.control,
        )
        .map_err(TraceRulesLoweringError::Analysis)?;
    lower_trace_rules(source, patterns, input.limits, input.control)
}

pub fn lower_analyzed_symbolic_trace_rules(
    input: AnalyzedTraceLoweringInput<'_>,
    patterns: Vec<TracePatternTemplateProjection<'_>>,
) -> Result<SymbolicTraceAlternatives, TraceRulesLoweringError> {
    let source = input
        .dispositions
        .analyzed_lowering_source(
            input.type_fact_index,
            input.item,
            input.analysis,
            input.control,
        )
        .map_err(TraceRulesLoweringError::Analysis)?;
    lower_symbolic_trace_rules(source, patterns, input.limits, input.control)
}

/// Patterns must correspond, in source order, to each clause's operands.
/// The caller must independently validate their canonical identities and the
/// source/model linkage before using this result as plan input.
pub fn lower_trace_rules(
    source: &TraceSpecClauseAlternatives,
    patterns: Vec<TracePatternProjection<'_>>,
    limits: AlternativeLimits,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<LoweredTraceRules, TraceRulesLoweringError> {
    let count = patterns.len();
    let alternatives = project_rules(
        source,
        count,
        patterns.into_iter().map(|p| (p.source, p.pattern)),
        limits,
        control,
    )?
    .into_iter()
    .map(|group| {
        group
            .into_iter()
            .map(|rule| match rule {
                ProjectedRule::Allow(pattern) => AlternativeRule::Allow(pattern),
                ProjectedRule::Deny(pattern) => AlternativeRule::Deny(pattern),
                ProjectedRule::Before { guard, target } => {
                    AlternativeRule::Before { guard, target }
                }
            })
            .collect()
    })
    .collect::<Vec<_>>();
    let monitor = CompiledTraceAlternatives::compile(&alternatives, limits)
        .map_err(TraceRulesLoweringError::Monitor)?;
    Ok(LoweredTraceRules {
        alternatives,
        monitor,
    })
}

/// Retains generic selectors and complete alternatives for later checked
/// application binding. Canonical projection and installation are separate.
pub fn lower_symbolic_trace_rules(
    source: &TraceSpecClauseAlternatives,
    patterns: Vec<TracePatternTemplateProjection<'_>>,
    limits: AlternativeLimits,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<SymbolicTraceAlternatives, TraceRulesLoweringError> {
    let count = patterns.len();
    let groups = project_rules(
        source,
        count,
        patterns.into_iter().map(|p| (p.source, p.pattern)),
        limits,
        control,
    )?
    .into_iter()
    .map(|group| {
        group
            .into_iter()
            .map(|rule| match rule {
                ProjectedRule::Allow(pattern) => SymbolicTraceRule::Allow(pattern),
                ProjectedRule::Deny(pattern) => SymbolicTraceRule::Deny(pattern),
                ProjectedRule::Before { guard, target } => {
                    SymbolicTraceRule::Before { guard, target }
                }
            })
            .collect()
    })
    .collect();
    SymbolicTraceAlternatives::new(groups, limits).map_err(TraceRulesLoweringError::Symbolic)
}

fn project_rules<'a, T>(
    source: &TraceSpecClauseAlternatives,
    pattern_count: usize,
    mut patterns: impl Iterator<Item = (&'a TraceSpecPattern, T)>,
    limits: AlternativeLimits,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<Vec<Vec<ProjectedRule<T>>>, TraceRulesLoweringError> {
    let ceiling = etas_code::residual::monitor::MAX_TRACE_TEMPLATE_NODES;
    if source.len() > limits.max_alternatives || source.len() > ceiling {
        return Err(TraceRulesLoweringError::AlternativeLimit);
    }
    if pattern_count > ceiling.saturating_mul(2)
        || pattern_count > limits.max_clauses.saturating_mul(2)
    {
        return Err(TraceRulesLoweringError::PatternCount);
    }
    let mut count = 0usize;
    let mut alternatives = Vec::with_capacity(source.len());
    for group in source {
        count = count
            .checked_add(group.len())
            .ok_or(TraceRulesLoweringError::ClauseLimit)?;
        if count > limits.max_clauses || count.checked_add(source.len()).is_none_or(|n| n > ceiling)
        {
            return Err(TraceRulesLoweringError::ClauseLimit);
        }
        let mut rules = Vec::with_capacity(group.len());
        for clause in group {
            if let Some(control) = control {
                control
                    .checkpoint()
                    .map_err(TraceRulesLoweringError::Interrupted)?;
            }
            let mut next = |expected: &TraceSpecPattern| {
                let (source, pattern) = patterns
                    .next()
                    .ok_or(TraceRulesLoweringError::PatternCount)?;
                if source != expected {
                    return Err(TraceRulesLoweringError::PatternMismatch);
                }
                Ok(pattern)
            };
            rules.push(match clause {
                TraceSpecClause::Allow { pattern, .. } => ProjectedRule::Allow(next(pattern)?),
                TraceSpecClause::Deny { pattern, .. } => ProjectedRule::Deny(next(pattern)?),
                TraceSpecClause::RequireBefore { guard, target, .. } => ProjectedRule::Before {
                    guard: next(guard)?,
                    target: next(target)?,
                },
                TraceSpecClause::RequireAfter {
                    target, obligation, ..
                } => ProjectedRule::Before {
                    guard: next(target)?,
                    target: next(obligation)?,
                },
                TraceSpecClause::Limit { .. } => {
                    return Err(TraceRulesLoweringError::UnsupportedLimit);
                }
            });
        }
        alternatives.push(rules);
    }
    if patterns.next().is_some() {
        return Err(TraceRulesLoweringError::PatternCount);
    }
    Ok(alternatives)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use etas_code::{
        CodeFingerprint, DefinitionKey, DefinitionKind, PackageKey,
        contract::CanonicalEffect,
        descriptor::EffectTargetInfo,
        residual::{monitor::ConcreteEvidenceLimits, pattern::ActionPatternLimits},
    };
    use etas_core::{SourceId, Span, TextRange, TextSize};

    pub(in crate::trace_spec) fn limits() -> AlternativeLimits {
        AlternativeLimits {
            evidence: ConcreteEvidenceLimits {
                pattern: ActionPatternLimits {
                    max_selectors: 4,
                    max_identity_segments: 8,
                    max_identity_bytes: 128,
                    max_literal_bytes: 128,
                },
                max_bindings: 8,
                max_value_bytes: 128,
            },
            max_alternatives: 4,
            max_clauses: 8,
            max_patterns: 8,
            max_checks_per_event: 32,
            max_product_work_per_event: 128,
        }
    }

    pub(in crate::trace_spec) fn pattern(name: &str) -> ActionPattern {
        let owner = DefinitionKey::new(
            PackageKey::from_canonical_fingerprint(CodeFingerprint::from_bytes([7; 32])),
            vec!["app".into()],
            vec!["Gate".into()],
            DefinitionKind::Effect,
        )
        .unwrap();
        ActionPattern::new(
            CanonicalEffect {
                target: EffectTargetInfo::Action {
                    effect: owner,
                    name: name.into(),
                },
                arguments: vec![],
            },
            limits().evidence.pattern,
        )
        .unwrap()
    }

    fn temporal(after: bool) -> TraceSpecClause {
        let row = crate::EffectRow::empty();
        let guard = super::super::TraceSpecPattern {
            row: row.clone(),
            label: "guard".into(),
            arg_bounds: vec![],
        };
        let target = super::super::TraceSpecPattern {
            row: row.clone(),
            label: "target".into(),
            arg_bounds: vec![],
        };
        let span = Span::new(SourceId(1), TextRange::new(TextSize(0), TextSize(1)));
        if after {
            TraceSpecClause::RequireAfter {
                target: guard,
                obligation: target,
                fact: crate::TraceSpecClauseFact::RequireAfter {
                    target: row.clone(),
                    target_label: String::new(),
                    obligation: row,
                    obligation_label: String::new(),
                },
                span,
                source_path: None,
                source_clause: None,
            }
        } else {
            TraceSpecClause::RequireBefore {
                guard,
                target,
                fact: crate::TraceSpecClauseFact::RequireBefore {
                    guard: row.clone(),
                    guard_label: String::new(),
                    target: row,
                    target_label: String::new(),
                },
                span,
                source_path: None,
                source_clause: None,
            }
        }
    }

    fn projections(source: &TraceSpecClauseAlternatives) -> Vec<TracePatternProjection<'_>> {
        source
            .iter()
            .flatten()
            .flat_map(|clause| {
                let (guard, target) = match clause {
                    TraceSpecClause::RequireBefore { guard, target, .. } => (guard, target),
                    TraceSpecClause::RequireAfter {
                        target, obligation, ..
                    } => (target, obligation),
                    _ => panic!("expected temporal clause"),
                };
                [
                    TracePatternProjection {
                        source: guard,
                        pattern: pattern("check"),
                    },
                    TracePatternProjection {
                        source: target,
                        pattern: pattern("send"),
                    },
                ]
            })
            .collect()
    }

    #[test]
    fn lowers_complete_alternatives_and_preserves_prior_direction() {
        let source = vec![vec![temporal(false)], vec![temporal(true)]];
        let guard = pattern("check");
        let target = pattern("send");
        let lowered = lower_trace_rules(&source, projections(&source), limits(), None).unwrap();
        assert_eq!(lowered.alternatives.len(), 2);
        for group in lowered.alternatives {
            assert!(
                matches!(&group[..], [AlternativeRule::Before { guard: actual_guard, target: actual_target }]
                if actual_guard.id() == guard.id() && actual_target.id() == target.id())
            );
        }
    }

    #[test]
    fn rejects_missing_extra_and_cancelled_pattern_lowering() {
        struct Stop;
        impl etas_utils::PipelineExecutionControl for Stop {
            fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
                Err(etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    reason: "cancel trace lowering".into(),
                })
            }
        }
        let source = vec![vec![temporal(false)]];
        let patterns = projections(&source);
        assert!(matches!(
            lower_trace_rules(&source, patterns[..1].to_vec(), limits(), None),
            Err(TraceRulesLoweringError::PatternCount)
        ));
        let mut extra = patterns.clone();
        extra.push(patterns[0].clone());
        assert!(matches!(
            lower_trace_rules(&source, extra, limits(), None),
            Err(TraceRulesLoweringError::PatternCount)
        ));
        assert!(matches!(
            lower_trace_rules(
                &source,
                vec![patterns[1].clone(), patterns[0].clone()],
                limits(),
                None
            ),
            Err(TraceRulesLoweringError::PatternMismatch)
        ));
        assert!(matches!(
            lower_trace_rules(&source, patterns, limits(), Some(&Stop)),
            Err(TraceRulesLoweringError::Interrupted(_))
        ));
        let unsupported = vec![vec![TraceSpecClause::Limit {
            fact: crate::TraceSpecClauseFact::Limit { count: 1 },
        }]];
        assert!(matches!(
            lower_trace_rules(&unsupported, vec![], limits(), None),
            Err(TraceRulesLoweringError::UnsupportedLimit)
        ));
        assert!(matches!(
            lower_trace_rules(&vec![vec![]], vec![], limits(), None),
            Err(TraceRulesLoweringError::Monitor(
                AlternativeError::EmptyAlternative
            ))
        ));
    }

    #[test]
    fn symbolic_and_concrete_lowering_share_clause_order_and_identity() {
        use etas_code::{
            descriptor::DescriptorLimits,
            residual::{
                identity::{GenericArgumentIdentity, GenericParameterId},
                pattern::{ActionPatternTemplate, ActionSelectorTemplate, PatternTemplateLimits},
                type_template::TypeTemplateLimits,
            },
        };
        let source = vec![vec![temporal(false)], vec![temporal(true)]];
        let parameter =
            GenericParameterId::from_canonical_fingerprint(CodeFingerprint::from_bytes([8; 32]));
        let template_limits = PatternTemplateLimits {
            pattern: limits().evidence.pattern,
            types: TypeTemplateLimits::default(),
            descriptor: DescriptorLimits::default(),
            max_binding_steps: 128,
        };
        let make = || {
            projections(&source)
                .into_iter()
                .map(|projection| TracePatternTemplateProjection {
                    source: projection.source,
                    pattern: ActionPatternTemplate::new(
                        projection.pattern.effect().target.clone(),
                        vec![ActionSelectorTemplate::TypeParameter(parameter)],
                        vec![],
                        template_limits,
                    )
                    .unwrap(),
                })
                .collect::<Vec<_>>()
        };
        let symbolic = lower_symbolic_trace_rules(&source, make(), limits(), None).unwrap();
        let actual = CodeFingerprint::from_bytes([9; 32]);
        let concrete = symbolic
            .instantiate(
                &std::collections::BTreeMap::from([(
                    parameter,
                    GenericArgumentIdentity::Type(actual),
                )]),
                &Default::default(),
                template_limits,
                limits(),
            )
            .unwrap();
        let explicit = projections(&source)
            .into_iter()
            .map(|projection| TracePatternProjection {
                source: projection.source,
                pattern: ActionPattern::new(
                    CanonicalEffect {
                        target: projection.pattern.effect().target.clone(),
                        arguments: vec![etas_code::contract::CanonicalSelector::Type(actual)],
                    },
                    limits().evidence.pattern,
                )
                .unwrap(),
            })
            .collect();
        assert_eq!(
            concrete.template_id(),
            lower_trace_rules(&source, explicit, limits(), None)
                .unwrap()
                .monitor
                .template_id()
        );
        let mut wrong = make();
        wrong.swap(0, 1);
        assert!(matches!(
            lower_symbolic_trace_rules(&source, wrong, limits(), None),
            Err(TraceRulesLoweringError::PatternMismatch)
        ));
        let mut missing = make();
        missing.pop();
        assert!(matches!(
            lower_symbolic_trace_rules(&source, missing, limits(), None),
            Err(TraceRulesLoweringError::PatternCount)
        ));
    }
}
