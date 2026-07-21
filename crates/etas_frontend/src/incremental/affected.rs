use etas_utils::UnitKey;

use crate::ProjectOutput;

pub(crate) fn affected_modules(output: &ProjectOutput) -> Vec<UnitKey> {
    output
        .affected_modules
        .as_ref()
        .map(|affected| affected.modules.clone())
        .unwrap_or_default()
}

pub(crate) fn affected_bodies(output: &ProjectOutput) -> Vec<UnitKey> {
    output
        .affected_modules
        .as_ref()
        .map(|affected| affected.bodies.clone())
        .unwrap_or_default()
}

pub(crate) fn affected_items(output: &ProjectOutput) -> Vec<UnitKey> {
    output
        .affected_modules
        .as_ref()
        .map(|affected| affected.items.clone())
        .unwrap_or_default()
}
