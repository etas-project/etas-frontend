use std::collections::{BTreeMap, BTreeSet};

use etas_code::DefinitionKey;
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
}

impl DefinitionCatalog {
    pub fn native_action_signatures(
        &self,
    ) -> Result<Vec<NativeActionSignature<'_>>, ProjectionError> {
        let checked = self.checked_program();
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
        let mut actions = BTreeMap::<ActionRef, NativeActionSignature<'_>>::new();
        for (symbol, signature) in &checked.types.action_signatures {
            let mut symbol = *symbol;
            let mut visited = BTreeSet::new();
            while let Some(target) = checked.hir.source_import_targets.get(&symbol) {
                if !visited.insert(symbol) {
                    return Err(invalid("cyclic source action import"));
                }
                symbol = *target;
            }
            let reference = if let Some(standard) = checked.hir.standard_action_targets.get(&symbol)
            {
                let descriptor = checked
                    .std_registry
                    .symbol(*standard)
                    .filter(|descriptor| {
                        matches!(descriptor.decl, etas_std::StdDecl::EffectAction(_))
                    })
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
                    .filter(|descriptor| {
                        matches!(descriptor.decl, etas_std::StdDecl::EffectAction(_))
                    })
                    .ok_or_else(|| invalid("action import has no canonical std descriptor"))?;
                standard_action(checked, descriptor)?
            };
            let descriptor = checked
                .effect_registry
                .action_signature(&reference)
                .ok_or_else(|| invalid("native action has no registry descriptor"))?;
            let origin = NativeActionSignature {
                effect: self.native_effect_identity(reference.tag)?.clone(),
                name: descriptor.name.clone(),
                action: reference.clone(),
                signature,
            };
            if let Some(previous) = actions.get(&reference) {
                if previous.signature != signature
                    || previous.effect != origin.effect
                    || previous.name != origin.name
                {
                    return Err(invalid(
                        "native action aliases have inconsistent checked signatures",
                    ));
                }
            } else {
                actions.insert(reference, origin);
            }
        }
        Ok(actions.into_values().collect())
    }

    pub fn native_spec_identity(
        &self,
        spec: &CheckedSpecRef,
    ) -> Result<&DefinitionKey, ProjectionError> {
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
    let stable = action
        .stable_id
        .ok_or_else(|| invalid("standard action descriptor has no stable identity"))?;
    let signature = checked
        .effect_registry
        .standard_action(etas_effects::EffectActionId(stable))
        .ok_or_else(|| {
            invalid("standard action descriptor has no checked effect registry binding")
        })?;
    Ok(ActionRef {
        tag: signature.owner,
        action: signature.id,
    })
}
