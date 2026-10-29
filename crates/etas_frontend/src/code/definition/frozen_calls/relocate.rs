use super::{projection::Projection, *};
use etas_code::{CodeFingerprint, DefinitionKey, descriptor::GenericOwner};
use etas_effects::*;
use etas_types::{
    EffectArgRef, TypeId,
    ty::{TypeLookupIndex, TypeRelocationInput},
};
use std::collections::{BTreeMap, HashMap};
mod captured;
mod graphs;

fn unit_parameters(hir: &etas_hir::HirRevision, unit: EffectUnit) -> &[etas_hir::SymbolId] {
    match unit {
        EffectUnit::Item(item) => match hir.items.get(item) {
            Some(etas_hir::HirItem::Flow(flow)) => &flow.params,
            Some(etas_hir::HirItem::Tool(tool)) => &tool.params,
            Some(etas_hir::HirItem::Agent(agent)) => &agent.params,
            _ => &[],
        },
        EffectUnit::AnonymousFlow { value, .. } => match hir.exprs.get(value) {
            Some(etas_hir::HirExpr::Lambda { params, .. }) => params,
            _ => &[],
        },
        EffectUnit::HandlerArm { .. } => &[],
    }
}

fn relocate_handler_scopes(
    handlers: Option<&[etas_effects::ParameterHandlerScopeRef]>,
    expressions: &HashMap<etas_hir::HirExprId, etas_hir::HirExprId>,
) -> Option<Vec<etas_effects::ParameterHandlerScopeRef>> {
    handlers?
        .iter()
        .map(|handler| match handler {
            etas_effects::ParameterHandlerScopeRef::Native(handler) => expressions
                .get(handler)
                .copied()
                .map(etas_effects::ParameterHandlerScopeRef::Native),
            external @ etas_effects::ParameterHandlerScopeRef::External { .. } => Some(*external),
        })
        .collect()
}

pub(super) struct Relocation<'a> {
    original: &'a crate::CheckedProgram,
    target: DescriptorInput<'a>,
    from: &'a Projection,
    to: &'a Projection,
    types: BTreeMap<GenericOwner, HashMap<TypeId, TypeId>>,
    target_types: TypeLookupIndex,
    effect_names: HashMap<String, String>,
    tags: BTreeMap<DefinitionKey, EffectTagId>,
    actions: BTreeMap<(DefinitionKey, String), ActionRef>,
    returned_items: HashMap<HirItemId, HirItemId>,
    returned_anonymous: BTreeMap<EffectUnit, EffectUnit>,
    parameters: HashMap<etas_hir::SymbolId, etas_hir::SymbolId>,
    native_exprs: HashMap<etas_hir::HirExprId, etas_hir::HirExprId>,
}

