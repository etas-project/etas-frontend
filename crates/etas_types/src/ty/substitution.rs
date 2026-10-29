use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
};

mod rows;
use rows::EffectRowSubstitution;

use crate::solver::Substitution;

use super::{
    EffectArgRef, EffectRowRef, FieldType, FlowType, HandlerProducedEffects, HandlerType,
    RecordType, ResourceHandleType, Type, TypeId, TypeInterner, TypeStore,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeSubstitutionError {
    InvalidApplication(TypeId),
    MissingType(TypeId),
    MissingTypeIdentity(TypeId),
    MissingEnumLayout(TypeId),
    MissingEffectIdentity(String),
    UnresolvedParameter(TypeId),
    OpenEffectRow(String),
    CyclicType(TypeId),
    CyclicEffectRow(String),
    UnmaterializedType(Type),
    UnresolvedType(TypeId),
    RepresentationLimit(usize),
    ExpansionDepth(usize),
    WorkLimit {
        resource: &'static str,
        limit: usize,
    },
    Interrupted(etas_utils::PipelineInterruption),
}

impl std::fmt::Display for TypeSubstitutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WorkLimit { resource, limit } => {
                write!(f, "type substitution exceeds {resource} limit {limit}")
            }
            Self::Interrupted(interruption) => {
                write!(f, "type substitution interrupted: {}", interruption.reason)
            }
            Self::ExpansionDepth(limit) => {
                write!(f, "type substitution exceeds expansion depth {limit}")
            }
            Self::InvalidApplication(ty) => write!(
                f,
                "generic application has inconsistent constructor arguments at {ty:?}"
            ),
            Self::RepresentationLimit(limit) => {
                write!(f, "nominal representation expansion exceeds {limit} types")
            }
            Self::MissingType(ty) => write!(f, "type store is missing {ty:?}"),
            Self::MissingTypeIdentity(ty) => {
                write!(f, "type has no checked relocation identity: {ty:?}")
            }
            Self::MissingEnumLayout(ty) => {
                write!(f, "enum has no checked payload layout: {ty:?}")
            }
            Self::MissingEffectIdentity(name) => {
                write!(f, "effect has no checked relocation identity: {name}")
            }
            Self::UnresolvedParameter(ty) => {
                write!(f, "unresolved generic value type at {ty:?}")
            }
            Self::OpenEffectRow(name) => write!(
                f,
                "type application requires a closed effect row, found `{name}`"
            ),
            Self::CyclicType(ty) => write!(f, "type graph contains a cycle at {ty:?}"),
            Self::CyclicEffectRow(name) => {
                write!(f, "effect-row substitution contains a cycle at `{name}`")
            }
            Self::UnmaterializedType(ty) => {
                write!(f, "specialized type was not materialized: {ty:?}")
            }
            Self::UnresolvedType(ty) => {
                write!(
                    f,
                    "closed type application contains an unresolved type: {ty:?}"
                )
            }
        }
    }
}

impl std::error::Error for TypeSubstitutionError {}

/// A shared budget for all substitutions in one compiler operation.
pub struct TypeSubstitutionBudget<'a> {
    max_nodes: usize,
    max_edges: usize,
    nodes: Cell<usize>,
    edges: Cell<usize>,
    control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
}

impl<'a> TypeSubstitutionBudget<'a> {
    pub fn new(
        max_nodes: usize,
        max_edges: usize,
        control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
    ) -> Self {
        Self {
            max_nodes,
            max_edges,
            nodes: Cell::new(0),
            edges: Cell::new(0),
            control,
        }
    }

    fn checkpoint(&self) -> Result<(), TypeSubstitutionError> {
        if let Some(control) = self.control {
            control
                .checkpoint()
                .map_err(TypeSubstitutionError::Interrupted)?;
        }
        Ok(())
    }

    fn charge(
        &self,
        resource: &'static str,
        count: &Cell<usize>,
        amount: usize,
        limit: usize,
    ) -> Result<(), TypeSubstitutionError> {
        self.checkpoint()?;
        let next = count
            .get()
            .checked_add(amount)
            .ok_or(TypeSubstitutionError::WorkLimit { resource, limit })?;
        if next > limit {
            return Err(TypeSubstitutionError::WorkLimit { resource, limit });
        }
        count.set(next);
        Ok(())
    }

    fn node(&self) -> Result<(), TypeSubstitutionError> {
        self.charge("nodes", &self.nodes, 1, self.max_nodes)
    }
    fn edges(&self, amount: usize) -> Result<(), TypeSubstitutionError> {
        self.charge("edges", &self.edges, amount, self.max_edges)
    }
}

enum SubstitutionTarget<'a> {
    Materialize(&'a mut TypeInterner),
    Existing(&'a TypeStore),
    Relocate(TypeRelocationInput<'a>),
    RelocateMaterialize(TypeRelocationMaterializationInput<'a>),
}

/// Immutable native type correspondence supplied by a checked program bridge.
/// Matching value shape or display names cannot populate declaration_types.
pub struct TypeRelocationInput<'a> {
    pub source: &'a TypeStore,
    pub target: &'a TypeStore,
    pub declaration_types: &'a HashMap<TypeId, TypeId>,
    pub target_types: &'a super::TypeLookupIndex,
    pub effect_names: &'a HashMap<String, String>,
}

pub struct TypeRelocationMaterializationInput<'a> {
    pub source: &'a TypeStore,
    pub target: &'a mut TypeInterner,
    pub declaration_types: &'a HashMap<TypeId, TypeId>,
    pub effect_names: &'a HashMap<String, String>,
}

pub struct TypeSubstitutionEngine<'a> {
    target: SubstitutionTarget<'a>,
    named: Option<&'a HashMap<String, TypeId>>,
    parameters: Option<&'a HashMap<etas_hir::SymbolId, TypeId>>,
    variables: Option<&'a Substitution>,
    effect_rows: Option<EffectRowSubstitution<'a>>,
    closed_named_types: Option<&'a (dyn Fn(TypeId) -> bool + Sync)>,
    active: HashSet<TypeId>,
    memo: HashMap<TypeId, TypeId>,
    memo_depths: HashMap<TypeId, usize>,
    expansion_depth: Option<usize>,
    budget: Option<&'a TypeSubstitutionBudget<'a>>,
    control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
}

