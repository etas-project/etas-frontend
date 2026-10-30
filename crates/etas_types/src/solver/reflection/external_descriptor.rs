use std::collections::BTreeMap;

use crate::{
    ExternalCallableGenericParamInput, ExternalCallableGenericParamKindInput,
    ExternalEffectRowInput, ExternalTypeInput, api::ReflectionSignatureInput,
};
use etas_code::descriptor::{
    DescriptorGraphInput, EffectRowInfoRef, EffectTargetInfo, GenericParameterKind,
    GenericParameterRef, TypeInfoRef,
};

#[path = "external_descriptor/effects.rs"]
mod effects;
#[cfg(test)]
#[path = "external_descriptor/tests.rs"]
mod tests;
#[path = "external_descriptor/types.rs"]
mod types;

/// Canonical identities shared by all comparisons in one environment.
pub struct ExternalDescriptorIdentities<'a> {
    registry: &'a etas_std::StdRegistry,
    declarations: BTreeMap<Vec<String>, etas_code::DefinitionKey>,
    effects: BTreeMap<String, EffectTargetInfo>,
}

impl<'a> ExternalDescriptorIdentities<'a> {
    pub fn new(
        registry: &'a etas_std::StdRegistry,
        identities: &ReflectionSignatureInput,
    ) -> Result<Self, &'static str> {
        let declarations = identities
            .declarations
            .iter()
            .cloned()
            .collect::<BTreeMap<_, _>>();
        let effects = identities
            .effect_targets
            .iter()
            .cloned()
            .collect::<BTreeMap<_, _>>();
        if declarations.len() != identities.declarations.len()
            || effects.len() != identities.effect_targets.len()
        {
            return Err("external descriptor identity index contains duplicate paths");
        }
        Ok(Self {
            registry,
            declarations,
            effects,
        })
    }
}

/// Compares public metadata to a portable checked descriptor without requiring
/// HIR or a caller's TypeStore. Missing identities and exhausted budgets fail.
pub struct ExternalDescriptorMatcher<'a> {
    graph: &'a DescriptorGraphInput,
    identities: &'a ExternalDescriptorIdentities<'a>,
    parameters: BTreeMap<String, GenericParameterRef>,
    remaining: &'a mut usize,
}

impl<'a> ExternalDescriptorMatcher<'a> {
    pub fn new(
        graph: &'a DescriptorGraphInput,
        identities: &'a ExternalDescriptorIdentities<'a>,
        public_parameters: &[ExternalCallableGenericParamInput],
        descriptor_parameters: &[GenericParameterRef],
        remaining: &'a mut usize,
    ) -> Result<Self, &'static str> {
        if public_parameters.len() != descriptor_parameters.len() {
            return Err("callable generic parameter count differs");
        }
        if *remaining < public_parameters.len() {
            return Err("external descriptor comparison exceeds its resource budget");
        }
        *remaining -= public_parameters.len();
        let mut parameters = BTreeMap::new();
        for (public, reference) in public_parameters.iter().zip(descriptor_parameters) {
            let binder = graph
                .binders
                .get(reference.0 as usize)
                .ok_or("missing generic binder")?;
            let kind_matches = matches!(
                (public.kind, binder.kind),
                (
                    ExternalCallableGenericParamKindInput::Type,
                    GenericParameterKind::Type
                ) | (
                    ExternalCallableGenericParamKindInput::Effect,
                    GenericParameterKind::EffectRow
                )
            );
            if !kind_matches
                || public.name != binder.name
                || parameters.insert(public.name.clone(), *reference).is_some()
            {
                return Err("callable generic binder identity differs");
            }
        }
        let mut matcher = Self {
            graph,
            identities,
            parameters,
            remaining,
        };
        for (public, reference) in public_parameters.iter().zip(descriptor_parameters) {
            let bounds = graph
                .binders
                .get(reference.0 as usize)
                .ok_or("missing generic binder")?
                .bounds
                .as_slice();
            if public.bounds.len() != bounds.len() {
                return Err("callable generic bounds differ");
            }
            let mut used = vec![false; bounds.len()];
            for bound in &public.bounds {
                let mut found = false;
                for (index, expected) in bounds.iter().enumerate() {
                    matcher.charge(0)?;
                    if used[index] || !matcher.identity(&bound.spec, &expected.definition)? {
                        continue;
                    }
                    if matcher.types(bound.args.iter(), &expected.arguments, &BTreeMap::new(), 0)? {
                        used[index] = true;
                        found = true;
                        break;
                    }
                }
                if !found {
                    return Err("callable generic bounds differ");
                }
            }
        }
        Ok(matcher)
    }

    pub fn matches(
        &mut self,
        input: &ExternalTypeInput,
        ty: TypeInfoRef,
    ) -> Result<bool, &'static str> {
        self.ty(input, ty, &BTreeMap::new(), 0)
    }

    pub fn matches_optional_effect_row(
        &mut self,
        input: Option<&ExternalEffectRowInput>,
        row: Option<EffectRowInfoRef>,
    ) -> Result<bool, &'static str> {
        self.optional_row(input, row, &BTreeMap::new(), 0)
    }

    fn charge(&mut self, depth: usize) -> Result<(), &'static str> {
        if depth > etas_code::descriptor::DescriptorLimits::default().max_expansion_depth
            || *self.remaining == 0
        {
            return Err("external descriptor comparison exceeds its resource budget");
        }
        *self.remaining -= 1;
        Ok(())
    }

    fn identity(
        &self,
        path: &[String],
        expected: &etas_code::DefinitionKey,
    ) -> Result<bool, &'static str> {
        Ok(self
            .identities
            .declarations
            .get(path)
            .ok_or("external type has no canonical definition identity")?
            == expected)
    }
}
