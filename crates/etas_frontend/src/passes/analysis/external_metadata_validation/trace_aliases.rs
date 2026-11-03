use std::collections::BTreeMap;

use crate::{
    ProjectEnvironmentInput, ProjectExternalEffectArgInput as Arg,
    ProjectExternalEffectRefInput as Effect, ProjectExternalEffectRowInput as Row,
    ProjectExternalRecordFieldInput as Field, ProjectExternalTraceSpecClauseInput as Clause,
    ProjectExternalTypeInput as Type,
};

/// Resolves only exact, already validated declaration and action bindings.
pub(super) struct TraceAliasComparator<'a> {
    declarations: BTreeMap<Vec<String>, &'a etas_code::DefinitionKey>,
    actions: BTreeMap<Vec<String>, (&'a etas_code::DefinitionKey, String)>,
    remaining: usize,
}

impl<'a> TraceAliasComparator<'a> {
    pub(super) fn new(environment: &'a ProjectEnvironmentInput) -> Self {
        let declarations = environment
            .external_definition_origins
            .iter()
            .flat_map(|origin| &origin.origins.declarations)
            .map(|binding| (binding.path(), &binding.definition))
            .collect::<BTreeMap<_, _>>();
        let actions = environment
            .external_public_metadata
            .iter()
            .flat_map(|metadata| &metadata.actions)
            .filter_map(|action| {
                let (name, owner) = action.path.split_last()?;
                let identity = declarations.get(owner)?;
                (identity.kind() == etas_code::DefinitionKind::Effect)
                    .then(|| (action.path.clone(), (*identity, name.clone())))
            })
            .collect();
        Self {
            declarations,
            actions,
            remaining: super::MAX_EXTERNAL_METADATA_GRAPH_NODES,
        }
    }

    fn step(&mut self) -> Result<(), String> {
        self.remaining = self.remaining.checked_sub(1).ok_or_else(|| {
            "trace spec identity comparison exceeds its resource budget".to_owned()
        })?;
        Ok(())
    }

    fn path(&mut self, left: &[String], right: &[String]) -> Result<bool, String> {
        self.step()?;
        if left == right {
            return Ok(true);
        }
        if let (Some(left), Some(right)) =
            (self.declarations.get(left), self.declarations.get(right))
        {
            return Ok(left == right);
        }
        if let (Some(left), Some(right)) = (self.actions.get(left), self.actions.get(right)) {
            return Ok(left == right);
        }
        Ok(false)
    }