impl<'a> TypeSubstitutionEngine<'a> {
    pub fn materializing(
        interner: &'a mut TypeInterner,
        named: &'a HashMap<String, TypeId>,
        variables: Option<&'a Substitution>,
    ) -> Self {
        Self {
            target: SubstitutionTarget::Materialize(interner),
            named: Some(named),
            parameters: None,
            variables,
            effect_rows: None,
            closed_named_types: None,
            active: HashSet::new(),
            memo: HashMap::new(),
            memo_depths: HashMap::new(),
            expansion_depth: None,
            budget: None,
            control: None,
        }
    }

    pub fn existing(store: &'a TypeStore, named: &'a HashMap<String, TypeId>) -> Self {
        Self {
            target: SubstitutionTarget::Existing(store),
            named: Some(named),
            parameters: None,
            variables: None,
            effect_rows: None,
            closed_named_types: None,
            active: HashSet::new(),
            memo: HashMap::new(),
            memo_depths: HashMap::new(),
            expansion_depth: None,
            budget: None,
            control: None,
        }
    }

    pub fn existing_with_variables(
        store: &'a TypeStore,
        named: &'a HashMap<String, TypeId>,
        variables: &'a Substitution,
    ) -> Self {
        Self {
            variables: Some(variables),
            ..Self::existing(store, named)
        }
    }

    pub fn with_expansion_depth(mut self, limit: usize) -> Self {
        self.expansion_depth = Some(limit);
        self.memo.clear();
        self.memo_depths.clear();
        self
    }

    pub fn with_budget(mut self, budget: &'a TypeSubstitutionBudget<'a>) -> Self {
        self.budget = Some(budget);
        self
    }

    pub fn with_execution_control(
        mut self,
        control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
    ) -> Self {
        self.control = control;
        self
    }

    fn checkpoint(&self) -> Result<(), TypeSubstitutionError> {
        if let Some(control) = self.control {
            control
                .checkpoint()
                .map_err(TypeSubstitutionError::Interrupted)?;
        }
        if let Some(budget) = self.budget {
            budget.checkpoint()?;
        }
        Ok(())
    }

    /// Runtime value contracts require closed arguments even when no payload
    /// element is present. Native named atoms are admitted by checked identity.
    pub fn require_closed_parameters(
        mut self,
        named_types: &'a (dyn Fn(TypeId) -> bool + Sync),
    ) -> Self {
        self.closed_named_types = Some(named_types);
        self.memo.clear();
        self.memo_depths.clear();
        self
    }

    pub fn relocating(input: TypeRelocationInput<'a>) -> Self {
        Self {
            target: SubstitutionTarget::Relocate(input),
            named: None,
            parameters: None,
            variables: None,
            effect_rows: None,
            closed_named_types: None,
            active: HashSet::new(),
            memo: HashMap::new(),
            memo_depths: HashMap::new(),
            expansion_depth: None,
            budget: None,
            control: None,
        }
    }

    pub fn relocating_materialized(input: TypeRelocationMaterializationInput<'a>) -> Self {
        Self {
            target: SubstitutionTarget::RelocateMaterialize(input),
            named: None,
            parameters: None,
            variables: None,
            effect_rows: None,
            closed_named_types: None,
            active: HashSet::new(),
            memo: HashMap::new(),
            memo_depths: HashMap::new(),
            expansion_depth: None,
            budget: None,
            control: None,
        }
    }

    fn is_relocation(&self) -> bool {
        matches!(
            self.target,
            SubstitutionTarget::Relocate(_) | SubstitutionTarget::RelocateMaterialize(_)
        )
    }

    /// Source binders are identified by symbols, not by reusable display names.
    pub fn with_source_parameters(
        mut self,
        parameters: &'a HashMap<etas_hir::SymbolId, TypeId>,
    ) -> Self {
        self.parameters = Some(parameters);
        self.memo.clear();
        self.memo_depths.clear();
        self
    }

    pub fn substitute(&mut self, ty: TypeId) -> Result<TypeId, TypeSubstitutionError> {
        self.checkpoint()?;
        if let Some(specialized) = self.memo.get(&ty).copied() {
            return Ok(specialized);
        }
        let mut pending = vec![(ty, false)];
        let mut entered = Vec::new();
        let result = (|| {
            while let Some((current, finish)) = pending.pop() {
                self.checkpoint()?;
                if self.memo.contains_key(&current) {
                    continue;
                }
                if finish {
                    if let Some(limit) = self.expansion_depth {
                        let depth = 1 + self
                            .dependencies(current)?
                            .iter()
                            .filter_map(|child| self.memo_depths.get(child))
                            .copied()
                            .max()
                            .unwrap_or(0);
                        if depth > limit {
                            return Err(TypeSubstitutionError::ExpansionDepth(limit));
                        }
                        self.memo_depths.insert(current, depth);
                    }
                    // Every immediate dependency is memoized by the postorder
                    // walk, so assembly never recursively expands the graph.
                    let specialized = self.assemble(current)?;
                    self.active.remove(&current);
                    self.memo.insert(current, specialized);
                    continue;
                }
                if self.active.contains(&current) {
                    return Err(TypeSubstitutionError::CyclicType(current));
                }
                self.check_expansion_depth()?;
                if let Some(budget) = self.budget {
                    budget.node()?;
                }
                self.active.insert(current);
                entered.push(current);
                pending.push((current, true));
                let dependencies = self.dependencies(current)?;
                if let Some(budget) = self.budget {
                    budget.edges(dependencies.len())?;
                }
                pending.extend(dependencies.into_iter().rev().map(|child| (child, false)));
            }
            self.memo
                .get(&ty)
                .copied()
                .ok_or(TypeSubstitutionError::MissingType(ty))
        })();
        for current in entered {
            self.active.remove(&current);
        }
        result
    }

    pub fn validate_closed_argument(&self, ty: TypeId) -> Result<(), TypeSubstitutionError> {
        let mut pending = vec![ty];
        let mut visited = HashSet::new();
        while let Some(current) = pending.pop() {
            if let Some(budget) = self.budget {
                budget.checkpoint()?;
            }
            if !visited.insert(current) {
                continue;
            }
            if let Some(budget) = self.budget {
                budget.node()?;
            }
            let data = self.require_type(current)?;
            match &data {
                Type::Var(_) | Type::Named(_) => {
                    return Err(TypeSubstitutionError::UnresolvedType(current));
                }
                Type::Function(flow) => {
                    let row = flow
                        .effects
                        .as_ref()
                        .ok_or(TypeSubstitutionError::UnresolvedType(current))?;
                    require_closed_argument_row(row)?;
                }
                Type::Handler(handler) => {
                    require_closed_argument_row(&handler.handled)?;
                    if let HandlerProducedEffects::Explicit(row) = &handler.produced {
                        require_closed_argument_row(row)?;
                    }
                }
                _ => {}
            }
            let dependencies = self.dependencies(current)?;
            if let Some(budget) = self.budget {
                budget.edges(dependencies.len())?;
            }
            pending.extend(dependencies);
        }
        Ok(())
    }

