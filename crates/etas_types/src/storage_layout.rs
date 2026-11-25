//! Closed storage layouts, distinct from marker-spec admission. Collection
//! layouts use their checked Etas adapters instead of private bucket/list state.
//! Named references terminate recursive schema expansion; anonymous shapes are
//! hashed structurally, independently of interner allocation and graph sharing.
use std::collections::{BTreeMap, HashMap};

use etas_code::{CodeFingerprint, DefinitionKey, MethodId};

use crate::{
    PrimitiveType, TrustWrapper, Type, TypeFacts, TypeId, TypeInterner, TypeSubstitutionEngine,
    TypeSubstitutionError,
    method::{MethodReceiver, MethodSignatureFact, MethodTarget},
    ty::{applied_representation, canonical_parameters_for_application},
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub struct StorageIdentity {
    pub declaration: DefinitionKey,
    pub arguments: Vec<CodeFingerprint>,
}

impl StorageIdentity {
    pub fn fingerprint(&self) -> CodeFingerprint {
        digest("etas.storage.type-identity.v1", self)
    }
}

/// The caller supplies checked declaration identities and closed argument
/// identities. Filesystem paths, TypeIds and callable body snapshots are not
/// persistent type identities.
pub trait StorageIdentities {
    fn identity(&mut self, store: &crate::TypeStore, ty: TypeId)
    -> Result<StorageIdentity, String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct StorageNodeId(pub usize);

#[derive(Clone, Debug)]
pub struct StorageField {
    pub name: String,
    /// Native enum payload position; record access uses the field name.
    pub position: usize,
    pub node: StorageNodeId,
}

#[derive(Clone, Debug)]
pub struct StorageVariant {
    pub name: String,
    pub named: bool,
    pub fields: Vec<StorageField>,
}

#[derive(Clone, Debug)]
pub struct StorageCall {
    pub identity: MethodId,
    pub target: MethodTarget,
    pub owner_bindings: Vec<(TypeId, TypeId)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum StorageCollection {
    Map,
    Set,
    Slice,
    Stack,
    Queue,
    Deque,
    BoundedDeque,
}

#[derive(Clone, Debug)]
pub enum StorageKind {
    Primitive(PrimitiveType),
    Tuple(Vec<StorageNodeId>),
    Array(StorageNodeId),
    List(StorageNodeId),
    Slice(StorageNodeId),
    Set(StorageNodeId),
    Map(StorageNodeId, StorageNodeId),
    Range(StorageNodeId),
    Option(StorageNodeId),
    Result(StorageNodeId, StorageNodeId),
    Record(Vec<StorageField>),
    Nominal {
        representation: StorageNodeId,
    },
    Enum(Vec<StorageVariant>),
    Trust {
        wrapper: TrustWrapper,
        inner: StorageNodeId,
    },
    Collection {
        kind: StorageCollection,
        view: StorageNodeId,
        capture: StorageCall,
        restore: StorageCall,
        fallible_restore: bool,
    },
}

#[derive(Clone, Debug)]
pub struct StorageNode {
    pub ty: TypeId,
    pub identity: Option<StorageIdentity>,
    pub kind: StorageKind,
}

#[derive(Clone, Debug)]
pub struct StorageLayout {
    pub root: StorageNodeId,
    pub nodes: Vec<StorageNode>,
    pub schema: CodeFingerprint,
}

#[derive(Debug)]
pub enum StorageLayoutError {
    Unsupported(TypeId),
    MissingRepresentation(TypeId),
    ProtectedProvenanceRequired(TypeId),
    Identity { ty: TypeId, reason: String },
    Adapter { ty: TypeId, reason: &'static str },
    InconsistentIdentity(StorageIdentity),
    AnonymousCycle(TypeId),
    Substitution(TypeSubstitutionError),
}

impl From<TypeSubstitutionError> for StorageLayoutError {
    fn from(value: TypeSubstitutionError) -> Self {
        Self::Substitution(value)
    }
}

impl std::fmt::Display for StorageLayoutError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(ty) => {
                write!(out, "type {ty:?} has no derivable storage representation")
            }
            Self::MissingRepresentation(ty) => {
                write!(out, "type {ty:?} requires an explicit storage codec")
            }
            Self::ProtectedProvenanceRequired(ty) => {
                write!(out, "type {ty:?} requires protected storage provenance")
            }
            Self::Identity { ty, reason } => write!(out, "storage identity for {ty:?}: {reason}"),
            Self::Adapter { ty, reason } => write!(out, "storage adapter for {ty:?}: {reason}"),
            Self::InconsistentIdentity(identity) => write!(
                out,
                "storage identity has inconsistent layouts: {identity:?}"
            ),
            Self::AnonymousCycle(ty) => write!(
                out,
                "recursive storage shape at {ty:?} requires a named declaration"
            ),
            Self::Substitution(error) => error.fmt(out),
        }
    }
}
impl std::error::Error for StorageLayoutError {}

impl StorageLayout {
    pub fn derive(
        interner: &mut TypeInterner,
        facts: &TypeFacts,
        root: TypeId,
        identities: &mut dyn StorageIdentities,
        maximum_nodes: usize,
    ) -> Result<Self, StorageLayoutError> {
        let mut builder = Builder {
            interner,
            facts,
            identities: Some(identities),
            maximum_nodes,
            pending: Vec::new(),
            slots: HashMap::new(),
            nodes: Vec::new(),
            adapters: facts.storage_methods().collect(),
        };
        let root = builder.reference(root)?;
        while let Some((id, ty)) = builder.pending.pop() {
            let node = builder.node(ty)?;
            builder.nodes[id.0] = Some(node);
        }
        let nodes = builder
            .nodes
            .into_iter()
            .map(|node| node.expect("completed storage node"))
            .collect::<Vec<_>>();
        let schema = schema(&nodes, root)?;
        Ok(Self {
            root,
            nodes,
            schema,
        })
    }
}

/// Materialize the same closed data graph during native invocation preparation.
/// Portable identity assignment is deliberately deferred; no placeholder
/// identity or schema is created by this operation.
pub fn materialize_storage_shapes(
    interner: &mut TypeInterner,
    facts: &TypeFacts,
    root: TypeId,
    maximum_nodes: usize,
) -> Result<Vec<StorageNode>, StorageLayoutError> {
    let mut builder = Builder {
        interner,
        facts,
        identities: None,
        maximum_nodes,
        pending: Vec::new(),
        slots: HashMap::new(),
        nodes: Vec::new(),
        adapters: facts.storage_methods().collect(),
    };
    builder.reference(root)?;
    while let Some((id, ty)) = builder.pending.pop() {
        let node = builder.node(ty)?;
        builder.nodes[id.0] = Some(node);
    }
    Ok(builder
        .nodes
        .into_iter()
        .map(|node| node.expect("completed storage shape"))
        .collect())
}

struct Builder<'a> {
    interner: &'a mut TypeInterner,
    facts: &'a TypeFacts,
    identities: Option<&'a mut dyn StorageIdentities>,
    maximum_nodes: usize,
    pending: Vec<(StorageNodeId, TypeId)>,
    slots: HashMap<TypeId, StorageNodeId>,
    nodes: Vec<Option<StorageNode>>,
    adapters: Vec<(&'a MethodSignatureFact, &'a MethodId)>,
}

