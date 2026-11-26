//! Preserve trace DAG edges within one independently encoded artifact.
use super::SharedActionTrace;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{cell::RefCell, collections::HashMap, sync::Arc};

#[derive(Default)]
struct Nodes {
    encoded: HashMap<usize, (u32, Arc<SharedActionTrace>)>,
    decoded: HashMap<u32, Arc<SharedActionTrace>>,
}

thread_local! {
    static NODES: RefCell<Option<Nodes>> = const { RefCell::new(None) };
}

/// Start a fresh reference namespace for a single serialized payload. Nested
/// payloads (for example separately selectable library bodies) stay independent.
pub struct TraceCodecScope(Option<Nodes>);

impl TraceCodecScope {
    pub fn enter() -> Self {
        Self(NODES.replace(Some(Nodes::default())))
    }
}

impl Drop for TraceCodecScope {
    fn drop(&mut self) {
        NODES.set(self.0.take());
    }
}

fn root_scope() -> Option<TraceCodecScope> {
    NODES
        .with_borrow(|nodes| nodes.is_none())
        .then(TraceCodecScope::enter)
}

#[derive(Serialize)]
enum EncodedNode<'a> {
    Reference(u32),
    Definition(u32, &'a SharedActionTrace),
}

#[derive(Deserialize)]
enum DecodedNode {
    Reference(u32),
    Definition(u32, SharedActionTrace),
}

pub fn serialize<S: Serializer>(
    node: &Arc<SharedActionTrace>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let _root = root_scope();
    let (id, seen) = NODES.with_borrow_mut(|nodes| {
        let nodes = nodes.as_mut().unwrap();
        let pointer = Arc::as_ptr(node) as usize;
        if let Some((id, _)) = nodes.encoded.get(&pointer) {
            return (*id, true);
        }
        let id = u32::try_from(nodes.encoded.len()).expect("trace node count fits u32");
        nodes.encoded.insert(pointer, (id, node.clone()));
        (id, false)
    });
    if seen {
        EncodedNode::Reference(id).serialize(serializer)
    } else {
        EncodedNode::Definition(id, node).serialize(serializer)
    }
}

pub fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Arc<SharedActionTrace>, D::Error> {
    let _root = root_scope();
    match DecodedNode::deserialize(deserializer)? {
        DecodedNode::Reference(id) => NODES
            .with_borrow(|nodes| nodes.as_ref().unwrap().decoded.get(&id).cloned())
            .ok_or_else(|| serde::de::Error::custom("unknown shared trace reference")),
        DecodedNode::Definition(id, node) => {
            let node = Arc::new(node);
            let previous = NODES
                .with_borrow_mut(|nodes| nodes.as_mut().unwrap().decoded.insert(id, node.clone()));
            if previous.is_some() {
                return Err(serde::de::Error::custom(
                    "duplicate shared trace definition",
                ));
            }
            Ok(node)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infer::domain::ActionTraceDomain;

    #[test]
    fn trace_roundtrip_preserves_shared_subgraphs() {
        let mut trace = ActionTraceDomain::TaskAwait {
            task_site: etas_hir::HirExprId(1),
        };
        for _ in 0..24 {
            trace = ActionTraceDomain::Parallel(vec![trace.clone(), trace]).shared();
        }
        let bytes = {
            let _scope = TraceCodecScope::enter();
            bincode::serde::encode_to_vec((&trace, &trace), bincode::config::standard()).unwrap()
        };
        assert!(bytes.len() < 16_384);
        let (decoded, consumed): ((ActionTraceDomain, ActionTraceDomain), _) = {
            let _scope = TraceCodecScope::enter();
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap()
        };
        assert_eq!(consumed, bytes.len());
        assert!(decoded.0 == trace);
        let (ActionTraceDomain::Shared(left), ActionTraceDomain::Shared(right)) = decoded else {
            panic!("shared roots");
        };
        assert!(Arc::ptr_eq(&left, &right));
        let mut cursor = &left.body;
        while let ActionTraceDomain::Parallel(children) = cursor {
            if let (ActionTraceDomain::Shared(left), ActionTraceDomain::Shared(right)) =
                (&children[0], &children[1])
            {
                assert!(Arc::ptr_eq(left, right));
                cursor = &left.body;
            } else {
                break;
            }
        }
    }
}