impl<'a> Relocation<'a> {
    pub fn new(
        original: &'a crate::CheckedProgram,
        target: DescriptorInput<'a>,
        from: &'a Projection,
        to: &'a Projection,
        overlay: &crate::code::draft::DefaultBodyOverlay,
    ) -> Result<Self, ProjectionError> {
        let mut target_types = BTreeMap::<(GenericOwner, CodeFingerprint), TypeId>::new();
        for origin in &to.native.type_origins {
            let key = (
                origin.scope.clone(),
                to.native.graph.type_fingerprint(origin.descriptor)?,
            );
            if target_types
                .insert(key, origin.local)
                .is_some_and(|previous| previous != origin.local)
            {
                return Err(invalid(
                    "canonical type has ambiguous native target identities",
                ));
            }
        }
        let mut types: BTreeMap<GenericOwner, HashMap<TypeId, TypeId>> = BTreeMap::new();
        for origin in &from.native.type_origins {
            let key = (
                origin.scope.clone(),
                from.native.graph.type_fingerprint(origin.descriptor)?,
            );
            if let Some(target) = target_types.get(&key) {
                types
                    .entry(origin.scope.clone())
                    .or_default()
                    .insert(origin.local, *target);
            }
        }
        let mut standard_targets = HashMap::new();
        for (ty, declaration) in &to.identities.types {
            if declaration.standard.is_some()
                && standard_targets
                    .insert(declaration.identity.clone(), *ty)
                    .is_some_and(|previous| previous != *ty)
            {
                return Err(invalid(
                    "standard declaration has conflicting checked target types",
                ));
            }
        }
        for id in from.callables.keys() {
            let scope = projection::owner(id)?;
            let mappings = types.entry(scope).or_default();
            for (ty, declaration) in &from.identities.types {
                if declaration.standard.is_some() {
                    let target = standard_targets.get(&declaration.identity).ok_or_else(|| {
                        invalid("standard summary type has no checked target declaration")
                    })?;
                    mappings.insert(*ty, *target);
                }
            }
        }
        let mut tags = BTreeMap::new();
        for tag in target.effect_registry.tags() {
            let identity = to.identities.effect_tag(tag.id)?.clone();
            if tags
                .insert(identity, tag.id)
                .is_some_and(|previous| previous != tag.id)
            {
                return Err(invalid("frozen effect identity has ambiguous target tags"));
            }
        }
        let mut actions = BTreeMap::new();
        for action in target.effect_registry.action_signatures() {
            let owner = to.identities.effect_tag(action.owner)?.clone();
            let reference = ActionRef {
                tag: action.owner,
                action: action.id,
            };
            if actions
                .insert((owner, action.name.clone()), reference.clone())
                .is_some_and(|previous| previous != reference)
            {
                return Err(invalid(
                    "frozen action identity has ambiguous target descriptors",
                ));
            }
        }
        let mut effect_names = std::collections::HashMap::new();
        for (name, tag) in original.effect_registry.named_tags() {
            let identity = from.identities.effect_tag(tag)?;
            if let Some(target_tag) = tags.get(identity) {
                let name_in_target = target
                    .effect_registry
                    .tag_name(*target_tag)
                    .ok_or_else(|| invalid("frozen target effect has no canonical name"))?;
                effect_names.insert(name.into(), name_in_target.into());
            }
        }
        for (name, action) in original.effect_registry.named_actions() {
            let signature = original
                .effect_registry
                .action_signature(action)
                .ok_or_else(|| invalid("frozen source action has no descriptor"))?;
            let owner = from.identities.effect_tag(signature.owner)?;
            if let Some(target_action) = actions.get(&(owner.clone(), signature.name.clone())) {
                let owner = target
                    .effect_registry
                    .tag_name(target_action.tag)
                    .ok_or_else(|| invalid("frozen target action has no canonical owner"))?;
                effect_names.insert(name.into(), format!("{owner}.{}", signature.name));
            }
        }
        let mut returned_items: HashMap<_, _> = from
            .callables
            .iter()
            .filter_map(|(identity, source)| {
                to.callables.get(identity).map(|target| (*source, *target))
            })
            .collect();
        for (identity, source) in &from.values {
            let target = to.values.get(identity).ok_or_else(|| {
                invalid("frozen value is absent from the candidate declaration world")
            })?;
            if returned_items.insert(*source, *target).is_some() {
                return Err(invalid(
                    "frozen return source has conflicting native declarations",
                ));
            }
        }
        let mut target_anonymous = HashMap::new();
        let unchanged_default = overlay
            .native_revision()
            .unchanged
            .then(|| overlay.frozen().default_body())
            .flatten()
            .and_then(|owner| returned_items.get(&owner).copied());
        for unit in EffectUnit::collect(target.hir) {
            if let EffectUnit::AnonymousFlow { owner, value, .. } = unit {
                let mut span = target
                    .hir
                    .exprs
                    .get(value)
                    .ok_or_else(|| invalid("anonymous return source has no expression"))?
                    .span(&target.hir.blocks);
                if Some(owner) == unchanged_default
                    && span.source == overlay.relocated().source().id
                {
                    span.source = overlay.slot().source;
                }
                if target_anonymous.insert((owner, span), unit).is_some() {
                    return Err(invalid(
                        "anonymous return source has ambiguous source ownership",
                    ));
                }
            }
        }
        let mut returned_anonymous = BTreeMap::new();
        for unit in EffectUnit::collect(&original.hir) {
            if let EffectUnit::AnonymousFlow { owner, value, .. } = unit
                && let Some(target_owner) = returned_items.get(&owner)
            {
                let span = original
                    .hir
                    .exprs
                    .get(value)
                    .ok_or_else(|| invalid("frozen anonymous return source has no expression"))?
                    .span(&original.hir.blocks);
                if let Some(target) = target_anonymous.get(&(*target_owner, span)) {
                    returned_anonymous.insert(unit, *target);
                }
            }
        }
        let mut parameters = HashMap::new();
        let pairs = returned_items
            .iter()
            .map(|(from, to)| (EffectUnit::Item(*from), EffectUnit::Item(*to)))
            .chain(returned_anonymous.iter().map(|(from, to)| (*from, *to)));
        for (from, to) in pairs {
            let source_params = unit_parameters(&original.hir, from);
            let target_params = unit_parameters(target.hir, to);
            if source_params.len() != target_params.len() {
                return Err(invalid(
                    "frozen callable parameter arity changed during relocation",
                ));
            }
            for (source, target) in source_params.iter().zip(target_params) {
                if parameters
                    .insert(*source, *target)
                    .is_some_and(|previous| previous != *target)
                {
                    return Err(invalid(
                        "frozen parameter has conflicting checked target identities",
                    ));
                }
            }
        }
        Ok(Self {
            native_exprs: captured::expression_bindings(original, target, &returned_items, overlay),
            original,
            target,
            from,
            to,
            types,
            target_types: TypeLookupIndex::new(target.type_store),
            effect_names,
            tags,
            actions,
            returned_items,
            returned_anonymous,
            parameters,
        })
    }