    pub(super) fn alternatives(
        &mut self,
        left: &[Vec<Clause>],
        right: &[Vec<Clause>],
    ) -> Result<bool, String> {
        self.step()?;
        if left.len() != right.len() {
            return Ok(false);
        }
        for (left, right) in left.iter().zip(right) {
            if left.len() != right.len() {
                return Ok(false);
            }
            for (left, right) in left.iter().zip(right) {
                if !self.clause(left, right)? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    fn clause(&mut self, left: &Clause, right: &Clause) -> Result<bool, String> {
        self.step()?;
        if left.kind != right.kind || left.selector_bounds.len() != right.selector_bounds.len() {
            return Ok(false);
        }
        for (left, right) in left.selector_bounds.iter().zip(&right.selector_bounds) {
            if left.operand != right.operand
                || left.argument != right.argument
                || left.binding != right.binding
                || !self.path(&left.effect_path, &right.effect_path)?
                || !self.path(&left.spec_path, &right.spec_path)?
                || !self.types(&left.spec_args, &right.spec_args)?
            {
                return Ok(false);
            }
        }
        for (left, right) in [
            (&left.pattern, &right.pattern),
            (&left.guard, &right.guard),
            (&left.target, &right.target),
            (&left.obligation, &right.obligation),
        ] {
            match (left, right) {
                (None, None) => {}
                (Some(left), Some(right)) if self.row(left, right)? => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    fn row(&mut self, left: &Row, right: &Row) -> Result<bool, String> {
        self.step()?;
        if left.tail != right.tail || left.effects.len() != right.effects.len() {
            return Ok(false);
        }
        for (left, right) in left.effects.iter().zip(&right.effects) {
            if !self.effect(left, right)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn effect(&mut self, left: &Effect, right: &Effect) -> Result<bool, String> {
        self.step()?;
        if !self.path(&left.path, &right.path)? || left.args.len() != right.args.len() {
            return Ok(false);
        }
        for (left, right) in left.args.iter().zip(&right.args) {
            let same = match (left, right) {
                (Arg::Type(left), Arg::Type(right)) => self.ty(left, right)?,
                (Arg::Path(left), Arg::Path(right)) => self.path(left, right)?,
                (Arg::String(left), Arg::String(right)) | (Arg::Int(left), Arg::Int(right)) => {
                    left == right
                }
                (Arg::Wildcard, Arg::Wildcard) => true,
                _ => false,
            };
            if !same {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn types(&mut self, left: &[Type], right: &[Type]) -> Result<bool, String> {
        if left.len() != right.len() {
            return Ok(false);
        }
        for (left, right) in left.iter().zip(right) {
            if !self.ty(left, right)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn fields(&mut self, left: &[Field], right: &[Field]) -> Result<bool, String> {
        if left.len() != right.len() {
            return Ok(false);
        }
        for (left, right) in left.iter().zip(right) {
            if left.name != right.name || !self.ty(&left.ty, &right.ty)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn optional_type(
        &mut self,
        left: &Option<Box<Type>>,
        right: &Option<Box<Type>>,
    ) -> Result<bool, String> {
        match (left, right) {
            (None, None) => Ok(true),
            (Some(left), Some(right)) => self.ty(left, right),
            _ => Ok(false),
        }
    }

    fn optional_row(&mut self, left: &Option<Row>, right: &Option<Row>) -> Result<bool, String> {
        match (left, right) {
            (None, None) => Ok(true),
            (Some(left), Some(right)) => self.row(left, right),
            _ => Ok(false),
        }
    }

    fn ty(&mut self, left: &Type, right: &Type) -> Result<bool, String> {
        self.step()?;
        Ok(match (left, right) {
            (Type::Primitive(a), Type::Primitive(b)) | (Type::Var(a), Type::Var(b)) => a == b,
            (Type::Named(a), Type::Named(b)) => self.path(a, b)?,
            (
                Type::Enum {
                    path: a,
                    variants: x,
                },
                Type::Enum {
                    path: b,
                    variants: y,
                },
            ) => self.path(a, b)? && self.fields(x, y)?,
            (Type::Applied { path: a, args: x }, Type::Applied { path: b, args: y }) => {
                self.path(a, b)? && self.types(x, y)?
            }
            (Type::Alias { path: a, target: x }, Type::Alias { path: b, target: y }) => {
                self.path(a, b)? && self.ty(x, y)?
            }
            (
                Type::Nominal {
                    path: a,
                    representation: x,
                },
                Type::Nominal {
                    path: b,
                    representation: y,
                },
            ) => self.path(a, b)? && self.optional_type(x, y)?,
            (Type::Array(a), Type::Array(b))
            | (Type::List(a), Type::List(b))
            | (Type::Set(a), Type::Set(b))
            | (Type::Range(a), Type::Range(b))
            | (Type::Slice(a), Type::Slice(b))
            | (Type::Option(a), Type::Option(b))
            | (Type::Message(a), Type::Message(b))
            | (Type::MemorySelection(a), Type::MemorySelection(b))
            | (Type::MemoryRegion(a), Type::MemoryRegion(b)) => self.ty(a, b)?,
            (Type::Map { key: a, value: x }, Type::Map { key: b, value: y })
            | (Type::Store { key: a, value: x }, Type::Store { key: b, value: y }) => {
                self.ty(a, b)? && self.ty(x, y)?
            }
            (Type::Result { ok: a, err: x }, Type::Result { ok: b, err: y }) => {
                self.ty(a, b)? && self.ty(x, y)?
            }
            (Type::Record { fields: a }, Type::Record { fields: b }) => self.fields(a, b)?,
            (Type::Tuple(a), Type::Tuple(b)) => self.types(a, b)?,
            (
                Type::Function {
                    input: a,
                    output: x,
                    effects: e,
                },
                Type::Function {
                    input: b,
                    output: y,
                    effects: f,
                },
            ) => self.types(a, b)? && self.ty(x, y)? && self.optional_row(e, f)?,
            (
                Type::Handler {
                    handled: a,
                    produced: x,
                    result: e,
                },
                Type::Handler {
                    handled: b,
                    produced: y,
                    result: f,
                },
            ) => self.row(a, b)? && self.optional_row(x, y)? && self.optional_type(e, f)?,
            (
                Type::Trust {
                    wrapper: a,
                    inner: x,
                },
                Type::Trust {
                    wrapper: b,
                    inner: y,
                },
            ) => a == b && self.ty(x, y)?,
            (Type::Prompt, Type::Prompt) | (Type::PromptPart, Type::PromptPart) => true,
            (
                Type::ResourceHandle { name: a, args: x },
                Type::ResourceHandle { name: b, args: y },
            ) => a == b && self.types(x, y)?,
            _ => false,
        })
    }
}
