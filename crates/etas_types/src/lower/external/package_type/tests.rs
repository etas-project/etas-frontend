use super::*;
use etas_package_metadata::{EffectArg, EffectRef, EffectRow, Type};

#[test]
fn nominal_applied_types_and_effect_row_binders_remain_structured() {
    let nominal = Type {
        kind: TypeKind::Applied,
        path: vec!["dep".into(), "Token".into()],
        children: vec![Type {
            kind: TypeKind::Var,
            name: "T".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let Ty::Applied { path, args } = package_type_input(&nominal).unwrap() else {
        panic!("lost nominal application");
    };
    assert_eq!(path, ["dep", "Token"]);
    assert!(matches!(args.as_slice(), [Ty::Var(name)] if name == "T"));
    let row = EffectRow {
        effects: vec![EffectRef {
            path: vec!["dep".into(), "Read".into()],
            args: vec![EffectArg {
                kind: EffectArgKind::Type,
                ty: Some(nominal),
                ..Default::default()
            }],
        }],
        tail: Some("E".into()),
    };
    let lowered = package_effect_row_input(&row).unwrap();
    assert_eq!(lowered.tail.as_deref(), Some("E"));
    assert_eq!(lowered.effects[0].path, ["dep", "Read"]);
    assert!(
        matches!(&lowered.effects[0].args[0], Arg::Type(Ty::Applied { path, .. }) if path == &["dep", "Token"])
    );
}

#[test]
fn projection_requires_children_and_respects_the_shared_metadata_depth_limit() {
    for kind in [
        TypeKind::Array,
        TypeKind::Function,
        TypeKind::Map,
        TypeKind::Handler,
    ] {
        assert!(
            package_type_input(&Type {
                kind,
                ..Default::default()
            })
            .is_err()
        );
    }
    let mut ty = Type {
        name: "unit".into(),
        ..Default::default()
    };
    for _ in 0..=etas_package_metadata::MAX_METADATA_GRAPH_DEPTH {
        ty = Type {
            kind: TypeKind::Array,
            children: vec![ty],
            ..Default::default()
        };
    }
    assert!(
        package_type_input(&ty)
            .unwrap_err()
            .message
            .contains("resource bounds")
    );
}

#[test]
fn projecting_a_nominal_path_does_not_create_a_missing_checked_type() {
    let ty = Type {
        kind: TypeKind::Named,
        path: vec!["dep".into(), "Missing".into()],
        ..Default::default()
    };
    let projected = package_type_input(&ty).unwrap();
    let output = crate::TypeOutput::default();
    assert!(
        super::super::external_type_id_from_output(&output, &projected)
            .unwrap_err()
            .message
            .contains("missing checked type")
    );
}