    fn ty(&self, scope: &GenericOwner, ty: TypeId) -> Result<TypeId, ProjectionError> {
        let declarations = self
            .types
            .get(scope)
            .ok_or_else(|| invalid("frozen callable has no checked type relocation scope"))?;
        if let Some(target) = declarations.get(&ty) {
            return Ok(*target);
        }
        etas_types::TypeSubstitutionEngine::relocating(TypeRelocationInput {
            source: &self.original.type_store,
            target: self.target.type_store,
            declaration_types: declarations,
            target_types: &self.target_types,
            effect_names: &self.effect_names,
        })
        .substitute(ty)
        .map_err(|error| {
            invalid(&format!(
                "frozen summary type relocation failed in {scope:?}: {error}; source {:?}",
                self.original.type_store.get(ty)
            ))
        })
    }

    fn tag(&self, tag: EffectTagId) -> Result<EffectTagId, ProjectionError> {
        self.tags
            .get(self.from.identities.effect_tag(tag)?)
            .copied()
            .ok_or_else(|| invalid("frozen effect has no canonical target binding"))
    }

    fn action(&self, action: &ActionRef) -> Result<ActionRef, ProjectionError> {
        let signature = self
            .original
            .effect_registry
            .action_signature(action)
            .ok_or_else(|| invalid("frozen action has no checked descriptor"))?;
        let owner = self.from.identities.effect_tag(signature.owner)?.clone();
        self.actions
            .get(&(owner, signature.name.clone()))
            .cloned()
            .ok_or_else(|| invalid("frozen action has no canonical target binding"))
    }

    fn effect(&self, effect: &Effect, scope: &GenericOwner) -> Result<Effect, ProjectionError> {
        Ok(match effect {
            Effect::Tag(tag) => Effect::Tag(self.tag(*tag)?),
            Effect::Applied { tag, args } => Effect::Applied {
                tag: self.tag(*tag)?,
                args: args
                    .iter()
                    .map(|ty| self.ty(scope, *ty))
                    .collect::<Result<_, _>>()?,
            },
            Effect::Action(action) => Effect::Action(self.action(action)?),
            Effect::AppliedAction(action) => Effect::AppliedAction(ActionInstanceRef {
                action: self.action(&action.action)?,
                args: action
                    .args
                    .iter()
                    .map(|arg| {
                        Ok(match arg {
                            EffectArgRef::Type(ty) => EffectArgRef::Type(self.ty(scope, *ty)?),
                            other => other.clone(),
                        })
                    })
                    .collect::<Result<_, ProjectionError>>()?,
            }),
            Effect::Error(ty) => Effect::Error(self.ty(scope, *ty)?),
            Effect::Var(variable) => Effect::Var(*variable),
        })
    }

