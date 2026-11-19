use std::collections::{BTreeMap, BTreeSet};

use etas_code::{CodeFingerprint, DefinitionKey, descriptor::DescriptorLimits};
use etas_effects::{ActionRef, Effect};
use etas_hir::{SymbolDef, SymbolId};
use etas_types::{CheckedSpecRef, EffectActionSignature};

use super::DefinitionCatalog;
use crate::code::descriptor::ProjectionError;

pub struct NativeActionSignature<'a> {
    pub effect: DefinitionKey,
    pub name: String,
    pub action: ActionRef,
    pub signature: &'a EffectActionSignature,
    pub overload: Option<CodeFingerprint>,
}

impl DefinitionCatalog {
    pub fn native_action_signatures(
        &self,
    ) -> Result<Vec<NativeActionSignature<'_>>, ProjectionError> {
        native_signatures(
            self.checked_program(),
            &self.identities,
            DescriptorLimits::default(),
        )
    }

    pub fn native_spec_identity<'a>(
        &'a self,
        spec: &'a CheckedSpecRef,
    ) -> Result<&'a DefinitionKey, ProjectionError> {
        match spec {
            CheckedSpecRef::Source(symbol) => self
                .identities
                .symbol(*symbol)
                .ok_or(ProjectionError::MissingDeclaration(*symbol)),
            CheckedSpecRef::Std(path)
            | CheckedSpecRef::External(etas_types::facts::specs::ExternalSpecIdentity {
                path,
                ..
            }) => self
                .identities
                .path(path)
                .ok_or_else(|| ProjectionError::MissingIdentity(path.join("."))),
            CheckedSpecRef::Canonical(identity)
                if identity.kind() == etas_code::DefinitionKind::Spec =>
            {
                Ok(identity)
            }
            CheckedSpecRef::Canonical(_) => Err(ProjectionError::Invalid(
                "canonical spec identity has the wrong kind".into(),
            )),
        }
    }
}

fn bind(
    uses: &mut BTreeMap<SymbolId, ActionRef>,
    symbol: SymbolId,
    action: ActionRef,
) -> Result<(), ProjectionError> {
    if uses
        .insert(symbol, action.clone())
        .is_some_and(|previous| previous != action)
    {
        return Err(invalid("action symbol has conflicting checked identities"));
    }
    Ok(())
}

fn action_ref(effect: &Effect) -> Result<ActionRef, ProjectionError> {
    match effect {
        Effect::Action(action) => Ok(action.clone()),
        Effect::AppliedAction(instance) => Ok(instance.action.clone()),
        _ => Err(invalid("checked action reference is not an action")),
    }
}

fn invalid(reason: &str) -> ProjectionError {
    ProjectionError::Invalid(reason.into())
}

fn standard_action(
    checked: &crate::CheckedProject,
    descriptor: &etas_std::StdSymbol,
) -> Result<ActionRef, ProjectionError> {
    let etas_std::StdDecl::EffectAction(action) = &descriptor.decl else {
        return Err(invalid(
            "checked standard action target is not an action descriptor",
        ));
    };
    let registry = &checked.effect_registry;
    let reference = match action.stable_id {
        Some(stable) => registry
            .standard_action(etas_effects::EffectActionId(stable))
            .map(|signature| ActionRef {
                tag: signature.owner,
                action: signature.id,
            }),
        None => registry.action_by_name(&descriptor.qualified_path.join(".")),
    };
    reference
        .ok_or_else(|| invalid("standard action descriptor has no checked effect registry binding"))
}