    pub fn validate_closed_effect_argument(
        &self,
        row: &EffectRowRef,
    ) -> Result<(), TypeSubstitutionError> {
        require_closed_argument_row(row)?;
        for effect in &row.effects {
            if let Some(budget) = self.budget {
                budget.node()?;
                budget.edges(effect.args.len())?;
            }
            for argument in &effect.args {
                if let EffectArgRef::Type(ty) = argument {
                    self.validate_closed_argument(*ty)?;
                }
            }
        }
        Ok(())
    }

    fn check_expansion_depth(&self) -> Result<(), TypeSubstitutionError> {
        if let Some(limit) = self.expansion_depth
            && self.active.len() >= limit
        {
            return Err(TypeSubstitutionError::ExpansionDepth(limit));
        }
        Ok(())
    }

    fn dependencies(&self, ty: TypeId) -> Result<Vec<TypeId>, TypeSubstitutionError> {
        let data = self.require_type(ty)?;
        let mapped = match &self.target {
            SubstitutionTarget::Relocate(input) => input.declaration_types.contains_key(&ty),
            SubstitutionTarget::RelocateMaterialize(input) => {
                input.declaration_types.contains_key(&ty)
            }
            _ => false,
        };
        if mapped {
            return Ok(vec![]);
        }
        if let Some(replacement) = self
            .replacement(&data)
            .filter(|replacement| *replacement != ty)
        {
            if matches!(data, Type::Named(_)) {
                return Ok(vec![]);
            }
            return Ok(vec![replacement]);
        }
        let mut dependencies = Vec::new();
        match data {
            Type::Array(inner)
            | Type::List(inner)
            | Type::Set(inner)
            | Type::Range { index: inner }
            | Type::Slice(inner)
            | Type::Option(inner)
            | Type::Refined { base: inner, .. }
            | Type::Trust { inner, .. }
            | Type::Schema(inner)
            | Type::Message(inner)
            | Type::MemorySelection(inner)
            | Type::MemoryRegion(inner) => dependencies.push(inner),
            Type::Map { key, value }
            | Type::Store { key, value }
            | Type::Result {
                ok: key,
                err: value,
            } => dependencies.extend([key, value]),
            Type::Record(record) => {
                dependencies.extend(record.fields.into_iter().map(|field| field.ty))
            }
            Type::Tuple(elements) => dependencies.extend(elements),
            Type::Function(flow) => {
                dependencies.extend(flow.input);
                dependencies.push(flow.output);
                if let Some(row) = flow.effects {
                    self.row_dependencies(row, &mut dependencies)?;
                }
            }
            Type::Handler(handler) => {
                self.row_dependencies(handler.handled, &mut dependencies)?;
                if let HandlerProducedEffects::Explicit(row) = handler.produced {
                    self.row_dependencies(row, &mut dependencies)?;
                }
                dependencies.extend(handler.result);
            }
            Type::Applied { constructor, args } => {
                if self.is_relocation() {
                    dependencies.push(TypeId(constructor.0));
                }
                dependencies.extend(args);
            }
            Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => {
                dependencies.push(schema)
            }
            Type::ResourceHandle(ResourceHandleType::ExternalTool { signature }) => {
                dependencies.push(signature)
            }
            Type::ResourceHandle(ResourceHandleType::Other { args, .. }) => {
                dependencies.extend(args)
            }
            Type::Primitive(_)
            | Type::IntegerLiteral { .. }
            | Type::Var(_)
            | Type::Enum(_)
            | Type::Named(_)
            | Type::Nominal(_)
            | Type::Prompt
            | Type::PromptPart
            | Type::MemoryPlace(_) => {}
        }
        Ok(dependencies)
    }

    fn row_dependencies(
        &self,
        row: EffectRowRef,
        dependencies: &mut Vec<TypeId>,
    ) -> Result<(), TypeSubstitutionError> {
        let expanded = self.expanded_row(row)?;
        for (index, effect) in expanded.row.effects.into_iter().enumerate() {
            if expanded.argument_start.is_some_and(|start| index >= start) {
                continue;
            }
            for arg in effect.args {
                if let EffectArgRef::Type(ty) = arg {
                    dependencies.push(ty);
                }
            }
        }
        Ok(())
    }