impl Builder<'_> {
    fn reference(&mut self, ty: TypeId) -> Result<StorageNodeId, StorageLayoutError> {
        if let Some(id) = self.slots.get(&ty) {
            return Ok(*id);
        }
        if self.nodes.len() >= self.maximum_nodes {
            return Err(TypeSubstitutionError::RepresentationLimit(self.maximum_nodes).into());
        }
        let id = StorageNodeId(self.nodes.len());
        self.nodes.push(None);
        self.slots.insert(ty, id);
        self.pending.push((id, ty));
        Ok(id)
    }

    fn identity(&mut self, ty: TypeId) -> Result<Option<StorageIdentity>, StorageLayoutError> {
        match &mut self.identities {
            Some(identities) => identities
                .identity(self.interner.store(), ty)
                .map(Some)
                .map_err(|reason| StorageLayoutError::Identity { ty, reason }),
            None => Ok(None),
        }
    }

    fn node(&mut self, ty: TypeId) -> Result<StorageNode, StorageLayoutError> {
        let data = self
            .interner
            .store()
            .get(ty)
            .cloned()
            .ok_or(TypeSubstitutionError::MissingType(ty))?;
        let (owner, arguments) = match &data {
            Type::Applied { constructor, args } => (TypeId(constructor.0), args.clone()),
            _ => (ty, Vec::new()),
        };
        let known = &self.facts.known_std_types;
        if known.storage_resources.contains(&owner)
            || known.opaque_values.contains(&owner)
            || crate::source_execution::is_scoped_handle(self.interner.store(), known, ty)
        {
            return Err(StorageLayoutError::Unsupported(ty));
        }

        if let Some(wrapper) = known.trust_wrappers.get(&owner).copied() {
            if !matches!(wrapper, TrustWrapper::Public | TrustWrapper::Untrusted) {
                return self.trust(ty, None, wrapper, arguments[0]);
            }
            let identity = self.identity(ty)?;
            return self.trust(ty, identity, wrapper, arguments[0]);
        }
        if let Some((kind, view, capture, restore, fallible_restore)) = self.adapter(ty)? {
            let identity = self.identity(ty)?;
            let view = self.reference(view)?;
            return Ok(StorageNode {
                ty,
                identity,
                kind: StorageKind::Collection {
                    kind,
                    view,
                    capture,
                    restore,
                    fallible_restore,
                },
            });
        }

        let mut identity = None;
        let kind = match data {
            Type::Primitive(value) => StorageKind::Primitive(value),
            Type::Tuple(values) => StorageKind::Tuple(
                values
                    .into_iter()
                    .map(|ty| self.reference(ty))
                    .collect::<Result<_, _>>()?,
            ),
            Type::Array(value) => StorageKind::Array(self.reference(value)?),
            Type::List(value) => StorageKind::List(self.reference(value)?),
            Type::Slice(value) => StorageKind::Slice(self.reference(value)?),
            Type::Set(value) => StorageKind::Set(self.reference(value)?),
            Type::Map { key, value } => {
                StorageKind::Map(self.reference(key)?, self.reference(value)?)
            }
            Type::Range { index } => StorageKind::Range(self.reference(index)?),
            Type::Option(value) => StorageKind::Option(self.reference(value)?),
            Type::Result { ok, err } => {
                StorageKind::Result(self.reference(ok)?, self.reference(err)?)
            }
            Type::Trust { wrapper, inner } => return self.trust(ty, None, wrapper, inner),
            Type::Record(record) => {
                let mut fields = record.fields.into_iter().enumerate().collect::<Vec<_>>();
                fields.sort_by(|(_, a), (_, b)| a.name.cmp(&b.name));
                StorageKind::Record(
                    fields
                        .into_iter()
                        .map(|(position, field)| {
                            Ok(StorageField {
                                name: field.name,
                                position,
                                node: self.reference(field.ty)?,
                            })
                        })
                        .collect::<Result<_, StorageLayoutError>>()?,
                )
            }
            Type::Nominal(_) | Type::Enum(_) | Type::Applied { .. } => {
                identity = self.identity(ty)?;
                match self.interner.store().get(owner) {
                    Some(Type::Enum(_)) => StorageKind::Enum(self.variants(owner, &arguments)?),
                    Some(Type::Nominal(_)) => {
                        let representation = applied_representation(self.interner, ty)?
                            .ok_or(StorageLayoutError::MissingRepresentation(ty))?;
                        StorageKind::Nominal {
                            representation: self.reference(representation)?,
                        }
                    }
                    _ => return Err(StorageLayoutError::Unsupported(ty)),
                }
            }
            _ => return Err(StorageLayoutError::Unsupported(ty)),
        };
        Ok(StorageNode { ty, identity, kind })
    }

    fn trust(
        &mut self,
        ty: TypeId,
        identity: Option<StorageIdentity>,
        wrapper: TrustWrapper,
        inner: TypeId,
    ) -> Result<StorageNode, StorageLayoutError> {
        match wrapper {
            TrustWrapper::Secret => return Err(StorageLayoutError::Unsupported(ty)),
            TrustWrapper::Trusted | TrustWrapper::Sanitized => {
                return Err(StorageLayoutError::ProtectedProvenanceRequired(ty));
            }
            TrustWrapper::Public | TrustWrapper::Untrusted => {}
        }
        let inner = self.reference(inner)?;
        Ok(StorageNode {
            ty,
            identity,
            kind: StorageKind::Trust { wrapper, inner },
        })
    }

    fn variants(
        &mut self,
        owner: TypeId,
        arguments: &[TypeId],
    ) -> Result<Vec<StorageVariant>, StorageLayoutError> {
        let layout = self
            .facts
            .enum_layouts
            .get(&owner)
            .ok_or(TypeSubstitutionError::MissingEnumLayout(owner))?;
        if layout.type_params.len() != arguments.len() {
            return Err(TypeSubstitutionError::InvalidApplication(owner).into());
        }
        let names = layout
            .type_params
            .iter()
            .cloned()
            .zip(arguments.iter().copied())
            .collect();
        let canonical =
            canonical_parameters_for_application(self.interner.store(), owner, arguments)?;
        let parameters = self
            .facts
            .type_generics
            .get(&owner)
            .into_iter()
            .flatten()
            .zip(arguments)
            .map(|(parameter, ty)| (parameter.subject, *ty))
            .collect();
        let mut variants = layout.variants.clone();
        variants.sort_by(|a, b| a.name.cmp(&b.name));
        variants
            .into_iter()
            .map(|variant| {
                let named = variant.field_names.is_some();
                let mut fields = variant
                    .fields
                    .into_iter()
                    .enumerate()
                    .map(|(position, ty)| {
                        let ty = TypeSubstitutionEngine::materializing(self.interner, &names, None)
                            .with_canonical_parameters(&canonical)
                            .with_type_parameters(&parameters)
                            .substitute(ty)?;
                        Ok(StorageField {
                            name: variant.field_names.as_ref().map_or_else(
                                || position.to_string(),
                                |names| names[position].clone(),
                            ),
                            position,
                            node: self.reference(ty)?,
                        })
                    })
                    .collect::<Result<Vec<_>, StorageLayoutError>>()?;
                if named {
                    fields.sort_by(|a, b| a.name.cmp(&b.name));
                }
                Ok(StorageVariant {
                    name: variant.name,
                    named,
                    fields,
                })
            })
            .collect()
    }

    fn adapter(
        &mut self,
        ty: TypeId,
    ) -> Result<
        Option<(StorageCollection, TypeId, StorageCall, StorageCall, bool)>,
        StorageLayoutError,
    > {
        let Some((view, view_identity, bindings)) =
            self.adapters.iter().find_map(|(method, identity)| {
                (method.name == etas_std::SOURCE_STORAGE_VIEW)
                    .then(|| {
                        crate::solver::spec_solver::match_implementation_subject(
                            self.interner.store(),
                            &method.owner_params,
                            method.receiver_type,
                            ty,
                        )
                        .map(|bindings| (*method, *identity, bindings))
                    })
                    .flatten()
            })
        else {
            return Ok(None);
        };
        let error = |reason| StorageLayoutError::Adapter { ty, reason };
        if view.receiver != (MethodReceiver::Value { slot: 0 })
            || view.signature.params.len() != 1
            || !view.signature.generic_params.is_empty()
        {
            return Err(error(
                "view must be an ordinary non-generic receiver method",
            ));
        }
        let owner = view_identity.implementation().subject();
        let kind = match owner.declaration().last().map(String::as_str) {
            Some("Map") => StorageCollection::Map,
            Some("Set") => StorageCollection::Set,
            Some("Slice") => StorageCollection::Slice,
            Some("Stack") => StorageCollection::Stack,
            Some("Queue") => StorageCollection::Queue,
            Some("Deque") => StorageCollection::Deque,
            Some("BoundedDeque") => StorageCollection::BoundedDeque,
            _ => return Err(error("unknown checked collection owner")),
        };
        let names = view
            .owner_params
            .iter()
            .map(|p| {
                Ok((
                    p.name.clone(),
                    *bindings
                        .get(&p.subject)
                        .ok_or_else(|| error("unbound view owner parameter"))?,
                ))
            })
            .collect::<Result<HashMap<_, _>, StorageLayoutError>>()?;
        let view_type = TypeSubstitutionEngine::materializing(self.interner, &names, None)
            .with_type_parameters(&bindings)
            .substitute(view.signature.output)?;
        let capture = storage_call(view, view_identity, &bindings)?;
        let Some((build, build_identity, bindings)) =
            self.adapters.iter().find_map(|(method, identity)| {
                (method.name == etas_std::SOURCE_STORAGE_BUILD
                    && identity.implementation().subject() == owner)
                    .then(|| {
                        crate::solver::spec_solver::match_implementation_subject(
                            self.interner.store(),
                            &method.owner_params,
                            method.receiver_type,
                            ty,
                        )
                        .map(|bindings| (*method, *identity, bindings))
                    })
                    .flatten()
            })
        else {
            return Err(error("view has no matching reconstruction method"));
        };
        if build.receiver != MethodReceiver::Associated
            || build.signature.params.len() != 1
            || !build.signature.generic_params.is_empty()
        {
            return Err(error(
                "reconstruction must be an ordinary non-generic associated method",
            ));
        }
        let names = build
            .owner_params
            .iter()
            .map(|p| {
                Ok((
                    p.name.clone(),
                    *bindings
                        .get(&p.subject)
                        .ok_or_else(|| error("unbound reconstruction owner parameter"))?,
                ))
            })
            .collect::<Result<HashMap<_, _>, StorageLayoutError>>()?;
        let mut engine = TypeSubstitutionEngine::materializing(self.interner, &names, None)
            .with_type_parameters(&bindings);
        let input = engine.substitute(build.signature.params[0])?;
        let output = engine.substitute(build.signature.output)?;
        if input != view_type {
            return Err(error("reconstruction input differs from the logical view"));
        }
        let fallible_restore = output != ty;
        if fallible_restore
            && crate::value_representation::option_parameter(
                self.interner.store(),
                &self.facts.known_std_types,
                output,
            ) != Some(ty)
        {
            return Err(error(
                "reconstruction result must be the collection or Option of it",
            ));
        }
        let restore = storage_call(build, build_identity, &bindings)?;
        Ok(Some((kind, view_type, capture, restore, fallible_restore)))
    }
}

