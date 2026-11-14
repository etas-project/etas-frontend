//! Stable wire ordering for HIR maps and sets. Arena identities remain local to
//! the archived graph; package/definition identities are carried by its owner.

use std::collections::HashMap;

use serde::{Serialize, Serializer, ser::SerializeMap};

pub(crate) use etas_utils::serde_ordered::{map, set};

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