    fn assemble(&mut self, ty: TypeId) -> Result<TypeId, TypeSubstitutionError> {
        let ty_data = self.require_type(ty)?;
        if let SubstitutionTarget::Relocate(input) = &self.target
            && let Some(mapped) = input.declaration_types.get(&ty)
        {
            input
                .target
                .get(*mapped)
                .ok_or(TypeSubstitutionError::MissingType(*mapped))?;
            return Ok(*mapped);
        }
        if let SubstitutionTarget::RelocateMaterialize(input) = &self.target
            && let Some(mapped) = input.declaration_types.get(&ty)
        {
            input
                .target
                .store()
                .get(*mapped)
                .ok_or(TypeSubstitutionError::MissingType(*mapped))?;
            return Ok(*mapped);
        }
        if let Some(replacement) = self
            .replacement(&ty_data)
            .filter(|replacement| *replacement != ty)
        {
            if matches!(ty_data, Type::Named(_)) {
                return self.substitute_argument(replacement);
            }
            return self.substitute(replacement);
        }
        let specialized = match ty_data {
            Type::Array(inner) => Type::Array(self.substitute(inner)?),
            Type::List(inner) => Type::List(self.substitute(inner)?),
            Type::Map { key, value } => Type::Map {
                key: self.substitute(key)?,
                value: self.substitute(value)?,
            },
            Type::Set(inner) => Type::Set(self.substitute(inner)?),
            Type::Range { index } => Type::Range {
                index: self.substitute(index)?,
            },
            Type::Slice(inner) => Type::Slice(self.substitute(inner)?),
            Type::Option(inner) => Type::Option(self.substitute(inner)?),
            Type::Result { ok, err } => Type::Result {
                ok: self.substitute(ok)?,
                err: self.substitute(err)?,
            },
            Type::Record(record) => Type::Record(RecordType {
                fields: record
                    .fields
                    .into_iter()
                    .map(|field| {
                        Ok(FieldType {
                            name: field.name,
                            ty: self.substitute(field.ty)?,
                        })
                    })
                    .collect::<Result<Vec<_>, TypeSubstitutionError>>()?,
            }),
            Type::Tuple(elements) => Type::Tuple(
                elements
                    .into_iter()
                    .map(|element| self.substitute(element))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Type::Function(flow) => Type::Function(FlowType {
                input: flow
                    .input
                    .into_iter()
                    .map(|input| self.substitute(input))
                    .collect::<Result<Vec<_>, _>>()?,
                output: self.substitute(flow.output)?,
                effects: flow
                    .effects
                    .map(|row| self.substitute_effect_row(row))
                    .transpose()?,
            }),
            Type::Handler(handler) => Type::Handler(HandlerType {
                handled: self.substitute_effect_row(handler.handled)?,
                produced: match handler.produced {
                    HandlerProducedEffects::Infer => HandlerProducedEffects::Infer,
                    HandlerProducedEffects::Explicit(row) => {
                        HandlerProducedEffects::Explicit(self.substitute_effect_row(row)?)
                    }
                },
                result: handler
                    .result
                    .map(|result| self.substitute(result))
                    .transpose()?,
            }),
            Type::Applied { constructor, args } => Type::Applied {
                constructor: if self.is_relocation() {
                    super::TypeConstructorId(self.substitute(TypeId(constructor.0))?.0)
                } else {
                    constructor
                },
                args: args
                    .into_iter()
                    .map(|arg| self.substitute(arg))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Type::Refined { base, predicate } => {
                if self.is_relocation() {
                    return Err(TypeSubstitutionError::MissingTypeIdentity(ty));
                }
                Type::Refined {
                    base: self.substitute(base)?,
                    predicate,
                }
            }
            Type::Trust { wrapper, inner } => Type::Trust {
                wrapper,
                inner: self.substitute(inner)?,
            },
            Type::Schema(inner) => Type::Schema(self.substitute(inner)?),
            Type::Message(inner) => Type::Message(self.substitute(inner)?),
            Type::MemorySelection(inner) => Type::MemorySelection(self.substitute(inner)?),
            Type::Store { key, value } => Type::Store {
                key: self.substitute(key)?,
                value: self.substitute(value)?,
            },
            Type::MemoryRegion(inner) => Type::MemoryRegion(self.substitute(inner)?),
            Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => {
                Type::ResourceHandle(ResourceHandleType::MemoryRegion {
                    schema: self.substitute(schema)?,
                })
            }
            Type::ResourceHandle(ResourceHandleType::ExternalTool { signature }) => {
                Type::ResourceHandle(ResourceHandleType::ExternalTool {
                    signature: self.substitute(signature)?,
                })
            }
            Type::ResourceHandle(ResourceHandleType::Other { name, args }) => {
                Type::ResourceHandle(ResourceHandleType::Other {
                    name,
                    args: args
                        .into_iter()
                        .map(|arg| self.substitute(arg))
                        .collect::<Result<Vec<_>, _>>()?,
                })
            }
            Type::Primitive(_) | Type::Prompt | Type::PromptPart | Type::MemoryPlace(_) => {
                if self.is_relocation() {
                    ty_data
                } else {
                    return Ok(ty);
                }
            }
            Type::Var(_) if self.closed_named_types.is_some() => {
                return Err(TypeSubstitutionError::UnresolvedParameter(ty));
            }
            Type::Named(_) if self.closed_named_types.is_some_and(|known| !known(ty)) => {
                return Err(TypeSubstitutionError::UnresolvedParameter(ty));
            }
            Type::IntegerLiteral { .. }
            | Type::Var(_)
            | Type::Enum(_)
            | Type::Named(_)
            | Type::Nominal(_) => {
                if self.is_relocation() {
                    return Err(TypeSubstitutionError::MissingTypeIdentity(ty));
                }
                return Ok(ty);
            }
        };
        self.materialize(ty, specialized)
    }

    fn substitute_argument(&mut self, ty: TypeId) -> Result<TypeId, TypeSubstitutionError> {
        self.require_type(ty)?;
        if self.variables.is_none() && self.closed_named_types.is_none() {
            return Ok(ty);
        }
        // Arguments belong to the caller's binder namespace. Only globally
        // unique solver variables may be normalized inside them.
        let named = self.named.take();
        let parameters = self.parameters.take();
        let rows = self.effect_rows.take();
        let memo = std::mem::take(&mut self.memo);
        let depths = std::mem::take(&mut self.memo_depths);
        let active = std::mem::take(&mut self.active);
        let result = self.substitute(ty);
        self.named = named;
        self.parameters = parameters;
        self.effect_rows = rows;
        self.memo = memo;
        self.memo_depths = depths;
        self.active = active;
        result
    }

    fn replacement(&self, ty: &Type) -> Option<TypeId> {
        match ty {
            Type::Named(name) => match self.parameters {
                Some(parameters) => name
                    .source_parameter
                    .and_then(|symbol| parameters.get(&symbol).copied()),
                None => self.named.and_then(|named| named.get(&name.name).copied()),
            },
            Type::Var(var) => self.variables.and_then(|variables| variables.get(*var)),
            _ => None,
        }
    }

    fn require_type(&self, ty: TypeId) -> Result<Type, TypeSubstitutionError> {
        self.type_data(ty)
            .ok_or(TypeSubstitutionError::MissingType(ty))
    }

    fn type_data(&self, ty: TypeId) -> Option<Type> {
        match &self.target {
            SubstitutionTarget::Materialize(interner) => interner.store().get(ty).cloned(),
            SubstitutionTarget::Existing(store) => store.get(ty).cloned(),
            SubstitutionTarget::Relocate(input) => input.source.get(ty).cloned(),
            SubstitutionTarget::RelocateMaterialize(input) => input.source.get(ty).cloned(),
        }
    }

    fn materialize(
        &mut self,
        original: TypeId,
        specialized: Type,
    ) -> Result<TypeId, TypeSubstitutionError> {
        if !self.is_relocation() && self.require_type(original)? == specialized {
            return Ok(original);
        }
        match &mut self.target {
            SubstitutionTarget::Materialize(interner) => Ok(interner.intern(specialized)),
            SubstitutionTarget::Existing(store) => store
                .iter()
                .find_map(|(id, candidate)| (candidate == &specialized).then_some(id))
                .ok_or(TypeSubstitutionError::UnmaterializedType(specialized)),
            SubstitutionTarget::Relocate(input) => input
                .target_types
                .get(input.target, &specialized)
                .ok_or(TypeSubstitutionError::UnmaterializedType(specialized)),
            SubstitutionTarget::RelocateMaterialize(input) => Ok(input.target.intern(specialized)),
        }
    }
}

fn require_closed_argument_row(row: &EffectRowRef) -> Result<(), TypeSubstitutionError> {
    match &row.tail {
        Some(tail) => Err(TypeSubstitutionError::OpenEffectRow(tail.clone())),
        None => Ok(()),
    }
}

pub fn substitute_named_params(
    interner: &mut TypeInterner,
    ty: TypeId,
    substitutions: &HashMap<String, TypeId>,
) -> Result<TypeId, TypeSubstitutionError> {
    TypeSubstitutionEngine::materializing(interner, substitutions, None).substitute(ty)
}

pub fn substitute_type_params(
    interner: &mut TypeInterner,
    ty: TypeId,
    named: &HashMap<String, TypeId>,
    variables: &Substitution,
) -> Result<TypeId, TypeSubstitutionError> {
    TypeSubstitutionEngine::materializing(interner, named, Some(variables)).substitute(ty)
}

pub fn substitute_effect_row_params(
    interner: &mut TypeInterner,
    row: EffectRowRef,
    named: &HashMap<String, TypeId>,
    variables: &Substitution,
) -> Result<EffectRowRef, TypeSubstitutionError> {
    TypeSubstitutionEngine::materializing(interner, named, Some(variables))
        .substitute_effect_row(row)
}

pub fn substitute_named_params_in_store(
    store: &TypeStore,
    ty: TypeId,
    substitutions: &HashMap<String, TypeId>,
) -> Result<TypeId, TypeSubstitutionError> {
    TypeSubstitutionEngine::existing(store, substitutions).substitute(ty)
}

#[cfg(test)]
mod closed_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectRef, NamedTypeRef, NominalTypeRef, PrimitiveType, TypeConstructorId};

