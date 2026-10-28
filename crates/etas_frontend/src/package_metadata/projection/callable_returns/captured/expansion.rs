use super::*;

enum Part<'a> {
    Type(&'a MetadataType),
    Row(&'a MetadataEffectRow),
    Effect(&'a MetadataEffectRef),
    Arg(&'a MetadataEffectArg),
}

/// Walk the substituted tree before copying arguments into its output slots.
/// Caller arguments are already scoped and must not be substituted again.
pub(super) fn validate(
    root: &MetadataType,
    types: &BTreeMap<String, MetadataType>,
    rows: &BTreeMap<String, MetadataEffectRow>,
) -> Result<(), PackageMetadataError> {
    let mut pending = vec![(Part::Type(root), 0, true)];
    let mut nodes = 0;
    while let Some((part, depth, substitute)) = pending.pop() {
        if let Part::Type(ty) = part
            && substitute
            && ty.kind == MetadataTypeKind::Var
            && let Some(argument) = types.get(&ty.name)
        {
            pending.push((Part::Type(argument), depth, false));
            continue;
        }
        nodes += 1;
        if nodes > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
            return Err(invalid(
                "captured type substitution exceeds the metadata node budget",
            ));
        }
        if depth > etas_package_metadata::MAX_METADATA_GRAPH_DEPTH {
            return Err(invalid(
                "captured type substitution exceeds the metadata depth budget",
            ));
        }
        match part {
            Part::Type(ty) => {
                pending.extend(
                    ty.children
                        .iter()
                        .chain(ty.fields.iter().map(|field| &field.ty))
                        .map(|ty| (Part::Type(ty), depth + 1, substitute)),
                );
                pending.extend(
                    ty.effects
                        .iter()
                        .chain(&ty.produced_effects)
                        .map(|row| (Part::Row(row), depth + 1, substitute)),
                );
            }
            Part::Row(row) => {
                pending.extend(
                    row.effects
                        .iter()
                        .map(|effect| (Part::Effect(effect), depth + 1, substitute)),
                );
                if substitute
                    && let Some(argument) = row.tail.as_ref().and_then(|tail| rows.get(tail))
                {
                    pending.extend(
                        argument
                            .effects
                            .iter()
                            .map(|effect| (Part::Effect(effect), depth + 1, false)),
                    );
                }
            }
            Part::Effect(effect) => pending.extend(
                effect
                    .args
                    .iter()
                    .map(|arg| (Part::Arg(arg), depth + 1, substitute)),
            ),
            Part::Arg(arg) => pending.extend(
                arg.ty
                    .iter()
                    .map(|ty| (Part::Type(ty), depth + 1, substitute)),
            ),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variable(name: &str) -> MetadataType {
        MetadataType {
            kind: MetadataTypeKind::Var,
            name: name.into(),
            ..Default::default()
        }
    }

    fn array(child: MetadataType) -> MetadataType {
        MetadataType {
            kind: MetadataTypeKind::Array,
            children: vec![child],
            ..Default::default()
        }
    }

    #[test]
    fn repeated_arguments_are_charged_before_they_are_copied() {
        let root = MetadataType {
            kind: MetadataTypeKind::Tuple,
            children: vec![variable("T"); etas_package_metadata::MAX_METADATA_GRAPH_NODES / 2],
            ..Default::default()
        };
        let types = [("T".into(), array(array(variable("U"))))].into();
        assert!(
            validate(&root, &types, &BTreeMap::new())
                .unwrap_err()
                .to_string()
                .contains("node budget")
        );
    }

    #[test]
    fn nested_argument_depth_is_checked_in_its_output_position() {
        let mut root = variable("T");
        for _ in 0..etas_package_metadata::MAX_METADATA_GRAPH_DEPTH {
            root = array(root);
        }
        let types = [("T".into(), array(variable("U")))].into();
        assert!(
            validate(&root, &types, &BTreeMap::new())
                .unwrap_err()
                .to_string()
                .contains("depth budget")
        );
    }

    #[test]
    fn same_named_caller_parameters_are_not_rebound() {
        let types = [("T".into(), array(variable("T")))].into();
        validate(&variable("T"), &types, &BTreeMap::new()).unwrap();
    }

    #[test]
    fn repeated_row_arguments_share_the_type_expansion_budget() {
        let callable = MetadataType {
            kind: MetadataTypeKind::Function,
            children: vec![MetadataType {
                name: "unit".into(),
                ..Default::default()
            }],
            effects: Some(MetadataEffectRow {
                tail: Some("E".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let root = MetadataType {
            kind: MetadataTypeKind::Tuple,
            children: vec![callable; 3],
            ..Default::default()
        };
        let rows = [(
            "E".into(),
            MetadataEffectRow {
                effects: vec![
                    MetadataEffectRef {
                        path: vec!["Console".into(), "stdout_write".into()],
                        ..Default::default()
                    };
                    etas_package_metadata::MAX_METADATA_GRAPH_NODES / 2
                ],
                tail: None,
            },
        )]
        .into();
        assert!(
            validate(&root, &BTreeMap::new(), &rows)
                .unwrap_err()
                .to_string()
                .contains("node budget")
        );
    }
}
