//! Finite, position-preserving paths through recursive payload types.
use std::collections::{BTreeMap, BTreeSet};

use etas_code::descriptor::ValueProjectionStep as Step;
use etas_types::{Type, TypeId};

use super::engine::EffectSemantics;

type Paths = BTreeSet<Vec<Step>>;
type Edges = BTreeMap<(usize, usize), Paths>;

impl EffectSemantics<'_> {
    pub(super) fn value_payload_paths(&self, root: TypeId) -> Option<Paths> {
        if let Some(paths) = self.type_queries.payload_paths.borrow().get(&root) {
            return Some(paths.clone());
        }
        let paths = self.collect_payload_paths(root)?;
        self.type_queries
            .payload_paths
            .borrow_mut()
            .insert(root, paths.clone());
        Some(paths)
    }

    fn collect_payload_paths(&self, root: TypeId) -> Option<Paths> {
        let mut indices = BTreeMap::from([(root, 0usize)]);
        let mut pending = vec![root];
        let mut edges = Edges::new();
        let mut leaves = BTreeSet::new();
        while let Some(ty) = pending.pop() {
            if let Some(control) = self.query_control {
                control.checkpoint().ok()?;
            }
            if indices.len() > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
                return None;
            }
            if !self.type_contains_flow(ty) && !self.type_has_parameters(ty) {
                continue;
            }
            let from = indices[&ty];
            let representation = if matches!(
                self.types.store.get(ty),
                Some(Type::Nominal(_) | Type::Applied { .. })
            ) {
                self.applied_representation(ty).ok()?.unwrap_or(ty)
            } else {
                ty
            };
            let children = if representation != ty {
                vec![(Vec::new(), representation)]
            } else {
                let steps = self.payload_steps(ty)?;
                if steps.is_empty() {
                    leaves.insert(from);
                }
                let mut children = Vec::new();
                for step in steps {
                    let child = self.project_value(ty, std::slice::from_ref(&step)).ok()??;
                    children.push((vec![step], child));
                }
                children
            };
            for (path, child) in children {
                let next = indices.len();
                let to = *indices.entry(child).or_insert_with(|| {
                    pending.push(child);
                    next
                });
                edges.entry((from, to)).or_default().insert(path);
            }
        }
        let start = indices.len();
        let end = start + 1;
        edges.entry((start, 0)).or_default().insert(Vec::new());
        for leaf in leaves {
            edges.entry((leaf, end)).or_default().insert(Vec::new());
        }
        let mut remaining = (0..start).collect::<BTreeSet<_>>();
        while !remaining.is_empty() {
            // Eliminate a type vertex with few connections first. Every star
            // therefore traverses back to the same checked type, never across
            // unrelated fields or through an invocation.
            let node = *remaining.iter().min_by_key(|node| {
                let incoming = edges.keys().filter(|(a, b)| b == *node && a != b).count();
                let outgoing = edges.keys().filter(|(a, b)| a == *node && a != b).count();
                (incoming.saturating_mul(outgoing), **node)
            })?;
            remaining.remove(&node);
            let loops = edges.remove(&(node, node)).unwrap_or_default();
            let alternatives = loops
                .into_iter()
                .filter(|path| !path.is_empty())
                .collect::<Vec<_>>();
            let repeated = if alternatives.is_empty() {
                Vec::new()
            } else {
                vec![Step::Repeat { alternatives }]
            };
            let incoming = edges
                .iter()
                .filter(|((_, b), _)| *b == node)
                .map(|((a, _), paths)| (*a, paths.clone()))
                .collect::<Vec<_>>();
            let outgoing = edges
                .iter()
                .filter(|((a, _), _)| *a == node)
                .map(|((_, b), paths)| (*b, paths.clone()))
                .collect::<Vec<_>>();
            edges.retain(|(a, b), _| *a != node && *b != node);
            for (from, prefixes) in &incoming {
                for (to, suffixes) in &outgoing {
                    let joined = edges.entry((*from, *to)).or_default();
                    for prefix in prefixes {
                        for suffix in suffixes {
                            let mut path = prefix.clone();
                            path.extend_from_slice(&repeated);
                            path.extend_from_slice(suffix);
                            if !Step::well_formed(
                                &path,
                                etas_package_metadata::MAX_METADATA_GRAPH_DEPTH,
                                etas_package_metadata::MAX_METADATA_GRAPH_NODES,
                            ) {
                                return None;
                            }
                            joined.insert(path);
                        }
                    }
                }
            }
        }
        let mut paths = edges.remove(&(start, end)).unwrap_or_default();
        paths.remove(&Vec::new());
        Some(paths)
    }

    fn payload_steps(&self, ty: TypeId) -> Option<Vec<Step>> {
        let steps = match self.types.store.get(ty)? {
            Type::Tuple(elements) => (0..elements.len())
                .map(|i| Step::TupleElement(i as u32))
                .collect::<Vec<_>>(),
            Type::Record(record) => record
                .fields
                .iter()
                .map(|field| Step::Field(field.name.clone()))
                .collect(),
            Type::Array(_) | Type::List(_) | Type::Set(_) | Type::Slice(_) => {
                vec![Step::SequenceElement]
            }
            Type::Map { .. } => vec![Step::MapKey, Step::MapValue],
            Type::Option(_) => vec![Step::VariantPayload {
                variant: "Some".into(),
                position: 0,
            }],
            Type::Result { .. } => vec![
                Step::VariantPayload {
                    variant: "Ok".into(),
                    position: 0,
                },
                Step::VariantPayload {
                    variant: "Err".into(),
                    position: 0,
                },
            ],
            Type::Enum(_) | Type::Applied { .. } => {
                let constructor = match self.types.store.get(ty)? {
                    Type::Applied { constructor, .. } => TypeId(constructor.0),
                    _ => ty,
                };
                self.types
                    .facts
                    .enum_layouts
                    .get(&constructor)
                    .map(|layout| {
                        layout
                            .variants
                            .iter()
                            .flat_map(|variant| {
                                (0..variant.fields.len()).map(|i| Step::VariantPayload {
                                    variant: variant.name.clone(),
                                    position: i as u32,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        };
        Some(steps)
    }
}