    fn typed_row(ty: TypeId) -> EffectRowRef {
        EffectRowRef {
            effects: vec![EffectRef {
                name: "Fs.read".to_owned(),
                args: vec![EffectArgRef::Type(ty)],
            }],
            tail: None,
        }
    }

    #[test]
    fn substitution_specializes_function_and_handler_effect_rows() {
        let mut interner = TypeInterner::new();
        let param = interner.intern(Type::Named(NamedTypeRef {
            source_parameter: None,
            name: "T".to_owned(),
        }));
        let concrete = interner.primitive(PrimitiveType::String);
        let function = interner.intern(Type::Function(FlowType {
            input: vec![param],
            output: param,
            effects: Some(typed_row(param)),
        }));
        let handler = interner.intern(Type::Handler(HandlerType {
            handled: typed_row(param),
            produced: HandlerProducedEffects::Explicit(typed_row(param)),
            result: Some(function),
        }));

        let substitutions = HashMap::from([("T".to_owned(), concrete)]);
        let specialized = substitute_named_params(&mut interner, handler, &substitutions)
            .expect("substitution should materialize");
        let Type::Handler(handler) = interner.store().get(specialized).unwrap() else {
            panic!("expected handler")
        };
        assert_eq!(
            handler.handled.effects[0].args,
            vec![EffectArgRef::Type(concrete)]
        );
        let HandlerProducedEffects::Explicit(produced) = &handler.produced else {
            panic!("expected explicit produced row")
        };
        assert_eq!(produced.effects[0].args, vec![EffectArgRef::Type(concrete)]);
        let Type::Function(flow) = interner.store().get(handler.result.unwrap()).unwrap() else {
            panic!("expected function result")
        };
        assert_eq!(
            flow.effects.as_ref().unwrap().effects[0].args,
            vec![EffectArgRef::Type(concrete)]
        );
    }

    #[test]
    fn readonly_substitution_specializes_nested_applied_types() {
        let mut interner = TypeInterner::new();
        let param = interner.intern(Type::Named(NamedTypeRef {
            source_parameter: None,
            name: "T".to_owned(),
        }));
        let concrete_arg = interner.primitive(PrimitiveType::I32);
        let wrapper = interner.intern(Type::Nominal(NominalTypeRef {
            read_only_erased_view: false,
            name: "Wrapper".to_owned(),
            params: vec!["T".to_owned()],
            representation: None,
        }));
        let generic = interner.intern(Type::Applied {
            constructor: TypeConstructorId(wrapper.0),
            args: vec![param],
        });
        let without_specialization = interner.store().clone();
        let concrete = interner.intern(Type::Applied {
            constructor: TypeConstructorId(wrapper.0),
            args: vec![concrete_arg],
        });
        let store = interner.into_store();
        let substitutions = HashMap::from([("T".to_owned(), concrete_arg)]);

        assert_eq!(
            substitute_named_params_in_store(&store, generic, &substitutions),
            Ok(concrete)
        );
        assert!(matches!(
            substitute_named_params_in_store(&without_specialization, generic, &substitutions),
            Err(TypeSubstitutionError::UnmaterializedType(Type::Applied { args, .. }))
                if args == vec![concrete_arg]
        ));
    }

    #[test]
    fn effect_row_bindings_specialize_nested_function_and_handler_rows() {
        let mut interner = TypeInterner::new();
        let param = interner.intern(Type::Named(NamedTypeRef {
            source_parameter: None,
            name: "T".into(),
        }));
        let concrete = interner.primitive(PrimitiveType::String);
        let mut open = typed_row(param);
        open.tail = Some("E".into());
        let function = interner.intern(Type::Function(FlowType {
            input: vec![param],
            output: param,
            effects: Some(open.clone()),
        }));
        let handler = interner.intern(Type::Handler(HandlerType {
            handled: open.clone(),
            produced: HandlerProducedEffects::Explicit(open),
            result: Some(function),
        }));
        let named = HashMap::from([("T".into(), concrete)]);
        let mut closed = typed_row(param);
        closed.effects.push(EffectRef {
            name: "Console.stdout_write".into(),
            args: vec![],
        });
        let rows = HashMap::from([
            (
                "E".into(),
                EffectRowRef {
                    effects: vec![],
                    tail: Some("F".into()),
                },
            ),
            ("F".into(), closed),
        ]);
        let specialized = TypeSubstitutionEngine::materializing(&mut interner, &named, None)
            .with_row_equations(&rows)
            .substitute(handler)
            .unwrap();
        let Type::Handler(handler) = interner.store().get(specialized).unwrap() else {
            panic!("handler")
        };
        let Type::Function(function) = interner.store().get(handler.result.unwrap()).unwrap()
        else {
            panic!("function")
        };
        let HandlerProducedEffects::Explicit(produced) = &handler.produced else {
            panic!("explicit row")
        };
        for row in [
            &handler.handled,
            produced,
            function.effects.as_ref().unwrap(),
        ] {
            assert_eq!(row.tail, None);
            assert_eq!(row.effects.len(), 2, "duplicate row members must be merged");
            assert_eq!(row.effects[0].args, [EffectArgRef::Type(concrete)]);
            assert_eq!(row.effects[1].name, "Console.stdout_write");
        }
        assert_eq!(function.input, [concrete]);
        assert_eq!(function.output, concrete);
    }

