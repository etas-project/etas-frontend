//! Checked effect identities to shared trace templates. This is not an
//! obligation, backend capability, installed monitor, or dispatch permit.

use etas_code::{
    DefinitionKey,
    contract::canonical_effect,
    descriptor::{EffectInfo, GenericOwner, StaticEffectArgInfo},
    residual::{
        identity::ClauseId,
        monitor::{
            AlternativeLimits, AlternativeRule, CompiledTraceAlternatives, MAX_TRACE_TEMPLATE_NODES,
        },
        pattern::{
            ActionPattern, ActionPatternError, ActionPatternTemplate, ActionPatternTemplateError,
            ActionSelectorTemplate, PatternTemplateLimits,
        },
    },
};
use etas_core::Span;
use etas_effects::{
    Effect,
    trace_spec::{TraceSpecClause, TraceSpecClauseAlternatives, TraceSpecPattern},
};

use super::{DefinitionDescriptorProjector, ProjectionError};

#[derive(Debug)]
pub enum TraceTemplateProjectionError {
    Projection(ProjectionError),
    MissingCallableScope,
    CallableOwnerMismatch,
    AnalyzedSourceMismatch,
    SourceLink(TraceSourceLinkError),
    SourceScope(etas_effects::trace_spec::TraceCallableScopeError),
    OpenPattern,
    BoundedPattern,
    MultiplePatterns,
    EffectFamily,
    ContainmentSelector,
    MissingActionSignature,
    SelectorArity,
    TypeSelector(etas_types::ty::TypeSubstitutionError),
    SymbolicType(etas_types::ty::ClosedStructuralTypeError),
    SymbolicPattern(ActionPatternTemplateError),
    UnresolvedEffectExtensions,
    UnsupportedLimit,
    AlternativeLimit,
    ClauseLimit,
    OutputShape,
    Pattern(ActionPatternError),
    Lower(etas_effects::trace_spec::TraceRulesLoweringError),
}

struct ProjectedPattern {
    effect: EffectInfo,
    family_members: Vec<DefinitionKey>,
    symbolic_selectors: std::collections::BTreeMap<usize, ActionSelectorTemplate>,
}

/// Descriptive lowering output. Source sites and rules must still be checked
/// against the retained HIR and source snapshot before they establish coverage.
pub struct TraceTemplateProjection {
    monitor: CompiledTraceAlternatives,
    alternatives: Vec<Vec<TraceTemplateClause>>,
    analyzed_source: Option<(
        usize,
        etas_effects::trace_spec::CheckedTraceSourceConformance,
    )>,
}

#[derive(Debug)]
pub enum TracePlanProjectionError {
    MissingAnalyzedSource,
    SourceMismatch,
    Source(etas_effects::trace_spec::TracePlanSourceError),
    MissingComponent(etas_code::residual::identity::ObligationId),
    WrongComponent(etas_code::residual::identity::ObligationId),
    MonitorMismatch(etas_code::residual::identity::ObligationId),
    Interrupted(etas_utils::PipelineInterruption),
}

pub struct TraceTemplateClause {
    pub source_clause: Option<ClauseId>,
    pub source_path: Option<Vec<u32>>,
    pub span: Span,
    pub rule: AlternativeRule,
}

/// A symbolic template is descriptive until source, binding, plan and runtime
/// coverage are separately validated.
pub struct SymbolicTraceTemplateProjection {
    pub template: etas_code::residual::monitor::SymbolicTraceAlternatives,
    pub sources: Vec<Vec<TraceTemplateSource>>,
    source_scope: Option<etas_effects::trace_spec::CheckedTraceCallableScope>,
}

pub struct TraceTemplateSource {
    pub source_clause: Option<ClauseId>,
    pub source_path: Option<Vec<u32>>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug)]
pub struct TraceSourceLinkLimits {
    pub max_sources: usize,
    pub max_sites: usize,
    pub max_clauses: usize,
    pub max_snapshot_bytes: usize,
    pub identity: etas_code::residual::identity::ObligationIdentityLimits,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceSourceLinkError {
    SourceLimit,
    SiteLimit,
    ClauseLimit,
    SnapshotBytesLimit,
    DuplicateSource(etas_core::SourceId),
    MissingSource(etas_core::SourceId),
    InvalidSourceKey(etas_core::SourceId),
    MissingClauseIdentity,
    MissingSourcePath(ClauseId),
    MissingClauseSite(ClauseId),
    MismatchedClauseSite(ClauseId),
}

impl TraceTemplateProjection {
    pub fn monitor(&self) -> &CompiledTraceAlternatives {
        &self.monitor
    }

    /// The projected monitor of an analyzed model. Taking it does not
    /// establish obligation coverage, backend support or installation.
    pub fn into_monitor(self) -> CompiledTraceAlternatives {
        self.monitor
    }

    pub fn alternatives(&self) -> &[Vec<TraceTemplateClause>] {
        &self.alternatives
    }

    /// Verifies the source-linked plan uses this analyzed model's complete
    /// monitor. Backend coverage and runtime installation remain separate.
    pub fn validate_plan(
        &self,
        input: etas_effects::trace_spec::TracePlanSourceInput<'_>,
    ) -> Result<(), TracePlanProjectionError> {
        use etas_code::residual::{model::ResidualKind, plan::SemanticPlanComponent};
        let (index, source) = self
            .analyzed_source
            .as_ref()
            .ok_or(TracePlanProjectionError::MissingAnalyzedSource)?;
        if input.bindings.len() > input.limits.max_bindings
            || input.expected.len() > input.limits.max_bindings
        {
            return Err(TracePlanProjectionError::Source(
                etas_effects::trace_spec::TracePlanSourceError::BindingLimit,
            ));
        }
        if input.analysis.source_conformance(*index) != Some(source)
            || input
                .bindings
                .iter()
                .any(|binding| binding.type_fact_index != *index)
        {
            return Err(TracePlanProjectionError::SourceMismatch);
        }
        for binding in input.bindings {
            if let Some(control) = input.control {
                control
                    .checkpoint()
                    .map_err(TracePlanProjectionError::Interrupted)?;
            }
            let id = binding.reference.obligation();
            let component = input
                .plan
                .component_for(id)
                .ok_or(TracePlanProjectionError::MissingComponent(id))?;
            let SemanticPlanComponent::Trace(component) = component else {
                return Err(TracePlanProjectionError::WrongComponent(id));
            };
            match component.obligation().kind() {
                ResidualKind::TraceContract { monitor }
                    if *monitor == self.monitor.template_id() => {}
                ResidualKind::TraceContract { .. } => {
                    return Err(TracePlanProjectionError::MonitorMismatch(id));
                }
                _ => return Err(TracePlanProjectionError::WrongComponent(id)),
            }
        }
        etas_effects::trace_spec::validate_trace_plan_sources(input)
            .map_err(TracePlanProjectionError::Source)
    }