pub(super) fn native_signatures<'a>(
    checked: &'a crate::CheckedProgram,
    identities: &crate::code::identity::DefinitionIdentityIndex,
    limits: DescriptorLimits,
) -> Result<Vec<NativeActionSignature<'a>>, ProjectionError> {
    let mut uses = BTreeMap::new();
    for fact in checked.effects.performed_actions.values() {
        if let Some(symbol) = fact.action_symbol {
            if fact.invocation.is_action() {
                bind(&mut uses, symbol, action_ref(&fact.invocation)?)?;
            }
        }
    }
    for (arm, fact) in &checked.types.handler_actions {
        let effect = checked
            .effects
            .handler_action_effects
            .get(arm)
            .ok_or_else(|| invalid("handler action has no checked effect identity"))?;
        if effect.is_action() {
            bind(&mut uses, fact.action_symbol, action_ref(effect)?)?;
        }
    }
    let mut declarations = BTreeSet::new();
    for (_, item) in checked.hir.items.iter() {
        match item {
            etas_hir::HirItem::Effect(effect) => {
                declarations.extend(effect.body.actions().iter().map(|action| action.symbol))
            }
            etas_hir::HirItem::Impl(implementation) => {
                for item in &implementation.items {
                    if let etas_hir::HirImplItem::Action(action) = item {
                        declarations.insert(action.symbol);
                    }
                }
            }
            _ => {}
        }
    }
    let mut actions =
        BTreeMap::<(DefinitionKey, String), Vec<(SymbolId, NativeActionSignature<'_>)>>::new();
    for (symbol, signature) in &checked.types.action_signatures {
        let mut symbol = *symbol;
        let mut visited = BTreeSet::new();
        while let Some(target) = checked.hir.source_import_targets.get(&symbol) {
            if !visited.insert(symbol) {
                return Err(invalid("cyclic source action import"));
            }
            symbol = *target;
        }
        let reference = if let Some(standard) = checked.hir.standard_action_targets.get(&symbol) {
            let descriptor = checked
                .std_registry
                .symbol(*standard)
                .filter(|descriptor| matches!(descriptor.decl, etas_std::StdDecl::EffectAction(_)))
                .ok_or_else(|| {
                    invalid("checked standard action target has no registry descriptor")
                })?;
            standard_action(checked, descriptor)?
        } else if let Some(action) = checked.effect_registry.action(symbol) {
            ActionRef {
                tag: action.owner,
                action: action.id,
            }
        } else if let Some(reference) = uses.get(&symbol) {
            reference.clone()
        } else {
            let data = checked
                .symbols
                .get(symbol)
                .ok_or(ProjectionError::MissingSymbol(symbol))?;
            let SymbolDef::ImportAlias { path, .. } = &data.def else {
                return Err(invalid("action signature has no checked native binding"));
            };
            let descriptor = checked
                .std_registry
                .lookup_qualified(path)
                .filter(|descriptor| matches!(descriptor.decl, etas_std::StdDecl::EffectAction(_)))
                .ok_or_else(|| invalid("action import has no canonical std descriptor"))?;
            standard_action(checked, descriptor)?
        };
        let descriptor = checked
            .effect_registry
            .action_signature(&reference)
            .ok_or_else(|| invalid("native action has no registry descriptor"))?;
        let origin = NativeActionSignature {
            effect: identities.effect_tag(reference.tag)?.clone(),
            name: descriptor.name.clone(),
            action: reference.clone(),
            signature,
            overload: None,
        };
        actions
            .entry((origin.effect.clone(), origin.name.clone()))
            .or_default()
            .push((symbol, origin));
    }
    let mut result = Vec::new();
    for (_, aliases) in actions {
        let first = &aliases[0].1;
        let definitions = aliases
            .iter()
            .filter_map(|(symbol, _)| {
                declarations
                    .contains(symbol)
                    .then(|| (*symbol, &checked.types.action_signatures[symbol]))
            })
            .collect::<BTreeMap<_, _>>();
        for (symbol, alias) in &aliases {
            let matches_signature = match definitions.get(symbol) {
                Some(signature) => **signature == *alias.signature,
                None if definitions.is_empty() => first.signature == alias.signature,
                None => definitions
                    .values()
                    .any(|signature| *signature == alias.signature),
            };
            if !matches_signature || alias.effect != first.effect || alias.name != first.name {
                let owner = first
                    .effect
                    .module()
                    .iter()
                    .chain(first.effect.declaration())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(".");
                return Err(ProjectionError::Invalid(format!(
                    "native action alias for {owner}.{} has an inconsistent checked signature: {:?}",
                    first.name, alias.signature,
                )));
            }
        }
        if definitions.is_empty() {
            result.push(aliases.into_iter().next().unwrap().1);
            continue;
        }
        let overloaded = definitions.len() > 1;
        let mut overloads = BTreeMap::new();
        for (symbol, signature) in definitions {
            let overload = if overloaded {
                let mut projector = crate::code::descriptor::DefinitionDescriptorProjector::new(
                    checked, identities, limits,
                );
                let inputs = projector.project_action_overload_inputs(
                    &first.effect,
                    &first.name,
                    signature,
                )?;
                Some(projector.finish()?.type_fingerprint(inputs)?)
            } else {
                None
            };
            let origin = NativeActionSignature {
                effect: first.effect.clone(),
                name: first.name.clone(),
                action: aliases
                    .iter()
                    .find(|(canonical, _)| *canonical == symbol)
                    .unwrap()
                    .1
                    .action
                    .clone(),
                signature,
                overload,
            };
            if overloads.insert(overload, origin).is_some() {
                return Err(invalid(
                    "action overloads have duplicate checked input signatures",
                ));
            }
        }
        result.extend(overloads.into_values());
    }
    Ok(result)
}