    #[test]
    fn effect_arguments_preserve_the_callers_type_and_row_binder_namespace() {
        let mut interner = TypeInterner::new();
        let param = interner.intern(Type::Named(NamedTypeRef {
            source_parameter: None,
            name: "T".into(),
        }));
        let string = interner.primitive(PrimitiveType::String);
        let mut template = typed_row(param);
        template.tail = Some("E".into());
        let mut argument = typed_row(param);
        argument.tail = Some("E".into());
        let names = HashMap::from([("T".into(), string)]);
        let rows = HashMap::from([("E".into(), argument)]);
        let mut engine = TypeSubstitutionEngine::materializing(&mut interner, &names, None)
            .with_effect_arguments(&rows);
        let actual = engine.substitute_effect_row(template).unwrap();
        assert_eq!(actual.tail.as_deref(), Some("E"));
        assert_eq!(actual.effects.len(), 2);
        assert_eq!(actual.effects[0].args, [EffectArgRef::Type(string)]);
        assert_eq!(actual.effects[1].args, [EffectArgRef::Type(param)]);
    }

    #[test]
    fn effect_arguments_are_simultaneous_not_transitive_solver_equations() {
        let mut interner = TypeInterner::new();
        let names = HashMap::new();
        let rows = HashMap::from([
            (
                "E".into(),
                EffectRowRef {
                    effects: vec![],
                    tail: Some("F".into()),
                },
            ),
            (
                "F".into(),
                EffectRowRef {
                    effects: vec![],
                    tail: None,
                },
            ),
        ]);
        let mut engine = TypeSubstitutionEngine::materializing(&mut interner, &names, None)
            .with_effect_arguments(&rows);
        let actual = engine
            .substitute_effect_row(EffectRowRef {
                effects: vec![],
                tail: Some("E".into()),
            })
            .unwrap();
        assert_eq!(actual.tail.as_deref(), Some("F"));
    }

    #[test]
    fn identity_effect_row_substitution_keeps_the_open_binder_without_recursion() {
        let mut interner = TypeInterner::new();
        let named = HashMap::new();
        let row = EffectRowRef {
            effects: vec![],
            tail: Some("E".into()),
        };
        let rows = HashMap::from([("E".into(), row.clone())]);
        let mut engine = TypeSubstitutionEngine::materializing(&mut interner, &named, None)
            .with_row_equations(&rows);
        assert_eq!(engine.substitute_effect_row(row.clone()).unwrap(), row);
    }

    #[test]
    fn cyclic_effect_row_bindings_are_rejected_on_each_query() {
        let mut interner = TypeInterner::new();
        let named = HashMap::new();
        let rows = HashMap::from([
            (
                "E".into(),
                EffectRowRef {
                    effects: vec![],
                    tail: Some("F".into()),
                },
            ),
            (
                "F".into(),
                EffectRowRef {
                    effects: vec![],
                    tail: Some("E".into()),
                },
            ),
        ]);
        let mut engine = TypeSubstitutionEngine::materializing(&mut interner, &named, None)
            .with_row_equations(&rows);
        for _ in 0..2 {
            assert_eq!(
                engine.substitute_effect_row(EffectRowRef {
                    effects: vec![],
                    tail: Some("E".into()),
                }),
                Err(TypeSubstitutionError::CyclicEffectRow("E".into()))
            );
        }
    }

    #[test]
    fn relocation_reuses_the_substitution_walk_for_nested_callable_and_handler_types() {
        let mut source = TypeInterner::new();
        let a = source.intern(Type::Nominal(NominalTypeRef {
            read_only_erased_view: false,
            name: "app.UserId".into(),
            params: vec![],
            representation: None,
        }));
        let array = source.intern(Type::Array(a));
        let function = source.intern(Type::Function(FlowType {
            input: vec![array],
            output: a,
            effects: Some(typed_row(a)),
        }));
        let handler = source.intern(Type::Handler(HandlerType {
            handled: typed_row(a),
            produced: HandlerProducedEffects::Explicit(typed_row(a)),
            result: Some(function),
        }));
        let mut target = TypeInterner::new();
        target.intern(Type::Nominal(NominalTypeRef {
            read_only_erased_view: false,
            name: "app.Unrelated".into(),
            params: vec![],
            representation: None,
        }));
        let a2 = target.intern(Type::Nominal(NominalTypeRef {
            read_only_erased_view: false,
            name: "app.UserId".into(),
            params: vec![],
            representation: None,
        }));
        assert_ne!(a, a2);
        let array2 = target.intern(Type::Array(a2));
        let function2 = target.intern(Type::Function(FlowType {
            input: vec![array2],
            output: a2,
            effects: Some(typed_row(a2)),
        }));
        let handler2 = target.intern(Type::Handler(HandlerType {
            handled: typed_row(a2),
            produced: HandlerProducedEffects::Explicit(typed_row(a2)),
            result: Some(function2),
        }));
        let source = source.into_store();
        let target = target.into_store();
        let declarations = HashMap::from([(a, a2)]);
        let types = super::super::TypeLookupIndex::new(&target);
        let effects = HashMap::from([("Fs.read".into(), "Fs.read".into())]);
        let mut engine = TypeSubstitutionEngine::relocating(TypeRelocationInput {
            source: &source,
            target: &target,
            declaration_types: &declarations,
            target_types: &types,
            effect_names: &effects,
        });
        assert_eq!(engine.substitute(handler), Ok(handler2));
        assert_eq!(engine.substitute(function), Ok(function2));
        assert_eq!(engine.substitute(array), Ok(array2));
    }