    /// Checks source-snapshot linkage only, not HIR rule coverage or runtime installation.
    pub fn validate_snapshot_links(
        &self,
        sources: &crate::SourceBundle,
        sites: &std::collections::BTreeMap<
            ClauseId,
            etas_code::residual::identity::SourceClauseSite,
        >,
        limits: TraceSourceLinkLimits,
    ) -> Result<(), TraceSourceLinkError> {
        validate_trace_source_links(
            self.alternatives.iter().flatten().map(|clause| {
                (
                    clause.source_clause,
                    clause.source_path.as_deref(),
                    clause.span,
                )
            }),
            sources,
            sites,
            limits,
        )
    }
}

impl SymbolicTraceTemplateProjection {
    pub fn source_scope(&self) -> Option<&etas_effects::trace_spec::CheckedTraceCallableScope> {
        self.source_scope.as_ref()
    }
    pub fn validate_snapshot_links(
        &self,
        sources: &crate::SourceBundle,
        sites: &std::collections::BTreeMap<
            ClauseId,
            etas_code::residual::identity::SourceClauseSite,
        >,
        limits: TraceSourceLinkLimits,
    ) -> Result<(), TraceSourceLinkError> {
        validate_trace_source_links(
            self.sources.iter().flatten().map(|clause| {
                (
                    clause.source_clause,
                    clause.source_path.as_deref(),
                    clause.span,
                )
            }),
            sources,
            sites,
            limits,
        )
    }
}

fn validate_trace_source_links<'a>(
    clauses: impl Iterator<Item = (Option<ClauseId>, Option<&'a [u32]>, Span)>,
    sources: &crate::SourceBundle,
    sites: &std::collections::BTreeMap<ClauseId, etas_code::residual::identity::SourceClauseSite>,
    limits: TraceSourceLinkLimits,
) -> Result<(), TraceSourceLinkError> {
    use etas_code::residual::identity::CheckedSourceSnapshot;
    if sources.sources.len() > limits.max_sources
        || sources.stable_source_keys.len() > limits.max_sources
    {
        return Err(TraceSourceLinkError::SourceLimit);
    }
    if sites.len() > limits.max_sites {
        return Err(TraceSourceLinkError::SiteLimit);
    }
    let mut files = std::collections::HashMap::new();
    for source in &sources.sources {
        if files.insert(source.id, source).is_some() {
            return Err(TraceSourceLinkError::DuplicateSource(source.id));
        }
    }
    let mut keys = std::collections::HashMap::new();
    let mut unique_keys = std::collections::HashSet::new();
    for entry in &sources.stable_source_keys {
        if keys.insert(entry.source, entry.key).is_some() || !unique_keys.insert(entry.key) {
            return Err(TraceSourceLinkError::InvalidSourceKey(entry.source));
        }
    }
    let mut snapshots = std::collections::HashMap::new();
    let mut bytes = 0usize;
    let mut clause_count = 0usize;
    for (source_clause, source_path, span) in clauses {
        clause_count = clause_count
            .checked_add(1)
            .ok_or(TraceSourceLinkError::ClauseLimit)?;
        if clause_count > limits.max_clauses {
            return Err(TraceSourceLinkError::ClauseLimit);
        }
        let id = source_clause.ok_or(TraceSourceLinkError::MissingClauseIdentity)?;
        let path = source_path.ok_or(TraceSourceLinkError::MissingSourcePath(id))?;
        let site = sites
            .get(&id)
            .ok_or(TraceSourceLinkError::MissingClauseSite(id))?;
        let source = span.source;
        let snapshot = match snapshots.entry(source) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let file = files
                    .get(&source)
                    .ok_or(TraceSourceLinkError::MissingSource(source))?;
                let key = keys
                    .get(&source)
                    .ok_or(TraceSourceLinkError::InvalidSourceKey(source))?;
                bytes = bytes
                    .checked_add(file.text.len())
                    .ok_or(TraceSourceLinkError::SnapshotBytesLimit)?;
                if bytes > limits.max_snapshot_bytes {
                    return Err(TraceSourceLinkError::SnapshotBytesLimit);
                }
                entry.insert(CheckedSourceSnapshot::new(*key, file.text.as_bytes()))
            }
        };
        let location = site.location();
        if site.id() != id
            || site.structural_path() != path
            || location.byte_start != u64::from(span.range.start.0)
            || location.byte_end != u64::from(span.range.end.0)
            || site.validate_snapshot(snapshot, limits.identity).is_err()
        {
            return Err(TraceSourceLinkError::MismatchedClauseSite(id));
        }
    }
    Ok(())
}

struct ProjectedClause<'a> {
    source_clause: Option<ClauseId>,
    source_path: Option<Vec<u32>>,
    span: Span,
    patterns: Vec<(&'a TraceSpecPattern, ProjectedPattern)>,
}

type TraceBinders = std::collections::BTreeMap<
    etas_hir::SymbolId,
    etas_code::residual::identity::GenericParameterId,
>;
type SymbolicSelectorInput<'a> = (
    &'a TraceBinders,
    etas_code::residual::type_template::TypeTemplateLimits,
);
type ProjectedTraceGroups<'a> = Vec<Vec<ProjectedClause<'a>>>;

#[derive(Clone, Copy)]
enum SymbolicLoweringMode<'a> {
    Descriptive(Option<&'a dyn etas_utils::PipelineExecutionControl>),
    Analyzed(etas_effects::trace_spec::AnalyzedTraceLoweringInput<'a>),
}

impl<'a> SymbolicLoweringMode<'a> {
    fn control(self) -> Option<&'a dyn etas_utils::PipelineExecutionControl> {
        match self {
            Self::Descriptive(control) => control,
            Self::Analyzed(input) => input.control,
        }
    }
}

fn symbolic_pattern(
    graph: &etas_code::descriptor::DescriptorGraph,
    projected: ProjectedPattern,
    limits: PatternTemplateLimits,
) -> Result<ActionPatternTemplate, TraceTemplateProjectionError> {
    let effect = canonical_effect(graph, &projected.effect)
        .map_err(|error| TraceTemplateProjectionError::Projection(error.into()))?;
    let mut symbolic = projected.symbolic_selectors;
    let selectors = effect
        .arguments
        .into_iter()
        .enumerate()
        .map(|(index, fixed)| match symbolic.remove(&index) {
            Some(selector) => selector,
            None => ActionSelectorTemplate::Fixed(fixed),
        })
        .collect();
    if !symbolic.is_empty() {
        return Err(TraceTemplateProjectionError::OutputShape);
    }
    ActionPatternTemplate::new(effect.target, selectors, projected.family_members, limits)
        .map_err(TraceTemplateProjectionError::SymbolicPattern)
}

