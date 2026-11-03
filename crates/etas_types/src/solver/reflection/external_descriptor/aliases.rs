use std::collections::BTreeMap;

use super::ExternalDescriptorMatcher;
use crate::{ExternalEffectArgInput, ExternalEffectRowInput, ExternalTypeInput as Ty};

impl ExternalDescriptorMatcher<'_> {
    pub(super) fn expand_alias(
        &mut self,
        path: &[String],
        args: &[Ty],
        depth: usize,
    ) -> Result<Option<Ty>, &'static str> {
        let identities = self.identities;
        let Some((params, target)) = identities.aliases.get(path) else {
            return Ok(None);
        };
        if params.len() != args.len() {
            return Err("external alias argument count differs");
        }
        let mut substitutions = BTreeMap::new();
        for (param, arg) in params.iter().zip(args) {
            self.charge_alias_clone(arg, depth + 1)?;
            substitutions.insert(param.clone(), arg.clone());
        }
        self.charge_alias_clone(target, depth + 1)?;
        let mut target = target.clone();
        self.substitute_alias_type(&mut target, &substitutions, depth + 1)?;
        Ok(Some(target))
    }

    fn substitute_alias_type(
        &mut self,
        ty: &mut Ty,
        substitutions: &BTreeMap<String, Ty>,
        depth: usize,
    ) -> Result<(), &'static str> {
        self.charge(depth)?;
        match ty {
            Ty::Var(name) => {
                if let Some(value) = substitutions.get(name) {
                    self.charge_alias_clone(value, depth + 1)?;
                    *ty = value.clone();
                }
            }
            Ty::Applied { args, .. } | Ty::Tuple(args) | Ty::ResourceHandle { args, .. } => {
                for item in args {
                    self.substitute_alias_type(item, substitutions, depth + 1)?;
                }
            }
            Ty::Alias { target, .. }
            | Ty::Array(target)
            | Ty::List(target)
            | Ty::Set(target)
            | Ty::Range(target)
            | Ty::Slice(target)
            | Ty::Option(target)
            | Ty::Message(target)
            | Ty::MemorySelection(target)
            | Ty::MemoryRegion(target)
            | Ty::Trust { inner: target, .. } => {
                self.substitute_alias_type(target, substitutions, depth + 1)?
            }
            Ty::Nominal { representation, .. } => {
                if let Some(target) = representation {
                    self.substitute_alias_type(target, substitutions, depth + 1)?;
                }
            }
            Ty::Map { key, value }
            | Ty::Store { key, value }
            | Ty::Result {
                ok: key,
                err: value,
            } => {
                self.substitute_alias_type(key, substitutions, depth + 1)?;
                self.substitute_alias_type(value, substitutions, depth + 1)?;
            }
            Ty::Record { fields }
            | Ty::Enum {
                variants: fields, ..
            } => {
                for field in fields {
                    self.substitute_alias_type(&mut field.ty, substitutions, depth + 1)?;
                }
            }
            Ty::Function {
                input,
                output,
                effects,
            } => {
                for item in input {
                    self.substitute_alias_type(item, substitutions, depth + 1)?;
                }
                self.substitute_alias_type(output, substitutions, depth + 1)?;
                if let Some(row) = effects {
                    self.substitute_alias_row(row, substitutions, depth + 1)?;
                }
            }
            Ty::Handler {
                handled,
                produced,
                result,
            } => {
                self.substitute_alias_row(handled, substitutions, depth + 1)?;
                if let Some(row) = produced {
                    self.substitute_alias_row(row, substitutions, depth + 1)?;
                }
                if let Some(ty) = result {
                    self.substitute_alias_type(ty, substitutions, depth + 1)?;
                }
            }
            Ty::Primitive(_) | Ty::Named(_) | Ty::Prompt | Ty::PromptPart => {}
        }
        Ok(())
    }

    fn substitute_alias_row(
        &mut self,
        row: &mut ExternalEffectRowInput,
        substitutions: &BTreeMap<String, Ty>,
        depth: usize,
    ) -> Result<(), &'static str> {
        self.charge(depth)?;
        for effect in &mut row.effects {
            self.charge(depth + 1)?;
            for arg in &mut effect.args {
                self.charge(depth + 1)?;
                if let ExternalEffectArgInput::Type(ty) = arg {
                    self.substitute_alias_type(ty, substitutions, depth + 1)?;
                }
            }
        }
        Ok(())
    }

    fn charge_alias_clone(&mut self, root: &Ty, depth: usize) -> Result<(), &'static str> {
        let mut pending = vec![(root, depth)];
        while let Some((ty, depth)) = pending.pop() {
            self.charge(depth)?;
            let text_bytes = match ty {
                Ty::Primitive(text) | Ty::Var(text) => text.len(),
                Ty::Named(path)
                | Ty::Applied { path, .. }
                | Ty::Alias { path, .. }
                | Ty::Nominal { path, .. } => path
                    .iter()
                    .fold(0usize, |sum, part| sum.saturating_add(part.len())),
                Ty::Enum { path, variants } => variants.iter().fold(
                    path.iter()
                        .fold(0usize, |sum, part| sum.saturating_add(part.len())),
                    |sum, field| sum.saturating_add(field.name.len()),
                ),
                Ty::Record { fields } => fields
                    .iter()
                    .fold(0usize, |sum, field| sum.saturating_add(field.name.len())),
                Ty::Trust { wrapper, .. } => wrapper.len(),
                Ty::ResourceHandle { name, .. } => name.len(),
                _ => 0,
            };
            self.charge_alias_bytes(text_bytes)?;
            let next = depth + 1;
            match ty {
                Ty::Applied { args, .. } | Ty::Tuple(args) | Ty::ResourceHandle { args, .. } => {
                    pending.extend(args.iter().map(|ty| (ty, next)))
                }
                Ty::Alias { target, .. }
                | Ty::Array(target)
                | Ty::List(target)
                | Ty::Set(target)
                | Ty::Range(target)
                | Ty::Slice(target)
                | Ty::Option(target)
                | Ty::Message(target)
                | Ty::MemorySelection(target)
                | Ty::MemoryRegion(target)
                | Ty::Trust { inner: target, .. } => pending.push((target, next)),
                Ty::Nominal { representation, .. } => {
                    if let Some(ty) = representation {
                        pending.push((ty, next));
                    }
                }
                Ty::Map { key, value }
                | Ty::Store { key, value }
                | Ty::Result {
                    ok: key,
                    err: value,
                } => {
                    pending.push((key, next));
                    pending.push((value, next));
                }
                Ty::Record { fields }
                | Ty::Enum {
                    variants: fields, ..
                } => pending.extend(fields.iter().map(|field| (&field.ty, next))),
                Ty::Function {
                    input,
                    output,
                    effects,
                } => {
                    pending.extend(input.iter().map(|ty| (ty, next)));
                    pending.push((output, next));
                    if let Some(row) = effects {
                        self.append_row_types(row, &mut pending, next)?;
                    }
                }
                Ty::Handler {
                    handled,
                    produced,
                    result,
                } => {
                    self.append_row_types(handled, &mut pending, next)?;
                    if let Some(row) = produced {
                        self.append_row_types(row, &mut pending, next)?;
                    }
                    if let Some(ty) = result {
                        pending.push((ty, next));
                    }
                }
                Ty::Primitive(_) | Ty::Var(_) | Ty::Named(_) | Ty::Prompt | Ty::PromptPart => {}
            }
        }
        Ok(())
    }

    fn append_row_types<'a>(
        &mut self,
        row: &'a ExternalEffectRowInput,
        pending: &mut Vec<(&'a Ty, usize)>,
        depth: usize,
    ) -> Result<(), &'static str> {
        self.charge(depth)?;
        self.charge_alias_bytes(row.tail.as_ref().map_or(0, String::len))?;
        for effect in &row.effects {
            self.charge(depth + 1)?;
            self.charge_alias_bytes(
                effect
                    .path
                    .iter()
                    .fold(0usize, |sum, part| sum.saturating_add(part.len())),
            )?;
            for arg in &effect.args {
                self.charge(depth + 1)?;
                match arg {
                    ExternalEffectArgInput::Type(ty) => pending.push((ty, depth)),
                    ExternalEffectArgInput::String(text) | ExternalEffectArgInput::Int(text) => {
                        self.charge_alias_bytes(text.len())?
                    }
                    ExternalEffectArgInput::Path(path) => self.charge_alias_bytes(
                        path.iter()
                            .fold(0usize, |sum, part| sum.saturating_add(part.len())),
                    )?,
                    ExternalEffectArgInput::Wildcard => {}
                }
            }
        }
        Ok(())
    }

    fn charge_alias_bytes(&mut self, bytes: usize) -> Result<(), &'static str> {
        let units = bytes.saturating_add(63) / 64;
        *self.remaining = self
            .remaining
            .checked_sub(units)
            .ok_or("external descriptor comparison exceeds its resource budget")?;
        Ok(())
    }
}