    #[test]
    fn relocation_rejects_unproven_nominal_names_and_open_effect_rows() {
        let mut interner = TypeInterner::new();
        let nominal = interner.intern(Type::Nominal(NominalTypeRef {
            read_only_erased_view: false,
            name: "app.SameName".into(),
            params: vec![],
            representation: None,
        }));
        let unit = interner.primitive(PrimitiveType::Unit);
        let function = interner.intern(Type::Function(FlowType {
            input: vec![],
            output: unit,
            effects: Some(EffectRowRef {
                effects: vec![],
                tail: Some("E".into()),
            }),
        }));
        let store = interner.into_store();
        let declarations = HashMap::new();
        let types = super::super::TypeLookupIndex::new(&store);
        let effects = HashMap::new();
        let mut engine = TypeSubstitutionEngine::relocating(TypeRelocationInput {
            source: &store,
            target: &store,
            declaration_types: &declarations,
            target_types: &types,
            effect_names: &effects,
        });
        assert_eq!(
            engine.substitute(nominal),
            Err(TypeSubstitutionError::MissingTypeIdentity(nominal))
        );
        assert_eq!(
            engine.substitute(function),
            Err(TypeSubstitutionError::OpenEffectRow("E".into()))
        );
    }

    #[test]
    fn relocation_materializes_aggregate_types_without_inventing_declaration_identity() {
        let mut source = TypeInterner::new();
        let nominal = source.intern(Type::Nominal(NominalTypeRef {
            read_only_erased_view: false,
            name: "app.Id".into(),
            params: vec![],
            representation: None,
        }));
        let option = source.intern(Type::Option(nominal));
        let source = source.into_store();
        let mut target = TypeInterner::new();
        target.primitive(PrimitiveType::Bool);
        let destination = target.intern(Type::Nominal(NominalTypeRef {
            read_only_erased_view: false,
            name: "app.Id".into(),
            params: vec![],
            representation: None,
        }));
        let declarations = HashMap::from([(nominal, destination)]);
        let effects = HashMap::new();
        let relocated =
            TypeSubstitutionEngine::relocating_materialized(TypeRelocationMaterializationInput {
                source: &source,
                target: &mut target,
                declaration_types: &declarations,
                effect_names: &effects,
            })
            .substitute(option)
            .unwrap();
        assert_eq!(
            target.store().get(relocated),
            Some(&Type::Option(destination))
        );
        let missing = HashMap::new();
        assert_eq!(
            TypeSubstitutionEngine::relocating_materialized(TypeRelocationMaterializationInput {
                source: &source,
                target: &mut target,
                declaration_types: &missing,
                effect_names: &effects,
            })
            .substitute(option),
            Err(TypeSubstitutionError::MissingTypeIdentity(nominal))
        );
    }

    #[test]
    fn deeply_nested_substitution_uses_a_heap_worklist() {
        let mut interner = TypeInterner::new();
        let parameter = interner.intern(Type::Named(NamedTypeRef {
            source_parameter: None,
            name: "T".into(),
        }));
        let concrete = interner.primitive(PrimitiveType::I32);
        let mut root = parameter;
        for _ in 0..10_000 {
            root = interner.intern(Type::Array(root));
        }
        let substitutions = HashMap::from([("T".into(), concrete)]);
        let mut result = TypeSubstitutionEngine::materializing(&mut interner, &substitutions, None)
            .substitute(root)
            .unwrap();
        for _ in 0..10_000 {
            let Some(Type::Array(inner)) = interner.store().get(result) else {
                panic!("array identity was lost");
            };
            result = *inner;
        }
        assert_eq!(result, concrete);
    }

    #[test]
    fn deep_effect_row_binding_chains_do_not_recurse_on_the_native_stack() {
        let mut interner = TypeInterner::new();
        let names = HashMap::new();
        let mut rows = HashMap::new();
        for index in 0..10_000 {
            rows.insert(
                format!("E{index}"),
                EffectRowRef {
                    effects: vec![],
                    tail: Some(format!("E{}", index + 1)),
                },
            );
        }
        let result = EffectRowRef {
            effects: vec![crate::EffectRef {
                name: "Console.stdout_write".into(),
                args: vec![],
            }],
            tail: None,
        };
        rows.insert("E10000".into(), result.clone());
        let actual = TypeSubstitutionEngine::materializing(&mut interner, &names, None)
            .with_row_equations(&rows)
            .substitute_effect_row(EffectRowRef {
                effects: vec![],
                tail: Some("E0".into()),
            })
            .unwrap();
        assert_eq!(actual, result);
    }

    #[test]
    fn failed_type_cycle_queries_do_not_poison_the_worklist() {
        let mut interner = TypeInterner::new();
        let parameter = interner.intern(Type::Var(crate::TypeVarId(0)));
        let cycle = interner.intern(Type::Array(parameter));
        let bindings = HashMap::new();
        let mut variables = Substitution::default();
        variables.insert(crate::TypeVarId(0), cycle);
        let mut engine =
            TypeSubstitutionEngine::materializing(&mut interner, &bindings, Some(&variables));
        for _ in 0..2 {
            assert_eq!(
                engine.substitute(parameter),
                Err(TypeSubstitutionError::CyclicType(parameter))
            );
        }
    }

    #[test]
    fn readonly_identity_substitution_does_not_rewrite_original_row_members() {
        let mut interner = TypeInterner::new();
        let unit = interner.primitive(PrimitiveType::Unit);
        let member = crate::EffectRef {
            name: "Console.stdout_write".into(),
            args: vec![],
        };
        let function = interner.intern(Type::Function(FlowType {
            input: vec![],
            output: unit,
            effects: Some(EffectRowRef {
                effects: vec![member.clone(), member],
                tail: None,
            }),
        }));
        let store = interner.into_store();
        let bindings = HashMap::new();
        assert_eq!(
            TypeSubstitutionEngine::existing(&store, &bindings).substitute(function),
            Ok(function)
        );
    }

    #[test]
    fn named_arguments_preserve_the_callers_identically_spelled_binder() {
        let mut interner = TypeInterner::new();
        let parameter = interner.intern(Type::Named(NamedTypeRef {
            source_parameter: None,
            name: "T".into(),
        }));
        let argument = interner.intern(Type::Array(parameter));
        let template = interner.intern(Type::Option(parameter));
        let expected = interner.intern(Type::Option(argument));
        let store = interner.into_store();
        let bindings = HashMap::from([("T".into(), argument)]);
        assert_eq!(
            TypeSubstitutionEngine::existing(&store, &bindings).substitute(template),
            Ok(expected)
        );
    }

