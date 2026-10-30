use super::ExternalDescriptorMatcher;
use crate::{ExternalTypeInput, TrustWrapper};
use etas_code::descriptor::{
    BuiltinTypeInfo as B, GenericParameterKind, GenericParameterRef, TypeInfo, TypeInfoRef,
};
use std::collections::{BTreeMap, BTreeSet};

impl ExternalDescriptorMatcher<'_> {
    pub(super) fn ty(
        &mut self,
        input: &ExternalTypeInput,
        mut reference: TypeInfoRef,
        scope: &BTreeMap<GenericParameterRef, TypeInfoRef>,
        depth: usize,
    ) -> Result<bool, &'static str> {
        self.charge(depth)?;
        if let ExternalTypeInput::Alias { target, .. } = input {
            return self.ty(target, reference, scope, depth + 1);
        }
        let mut seen = BTreeSet::new();
        while let Some(TypeInfo::Parameter(parameter)) = self.graph.types.get(reference.0 as usize)
        {
            let Some(next) = scope.get(parameter) else {
                break;
            };
            if !seen.insert(*parameter) {
                return Err("cyclic descriptor generic substitution");
            }
            reference = *next;
        }
        let graph = self.graph;
        let expected = graph
            .types
            .get(reference.0 as usize)
            .ok_or("missing type descriptor")?;
        if let TypeInfo::Alias {
            target, parameters, ..
        } = expected
        {
            if !parameters.is_empty() {
                return Err("unapplied generic type alias");
            }
            return self.ty(input, *target, scope, depth + 1);
        }
        if let TypeInfo::Applied {
            constructor,
            arguments,
        } = expected
            && let Some(TypeInfo::Alias {
                parameters, target, ..
            }) = graph.types.get(constructor.0 as usize)
        {
            if parameters.len() != arguments.len() {
                return Err("invalid descriptor alias application");
            }
            let mut nested = scope.clone();
            for (parameter, argument) in parameters.iter().zip(arguments) {
                nested.insert(*parameter, *argument);
            }
            return self.ty(input, *target, &nested, depth + 1);
        }
        match (input, expected) {
            (ExternalTypeInput::Primitive(name), TypeInfo::Primitive(expected)) => {
                Ok(crate::lower::external::primitive_from_external_name(name)
                    .map(super::super::descriptor_types::primitive)
                    == Some(*expected))
            }
            (ExternalTypeInput::Var(name), TypeInfo::Parameter(expected)) => {
                Ok(self.parameters.get(name) == Some(expected))
            }
            (
                ExternalTypeInput::Named(path),
                TypeInfo::Nominal { definition, .. } | TypeInfo::Enum { definition, .. },
            )
            | (ExternalTypeInput::Nominal { path, .. }, TypeInfo::Nominal { definition, .. })
            | (ExternalTypeInput::Enum { path, .. }, TypeInfo::Enum { definition, .. }) => {
                self.identity(path, definition)
            }
            (
                ExternalTypeInput::Applied { path, args },
                TypeInfo::Applied {
                    constructor,
                    arguments,
                },
            ) => {
                let declaration = match graph.types.get(constructor.0 as usize) {
                    Some(
                        TypeInfo::Nominal { definition, .. } | TypeInfo::Enum { definition, .. },
                    ) => definition,
                    _ => return Err("applied descriptor constructor is not a nominal type"),
                };
                if !self.identity(path, declaration)? {
                    return Ok(false);
                }
                self.types(args.iter(), arguments, scope, depth)
            }
            (ExternalTypeInput::Tuple(inputs), TypeInfo::Tuple(expected)) => {
                self.types(inputs.iter(), expected, scope, depth)
            }
            (ExternalTypeInput::Record { fields }, TypeInfo::Record(expected)) => {
                if fields.len() != expected.len() {
                    return Ok(false);
                }
                let mut by_name = BTreeMap::new();
                for field in expected {
                    self.charge(depth + 1)?;
                    if by_name.insert(field.name.as_str(), field.ty).is_some() {
                        return Err("duplicate descriptor record field");
                    }
                }
                let mut seen = BTreeSet::new();
                for field in fields {
                    if !seen.insert(field.name.as_str()) {
                        return Err("duplicate external record field");
                    }
                    let Some(other) = by_name.get(field.name.as_str()) else {
                        return Ok(false);
                    };
                    if !self.ty(&field.ty, *other, scope, depth + 1)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (
                ExternalTypeInput::Function {
                    input,
                    output,
                    effects,
                },
                TypeInfo::Function(expected),
            ) => {
                if !expected.generics.is_empty() || input.len() != expected.parameters.len() {
                    return Ok(false);
                }
                for (input, expected) in input.iter().zip(&expected.parameters) {
                    if !self.ty(input, expected.ty, scope, depth + 1)? {
                        return Ok(false);
                    }
                }
                Ok(self.ty(output, expected.output, scope, depth + 1)?
                    && self.optional_row(effects.as_ref(), expected.effects, scope, depth)?)
            }
            (
                ExternalTypeInput::Handler {
                    handled,
                    produced,
                    result,
                },
                TypeInfo::Handler {
                    handled: expected,
                    produced: expected_produced,
                    result: expected_result,
                },
            ) => {
                if !self.row(handled, *expected, scope, depth + 1)?
                    || !self.optional_row(produced.as_ref(), *expected_produced, scope, depth)?
                {
                    return Ok(false);
                }
                match (result, expected_result) {
                    (Some(actual), Some(expected)) => self.ty(actual, *expected, scope, depth + 1),
                    (None, None) => Ok(true),
                    _ => Ok(false),
                }
            }
            (
                _,
                TypeInfo::Builtin {
                    constructor,
                    arguments,
                },
            ) => {
                let Some((actual, inputs)) = self.builtin(input)? else {
                    return Ok(false);
                };
                if actual != *constructor {
                    return Ok(false);
                }
                if actual == B::CallableDefinition {
                    let [argument] = inputs.as_slice() else {
                        return Err("callable definition has invalid generic arity");
                    };
                    if !self.is_callable_argument(argument, depth + 1)? {
                        return Err("callable definition requires a callable type argument");
                    }
                }
                self.types(inputs.into_iter(), arguments, scope, depth)
            }
            _ => Ok(false),
        }
    }

    fn is_callable_argument(
        &mut self,
        input: &ExternalTypeInput,
        depth: usize,
    ) -> Result<bool, &'static str> {
        self.charge(depth)?;
        match input {
            ExternalTypeInput::Alias { target, .. } => self.is_callable_argument(target, depth + 1),
            ExternalTypeInput::Function { .. } => Ok(true),
            ExternalTypeInput::Var(name) => {
                Ok(self.parameters.get(name).is_some_and(|reference| {
                    self.graph
                        .binders
                        .get(reference.0 as usize)
                        .is_some_and(|binder| binder.kind == GenericParameterKind::Type)
                }))
            }
            _ => Ok(false),
        }
    }

    pub(super) fn types<'b>(
        &mut self,
        input: impl ExactSizeIterator<Item = &'b ExternalTypeInput>,
        expected: &[TypeInfoRef],
        scope: &BTreeMap<GenericParameterRef, TypeInfoRef>,
        depth: usize,
    ) -> Result<bool, &'static str> {
        if input.len() != expected.len() {
            return Ok(false);
        }
        for (input, expected) in input.zip(expected) {
            if !self.ty(input, *expected, scope, depth + 1)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn builtin<'b>(
        &self,
        input: &'b ExternalTypeInput,
    ) -> Result<Option<(B, Vec<&'b ExternalTypeInput>)>, &'static str> {
        let result = match input {
            ExternalTypeInput::Array(x) => (B::Array, vec![x.as_ref()]),
            ExternalTypeInput::List(x) => (B::List, vec![x.as_ref()]),
            ExternalTypeInput::Map { key, value } => (B::Map, vec![key.as_ref(), value.as_ref()]),
            ExternalTypeInput::Set(x) => (B::Set, vec![x.as_ref()]),
            ExternalTypeInput::Range(x) => (B::Range, vec![x.as_ref()]),
            ExternalTypeInput::Slice(x) => (B::Slice, vec![x.as_ref()]),
            ExternalTypeInput::Option(x) => (B::Option, vec![x.as_ref()]),
            ExternalTypeInput::Result { ok, err } => (B::Result, vec![ok.as_ref(), err.as_ref()]),
            ExternalTypeInput::Prompt => (B::Prompt, vec![]),
            ExternalTypeInput::PromptPart => (B::PromptPart, vec![]),
            ExternalTypeInput::Message(x) => (B::Message, vec![x.as_ref()]),
            ExternalTypeInput::MemorySelection(x) => (B::MemorySelection, vec![x.as_ref()]),
            ExternalTypeInput::Store { key, value } => (B::Store, vec![key.as_ref(), value.as_ref()]),
            ExternalTypeInput::MemoryRegion(x) => (B::MemoryRegion, vec![x.as_ref()]),
            ExternalTypeInput::ResourceHandle { name, args } => (match name.as_str() {
                "MemoryRegion" => B::MemoryRegionHandle,
                "ExternalTool" => B::ExternalToolHandle,
                _ => return Err("external resource handle has no portable descriptor identity"),
            }, args.iter().collect()),
            ExternalTypeInput::Trust { wrapper, inner } => (match crate::lower::external::external_trust_wrapper(wrapper).ok_or("invalid trust wrapper")? {
                TrustWrapper::Trusted => B::Trusted,
                TrustWrapper::Untrusted => B::Untrusted,
                TrustWrapper::Secret => B::Secret,
                TrustWrapper::Public => B::Public,
                TrustWrapper::Sanitized => B::Sanitized,
            }, vec![inner.as_ref()]),
            ExternalTypeInput::Applied { path, args } if self.identities.registry.lookup_qualified(path).is_some_and(|symbol| matches!(&symbol.decl, etas_std::StdDecl::Type(decl) if decl.code_role == Some(etas_std::CodeTypeRole::CallableDefinition))) => (B::CallableDefinition, args.iter().collect()),
            _ => return Ok(None),
        };
        Ok(Some(result))
    }
}