    fn row(&self, row: &EffectRow, scope: &GenericOwner) -> Result<EffectRow, ProjectionError> {
        Ok(EffectRow {
            effects: row
                .effects
                .iter()
                .map(|effect| self.effect(effect, scope))
                .collect::<Result<_, _>>()?,
            open: row.open,
        })
    }

    pub(in crate::code::definition::frozen_calls) fn reference_bindings(
        &self,
        reference: &CallableReturnReference,
        scope: &GenericOwner,
    ) -> Result<super::ReferenceBindings, ProjectionError> {
        let target = match &reference.target {
            CallableReferenceTarget::Source(item) => {
                let item =
                    self.returned_items.get(item).copied().ok_or_else(|| {
                        invalid("returned reference has no canonical target binding")
                    })?;
                CallableReferenceTarget::Source(item)
            }
            CallableReferenceTarget::StdIntrinsic(intrinsic) => {
                CallableReferenceTarget::StdIntrinsic(*intrinsic)
            }
        };
        let type_bindings = reference
            .type_bindings
            .iter()
            .map(|(name, ty)| Ok((name.clone(), self.ty(scope, *ty)?)))
            .collect::<Result<_, ProjectionError>>()?;
        let effect_row_bindings = reference
            .effect_row_bindings
            .iter()
            .map(|(name, row)| Ok((name.clone(), self.reference_row(row, scope)?)))
            .collect::<Result<_, ProjectionError>>()?;
        Ok(super::ReferenceBindings {
            owner: *self
                .returned_items
                .get(&reference.owner)
                .ok_or_else(|| invalid("returned reference has no canonical creation owner"))?,
            target,
            signature: self.ty(scope, reference.signature)?,
            types: type_bindings,
            rows: effect_row_bindings,
        })
    }

    fn returned_reference(
        &self,
        reference: &CallableReturnReference,
        scope: &GenericOwner,
    ) -> Result<CallableReturnReference, ProjectionError> {
        let binding = self.reference_bindings(reference, scope)?;
        Ok(CallableReturnReference {
            value: *self.native_exprs.get(&reference.value).ok_or_else(|| {
                invalid("returned reference has no unchanged creation expression")
            })?,
            owner: binding.owner,
            environment: reference.environment,
            target: binding.target,
            signature: binding.signature,
            type_bindings: binding.types,
            effect_row_bindings: binding.rows,
        })
    }

    fn reference_row(
        &self,
        row: &etas_types::EffectRowRef,
        scope: &GenericOwner,
    ) -> Result<etas_types::EffectRowRef, ProjectionError> {
        let effects = row
            .effects
            .iter()
            .map(|effect| {
                let name = self
                    .effect_names
                    .get(&effect.name)
                    .cloned()
                    .ok_or_else(|| {
                        invalid("returned reference effect has no canonical target binding")
                    })?;
                let args = effect
                    .args
                    .iter()
                    .map(|arg| {
                        Ok(match arg {
                            EffectArgRef::Type(ty) => EffectArgRef::Type(self.ty(scope, *ty)?),
                            other => other.clone(),
                        })
                    })
                    .collect::<Result<_, ProjectionError>>()?;
                Ok(etas_types::EffectRef { name, args })
            })
            .collect::<Result<_, ProjectionError>>()?;
        Ok(etas_types::EffectRowRef {
            effects,
            tail: row.tail.clone(),
        })
    }

