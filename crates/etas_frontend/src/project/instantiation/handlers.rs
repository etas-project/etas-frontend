//! Materialize known request instances without constraining parametric handlers.
//! A missing request summary never changes whether a handler type-checks.
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap},
};

use etas_effects::{ActionRef, Effect, EffectFacts, RequestInstantiationSummary};
use etas_hir::{HirExprId, HirHandlerArmId, HirOwner, HirRevision};
use etas_types::{CallableGenericParamKind, EffectRowRef, TypeId, TypeOutput};

use super::InstantiationError;

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct Arguments {
    pub types: BTreeMap<TypeId, TypeId>,
    pub rows: HashMap<String, EffectRowRef>,
}

impl Arguments {
    pub fn apply<'a>(
        &self,
        interner: &mut etas_types::TypeInterner,
        names: &HashMap<String, TypeId>,
        parameters: &'a HashMap<TypeId, TypeId>,
        rows: &'a HashMap<String, EffectRowRef>,
        depth: usize,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<
        (
            Cow<'a, HashMap<TypeId, TypeId>>,
            Cow<'a, HashMap<String, EffectRowRef>>,
        ),
        InstantiationError,
    > {
        if self.types.is_empty() && self.rows.is_empty() {
            return Ok((Cow::Borrowed(parameters), Cow::Borrowed(rows)));
        }
        let mut result = parameters.clone();
        let mut result_rows = rows.clone();
        let mut substitution =
            etas_types::TypeSubstitutionEngine::materializing(interner, names, None)
                .with_type_parameters(parameters)
                .with_effect_arguments(rows)
                .with_expansion_depth(depth)
                .with_execution_control(control);
        for (parameter, argument) in &self.types {
            result.insert(*parameter, substitution.substitute(*argument)?);
        }
        for (parameter, argument) in &self.rows {
            result_rows.insert(
                parameter.clone(),
                substitution.substitute_effect_row(argument.clone())?,
            );
        }
        Ok((Cow::Owned(result), Cow::Owned(result_rows)))
    }
}

#[derive(Default)]
pub(crate) struct HandlerContexts {
    arms: BTreeMap<HirHandlerArmId, Vec<Arguments>>,
}

fn action(effect: &Effect) -> Option<&ActionRef> {
    match effect {
        Effect::Action(action) => Some(action),
        Effect::AppliedAction(instance) => Some(&instance.action),
        _ => None,
    }
}

impl HandlerContexts {
    pub fn new(types: &TypeOutput, effects: &EffectFacts) -> Self {
        let requests = effects
            .handled_body_requests
            .values()
            .filter_map(|summary| match summary {
                RequestInstantiationSummary::Available { actions, .. } => Some(actions),
                RequestInstantiationSummary::Unavailable => None,
            })
            .flatten()
            .collect::<BTreeSet<_>>();
        let mut arms = BTreeMap::new();
        for (arm, handler) in &types.facts.handler_actions {
            let Some(target) = effects.handler_action_effects.get(arm).and_then(action) else {
                continue;
            };
            let Some(signature) = types.facts.action_signatures.get(&handler.action_symbol) else {
                continue;
            };
            if signature.generic_params.is_empty() {
                continue;
            }
            let mut instances = Vec::new();
            for request in &requests {
                if action(&request.action) != Some(target) {
                    continue;
                }
                let Some(bindings) = &request.type_bindings else {
                    continue;
                };
                let names = bindings.iter().cloned().collect::<HashMap<_, _>>();
                let mut arguments = Arguments::default();
                for parameter in &signature.generic_params {
                    if parameter.kind == CallableGenericParamKind::Type {
                        if let Some(actual) = names.get(&parameter.name) {
                            arguments.types.insert(parameter.subject, *actual);
                        }
                    }
                }
                let rows = signature
                    .generic_params
                    .iter()
                    .filter(|parameter| parameter.kind == CallableGenericParamKind::Effect)
                    .map(|parameter| parameter.name.clone())
                    .collect::<Vec<_>>();
                if !rows.is_empty() {
                    let Some(actual) = &request.signature else {
                        continue;
                    };
                    let mut complete = true;
                    for (expected, actual) in signature
                        .params
                        .iter()
                        .zip(&actual.params)
                        .chain(std::iter::once((&signature.output, &actual.output)))
                    {
                        let inferred = etas_types::solver::effect_row::infer_bindings_from_types(
                            &types.store,
                            *expected,
                            *actual,
                            &names,
                            &Default::default(),
                            &rows,
                            &mut arguments.rows,
                        );
                        complete &= inferred.matches
                            && inferred.deferred.is_empty()
                            && inferred.deferred_fixed.is_empty();
                    }
                    if !complete || rows.iter().any(|name| !arguments.rows.contains_key(name)) {
                        continue;
                    }
                }
                if arguments != Arguments::default() && !instances.contains(&arguments) {
                    instances.push(arguments);
                }
            }
            if !instances.is_empty() {
                arms.insert(*arm, instances);
            }
        }
        Self { arms }
    }

    pub(super) fn at(
        &self,
        hir: &HirRevision,
        site: HirExprId,
        limit: usize,
    ) -> Result<Vec<Arguments>, InstantiationError> {
        if self.arms.is_empty() {
            return Ok(vec![Arguments::default()]);
        }
        let index = hir.index();
        let mut owner = HirOwner::Expr(site);
        let mut parents = Vec::new();
        loop {
            owner = match owner {
                HirOwner::HandlerArm(arm) => {
                    parents.push(arm);
                    let Some(owner) = index.handler_arm_owner.get(&arm) else {
                        break;
                    };
                    *owner
                }
                HirOwner::Body(body) => *index.body_owner.get(&body).expect("checked body owner"),
                HirOwner::Block(block) => {
                    *index.block_owner.get(&block).expect("checked block owner")
                }
                HirOwner::Stmt(stmt) => HirOwner::Block(
                    *index
                        .stmt_block
                        .get(&stmt)
                        .expect("checked statement block"),
                ),
                HirOwner::Expr(expr) => *index
                    .expr_owner
                    .get(&expr)
                    .expect("checked expression owner"),
                HirOwner::Pattern(pat) => {
                    *index.pat_owner.get(&pat).expect("checked pattern owner")
                }
                HirOwner::Item(_) | HirOwner::Module(_) => break,
            };
        }
        let mut contexts = vec![Arguments::default()];
        for arm in parents.into_iter().rev() {
            let Some(instances) = self.arms.get(&arm) else {
                continue;
            };
            // Each matched request binds the arm's generic parameters. The
            // arm's unspecialized signature is not another invocation.
            let previous = std::mem::take(&mut contexts);
            for outer in &previous {
                for instance in instances {
                    let mut context = outer.clone();
                    context.types.extend(
                        instance
                            .types
                            .iter()
                            .map(|(parameter, argument)| (*parameter, *argument)),
                    );
                    context.rows.extend(
                        instance
                            .rows
                            .iter()
                            .map(|(name, row)| (name.clone(), row.clone())),
                    );
                    if contexts.contains(&context) {
                        continue;
                    }
                    if contexts.len() >= limit {
                        return Err(
                            "Handler invocation contexts exceed the native instance limit".into(),
                        );
                    }
                    contexts.push(context);
                }
            }
        }
        Ok(contexts)
    }
}
