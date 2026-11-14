//! Stable wire ordering for HIR maps and sets. Arena identities remain local to
//! the archived graph; package/definition identities are carried by its owner.

use std::collections::{HashMap, HashSet};

use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq},
};

pub(crate) fn map<S: Serializer, K: Serialize + Ord, V: Serialize>(
    value: &HashMap<K, V>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut entries = value.iter().collect::<Vec<_>>();
    entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
    let mut map = serializer.serialize_map(Some(entries.len()))?;
    for (key, value) in entries {
        map.serialize_entry(key, value)?;
    }
    map.end()
}

pub(crate) fn set<S: Serializer, T: Serialize + Ord>(
    value: &HashSet<T>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut entries = value.iter().collect::<Vec<_>>();
    entries.sort_unstable();
    let mut set = serializer.serialize_seq(Some(entries.len()))?;
    for value in entries {
        set.serialize_element(value)?;
    }
    set.end()
}

pub(crate) fn sources<S: Serializer, V: Serialize>(
    value: &HashMap<etas_core::SourceId, V>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut entries = value.iter().collect::<Vec<_>>();
    entries.sort_unstable_by_key(|(source, _)| source.0);
    let mut map = serializer.serialize_map(Some(entries.len()))?;
    for (source, value) in entries {
        map.serialize_entry(source, value)?;
    }
    map.end()
}
