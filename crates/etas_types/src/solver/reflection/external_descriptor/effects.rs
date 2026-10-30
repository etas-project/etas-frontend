use super::ExternalDescriptorMatcher;
use crate::{ExternalEffectArgInput, ExternalEffectRefInput, ExternalEffectRowInput};
use etas_code::descriptor::{
    EffectInfo, EffectRowInfoRef, EffectTargetInfo, GenericParameterRef, StaticEffectArgInfo,
    TypeInfoRef,
};
use std::collections::BTreeMap;

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum EffectKey {
    Effect(etas_code::DefinitionKey),
    Action(etas_code::DefinitionKey, String),
}

impl From<&EffectTargetInfo> for EffectKey {
    fn from(target: &EffectTargetInfo) -> Self {
        match target {
            EffectTargetInfo::Effect(effect) => Self::Effect(effect.clone()),
            EffectTargetInfo::Action { effect, name } => Self::Action(effect.clone(), name.clone()),
        }
    }
}

impl ExternalDescriptorMatcher<'_> {
    pub(super) fn optional_row(
        &mut self,
        actual: Option<&ExternalEffectRowInput>,
        expected: Option<EffectRowInfoRef>,
        scope: &BTreeMap<GenericParameterRef, TypeInfoRef>,
        depth: usize,
    ) -> Result<bool, &'static str> {
        match (actual, expected) {
            (None, None) => Ok(true),
            (Some(actual), Some(expected)) => self.row(actual, expected, scope, depth + 1),
            _ => Ok(false),
        }
    }

    pub(super) fn row(
        &mut self,
        actual: &ExternalEffectRowInput,
        expected: EffectRowInfoRef,
        scope: &BTreeMap<GenericParameterRef, TypeInfoRef>,
        depth: usize,
    ) -> Result<bool, &'static str> {
        self.charge(depth)?;
        let graph = self.graph;
        let expected = graph
            .rows
            .get(expected.0 as usize)
            .ok_or("missing effect row descriptor")?;
        let tail_matches = match (&actual.tail, expected.tail) {
            (None, None) => true,
            (Some(name), Some(expected)) => self.parameters.get(name) == Some(&expected),
            _ => false,
        };
        if !tail_matches || actual.effects.len() != expected.effects.len() {
            return Ok(false);
        }
        let mut candidates = BTreeMap::<EffectKey, Vec<usize>>::new();
        for (index, effect) in expected.effects.iter().enumerate() {
            self.charge(depth + 1)?;
            candidates
                .entry(EffectKey::from(&effect.target))
                .or_default()
                .push(index);
        }
        for actual in &actual.effects {
            self.charge(depth + 1)?;
            let target = self
                .identities
                .effects
                .get(&actual.path.join("."))
                .ok_or("external effect has no canonical identity")?;
            let Some(candidates) = candidates.get_mut(&EffectKey::from(target)) else {
                return Ok(false);
            };
            let mut found = false;
            for candidate in 0..candidates.len() {
                if self.effect(
                    actual,
                    &expected.effects[candidates[candidate]],
                    scope,
                    depth + 1,
                )? {
                    candidates.swap_remove(candidate);
                    found = true;
                    break;
                }
            }
            if !found {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn effect(
        &mut self,
        actual: &ExternalEffectRefInput,
        expected: &EffectInfo,
        scope: &BTreeMap<GenericParameterRef, TypeInfoRef>,
        depth: usize,
    ) -> Result<bool, &'static str> {
        self.charge(depth)?;
        if self
            .identities
            .effects
            .get(&actual.path.join("."))
            .ok_or("external effect has no canonical identity")?
            != &expected.target
            || actual.args.len() != expected.arguments.len()
        {
            return Ok(false);
        }
        for (actual, expected) in actual.args.iter().zip(&expected.arguments) {
            let equal = match (actual, expected) {
                (ExternalEffectArgInput::Type(actual), StaticEffectArgInfo::Type(expected)) => {
                    self.ty(actual, *expected, scope, depth + 1)?
                }
                (ExternalEffectArgInput::Path(path), StaticEffectArgInfo::Resource(expected)) => {
                    self.identity(path, expected)?
                }
                (ExternalEffectArgInput::Path(path), StaticEffectArgInfo::Callable(expected)) => {
                    if let Some(declaration) = expected.as_declaration() {
                        self.identity(path, declaration)?
                    } else {
                        self.identities
                            .methods
                            .get(path)
                            .ok_or("external callable selector has no checked method identity")?
                            == expected
                    }
                }
                (ExternalEffectArgInput::String(actual), StaticEffectArgInfo::String(expected))
                | (ExternalEffectArgInput::Int(actual), StaticEffectArgInfo::Integer(expected)) => {
                    actual == expected
                }
                (ExternalEffectArgInput::Wildcard, StaticEffectArgInfo::Wildcard) => true,
                _ => false,
            };
            if !equal {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
