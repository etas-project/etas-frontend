use std::collections::BTreeMap;

use etas_code::{
    CallableId,
    descriptor::{AgentHeaderInfo, AgentMethodInfo},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalMethodSelection {
    Selected(CallableId),
    Private,
    Ambiguous,
}

/// Applies the same checked method selection to reflection and metadata paths.
pub fn select_external_methods(
    header: &AgentHeaderInfo,
) -> Result<Option<BTreeMap<String, ExternalMethodSelection>>, &'static str> {
    let Some(methods) = &header.method_info else {
        return Ok(None);
    };
    let mut candidates = BTreeMap::<String, (bool, Option<&AgentMethodInfo>)>::new();
    for method in methods {
        let id = method
            .callable
            .as_method()
            .ok_or("agent method has no checked method identity")?;
        if id.implementation().subject() != &header.identity {
            return Err("agent method identity has the wrong owner");
        }
        match candidates.entry(id.name().to_owned()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert((method.inherent, Some(method)));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let (inherent, chosen) = entry.get_mut();
                if method.inherent && !*inherent {
                    *inherent = true;
                    *chosen = Some(method);
                } else if method.inherent == *inherent {
                    *chosen = None;
                }
            }
        }
    }
    let mut selected = candidates
        .into_iter()
        .map(|(name, (_, method))| {
            let selection = match method {
                Some(method) if method.is_public => {
                    ExternalMethodSelection::Selected(method.callable.clone())
                }
                Some(_) => ExternalMethodSelection::Private,
                None => ExternalMethodSelection::Ambiguous,
            };
            (name, selection)
        })
        .collect::<BTreeMap<_, _>>();
    selected.remove("run");
    if let Some(default) = &header.default_entry {
        let method = methods
            .iter()
            .find(|method| &method.callable == default)
            .ok_or("agent default method has no checked selection facts")?;
        let selection = if method.is_public {
            ExternalMethodSelection::Selected(default.clone())
        } else {
            ExternalMethodSelection::Private
        };
        selected.insert("run".into(), selection);
    }
    Ok(Some(selected))
}
