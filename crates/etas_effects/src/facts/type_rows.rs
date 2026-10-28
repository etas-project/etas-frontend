use crate::{Effect, EffectRegistry, EffectRow, effect_var_id_from_name};
use etas_types::{EffectArgRef, EffectRef, EffectRowRef};

/// Reconstruct the type-level row from solved registry identities and an
/// explicitly supplied lexical binder namespace. Display text is not evidence.
pub fn checked_type_effect_row(
    row: &EffectRow,
    registry: &EffectRegistry,
    lexical_parameters: &[String],
) -> Result<EffectRowRef, String> {
    let tail = row
        .open
        .map(|id| {
            let mut names = lexical_parameters
                .iter()
                .filter(|name| effect_var_id_from_name(name) == id);
            match (names.next(), names.next()) {
                (Some(name), None) => Ok(name.clone()),
                _ => Err("solved generic row has no unique checked lexical binder".to_owned()),
            }
        })
        .transpose()?;
    let mut effects = Vec::new();
    for effect in row.effects.iter() {
        let (name, args) = match effect {
            Effect::Tag(tag) => (
                registry
                    .tag_name(*tag)
                    .ok_or("solved row has no tag descriptor")?
                    .to_owned(),
                vec![],
            ),
            Effect::Applied { tag, args } => (
                registry
                    .tag_name(*tag)
                    .ok_or("solved row has no tag descriptor")?
                    .to_owned(),
                args.iter().copied().map(EffectArgRef::Type).collect(),
            ),
            Effect::Error(error) => (
                registry
                    .tag_name(crate::ERROR_TAG)
                    .ok_or("solved row has no Error descriptor")?
                    .to_owned(),
                vec![EffectArgRef::Type(*error)],
            ),
            Effect::Action(action) => (action_name(registry, action)?, vec![]),
            Effect::AppliedAction(action) => {
                (action_name(registry, &action.action)?, action.args.clone())
            }
            Effect::Var(_) => {
                return Err("solved generic row contains a non-tail effect variable".into());
            }
        };
        effects.push(EffectRef { name, args });
    }
    Ok(EffectRowRef { effects, tail })
}

fn action_name(registry: &EffectRegistry, action: &crate::ActionRef) -> Result<String, String> {
    let descriptor = registry
        .action_signature(action)
        .ok_or("solved row has no action descriptor")?;
    let owner = registry
        .tag_name(action.tag)
        .ok_or("solved action has no owner descriptor")?;
    Ok(format!("{owner}.{}", descriptor.name))
}