fn validate_spec_snapshot(
    sources: &crate::SourceBundle,
    instance: &etas_code::residual::identity::SourceSpecInstance,
    clause_spans: impl Iterator<Item = Span>,
    owner_source: etas_core::SourceId,
    limits: TraceSourceLinkLimits,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<(), TraceTemplateProjectionError> {
    use TraceTemplateProjectionError as Error;
    if sources.sources.len() > limits.max_sources
        || sources.stable_source_keys.len() > limits.max_sources
    {
        return Err(Error::SourceLink(TraceSourceLinkError::SourceLimit));
    }
    let mut spec_ids = sources
        .stable_source_keys
        .iter()
        .filter(|entry| entry.key == instance.declaration().source)
        .map(|entry| entry.source);
    let spec_id = spec_ids.next().ok_or(Error::AnalyzedSourceMismatch)?;
    if spec_ids.next().is_some() {
        return Err(Error::AnalyzedSourceMismatch);
    }
    let mut files = std::collections::HashMap::with_capacity(sources.sources.len());
    for file in &sources.sources {
        if let Some(control) = control {
            control.checkpoint().map_err(|error| {
                Error::Lower(etas_effects::trace_spec::TraceRulesLoweringError::Interrupted(error))
            })?;
        }
        if files.insert(file.id, file).is_some() {
            return Err(Error::AnalyzedSourceMismatch);
        }
    }
    let mut needed = std::collections::BTreeSet::from([spec_id.0, owner_source.0]);
    needed.extend(clause_spans.map(|span| span.source.0));
    let mut total = 0usize;
    for raw_id in needed {
        let id = etas_core::SourceId(raw_id);
        if let Some(control) = control {
            control.checkpoint().map_err(|error| {
                Error::Lower(etas_effects::trace_spec::TraceRulesLoweringError::Interrupted(error))
            })?;
        }
        let file = files.get(&id).ok_or(Error::AnalyzedSourceMismatch)?;
        total = total
            .checked_add(file.text.len())
            .ok_or(Error::SourceLink(TraceSourceLinkError::SnapshotBytesLimit))?;
        if total > limits.max_snapshot_bytes {
            return Err(Error::SourceLink(TraceSourceLinkError::SnapshotBytesLimit));
        }
        if id == spec_id {
            let key = sources
                .stable_source_key(id)
                .map_err(|_| Error::AnalyzedSourceMismatch)?;
            let snapshot = etas_code::residual::identity::CheckedSourceSnapshot::new(
                key,
                file.text.as_bytes(),
            );
            instance
                .validate_snapshot(&snapshot, limits.identity)
                .map_err(|_| Error::AnalyzedSourceMismatch)?;
        }
    }
    Ok(())
}

impl DefinitionDescriptorProjector<'_> {
    /// Consumes the already projected callable scope and lowers complete
    /// alternatives using canonical type/effect identities from checked facts.
    /// Unsupported semantics are rejected, never widened or dropped.
    pub fn finish_trace_template(
        self,
        owner: &GenericOwner,
        alternatives: &TraceSpecClauseAlternatives,
        limits: AlternativeLimits,
    ) -> Result<TraceTemplateProjection, TraceTemplateProjectionError> {
        self.finish_trace_template_impl(owner, alternatives, limits, None)
    }

    fn validate_analyzed_trace_source<'a>(
        &self,
        owner: &GenericOwner,
        input: etas_effects::trace_spec::AnalyzedTraceLoweringInput<'a>,
        source_limits: TraceSourceLinkLimits,
    ) -> Result<&'a TraceSpecClauseAlternatives, TraceTemplateProjectionError> {
        let item = self
            .checked
            .hir
            .items
            .get(input.item)
            .ok_or(TraceTemplateProjectionError::CallableOwnerMismatch)?;
        let symbol = item
            .symbol()
            .ok_or(TraceTemplateProjectionError::CallableOwnerMismatch)?;
        let matches_owner = match owner {
            GenericOwner::Declaration(id) => self.identities.symbol(symbol) == Some(id),
            GenericOwner::Method(id) => self.identities.method(symbol) == Some(id),
            GenericOwner::Implementation(_) | GenericOwner::ImplementationPattern(_) => false,
        };
        if !matches_owner {
            return Err(TraceTemplateProjectionError::CallableOwnerMismatch);
        }
        let kind = match (owner, item) {
            (GenericOwner::Method(_), _) => {
                etas_code::residual::identity::ResidualOwnerKind::Method
            }
            (GenericOwner::Declaration(_), etas_hir::HirItem::Flow(_)) => {
                etas_code::residual::identity::ResidualOwnerKind::Flow
            }
            (GenericOwner::Declaration(_), etas_hir::HirItem::Agent(_)) => {
                etas_code::residual::identity::ResidualOwnerKind::Agent
            }
            (GenericOwner::Declaration(_), etas_hir::HirItem::Tool(_)) => {
                etas_code::residual::identity::ResidualOwnerKind::Tool
            }
            _ => return Err(TraceTemplateProjectionError::CallableOwnerMismatch),
        };
        let source = input
            .analysis
            .source_conformance(input.type_fact_index)
            .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        let analyzed_owner = source
            .source_owner
            .as_ref()
            .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        let span = item.span();
        let sources = self
            .checked
            .sources
            .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        if sources.sources.len() > source_limits.max_sources
            || sources.stable_source_keys.len() > source_limits.max_sources
        {
            return Err(TraceTemplateProjectionError::SourceLink(
                TraceSourceLinkError::SourceLimit,
            ));
        }
        if let Some(control) = input.control {
            control.checkpoint().map_err(|error| {
                TraceTemplateProjectionError::Lower(
                    etas_effects::trace_spec::TraceRulesLoweringError::Interrupted(error),
                )
            })?;
        }
        let mut matching = sources
            .sources
            .iter()
            .filter(|source| source.id == span.source);
        let file = matching
            .next()
            .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        if matching.next().is_some() {
            return Err(TraceTemplateProjectionError::AnalyzedSourceMismatch);
        }
        if file.text.len() > source_limits.max_snapshot_bytes {
            return Err(TraceTemplateProjectionError::SourceLink(
                TraceSourceLinkError::SnapshotBytesLimit,
            ));
        }
        let key = sources
            .stable_source_key(span.source)
            .map_err(|_| TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        let snapshot =
            etas_code::residual::identity::CheckedSourceSnapshot::new(key, file.text.as_bytes());
        let location = snapshot
            .location(u64::from(span.range.start.0), u64::from(span.range.end.0))
            .map_err(|_| TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        let id = etas_code::residual::identity::ResidualOwnerId::from_checked_source_declaration(
            &snapshot, &location, kind,
        )
        .map_err(|_| TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        if analyzed_owner.declaration() != &location
            || analyzed_owner.kind() != kind
            || analyzed_owner.id() != id
        {
            return Err(TraceTemplateProjectionError::AnalyzedSourceMismatch);
        }
        let alternatives = input
            .dispositions
            .analyzed_lowering_source(
                input.type_fact_index,
                input.item,
                input.analysis,
                input.control,
            )
            .map_err(|error| {
                TraceTemplateProjectionError::Lower(
                    etas_effects::trace_spec::TraceRulesLoweringError::Analysis(error),
                )
            })?;
        let trace_facts = &self
            .checked
            .effects
            .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?
            .trace_specs;
        let mut current = trace_facts
            .conformances
            .get(&input.item)
            .into_iter()
            .flatten()
            .filter(|fact| fact.span == source.span);
        let fact = current
            .next()
            .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        if current.next().is_some()
            || fact.source_spec_instance != source.source_instance
            || &fact.alternatives != alternatives
        {
            return Err(TraceTemplateProjectionError::AnalyzedSourceMismatch);
        }
        Ok(alternatives)
    }

    /// Projects only the model captured by verified trace analysis. Source
    /// linkage is checked but plan coverage and installation remain separate.
    pub fn finish_analyzed_trace_template(
        self,
        owner: &GenericOwner,
        input: etas_effects::trace_spec::AnalyzedTraceLoweringInput<'_>,
        source_limits: TraceSourceLinkLimits,
    ) -> Result<TraceTemplateProjection, TraceTemplateProjectionError> {
        let alternatives = self.validate_analyzed_trace_source(owner, input, source_limits)?;
        let source = input
            .analysis
            .source_conformance(input.type_fact_index)
            .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        let instance = source
            .source_instance
            .as_ref()
            .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        let sources = self
            .checked
            .sources
            .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        let trace_facts = &self
            .checked
            .effects
            .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?
            .trace_specs;
        let span = self
            .checked
            .hir
            .items
            .get(input.item)
            .ok_or(TraceTemplateProjectionError::CallableOwnerMismatch)?
            .span();
        let mut projection =
            self.finish_trace_template_impl(owner, alternatives, input.limits, Some(input))?;
        projection
            .validate_snapshot_links(sources, &trace_facts.source_clause_sites, source_limits)
            .map_err(TraceTemplateProjectionError::SourceLink)?;
        validate_spec_snapshot(
            sources,
            instance,
            projection
                .alternatives
                .iter()
                .flatten()
                .map(|clause| clause.span),
            span.source,
            source_limits,
            input.control,
        )?;
        projection.analyzed_source = Some((input.type_fact_index, source.clone()));
        Ok(projection)
    }

    /// Projects the analyzed model with binders recovered from the checked
    /// callable and source owner. This is not plan coverage or installation.
    pub fn finish_analyzed_symbolic_trace_template(
        self,
        owner: &GenericOwner,
        input: etas_effects::trace_spec::AnalyzedTraceLoweringInput<'_>,
        source_limits: TraceSourceLinkLimits,
        template_limits: PatternTemplateLimits,
    ) -> Result<SymbolicTraceTemplateProjection, TraceTemplateProjectionError> {
        let alternatives = self.validate_analyzed_trace_source(owner, input, source_limits)?;
        let source = input
            .analysis
            .source_conformance(input.type_fact_index)
            .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        let owner_source = source
            .source_owner
            .as_ref()
            .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        let instance = source
            .source_instance
            .as_ref()
            .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        let sources = self
            .checked
            .sources
            .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        let trace_facts = &self
            .checked
            .effects
            .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?
            .trace_specs;
        let item = self
            .checked
            .hir
            .items
            .get(input.item)
            .ok_or(TraceTemplateProjectionError::CallableOwnerMismatch)?;
        let span = item.span();
        let file = sources
            .sources
            .iter()
            .find(|file| file.id == span.source)
            .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        let key = sources
            .stable_source_key(span.source)
            .map_err(|_| TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
        let snapshots = [etas_effects::EffectSourceSnapshot {
            source: span.source,
            key,
            text: &file.text,
        }];
        let checked_scope = etas_effects::trace_spec::checked_trace_callable_scope(
            etas_effects::trace_spec::TraceCallableScopeInput {
                analysis: input.analysis,
                dispositions: input.dispositions,
                type_fact_index: input.type_fact_index,
                item: input.item,
                hir: self.checked.hir,
                snapshots: &snapshots,
                limits: etas_effects::trace_spec::TraceObligationSourceLimits {
                    max_snapshots: 1,
                    max_snapshot_bytes: source_limits.max_snapshot_bytes,
                    identity: source_limits.identity,
                },
                control: input.control,
            },
        )
        .map_err(TraceTemplateProjectionError::SourceScope)?;
        if checked_scope.owner != owner_source.id() {
            return Err(TraceTemplateProjectionError::AnalyzedSourceMismatch);
        }
        let params = match item {
            etas_hir::HirItem::Flow(flow) => flow.type_params.as_slice(),
            etas_hir::HirItem::Tool(tool) => tool.type_params.as_slice(),
            etas_hir::HirItem::Agent(_) => &[],
            _ => return Err(TraceTemplateProjectionError::CallableOwnerMismatch),
        };
        if params.len() > source_limits.identity.max_generic_arguments {
            return Err(TraceTemplateProjectionError::AnalyzedSourceMismatch);
        }
        let mut binders = TraceBinders::new();
        for (position, symbol) in params.iter().enumerate() {
            if let Some(control) = input.control {
                control.checkpoint().map_err(|error| {
                    TraceTemplateProjectionError::Lower(
                        etas_effects::trace_spec::TraceRulesLoweringError::Interrupted(error),
                    )
                })?;
            }
            let data = self
                .checked
                .hir
                .symbols
                .get(*symbol)
                .ok_or(TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
            let position = u32::try_from(position)
                .map_err(|_| TraceTemplateProjectionError::AnalyzedSourceMismatch)?;
            match &data.def {
                etas_hir::SymbolDef::TypeParam {
                    owner, param_index, ..
                } if *owner == input.item && *param_index == position => {
                    let id = etas_code::residual::identity::GenericParameterId::from_source_owner(
                        checked_scope.owner,
                        position,
                        etas_code::descriptor::GenericParameterKind::Type,
                    );
                    if binders.insert(*symbol, id).is_some() {
                        return Err(TraceTemplateProjectionError::AnalyzedSourceMismatch);
                    }
                }
                etas_hir::SymbolDef::EffectParam { owner, param_index }
                    if *owner == input.item && *param_index == position => {}
                _ => return Err(TraceTemplateProjectionError::AnalyzedSourceMismatch),
            }
        }
        let mut projection = self.finish_symbolic_trace_template_impl(
            owner,
            alternatives,
            &binders,
            input.limits,
            template_limits,
            SymbolicLoweringMode::Analyzed(input),
        )?;
        projection
            .validate_snapshot_links(sources, &trace_facts.source_clause_sites, source_limits)
            .map_err(TraceTemplateProjectionError::SourceLink)?;
        validate_spec_snapshot(
            sources,
            instance,
            projection
                .sources
                .iter()
                .flatten()
                .map(|clause| clause.span),
            span.source,
            source_limits,
            input.control,
        )?;
        projection.source_scope = Some(checked_scope);
        Ok(projection)
    }

    fn finish_trace_template_impl(
        self,
        owner: &GenericOwner,
        alternatives: &TraceSpecClauseAlternatives,
        limits: AlternativeLimits,
        analysis: Option<etas_effects::trace_spec::AnalyzedTraceLoweringInput<'_>>,
    ) -> Result<TraceTemplateProjection, TraceTemplateProjectionError> {
        let (graph, groups) = self.project_trace_patterns(
            owner,
            alternatives,
            limits,
            analysis.and_then(|input| input.control),
            None,
        )?;
        let pattern =
            |projected: ProjectedPattern| -> Result<ActionPattern, TraceTemplateProjectionError> {
                if !projected.symbolic_selectors.is_empty() {
                    return Err(TraceTemplateProjectionError::OutputShape);
                }
                let effect = canonical_effect(&graph, &projected.effect)
                    .map_err(|error| TraceTemplateProjectionError::Projection(error.into()))?;
                if matches!(
                    effect.target,
                    etas_code::descriptor::EffectTargetInfo::Effect(_)
                ) {
                    ActionPattern::from_effect_family(
                        effect,
                        projected.family_members,
                        limits.evidence.pattern,
                    )
                } else {
                    ActionPattern::new(effect, limits.evidence.pattern)
                }
                .map_err(TraceTemplateProjectionError::Pattern)
            };
        let mut projected_patterns = Vec::new();
        let mut sources = Vec::with_capacity(groups.len());
        for group in groups {
            let mut source_group = Vec::with_capacity(group.len());
            for clause in group {
                for (source, operand) in clause.patterns {
                    projected_patterns.push(etas_effects::trace_spec::TracePatternProjection {
                        source,
                        pattern: pattern(operand)?,
                    });
                }
                source_group.push((clause.source_clause, clause.source_path, clause.span));
            }
            sources.push(source_group);
        }
        let lowered = match analysis {
            Some(input) => {
                etas_effects::trace_spec::lower_analyzed_trace_rules(input, projected_patterns)
            }
            None => etas_effects::trace_spec::lower_trace_rules(
                alternatives,
                projected_patterns,
                limits,
                None,
            ),
        }
        .map_err(TraceTemplateProjectionError::Lower)?;
        if sources.len() != lowered.alternatives.len()
            || sources
                .iter()
                .zip(&lowered.alternatives)
                .any(|(source, rules)| source.len() != rules.len())
        {
            return Err(TraceTemplateProjectionError::OutputShape);
        }
        let rules = sources
            .into_iter()
            .zip(lowered.alternatives)
            .map(|(sources, rules)| {
                sources
                    .into_iter()
                    .zip(rules)
                    .map(
                        |((source_clause, source_path, span), rule)| TraceTemplateClause {
                            source_clause,
                            source_path,
                            span,
                            rule,
                        },
                    )
                    .collect()
            })
            .collect();
        Ok(TraceTemplateProjection {
            monitor: lowered.monitor,
            alternatives: rules,
            analyzed_source: None,
        })
    }

    fn project_trace_patterns<'a>(
        mut self,
        owner: &GenericOwner,
        alternatives: &'a TraceSpecClauseAlternatives,
        limits: AlternativeLimits,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
        symbolic: Option<SymbolicSelectorInput<'_>>,
    ) -> Result<
        (
            etas_code::descriptor::DescriptorGraph,
            ProjectedTraceGroups<'a>,
        ),
        TraceTemplateProjectionError,
    > {
        if alternatives.len() > limits.max_alternatives
            || alternatives.len() > MAX_TRACE_TEMPLATE_NODES
        {
            return Err(TraceTemplateProjectionError::AlternativeLimit);
        }
        let scope = self
            .scopes
            .iter()
            .position(|scope| &scope.owner == owner)
            .ok_or(TraceTemplateProjectionError::MissingCallableScope)?;
        let type_budget = etas_types::ty::TypeSubstitutionBudget::new(
            self.limits.max_nodes,
            self.limits.max_edges,
            control,
        );
        let mut clauses = 0usize;
        let mut groups = Vec::with_capacity(alternatives.len());
        for alternative in alternatives {
            if let Some(control) = control {
                control.checkpoint().map_err(|error| {
                    TraceTemplateProjectionError::Lower(
                        etas_effects::trace_spec::TraceRulesLoweringError::Interrupted(error),
                    )
                })?;
            }
            clauses = clauses
                .checked_add(alternative.len())
                .ok_or(TraceTemplateProjectionError::ClauseLimit)?;
            if clauses > limits.max_clauses
                || clauses
                    .checked_add(alternatives.len())
                    .is_none_or(|nodes| nodes > MAX_TRACE_TEMPLATE_NODES)
            {
                return Err(TraceTemplateProjectionError::ClauseLimit);
            }
            let mut group = Vec::with_capacity(alternative.len());
            for clause in alternative {
                if let Some(control) = control {
                    control.checkpoint().map_err(|error| {
                        TraceTemplateProjectionError::Lower(
                            etas_effects::trace_spec::TraceRulesLoweringError::Interrupted(error),
                        )
                    })?;
                }
                if clause.source_path().is_some_and(|path| {
                    path.len() > etas_package_metadata::MAX_METADATA_GRAPH_DEPTH
                }) {
                    return Err(TraceTemplateProjectionError::ClauseLimit);
                }
                let span = match clause {
                    TraceSpecClause::Allow { span, .. }
                    | TraceSpecClause::Deny { span, .. }
                    | TraceSpecClause::RequireBefore { span, .. }
                    | TraceSpecClause::RequireAfter { span, .. } => *span,
                    TraceSpecClause::Limit { .. } => {
                        return Err(TraceTemplateProjectionError::UnsupportedLimit);
                    }
                };
                let patterns = match clause {
                    TraceSpecClause::Allow { pattern, .. }
                    | TraceSpecClause::Deny { pattern, .. } => vec![(
                        pattern,
                        self.residual_action(pattern, scope, limits, &type_budget, symbolic)?,
                    )],
                    TraceSpecClause::RequireBefore { guard, target, .. } => vec![
                        (
                            guard,
                            self.residual_action(guard, scope, limits, &type_budget, symbolic)?,
                        ),
                        (
                            target,
                            self.residual_action(target, scope, limits, &type_budget, symbolic)?,
                        ),
                    ],
                    TraceSpecClause::RequireAfter {
                        target, obligation, ..
                    } => vec![
                        (
                            target,
                            self.residual_action(target, scope, limits, &type_budget, symbolic)?,
                        ),
                        (
                            obligation,
                            self.residual_action(
                                obligation,
                                scope,
                                limits,
                                &type_budget,
                                symbolic,
                            )?,
                        ),
                    ],
                    TraceSpecClause::Limit { .. } => {
                        return Err(TraceTemplateProjectionError::UnsupportedLimit);
                    }
                };
                group.push(ProjectedClause {
                    source_clause: clause.source_clause(),
                    source_path: clause.source_path().map(<[u32]>::to_vec),
                    span,
                    patterns,
                });
            }
            groups.push(group);
        }
        let graph = self
            .finish()
            .map_err(TraceTemplateProjectionError::Projection)?;
        Ok((graph, groups))
    }

    /// Projects all alternatives with caller-supplied checked binder facts.
    /// The result is descriptive, not a verified residual plan.
    pub fn finish_symbolic_trace_template(
        self,
        owner: &GenericOwner,
        alternatives: &TraceSpecClauseAlternatives,
        binders: &TraceBinders,
        monitor_limits: AlternativeLimits,
        template_limits: PatternTemplateLimits,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<SymbolicTraceTemplateProjection, TraceTemplateProjectionError> {
        self.finish_symbolic_trace_template_impl(
            owner,
            alternatives,
            binders,
            monitor_limits,
            template_limits,
            SymbolicLoweringMode::Descriptive(control),
        )
    }

    fn finish_symbolic_trace_template_impl(
        self,
        owner: &GenericOwner,
        alternatives: &TraceSpecClauseAlternatives,
        binders: &TraceBinders,
        monitor_limits: AlternativeLimits,
        template_limits: PatternTemplateLimits,
        mode: SymbolicLoweringMode<'_>,
    ) -> Result<SymbolicTraceTemplateProjection, TraceTemplateProjectionError> {
        let (graph, groups) = self.project_trace_patterns(
            owner,
            alternatives,
            monitor_limits,
            mode.control(),
            Some((binders, template_limits.types)),
        )?;
        let mut patterns = Vec::new();
        let mut sources = Vec::with_capacity(groups.len());
        for group in groups {
            let mut source_group = Vec::with_capacity(group.len());
            for clause in group {
                for (source, projected) in clause.patterns {
                    patterns.push(etas_effects::trace_spec::TracePatternTemplateProjection {
                        source,
                        pattern: symbolic_pattern(&graph, projected, template_limits)?,
                    });
                }
                source_group.push(TraceTemplateSource {
                    source_clause: clause.source_clause,
                    source_path: clause.source_path,
                    span: clause.span,
                });
            }
            sources.push(source_group);
        }
        let template = match mode {
            SymbolicLoweringMode::Analyzed(input) => {
                etas_effects::trace_spec::lower_analyzed_symbolic_trace_rules(input, patterns)
            }
            SymbolicLoweringMode::Descriptive(control) => {
                etas_effects::trace_spec::lower_symbolic_trace_rules(
                    alternatives,
                    patterns,
                    monitor_limits,
                    control,
                )
            }
        }
        .map_err(TraceTemplateProjectionError::Lower)?;
        if sources.len() != template.alternatives().len()
            || sources
                .iter()
                .zip(template.alternatives())
                .any(|(source, rules)| source.len() != rules.len())
        {
            return Err(TraceTemplateProjectionError::OutputShape);
        }
        Ok(SymbolicTraceTemplateProjection {
            template,
            sources,
            source_scope: None,
        })
    }

    /// Projects a symbolic action using caller-supplied checked binder facts.
    /// The resulting template does not prove source linkage or installation.
    pub fn finish_symbolic_trace_action(
        mut self,
        owner: &GenericOwner,
        pattern: &TraceSpecPattern,
        binders: &std::collections::BTreeMap<
            etas_hir::SymbolId,
            etas_code::residual::identity::GenericParameterId,
        >,
        monitor_limits: AlternativeLimits,
        template_limits: PatternTemplateLimits,
    ) -> Result<ActionPatternTemplate, TraceTemplateProjectionError> {
        let scope = self
            .scopes
            .iter()
            .position(|scope| &scope.owner == owner)
            .ok_or(TraceTemplateProjectionError::MissingCallableScope)?;
        let budget = etas_types::ty::TypeSubstitutionBudget::new(
            self.limits.max_nodes,
            self.limits.max_edges,
            None,
        );
        let projected = self.residual_action(
            pattern,
            scope,
            monitor_limits,
            &budget,
            Some((binders, template_limits.types)),
        )?;
        let graph = self
            .finish()
            .map_err(TraceTemplateProjectionError::Projection)?;
        symbolic_pattern(&graph, projected, template_limits)
    }

    fn residual_action(
        &mut self,
        pattern: &TraceSpecPattern,
        scope: usize,
        limits: AlternativeLimits,
        budget: &etas_types::ty::TypeSubstitutionBudget<'_>,
        symbolic: Option<SymbolicSelectorInput<'_>>,
    ) -> Result<ProjectedPattern, TraceTemplateProjectionError> {
        if pattern.row.open.is_some() {
            return Err(TraceTemplateProjectionError::OpenPattern);
        }
        if !pattern.arg_bounds.is_empty() {
            return Err(TraceTemplateProjectionError::BoundedPattern);
        }
        let mut effects = pattern.row.effects.iter();
        let effect = effects
            .next()
            .ok_or(TraceTemplateProjectionError::MultiplePatterns)?;
        if effects.next().is_some() {
            return Err(TraceTemplateProjectionError::MultiplePatterns);
        }
        if let Effect::Tag(owner) = effect {
            if !self
                .checked
                .effect_registry
                .unresolved_dependency_extensions()
                .is_empty()
            {
                return Err(TraceTemplateProjectionError::UnresolvedEffectExtensions);
            }
            let mut family_members = Vec::new();
            let mut visited = 0usize;
            for tag in self.checked.effect_registry.tags() {
                visited = visited
                    .checked_add(1)
                    .ok_or(TraceTemplateProjectionError::ClauseLimit)?;
                self.ensure_edges(visited)
                    .map_err(TraceTemplateProjectionError::Projection)?;
                if tag.id != *owner && self.checked.effect_registry.tag_extends(tag.id, *owner) {
                    if family_members.len() >= limits.evidence.pattern.max_identity_segments {
                        return Err(TraceTemplateProjectionError::Pattern(
                            ActionPatternError::IdentityLimit,
                        ));
                    }
                    family_members.push(
                        self.identities
                            .effect_tag(tag.id)
                            .map_err(TraceTemplateProjectionError::Projection)?
                            .clone(),
                    );
                }
            }
            for _ in 0..visited {
                self.edge()
                    .map_err(TraceTemplateProjectionError::Projection)?;
            }
            let effect = self
                .analysis_effect(effect, scope)
                .map_err(TraceTemplateProjectionError::Projection)?;
            return Ok(ProjectedPattern {
                effect,
                family_members,
                symbolic_selectors: Default::default(),
            });
        }
        let (action, arguments) = match effect {
            Effect::Action(action) => (action, None),
            Effect::AppliedAction(action) => (&action.action, Some(action.args.as_slice())),
            _ => return Err(TraceTemplateProjectionError::EffectFamily),
        };
        let signature = self
            .checked
            .effect_registry
            .action_signature(action)
            .ok_or(TraceTemplateProjectionError::MissingActionSignature)?;
        let selector_count = signature.effect_args.len();
        if selector_count > limits.evidence.pattern.max_selectors {
            return Err(TraceTemplateProjectionError::Pattern(
                ActionPatternError::SelectorLimit,
            ));
        }
        let mut symbolic_selectors = std::collections::BTreeMap::new();
        if let Some(arguments) = arguments {
            if arguments.len() != selector_count {
                return Err(TraceTemplateProjectionError::SelectorArity);
            }
            let empty = std::collections::HashMap::new();
            let types =
                etas_types::ty::TypeSubstitutionEngine::existing(self.checked.type_store, &empty)
                    .with_budget(budget);
            for (index, (kind, argument)) in signature.effect_args.iter().zip(arguments).enumerate()
            {
                if matches!(
                    kind,
                    etas_effects::EffectActionArgKind::MemoryPlace
                        | etas_effects::EffectActionArgKind::StaticResourcePath { .. }
                ) && !matches!(argument, etas_types::EffectArgRef::Wildcard)
                {
                    return Err(TraceTemplateProjectionError::ContainmentSelector);
                }
                if let etas_types::EffectArgRef::Type(ty) = argument {
                    if let Err(error) = types.validate_closed_argument(*ty) {
                        let Some((binders, type_limits)) = symbolic else {
                            return Err(TraceTemplateProjectionError::TypeSelector(error));
                        };
                        if !matches!(
                            error,
                            etas_types::ty::TypeSubstitutionError::UnresolvedType(_)
                        ) {
                            return Err(TraceTemplateProjectionError::TypeSelector(error));
                        }
                        let selector = etas_types::ty::symbolic_action_type_selector(
                            self.checked.type_store,
                            *ty,
                            binders,
                            type_limits,
                        )
                        .map_err(TraceTemplateProjectionError::SymbolicType)?
                        .ok_or(TraceTemplateProjectionError::TypeSelector(error))?;
                        symbolic_selectors.insert(index, selector);
                    }
                }
            }
        }
        let mut projected = self
            .analysis_effect(effect, scope)
            .map_err(TraceTemplateProjectionError::Projection)?;
        if arguments.is_none() {
            self.ensure_edges(selector_count)
                .map_err(TraceTemplateProjectionError::Projection)?;
            for _ in 0..selector_count {
                self.edge()
                    .map_err(TraceTemplateProjectionError::Projection)?;
                projected.arguments.push(StaticEffectArgInfo::Wildcard);
            }
        }
        Ok(ProjectedPattern {
            effect: projected,
            family_members: Vec::new(),
            symbolic_selectors,
        })
    }
}

#[cfg(test)]
mod analyzed_method_tests {
    use super::*;
    use crate::{
        Frontend, ProjectInput, SourceInput,
        code::identity::{DefinitionIdentityIndex, DefinitionOrigins},
    };
    use etas_code::{
        PackageKey, StableSourceKey, StableSourceOrigin,
        descriptor::DescriptorLimits,
        residual::{
            identity::ObligationIdentityLimits, monitor::ConcreteEvidenceLimits,
            pattern::ActionPatternLimits,
        },
    };
    use std::collections::BTreeMap;

    #[test]
    fn generic_agent_method_projects_concrete_and_rejects_unbound_trace() {
        assert_eq!(
            check_method_trace("i32", false),
            check_method_trace("T", true)
        );
    }

    fn check_method_trace(
        selector: &str,
        symbolic: bool,
    ) -> etas_code::residual::identity::MonitorTemplateId {
        let text = "module app; effect Gate { action a<T>(value: T) -> unit; action b<T>(value: T) -> unit; } spec Safe<T>: trace = (-Gate.a<T> | +Gate.a<T>) & (Gate.b<T> << Gate.a<T>); agent Worker; impl agent Worker { flow helper<T>(value: T) -> unit ![] ~ Safe<SELECTOR> { return; } } flow main() -> unit { return; }".replace("SELECTOR", selector);
        let mut input = ProjectInput::single_source(SourceInput::anonymous(text));
        input.options.entry_policy = crate::EntryPolicy::Optional;
        let package = PackageKey::from_name_and_version("app", "1").unwrap();
        input.environment.local_source_package = Some(package);
        input
            .environment
            .stable_source_keys
            .push(crate::ProjectStableSourceKeyInput {
                source: input.sources[0].id,
                key: StableSourceKey::new(
                    StableSourceOrigin::Package(package),
                    &["src".into(), "app.es".into()],
                )
                .unwrap(),
            });
        let output = Frontend.check_project(input);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let checked = output.checked.unwrap();
        let method = checked
            .hir
            .agent_methods
            .values()
            .find(|method| method.implementation.is_some())
            .unwrap();
        let etas_hir::HirItem::Flow(flow) = &checked.hir.items[method.body] else {
            panic!("method flow")
        };
        let etas_types::ItemSignature::Flow(signature) =
            &checked.types.item_signatures[&method.body]
        else {
            panic!("method signature")
        };
        let source = &checked.sources.sources[0];
        let snapshots = [etas_effects::EffectSourceSnapshot {
            source: source.id,
            key: checked.sources.stable_source_key(source.id).unwrap(),
            text: &source.text,
        }];
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
        let dispositions =
            etas_effects::trace_spec::collect_trace_static_dispositions(&analysis, None, None)
                .unwrap();
        let origins = DefinitionOrigins {
            source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
            standard_package: crate::code::identity::standard_package_key(&checked.std_registry)
                .unwrap(),
            external_definitions: BTreeMap::new(),
        };
        let identities = DefinitionIdentityIndex::build(&checked, &origins).unwrap();
        let owner = GenericOwner::Method(identities.method(flow.symbol).unwrap().clone());
        let mut projector =
            DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
        let names = flow
            .params
            .iter()
            .map(|symbol| Some(checked.hir.symbols.get(*symbol).unwrap().name.clone()))
            .collect::<Vec<_>>();
        projector
            .project_signature(
                owner.clone(),
                signature,
                &names,
                flow.return_type.map(|ty| checked.types.type_refs[&ty]),
            )
            .unwrap();
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
            max_clauses: 8,
            max_patterns: 8,
            max_checks_per_event: 32,
            max_product_work_per_event: 128,
        };
        let input = etas_effects::trace_spec::AnalyzedTraceLoweringInput {
            analysis: &analysis,
            dispositions: &dispositions,
            type_fact_index: models.referenced_fact_indices[&method.body][0],
            item: method.body,
            limits,
            control: None,
        };
        let source_limits = TraceSourceLinkLimits {
            max_sources: 4,
            max_sites: 8,
            max_clauses: 8,
            max_snapshot_bytes: 4096,
            identity: ObligationIdentityLimits {
                max_generic_arguments: 8,
                max_structural_site_depth: 64,
            },
        };
        let result = projector.finish_analyzed_trace_template(&owner, input, source_limits);
        if symbolic {
            assert!(matches!(
                result,
                Err(TraceTemplateProjectionError::TypeSelector(
                    etas_types::ty::TypeSubstitutionError::UnresolvedType(_)
                ))
            ));
            let checked_source = analysis.source_conformance(input.type_fact_index).unwrap();
            let source_owner = checked_source.source_owner.as_ref().unwrap();
            let parameter = etas_code::residual::identity::GenericParameterId::from_source_owner(
                source_owner.id(),
                0,
                etas_code::descriptor::GenericParameterKind::Type,
            );
            let scope_input = etas_effects::trace_spec::TraceCallableScopeInput {
                analysis: &analysis,
                dispositions: &dispositions,
                type_fact_index: input.type_fact_index,
                item: method.body,
                hir: &checked.hir,
                snapshots: &snapshots,
                limits: etas_effects::trace_spec::TraceObligationSourceLimits {
                    max_snapshots: 4,
                    max_snapshot_bytes: 4096,
                    identity: source_limits.identity,
                },
                control: None,
            };
            let identity_input = |work| etas_effects::trace_spec::TraceObligationIdentityInput {
                scope: scope_input,
                alternative_index: 0,
                clause_index: 0,
                clause_sites: &models.source_clause_sites,
                contract: etas_code::residual::identity::BehaviorContractRef::SourceConstraint,
                rule_kind: etas_code::residual::identity::ResidualRuleKind::TraceContract,
                rule_suffix: &[],
                max_identity_work: work,
            };
            let identity =
                etas_effects::trace_spec::trace_obligation_identity(identity_input(64)).unwrap();
            let frozen_contract = etas_code::residual::identity::BehaviorContractRef::FrozenInput(
                etas_code::residual::identity::BehaviorContractId::from_canonical_fingerprint(
                    etas_code::CodeFingerprint::from_bytes([0x57; 32]),
                ),
            );
            let frozen = etas_effects::trace_spec::trace_obligation_identity(
                etas_effects::trace_spec::TraceObligationIdentityInput {
                    contract: frozen_contract,
                    ..identity_input(64)
                },
            )
            .unwrap();
            assert_eq!(frozen.inputs().contract, frozen_contract);
            assert_ne!(identity.id(), frozen.id());
            let action = etas_effects::trace_spec::trace_obligation_identity(
                etas_effects::trace_spec::TraceObligationIdentityInput {
                    rule_kind: etas_code::residual::identity::ResidualRuleKind::ActionPattern,
                    rule_suffix: &[0],
                    ..identity_input(64)
                },
            )
            .unwrap();
            assert_ne!(identity.id(), action.id());
            assert_eq!(
                identity.inputs().static_bindings.get(&parameter),
                Some(&etas_code::residual::identity::StaticBindingRecipe::TypeParameter(parameter))
            );
            assert_eq!(identity.inputs().static_bindings.len(), 1);
            let reference = identity.reference(1, None).unwrap();
            assert_eq!(reference.obligation(), identity.id());
            identity.validate_reference(&reference, 1, None).unwrap();
            let missing = etas_code::residual::identity::ResidualObligationRef::new(
                identity.id(),
                Default::default(),
            )
            .unwrap();
            assert!(matches!(
                identity.validate_reference(&missing, 1, None),
                Err(etas_effects::trace_spec::TraceObligationReferenceError::BindingSetMismatch)
            ));
            let other_parameter =
                etas_code::residual::identity::GenericParameterId::from_canonical_fingerprint(
                    etas_code::CodeFingerprint::from_bytes([0x56; 32]),
                );
            let substituted_key = etas_code::residual::identity::ResidualObligationRef::new(
                identity.id(),
                [(
                    other_parameter,
                    etas_code::residual::identity::GenericArgumentIdentity::TypeParameter(
                        other_parameter,
                    ),
                )]
                .into_iter()
                .collect(),
            )
            .unwrap();
            assert!(matches!(
                identity.validate_reference(&substituted_key, 1, None),
                Err(etas_effects::trace_spec::TraceObligationReferenceError::BindingSetMismatch)
            ));
            assert!(matches!(
                identity.validate_reference(&reference, 0, None),
                Err(etas_effects::trace_spec::TraceObligationReferenceError::BindingLimit)
            ));
            let wrong_kind = etas_code::residual::identity::ResidualObligationRef::new(
                identity.id(),
                [(
                    parameter,
                    etas_code::residual::identity::GenericArgumentIdentity::EffectRowParameter(
                        parameter,
                    ),
                )]
                .into_iter()
                .collect(),
            )
            .unwrap();
            assert!(matches!(
                identity.validate_reference(&wrong_kind, 1, None),
                Err(etas_effects::trace_spec::TraceObligationReferenceError::BindingKindMismatch(id)) if id == parameter
            ));
            let wrong_obligation = etas_code::residual::identity::ResidualObligationRef::new(
                etas_code::residual::identity::ObligationId::from_canonical_fingerprint(
                    etas_code::CodeFingerprint::from_bytes([0x55; 32]),
                ),
                reference.substitutions().clone(),
            )
            .unwrap();
            assert!(matches!(
                identity.validate_reference(&wrong_obligation, 1, None),
                Err(etas_effects::trace_spec::TraceObligationReferenceError::ObligationMismatch)
            ));
            assert_eq!(
                reference.substitutions().get(&parameter),
                Some(
                    &etas_code::residual::identity::GenericArgumentIdentity::TypeParameter(
                        parameter
                    )
                )
            );
            assert!(matches!(
                identity.reference(0, None),
                Err(etas_effects::trace_spec::TraceObligationReferenceError::BindingLimit)
            ));
            assert_eq!(
                identity.id(),
                etas_effects::trace_spec::trace_obligation_identity(identity_input(64))
                    .unwrap()
                    .id()
            );
            let later = etas_effects::trace_spec::trace_obligation_identity(
                etas_effects::trace_spec::TraceObligationIdentityInput {
                    clause_index: 1,
                    ..identity_input(64)
                },
            )
            .unwrap();
            assert_ne!(identity.id(), later.id());
            let mut missing_sites = models.source_clause_sites.clone();
            missing_sites.remove(&identity.inputs().clause);
            assert!(matches!(
                etas_effects::trace_spec::trace_obligation_identity(
                    etas_effects::trace_spec::TraceObligationIdentityInput {
                        clause_sites: &missing_sites,
                        ..identity_input(64)
                    }
                ),
                Err(etas_effects::trace_spec::TraceObligationIdentityError::MissingClauseSite)
            ));
            assert!(matches!(
                etas_effects::trace_spec::trace_obligation_identity(identity_input(0)),
                Err(etas_effects::trace_spec::TraceObligationIdentityError::ResourceLimit)
            ));
            let alternatives = dispositions
                .analyzed_lowering_source(input.type_fact_index, method.body, &analysis, None)
                .unwrap();
            let TraceSpecClause::Deny { pattern, .. } = &alternatives[0][0] else {
                panic!("deny")
            };
            let Effect::AppliedAction(action) = pattern.row.effects.iter().next().unwrap() else {
                panic!("generic action")
            };
            let [etas_types::EffectArgRef::Type(ty)] = action.args.as_slice() else {
                panic!("type selector")
            };
            let type_limits = etas_code::residual::type_template::TypeTemplateLimits::default();
            let selector = etas_types::ty::symbolic_action_type_selector(
                &checked.type_store,
                *ty,
                &BTreeMap::from([(flow.type_params[0], parameter)]),
                type_limits,
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                selector,
                etas_code::residual::pattern::ActionSelectorTemplate::TypeParameter(parameter)
            );
            let template_limits = etas_code::residual::pattern::PatternTemplateLimits {
                pattern: limits.evidence.pattern,
                types: type_limits,
                descriptor: DescriptorLimits::default(),
                max_binding_steps: 64,
            };
            let mut projector = DefinitionDescriptorProjector::new(
                &checked,
                &identities,
                DescriptorLimits::default(),
            );
            projector
                .project_signature(
                    owner.clone(),
                    signature,
                    &names,
                    flow.return_type.map(|ty| checked.types.type_refs[&ty]),
                )
                .unwrap();
            let template = projector
                .finish_symbolic_trace_action(
                    &owner,
                    pattern,
                    &BTreeMap::from([(flow.type_params[0], parameter)]),
                    limits,
                    template_limits,
                )
                .unwrap();
            assert_eq!(template.selectors(), &[selector]);
            let concrete = etas_code::residual::type_template::StaticTypeTemplate::new(
                vec![
                    etas_code::residual::type_template::StaticTypeNode::Primitive(
                        etas_code::descriptor::PrimitiveInfo::I32,
                    ),
                ],
                0,
                type_limits,
            )
            .unwrap()
            .closed_descriptor_fingerprint(DescriptorLimits::default())
            .unwrap()
            .unwrap();
            let resolved_reference = reference
                .substitute(
                    &BTreeMap::from([(
                        parameter,
                        etas_code::residual::identity::GenericArgumentIdentity::Type(concrete),
                    )]),
                    16,
                )
                .unwrap();
            identity
                .validate_reference(&resolved_reference, 1, None)
                .unwrap();
            assert_eq!(resolved_reference.obligation(), identity.id());
            assert_eq!(
                resolved_reference.substitutions().get(&parameter),
                Some(&etas_code::residual::identity::GenericArgumentIdentity::Type(concrete))
            );
            let pattern = template
                .instantiate(
                    &BTreeMap::from([(
                        parameter,
                        etas_code::residual::identity::GenericArgumentIdentity::Type(concrete),
                    )]),
                    &etas_code::residual::type_binding::TypeArgumentDescriptors::default(),
                    template_limits,
                )
                .unwrap();
            assert_eq!(
                pattern.effect().arguments,
                vec![etas_code::contract::CanonicalSelector::Type(concrete)]
            );
            let mut projector = DefinitionDescriptorProjector::new(
                &checked,
                &identities,
                DescriptorLimits::default(),
            );
            projector
                .project_signature(
                    owner.clone(),
                    signature,
                    &names,
                    flow.return_type.map(|ty| checked.types.type_refs[&ty]),
                )
                .unwrap();
            let symbolic_trace = projector
                .finish_analyzed_symbolic_trace_template(
                    &owner,
                    input,
                    source_limits,
                    template_limits,
                )
                .unwrap();
            assert_eq!(symbolic_trace.sources.len(), 2);
            assert_eq!(
                symbolic_trace.source_scope().unwrap().owner,
                source_owner.id()
            );
            let etas_code::residual::monitor::SymbolicTraceRule::Before { guard, target } =
                &symbolic_trace.template.alternatives()[0][1]
            else {
                panic!("before")
            };
            assert!(
                matches!(guard.target(), etas_code::descriptor::EffectTargetInfo::Action { name, .. } if name == "a")
            );
            assert!(
                matches!(target.target(), etas_code::descriptor::EffectTargetInfo::Action { name, .. } if name == "b")
            );
            assert!(matches!(
                symbolic_trace.template.instantiate(&BTreeMap::new(), &etas_code::residual::type_binding::TypeArgumentDescriptors::default(), template_limits, limits),
                Err(etas_code::residual::monitor::SymbolicTraceError::Pattern {
                    error: etas_code::residual::pattern::ActionPatternTemplateError::MissingBinding(id), ..
                }) if id == parameter
            ));
            let mut limited_projector = DefinitionDescriptorProjector::new(
                &checked,
                &identities,
                DescriptorLimits::default(),
            );
            limited_projector
                .project_signature(
                    owner.clone(),
                    signature,
                    &names,
                    flow.return_type.map(|ty| checked.types.type_refs[&ty]),
                )
                .unwrap();
            let restricted = TraceSourceLinkLimits {
                identity: ObligationIdentityLimits {
                    max_generic_arguments: 0,
                    ..source_limits.identity
                },
                ..source_limits
            };
            assert!(matches!(
                limited_projector.finish_analyzed_symbolic_trace_template(
                    &owner,
                    input,
                    restricted,
                    template_limits
                ),
                Err(TraceTemplateProjectionError::AnalyzedSourceMismatch)
            ));
            let mut altered = (*checked).clone();
            let mut edit = altered.hir.clone().edit();
            let symbol = edit
                .program_mut()
                .symbols
                .get_mut(flow.type_params[0])
                .unwrap();
            let etas_hir::SymbolDef::TypeParam { param_index, .. } = &mut symbol.def else {
                panic!("type parameter")
            };
            *param_index = 99;
            altered.hir = edit.finish().unwrap();
            let mut altered_projector = DefinitionDescriptorProjector::new(
                &altered,
                &identities,
                DescriptorLimits::default(),
            );
            altered_projector
                .project_signature(
                    owner.clone(),
                    signature,
                    &names,
                    flow.return_type.map(|ty| checked.types.type_refs[&ty]),
                )
                .unwrap();
            assert!(matches!(
                altered_projector.finish_analyzed_symbolic_trace_template(
                    &owner,
                    input,
                    source_limits,
                    template_limits
                ),
                Err(TraceTemplateProjectionError::AnalyzedSourceMismatch)
            ));
            symbolic_trace
                .template
                .instantiate(
                    &BTreeMap::from([(
                        parameter,
                        etas_code::residual::identity::GenericArgumentIdentity::Type(concrete),
                    )]),
                    &etas_code::residual::type_binding::TypeArgumentDescriptors::default(),
                    template_limits,
                    limits,
                )
                .unwrap()
                .template_id()
        } else {
            let projection = result.unwrap();
            assert_eq!(projection.alternatives().len(), 2);
            assert!(matches!(
                projection.alternatives()[0][0].rule,
                AlternativeRule::Deny(_)
            ));
            projection.monitor().template_id()
        }
    }
}