    #[test]
    fn simultaneous_named_arguments_still_normalize_solver_variables() {
        let mut interner = TypeInterner::new();
        let parameter = interner.intern(Type::Named(NamedTypeRef {
            source_parameter: None,
            name: "T".into(),
        }));
        let variable = interner.intern(Type::Var(crate::TypeVarId(0)));
        let concrete = interner.primitive(PrimitiveType::I32);
        let bindings = HashMap::from([("T".into(), variable)]);
        let mut variables = Substitution::default();
        variables.insert(crate::TypeVarId(0), concrete);
        assert_eq!(
            TypeSubstitutionEngine::materializing(&mut interner, &bindings, Some(&variables))
                .substitute(parameter),
            Ok(concrete)
        );
    }

    #[test]
    fn source_parameter_substitution_does_not_capture_same_spelled_binders() {
        let mut interner = TypeInterner::new();
        let own = interner.intern(Type::Named(NamedTypeRef {
            source_parameter: Some(etas_hir::SymbolId(31)),
            name: "T".into(),
        }));
        let other = interner.intern(Type::Named(NamedTypeRef {
            source_parameter: Some(etas_hir::SymbolId(32)),
            name: "T".into(),
        }));
        let external = interner.intern(Type::Named(NamedTypeRef {
            source_parameter: None,
            name: "T".into(),
        }));
        let boolean = interner.primitive(PrimitiveType::Bool);
        let integer = interner.primitive(PrimitiveType::I32);
        let template = interner.intern(Type::Tuple(vec![own, other, external]));
        let expected = interner.intern(Type::Tuple(vec![boolean, other, external]));
        let names = HashMap::from([("T".into(), integer)]);
        let parameters = HashMap::from([(etas_hir::SymbolId(31), boolean)]);
        assert_eq!(
            TypeSubstitutionEngine::materializing(&mut interner, &names, None)
                .with_source_parameters(&parameters)
                .substitute(template),
            Ok(expected)
        );
    }

    #[test]
    fn relocation_budget_is_shared_across_engines_and_bounds_edges() {
        let mut source = TypeStore::new();
        let boolean = source.intern(Type::Primitive(PrimitiveType::Bool));
        let tuple = source.intern(Type::Tuple(vec![boolean, boolean]));
        let declarations = HashMap::new();
        let names = HashMap::new();
        let budget = super::TypeSubstitutionBudget::new(2, 2, None);
        let mut target = TypeInterner::new();
        let mut engine = TypeSubstitutionEngine::relocating_materialized(
            super::TypeRelocationMaterializationInput {
                source: &source,
                target: &mut target,
                declaration_types: &declarations,
                effect_names: &names,
            },
        )
        .with_budget(&budget);
        assert!(engine.substitute(tuple).is_ok());
        let mut target = TypeInterner::new();
        let mut another = TypeSubstitutionEngine::relocating_materialized(
            super::TypeRelocationMaterializationInput {
                source: &source,
                target: &mut target,
                declaration_types: &declarations,
                effect_names: &names,
            },
        )
        .with_budget(&budget);
        assert_eq!(
            another.substitute(boolean),
            Err(TypeSubstitutionError::WorkLimit {
                resource: "nodes",
                limit: 2
            })
        );
        let edge_budget = super::TypeSubstitutionBudget::new(10, 1, None);
        let mut target = TypeInterner::new();
        let mut limited = TypeSubstitutionEngine::relocating_materialized(
            super::TypeRelocationMaterializationInput {
                source: &source,
                target: &mut target,
                declaration_types: &declarations,
                effect_names: &names,
            },
        )
        .with_budget(&edge_budget);
        assert_eq!(
            limited.substitute(tuple),
            Err(TypeSubstitutionError::WorkLimit {
                resource: "edges",
                limit: 1
            })
        );
    }

    #[test]
    fn relocation_propagates_cancellation_without_returning_a_type() {
        struct Cancel;
        impl etas_utils::PipelineExecutionControl for Cancel {
            fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
                Err(etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    reason: "cancel type relocation".into(),
                })
            }
        }
        let mut source = TypeStore::new();
        let boolean = source.intern(Type::Primitive(PrimitiveType::Bool));
        let declarations = HashMap::new();
        let names = HashMap::new();
        let budget = super::TypeSubstitutionBudget::new(10, 10, Some(&Cancel));
        let mut target = TypeInterner::new();
        let mut engine = TypeSubstitutionEngine::relocating_materialized(
            super::TypeRelocationMaterializationInput {
                source: &source,
                target: &mut target,
                declaration_types: &declarations,
                effect_names: &names,
            },
        )
        .with_budget(&budget);
        assert!(matches!(
            engine.substitute(boolean),
            Err(TypeSubstitutionError::Interrupted(_))
        ));
    }

    #[test]
    fn substitution_observes_cancellation_within_a_single_type_and_effect_row() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct StopAfter(AtomicUsize);
        impl etas_utils::PipelineExecutionControl for StopAfter {
            fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
                if self.0.fetch_add(1, Ordering::SeqCst) >= 2 {
                    return Err(etas_utils::PipelineInterruption {
                        kind: etas_utils::PipelineInterruptionKind::Cancelled,
                        reason: "cancel substitution".into(),
                    });
                }
                Ok(())
            }
        }

        let mut interner = TypeInterner::new();
        let boolean = interner.primitive(PrimitiveType::Bool);
        let tuple = interner.intern(Type::Tuple(vec![boolean, boolean]));
        let names = HashMap::new();
        let control = StopAfter(AtomicUsize::new(0));
        let mut engine = TypeSubstitutionEngine::materializing(&mut interner, &names, None)
            .with_execution_control(Some(&control));
        assert!(matches!(
            engine.substitute(tuple),
            Err(TypeSubstitutionError::Interrupted(_))
        ));
        assert_eq!(control.0.load(Ordering::SeqCst), 3);

        let control = StopAfter(AtomicUsize::new(0));
        let mut engine = TypeSubstitutionEngine::materializing(&mut interner, &names, None)
            .with_execution_control(Some(&control));
        let row = crate::EffectRowRef {
            effects: vec![
                crate::EffectRef {
                    name: "A".into(),
                    args: vec![],
                },
                crate::EffectRef {
                    name: "B".into(),
                    args: vec![],
                },
            ],
            tail: None,
        };
        assert!(matches!(
            engine.substitute_effect_row(row),
            Err(TypeSubstitutionError::Interrupted(_))
        ));
        assert_eq!(control.0.load(Ordering::SeqCst), 3);
    }
}
