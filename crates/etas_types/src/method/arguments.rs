pub(crate) fn slots(
    parameters: &[Option<String>],
    arguments: &[Option<String>],
) -> Result<Vec<u32>, String> {
    if parameters.len() != arguments.len() {
        return Err(format!(
            "method expects {} explicit argument(s), got {}",
            parameters.len().saturating_sub(1),
            arguments.len().saturating_sub(1)
        ));
    }
    let mut used = vec![false; parameters.len()];
    let mut next = 0;
    let mut slots = Vec::with_capacity(arguments.len());
    for name in arguments {
        let slot = if let Some(name) = name {
            parameters
                .iter()
                .position(|parameter| parameter.as_ref() == Some(name))
                .ok_or_else(|| format!("unknown method argument `{name}`"))?
        } else {
            while next < used.len() && used[next] {
                next += 1;
            }
            next
        };
        if slot >= used.len() || used[slot] {
            return Err("method argument is supplied more than once".into());
        }
        used[slot] = true;
        slots.push(slot as u32);
    }
    Ok(slots)
}
