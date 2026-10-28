use std::collections::HashMap;

use crate::artifact::fingerprint_text;
use etas_cache::ArtifactFingerprint;
use etas_types::TypeFacts;
use serde::Serialize;

pub(super) fn fingerprint(facts: &TypeFacts) -> Result<ArtifactFingerprint, String> {
    let mut parts = Vec::new();
    append(&mut parts, "known-std-types", &facts.known_std_types)?;
    append(&mut parts, "enum-layouts", sorted(&facts.enum_layouts))?;
    append(
        &mut parts,
        "action-signatures",
        sorted(&facts.action_signatures),
    )?;
    append(
        &mut parts,
        "qualified-actions",
        sorted(&facts.qualified_action_signatures),
    )?;
    append(&mut parts, "type-generics", sorted(&facts.type_generics))?;
    append(&mut parts, "signatures", sorted(&facts.spec_signatures))?;
    append(
        &mut parts,
        "parameter-bounds",
        sorted(&facts.type_param_bounds),
    )?;
    append(&mut parts, "std-aliases", sorted(&facts.std_spec_aliases))?;
    append(&mut parts, "std-impls", &facts.std_spec_impls)?;
    append(
        &mut parts,
        "impls",
        facts
            .spec_impls
            .iter()
            .map(|fact| {
                (
                    fact.self_type,
                    fact.spec_symbol,
                    &fact.args,
                    &fact.methods,
                    fact.impl_group,
                )
            })
            .collect::<Vec<_>>(),
    )?;
    append(
        &mut parts,
        "type-satisfactions",
        facts
            .type_spec_satisfactions
            .iter()
            .map(|fact| {
                (
                    fact.self_type,
                    fact.spec_symbol,
                    &fact.args,
                    fact.impl_group,
                )
            })
            .collect::<Vec<_>>(),
    )?;
    append(
        &mut parts,
        "callable-satisfactions",
        facts
            .callable_spec_satisfactions
            .iter()
            .map(|fact| {
                (
                    fact.item,
                    fact.symbol,
                    fact.spec_symbol,
                    &fact.args,
                    &fact.effect_bound,
                )
            })
            .collect::<Vec<_>>(),
    )?;
    append(
        &mut parts,
        "trace-conformances",
        facts
            .trace_spec_conformances
            .iter()
            .map(|fact| (fact.item, fact.symbol, &fact.target))
            .collect::<Vec<_>>(),
    )?;
    append(
        &mut parts,
        "external-aliases",
        sorted(&facts.external_specs.aliases),
    )?;
    let mut signatures = facts.external_specs.signatures.iter().collect::<Vec<_>>();
    signatures.sort_by(|(left, _), (right, _)| {
        (left.package, &left.path).cmp(&(right.package, &right.path))
    });
    append(&mut parts, "external-signatures", signatures)?;
    append(&mut parts, "external-impls", &facts.external_specs.impls)?;
    append(
        &mut parts,
        "external-callables",
        facts
            .external_callable_spec_satisfactions
            .iter()
            .map(|fact| (&fact.item, &fact.spec, &fact.args))
            .collect::<Vec<_>>(),
    )?;
    append(
        &mut parts,
        "external-traces",
        facts
            .external_trace_spec_conformances
            .iter()
            .map(|fact| (&fact.item, &fact.target))
            .collect::<Vec<_>>(),
    )?;
    Ok(fingerprint_text(
        &parts.iter().map(String::as_str).collect::<Vec<_>>(),
    ))
}

fn sorted<K: Ord, V>(map: &HashMap<K, V>) -> Vec<(&K, &V)> {
    let mut bindings = map.iter().collect::<Vec<_>>();
    bindings.sort_by(|(left, _), (right, _)| left.cmp(right));
    bindings
}

fn append(parts: &mut Vec<String>, name: &str, value: impl Serialize) -> Result<(), String> {
    parts.push(format!(
        "{name}:{}",
        serde_json::to_string(&value)
            .map_err(|error| format!("invalid checked spec environment: {error}"))?
    ));
    Ok(())
}