    fn trace(
        &self,
        root: &ActionTraceDomain,
        scope: &GenericOwner,
    ) -> Result<ActionTraceDomain, ProjectionError> {
        enum Work<'t> {
            Visit(&'t ActionTraceDomain),
            Seq(usize),
            Choice(usize),
            Repeat,
        }
        let mut work = vec![Work::Visit(root)];
        let mut values = Vec::new();
        while let Some(task) = work.pop() {
            match task {
                Work::Visit(trace) => match trace {
                    ActionTraceDomain::Empty => values.push(ActionTraceDomain::Empty),
                    ActionTraceDomain::Event(event) => {
                        values.push(ActionTraceDomain::Event(ActionEvent {
                            action: self.effect(&event.action, scope)?,
                            span: event.span,
                            source: event.source.clone(),
                        }))
                    }
                    ActionTraceDomain::ParameterCall {
                        target,
                        parameter,
                        span,
                    } => values.push(ActionTraceDomain::ParameterCall {
                        target: self.parameter_target(*target)?,
                        parameter: parameter.clone(),
                        span: *span,
                    }),
                    ActionTraceDomain::Seq(parts) | ActionTraceDomain::Choice(parts) => {
                        work.push(if matches!(trace, ActionTraceDomain::Seq(_)) {
                            Work::Seq(parts.len())
                        } else {
                            Work::Choice(parts.len())
                        });
                        work.extend(parts.iter().rev().map(Work::Visit));
                    }
                    ActionTraceDomain::Repeat(inner) => {
                        work.push(Work::Repeat);
                        work.push(Work::Visit(inner));
                    }
                    ActionTraceDomain::UnknownOrder(actions) => {
                        values.push(ActionTraceDomain::UnknownOrder(
                            actions
                                .iter()
                                .map(|action| self.effect(action, scope))
                                .collect::<Result<_, _>>()?,
                        ))
                    }
                    ActionTraceDomain::Widened {
                        actions,
                        parameter_calls,
                    } => values.push(ActionTraceDomain::Widened {
                        actions: actions
                            .iter()
                            .map(|action| self.effect(action, scope))
                            .collect::<Result<_, _>>()?,
                        parameter_calls: parameter_calls
                            .iter()
                            .map(|call| {
                                Ok(etas_effects::infer::domain::ParameterCallRef {
                                    target: self.parameter_target(call.target)?,
                                    parameter: call.parameter.clone(),
                                    span: call.span,
                                })
                            })
                            .collect::<Result<_, ProjectionError>>()?,
                    }),
                },
                Work::Seq(count) | Work::Choice(count) => {
                    let start = values
                        .len()
                        .checked_sub(count)
                        .ok_or_else(|| invalid("frozen trace relocation lost child nodes"))?;
                    let parts = values.split_off(start);
                    values.push(if matches!(task, Work::Seq(_)) {
                        ActionTraceDomain::Seq(parts)
                    } else {
                        ActionTraceDomain::Choice(parts)
                    });
                }
                Work::Repeat => {
                    let inner = values
                        .pop()
                        .ok_or_else(|| invalid("frozen repeat trace has no child"))?;
                    values.push(ActionTraceDomain::Repeat(Box::new(inner)));
                }
            }
        }
        if values.len() != 1 {
            return Err(invalid("frozen trace relocation has invalid roots"));
        }
        values
            .pop()
            .ok_or_else(|| invalid("frozen trace relocation has no root"))
    }

    fn parameter_target(
        &self,
        target: ParameterCallTarget,
    ) -> Result<ParameterCallTarget, ProjectionError> {
        match target {
            ParameterCallTarget::Native(symbol) => self
                .parameters
                .get(&symbol)
                .copied()
                .map(ParameterCallTarget::Native)
                .ok_or_else(|| invalid("frozen trace parameter has no checked target identity")),
            ParameterCallTarget::Graph { .. } => Ok(target),
            ParameterCallTarget::Interface(_) => Err(invalid(
                "an uninstantiated external interface cannot be a native frozen trace",
            )),
        }
    }