fn storage_call(
    method: &MethodSignatureFact,
    identity: &MethodId,
    bindings: &HashMap<TypeId, TypeId>,
) -> Result<StorageCall, StorageLayoutError> {
    if !matches!(
        method.target,
        MethodTarget::Source { body: Some(_), .. } | MethodTarget::External { .. }
    ) {
        return Err(StorageLayoutError::Adapter {
            ty: method.receiver_type,
            reason: "adapter requires a checked Etas implementation",
        });
    }
    let owner_bindings = method
        .owner_params
        .iter()
        .map(|p| {
            bindings
                .get(&p.subject)
                .copied()
                .map(|ty| (p.subject, ty))
                .ok_or(StorageLayoutError::Adapter {
                    ty: method.receiver_type,
                    reason: "unbound adapter owner parameter",
                })
        })
        .collect::<Result<_, _>>()?;
    Ok(StorageCall {
        identity: identity.clone(),
        target: method.target.clone(),
        owner_bindings,
    })
}

fn digest(domain: &'static str, value: &impl serde::Serialize) -> CodeFingerprint {
    CodeFingerprint::of_bytes(
        domain,
        &serde_json::to_vec(value).expect("storage schema serializes"),
    )
}

fn shape(node: &StorageNode) -> (String, Vec<StorageNodeId>) {
    let mut children = Vec::new();
    let label = match &node.kind {
        StorageKind::Primitive(primitive) => {
            serde_json::to_string(&("primitive", primitive)).unwrap()
        }
        StorageKind::Tuple(values) => {
            children.extend(values);
            "tuple".into()
        }
        StorageKind::Array(inner) => {
            children.push(*inner);
            "array".into()
        }
        StorageKind::List(inner) => {
            children.push(*inner);
            "list".into()
        }
        StorageKind::Slice(inner) => {
            children.push(*inner);
            "slice".into()
        }
        StorageKind::Set(inner) => {
            children.push(*inner);
            "set".into()
        }
        StorageKind::Map(key, value) => {
            children.extend([*key, *value]);
            "map".into()
        }
        StorageKind::Range(inner) => {
            children.push(*inner);
            "range".into()
        }
        StorageKind::Option(inner) => {
            children.push(*inner);
            "option".into()
        }
        StorageKind::Result(ok, err) => {
            children.extend([*ok, *err]);
            "result".into()
        }
        StorageKind::Record(fields) => {
            children.extend(fields.iter().map(|field| field.node));
            serde_json::to_string(&(
                "record",
                fields.iter().map(|field| &field.name).collect::<Vec<_>>(),
            ))
            .unwrap()
        }
        StorageKind::Nominal { representation } => {
            children.push(*representation);
            "nominal".into()
        }
        StorageKind::Enum(variants) => {
            children.extend(
                variants
                    .iter()
                    .flat_map(|variant| variant.fields.iter().map(|field| field.node)),
            );
            serde_json::to_string(&(
                "enum",
                variants
                    .iter()
                    .map(|variant| {
                        (
                            &variant.name,
                            variant.named,
                            variant
                                .fields
                                .iter()
                                .map(|field| &field.name)
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect::<Vec<_>>(),
            ))
            .unwrap()
        }
        StorageKind::Trust { wrapper, inner } => {
            children.push(*inner);
            serde_json::to_string(&("trust", wrapper)).unwrap()
        }
        StorageKind::Collection { kind, view, .. } => {
            children.push(*view);
            serde_json::to_string(&("collection", kind)).unwrap()
        }
    };
    (label, children)
}

fn schema(
    nodes: &[StorageNode],
    root: StorageNodeId,
) -> Result<CodeFingerprint, StorageLayoutError> {
    let mut anonymous = vec![None; nodes.len()];
    let mut active = vec![false; nodes.len()];
    let mut definitions = BTreeMap::new();
    for (index, node) in nodes.iter().enumerate() {
        let Some(identity) = &node.identity else {
            continue;
        };
        let fingerprint = shape_hash(nodes, StorageNodeId(index), &mut anonymous, &mut active)?;
        if definitions
            .insert(identity.clone(), fingerprint)
            .is_some_and(|old| old != fingerprint)
        {
            return Err(StorageLayoutError::InconsistentIdentity(identity.clone()));
        }
    }
    let root = expression_hash(nodes, root, &mut anonymous, &mut active)?;
    Ok(digest(
        "etas.storage.schema.v1",
        &(root, definitions.into_iter().collect::<Vec<_>>()),
    ))
}

fn expression_hash(
    nodes: &[StorageNode],
    root: StorageNodeId,
    memo: &mut [Option<CodeFingerprint>],
    active: &mut [bool],
) -> Result<CodeFingerprint, StorageLayoutError> {
    if let Some(identity) = &nodes[root.0].identity {
        return Ok(digest("etas.storage.named-reference.v1", identity));
    }
    shape_hash(nodes, root, memo, active)
}

fn shape_hash(
    nodes: &[StorageNode],
    root: StorageNodeId,
    memo: &mut [Option<CodeFingerprint>],
    active: &mut [bool],
) -> Result<CodeFingerprint, StorageLayoutError> {
    let mut work = vec![(root, false)];
    while let Some((id, finish)) = work.pop() {
        if memo[id.0].is_some() {
            continue;
        }
        let (label, children) = shape(&nodes[id.0]);
        if finish {
            let children = children
                .into_iter()
                .map(|child| {
                    nodes[child.0].identity.as_ref().map_or_else(
                        || memo[child.0].expect("completed anonymous child"),
                        |identity| digest("etas.storage.named-reference.v1", identity),
                    )
                })
                .collect::<Vec<_>>();
            memo[id.0] = Some(digest("etas.storage.shape.v1", &(label, children)));
            active[id.0] = false;
        } else {
            if active[id.0] {
                return Err(StorageLayoutError::AnonymousCycle(nodes[id.0].ty));
            }
            active[id.0] = true;
            work.push((id, true));
            work.extend(
                children
                    .into_iter()
                    .rev()
                    .filter(|child| nodes[child.0].identity.is_none())
                    .map(|child| (child, false)),
            );
        }
    }
    Ok(memo[root.0].expect("completed root shape"))
}