    fn variant(&self, symbol: etas_hir::SymbolId) -> Result<etas_hir::SymbolId, ProjectionError> {
        let source = self
            .original
            .symbols
            .get(symbol)
            .ok_or(ProjectionError::MissingSymbol(symbol))?;
        let etas_hir::SymbolDef::EnumVariant {
            enum_item,
            variant_index,
        } = source.def
        else {
            return Err(invalid("frozen outcome has no checked enum constructor"));
        };
        let owner = self
            .original
            .hir
            .items
            .get(enum_item)
            .and_then(etas_hir::HirItem::symbol)
            .and_then(|symbol| self.from.identities.symbol(symbol))
            .ok_or_else(|| invalid("frozen enum has no canonical identity"))?;
        let mut targets = self.target.hir.items.iter().filter_map(|(_, item)| {
            let etas_hir::HirItem::Enum(declaration) = item else {
                return None;
            };
            (self.to.identities.symbol(declaration.symbol) == Some(owner)).then_some(declaration)
        });
        let declaration = targets
            .next()
            .ok_or_else(|| invalid("frozen enum has no target declaration"))?;
        if targets.next().is_some() {
            return Err(invalid("frozen enum target is ambiguous"));
        }
        let variant = declaration
            .variants
            .get(variant_index as usize)
            .ok_or_else(|| invalid("frozen enum constructor slot changed"))?;
        let target = self
            .target
            .hir
            .symbols
            .get(variant.symbol)
            .ok_or(ProjectionError::MissingSymbol(variant.symbol))?;
        if target.name != source.name {
            return Err(invalid("frozen enum constructor identity changed"));
        }
        Ok(variant.symbol)
    }

    fn requirements(
        &self,
        source: &RequirementSet,
        scope: &GenericOwner,
    ) -> Result<RequirementSet, ProjectionError> {
        source
            .iter()
            .map(|fact| {
                Ok(match fact {
                    RequirementFact::Limit(limit) => RequirementFact::Limit(limit.clone()),
                    RequirementFact::TraceSpec(clause) => {
                        RequirementFact::TraceSpec(match clause {
                            TraceSpecClauseFact::Allow { pattern, label } => {
                                TraceSpecClauseFact::Allow {
                                    pattern: self.row(pattern, scope)?,
                                    label: label.clone(),
                                }
                            }
                            TraceSpecClauseFact::Deny { pattern, label } => {
                                TraceSpecClauseFact::Deny {
                                    pattern: self.row(pattern, scope)?,
                                    label: label.clone(),
                                }
                            }
                            TraceSpecClauseFact::RequireBefore {
                                guard,
                                guard_label,
                                target,
                                target_label,
                            } => TraceSpecClauseFact::RequireBefore {
                                guard: self.row(guard, scope)?,
                                guard_label: guard_label.clone(),
                                target: self.row(target, scope)?,
                                target_label: target_label.clone(),
                            },
                            TraceSpecClauseFact::RequireAfter {
                                target,
                                target_label,
                                obligation,
                                obligation_label,
                            } => TraceSpecClauseFact::RequireAfter {
                                target: self.row(target, scope)?,
                                target_label: target_label.clone(),
                                obligation: self.row(obligation, scope)?,
                                obligation_label: obligation_label.clone(),
                            },
                            other => other.clone(),
                        })
                    }
                })
            })
            .collect()
    }

    pub fn summary(
        &self,
        source: &EffectSummary,
        scope: &GenericOwner,
        source_owner: HirItemId,
        returned: Option<&std::collections::BTreeSet<CallableReturnSource>>,
    ) -> Result<EffectSummary, ProjectionError> {
        let default_handler_effects = source
            .default_handler_effects
            .as_ref()
            .map(|effects| {
                Ok::<_, ProjectionError>(DefaultHandlerEffects {
                    direct: self.row(&effects.direct, scope)?,
                    handlers: effects
                        .handlers
                        .iter()
                        .map(|handler| {
                            Ok(DefaultHandlerContribution {
                                action: self.effect(&handler.action, scope)?,
                                produced: self.row(&handler.produced, scope)?,
                            })
                        })
                        .collect::<Result<_, ProjectionError>>()?,
                })
            })
            .transpose()?;
        let exits = source
            .exits
            .as_ref()
            .map(|exits| {
                Ok::<_, ProjectionError>(CallableExitSummary {
                    handler_gaps: exits.handler_gaps.iter().map(|evidence| {
                        Ok(etas_effects::HandlerGapEvidence {
                            source: evidence.source.clone(),
                            normal_control: evidence.normal_control,
                            handled: self.row(&evidence.handled, scope)?,
                            body_escaping: self.row(&evidence.body_escaping, scope)?,
                        })
                    }).collect::<Result<_, ProjectionError>>()?,
                    parameter_exits: exits.parameter_exits.iter().map(|exit| {
                        Ok(etas_effects::ParameterExitFact {
                            target: self.parameter_target(exit.target)?,
                            output: self.ty(scope, exit.output)?,
                            source: exit.source.clone(),
                            handler_free: exit.handler_free,
                            enclosing_handled: exit.enclosing_handled.as_ref().map(|row| self.row(row, scope)).transpose()?,
                            enclosing_handlers: relocate_handler_scopes(exit.enclosing_handlers.as_deref(), &self.native_exprs),
                            arguments: exit.arguments.as_ref().map(|arguments| arguments.iter().map(|argument| Ok(etas_effects::ParameterExitArgument {
                                name: argument.name.clone(), ty: self.ty(scope, argument.ty)?, source: argument.source.clone(), value: argument.value.clone(),
                            })).collect::<Result<Vec<_>, ProjectionError>>()).transpose()?,
                            prefix: self.trace(&exit.prefix, scope)?,
                        })
                    }).collect::<Result<_, ProjectionError>>()?,
                    exits: exits
                        .exits
                        .iter()
                        .map(|exit| {
                            Ok(ExitTraceFact {
                                outcome: match &exit.outcome {
                                    CallableOutcome::Returned { ty, observation } => {
                                        CallableOutcome::Returned {
                                            ty: self.ty(scope, *ty)?,
                                            observation: match observation {
                                                ReturnObservation::Variant(symbol) => {
                                                    ReturnObservation::Variant(
                                                        self.variant(*symbol)?,
                                                    )
                                                }
                                                ReturnObservation::CallableSources(sources) => {
                                                    if let Some(returned) = returned {
                                                        ReturnObservation::CallableSources(returned.clone())
                                                    } else {
                                                    ReturnObservation::CallableSources(sources.iter().map(|source| {
                                                        Ok(match source {
                                                            CallableReturnSource::Captured { unit, environment } => CallableReturnSource::Captured { unit: self.captured_unit(*unit)?, environment: *environment },
                                                            CallableReturnSource::Parameter(symbol) => CallableReturnSource::Parameter(
                                                                self.parameters.get(symbol).copied().ok_or_else(|| invalid("frozen returned parameter has no checked target identity"))?
                                                            ),
                                                            CallableReturnSource::SourceItem(item) => CallableReturnSource::SourceItem(
                                                                *self.returned_items.get(item).ok_or_else(|| invalid("returned callable has no canonical target binding"))?
                                                            ),
                                                            CallableReturnSource::Reference(reference) => CallableReturnSource::Reference(self.returned_reference(reference, scope)?),
                                                            CallableReturnSource::AnonymousFlow(unit) => CallableReturnSource::AnonymousFlow(
                                                                *self.returned_anonymous.get(unit).ok_or_else(|| invalid("frozen anonymous return source has no unchanged canonical body binding"))?
                                                            ),
                                                            other => other.clone(),
                                                        })
                                                    }).collect::<Result<_, ProjectionError>>()?)
                                                    }
                                                }
                                                other => other.clone(),
                                            },
                                        }
                                    }
                                    CallableOutcome::Raised { error } => CallableOutcome::Raised {
                                        error: self.ty(scope, *error)?,
                                    },
                                    CallableOutcome::NonReturn => CallableOutcome::NonReturn,
                                    CallableOutcome::HandlerFinished { .. } | CallableOutcome::HandlerResumed { .. } => return Err(invalid("handler-local control cannot escape into a frozen callable summary")),
                                },
                                trace: self.trace(&exit.trace, scope)?,
                            })
                        })
                        .collect::<Result<_, ProjectionError>>()?,
                    // A source-body gap cannot be rebound to a coincidentally equal
                    // expression ID in the candidate. Preserve the unproved callee.
                    gaps: if exits.gaps.is_empty() {
                        Default::default()
                    } else {
                        let span = self
                            .original
                            .hir
                            .items
                            .get(source_owner)
                            .ok_or_else(|| invalid("frozen callee declaration is missing"))?
                            .span();
                        exits.gaps.iter().map(|gap| {
                            Ok(match gap {
                                OutcomeAnalysisGap::ParameterCall { target, output, source } => OutcomeAnalysisGap::ParameterCall {
                                    target: self.parameter_target(*target)?, output: self.ty(scope, *output)?, source: source.clone(),
                                },
                                OutcomeAnalysisGap::Handler(source) => OutcomeAnalysisGap::Handler(source.clone()),
                                _ => OutcomeAnalysisGap::FrozenCallee(span.into()),
                            })
                        }).collect::<Result<_, ProjectionError>>()?
                    },
                })
            })
            .transpose()?;
        Ok(EffectSummary {
            handler_requests: match &source.handler_requests {
                RequestInstantiationSummary::Unavailable => {
                    RequestInstantiationSummary::Unavailable
                }
                RequestInstantiationSummary::Available {
                    actions,
                    parameters,
                } => RequestInstantiationSummary::Available {
                    actions: actions
                        .iter()
                        .map(|instance| {
                            Ok(ActionRequestInstance {
                                action: self.effect(&instance.action, scope)?,
                                signature: instance
                                    .signature
                                    .as_ref()
                                    .map(|signature| {
                                        Ok::<_, ProjectionError>(
                                            etas_types::ActionRequestSignature {
                                                params: signature
                                                    .params
                                                    .iter()
                                                    .map(|ty| self.ty(scope, *ty))
                                                    .collect::<Result<_, _>>()?,
                                                output: self.ty(scope, signature.output)?,
                                            },
                                        )
                                    })
                                    .transpose()?,
                                type_bindings: instance
                                    .type_bindings
                                    .as_ref()
                                    .map(|bindings| {
                                        bindings
                                            .iter()
                                            .map(|(name, ty)| {
                                                Ok((name.clone(), self.ty(scope, *ty)?))
                                            })
                                            .collect::<Result<_, ProjectionError>>()
                                    })
                                    .transpose()?,
                            })
                        })
                        .collect::<Result<_, ProjectionError>>()?,
                    parameters: parameters
                        .iter()
                        .map(|parameter| {
                            Ok(ParameterRequest {
                                target: self.parameter_target(parameter.target)?,
                                parameter: parameter.parameter.clone(),
                                span: parameter.span,
                                excluded: parameter
                                    .excluded
                                    .iter()
                                    .map(|effect| self.effect(effect, scope))
                                    .collect::<Result<_, _>>()?,
                            })
                        })
                        .collect::<Result<_, ProjectionError>>()?,
                },
            },
            default_handler_effects,
            exits,
            escaping_effects: self.row(&source.escaping_effects, scope)?,
            requested_actions: self.row(&source.requested_actions, scope)?,
            default_actions: self.row(&source.default_actions, scope)?,
            handled_actions: self.row(&source.handled_actions, scope)?,
            action_trace: self.trace(&source.action_trace, scope)?,
            residual_checks: source.residual_checks.clone(),
            trace_spec_obligations: self.requirements(&source.trace_spec_obligations, scope)?,
            requirements: self.requirements(&source.requirements, scope)?,
            determinism: source.determinism,
            independent_determinism: source.independent_determinism,
            support: source.support.clone(),
        })
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;
    use etas_hir::HirExprId;

    #[test]
    fn relocates_ordered_scopes_only_when_every_expression_is_checked() {
        use etas_effects::ParameterHandlerScopeRef::{External, Native};
        let expressions =
            HashMap::from([(HirExprId(1), HirExprId(11)), (HirExprId(2), HirExprId(12))]);
        let external = External {
            graph: etas_code::CodeFingerprint::from_bytes([7; 32]),
            parameter_call: 3,
            scope: 1,
        };
        assert_eq!(
            relocate_handler_scopes(
                Some(&[external, Native(HirExprId(2)), Native(HirExprId(1))]),
                &expressions
            ),
            Some(vec![external, Native(HirExprId(12)), Native(HirExprId(11))])
        );
        assert_eq!(
            relocate_handler_scopes(Some(&[]), &expressions),
            Some(vec![])
        );
        assert_eq!(
            relocate_handler_scopes(
                Some(&[Native(HirExprId(1)), Native(HirExprId(3))]),
                &expressions
            ),
            None
        );
        assert_eq!(relocate_handler_scopes(None, &expressions), None);
    }
}
