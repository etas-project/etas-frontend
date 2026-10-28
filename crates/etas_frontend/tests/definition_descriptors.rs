use std::collections::{BTreeMap, HashMap};

use etas_code::{CodeFingerprint, DefinitionKind, PackageKey, descriptor::*};
use etas_frontend::{
    CheckedProgram, Frontend, ProjectInput, SourceInput,
    code::{
        descriptor::{DefinitionDescriptorProjector, ProjectionError},
        identity::{DefinitionIdentityIndex, DefinitionOrigins},
    },
};
use etas_hir::{HirItem, HirItemId};
use etas_types::ItemSignature;

fn checked(source: &str) -> CheckedProgram {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = etas_frontend::EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.unwrap()
}

fn origins(checked: &CheckedProgram, source: &str) -> DefinitionOrigins {
    let key = |value: &str| {
        PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
            "etas.test.package-origin.v1",
            value.as_bytes(),
        ))
    };
    DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(checked, key(source)).unwrap(),
        standard_package: key("locked:etas.std:phase1"),
        external_definitions: BTreeMap::new(),
    }
}

fn flow(checked: &CheckedProgram, name: &str) -> HirItemId {
    checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow) if checked.hir.symbols.get(flow.symbol).unwrap().name == name => {
                Some(id)
            }
            _ => None,
        })
        .unwrap()
}

fn project_flow(
    projector: &mut DefinitionDescriptorProjector<'_>,
    checked: &CheckedProgram,
    identities: &DefinitionIdentityIndex,
    item: HirItemId,
) -> TypeInfoRef {
    let HirItem::Flow(decl) = &checked.hir.items[item] else {
        panic!("flow")
    };
    let ItemSignature::Flow(signature) = &checked.types.item_signatures[&item] else {
        panic!("signature")
    };
    let names = decl
        .params
        .iter()
        .map(|symbol| Some(checked.hir.symbols.get(*symbol).unwrap().name.clone()))
        .collect::<Vec<_>>();
    projector
        .project_signature(
            GenericOwner::Declaration(identities.symbol(decl.symbol).unwrap().clone()),
            signature,
            &names,
            decl.return_type.map(|ty| checked.types.type_refs[&ty]),
        )
        .unwrap()
        .ty
}

fn signature(graph: &DescriptorGraph, reference: TypeInfoRef) -> &SignatureInfo {
    let TypeInfo::Function(signature) = graph.type_info(reference).unwrap() else {
        panic!("function")
    };
    signature
}

#[test]
fn checked_source_projection_keeps_generic_binder_ownership_and_bounds() {
    let checked = checked(
        r#"
module app.types;
spec First;
spec Second;
flow first<T ~ First>(value: T) -> T { return value; }
flow second<T ~ Second>(value: T) -> T { return value; }
"#,
    );
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "locked:app@1")).unwrap();
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    let first = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "first"),
    );
    let second = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "second"),
    );
    let graph = projector.finish().unwrap();
    let first = signature(&graph, first);
    let second = signature(&graph, second);
    assert_ne!(first.generics, second.generics);
    assert_ne!(first.output, second.output);
    for signature in [first, second] {
        let TypeInfo::Parameter(binder) = graph.type_info(signature.output).unwrap() else {
            panic!("type parameter")
        };
        assert_eq!(signature.generics, [*binder]);
        assert_eq!(signature.parameters[0].ty, signature.output);
        assert_eq!(signature.parameters[0].name.as_deref(), Some("value"));
        assert_eq!(graph.binder(*binder).unwrap().bounds.len(), 1);
    }
    assert!(graph.data().types.len() < checked.type_store.len());
}

#[test]
fn recursive_adt_descriptors_project_finite_shared_graphs() {
    let checked = checked(
        r#"
module app.types;
enum Tree<T> { Leaf(T), Branch { left: Tree<T>, right: Tree<T> }, }
type Directory = { entries: Array<Entry> };
enum Entry { Folder(Directory), File(string), }
flow tree(value: Tree<i32>) -> Tree<i32> { return value; }
flow directory(value: Directory) -> Directory { return value; }
"#,
    );
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "locked:app@1")).unwrap();
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    let tree = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "tree"),
    );
    let directory = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "directory"),
    );
    let graph = projector.finish().unwrap();
    assert!(graph.data().types.len() < 32);
    assert!(graph.type_fingerprint(tree).is_ok());
    assert!(graph.type_fingerprint(directory).is_ok());
    let tree_enum = graph
        .data()
        .types
        .iter()
        .find_map(|ty| match ty {
            TypeInfo::Enum {
                definition,
                variants,
                ..
            } if definition.declaration() == ["Tree"] => Some(variants),
            _ => None,
        })
        .unwrap();
    assert!(
        matches!(&tree_enum[1].payload, VariantPayloadInfo::Named(fields) if fields.len() == 2 && fields[0].name == "left")
    );
}

#[test]
fn open_effect_rows_and_selectors_preserve_checked_binder_identity() {
    let checked = checked(
        r#"
module app.types;
flow apply<T, effect E>(value: T, callback: (T) -> T ![E]) -> T ![Error<T>, E] {
    return callback(value);
}
"#,
    );
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "locked:app@1")).unwrap();
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    let root = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "apply"),
    );
    let graph = projector.finish().unwrap();
    let signature = signature(&graph, root);
    let row = graph.effect_row(signature.effects.unwrap()).unwrap();
    let tail = row.tail.unwrap();
    assert!(signature.generics.contains(&tail));
    assert_eq!(
        graph.binder(tail).unwrap().kind,
        GenericParameterKind::EffectRow
    );
    assert_eq!(row.effects.len(), 1);
    assert_eq!(
        row.effects[0].arguments,
        [StaticEffectArgInfo::Type(signature.output)]
    );
    let TypeInfo::Function(callback) = graph.type_info(signature.parameters[1].ty).unwrap() else {
        panic!("callback")
    };
    assert_eq!(
        graph.effect_row(callback.effects.unwrap()).unwrap().tail,
        Some(tail)
    );
}

#[test]
fn package_identity_is_explicit_and_independent_of_hir_arena_positions() {
    let build = |source: &str, origin: &str| {
        let checked = checked(source);
        let identities =
            DefinitionIdentityIndex::build(&checked, &origins(&checked, origin)).unwrap();
        let mut projector =
            DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
        let root = project_flow(
            &mut projector,
            &checked,
            &identities,
            flow(&checked, "identity"),
        );
        projector.finish().unwrap().type_fingerprint(root).unwrap()
    };
    let a = build(
        "module app; flow identity<T>(value: T) -> T { return value; }",
        "origin:a",
    );
    let b = build(
        "module app; type Unrelated; flow identity<T>(value: T) -> T { return value; }",
        "origin:a",
    );
    let other_package = build(
        "module app; flow identity<T>(value: T) -> T { return value; }",
        "origin:b",
    );
    assert_eq!(a, b);
    assert_ne!(a, other_package);
}

#[test]
fn missing_ownership_and_small_projection_budgets_fail_closed() {
    let checked = checked("module app; flow identity<T>(value: T) -> T { return value; }");
    let mut owner = origins(&checked, "origin:a");
    owner.source_modules = HashMap::new();
    assert!(matches!(
        DefinitionIdentityIndex::build(&checked, &owner),
        Err(ProjectionError::MissingPackage(_))
    ));
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "origin:a")).unwrap();
    let mut projector = DefinitionDescriptorProjector::new(
        &checked,
        &identities,
        DescriptorLimits {
            max_nodes: 1,
            ..DescriptorLimits::default()
        },
    );
    let item = flow(&checked, "identity");
    let HirItem::Flow(decl) = &checked.hir.items[item] else {
        panic!("flow")
    };
    let ItemSignature::Flow(signature) = &checked.types.item_signatures[&item] else {
        panic!("signature")
    };
    assert!(matches!(
        projector.project_signature(
            GenericOwner::Declaration(identities.symbol(decl.symbol).unwrap().clone()),
            signature,
            &[Some("value".into())],
            Some(signature.output)
        ),
        Err(ProjectionError::Limit { .. })
    ));
}

#[test]
fn nominal_agent_and_wide_integer_types_remain_distinct_descriptors() {
    let checked = checked(
        r#"
module app;
agent Worker;
flow value(worker: Worker, signed: i128, unsigned: u128) -> Worker { return worker; }
"#,
    );
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "origin:a")).unwrap();
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    let root = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "value"),
    );
    let graph = projector.finish().unwrap();
    let signature = signature(&graph, root);
    assert!(
        matches!(graph.type_info(signature.output), Some(TypeInfo::Nominal { definition, representation: None, .. }) if definition.kind() == DefinitionKind::Agent)
    );
    assert_eq!(
        graph.type_info(signature.parameters[1].ty),
        Some(&TypeInfo::Primitive(PrimitiveInfo::I128))
    );
    assert_eq!(
        graph.type_info(signature.parameters[2].ty),
        Some(&TypeInfo::Primitive(PrimitiveInfo::U128))
    );
}

#[test]
fn std_nominal_fields_and_enum_payloads_come_from_the_canonical_registry() {
    let checked = checked(
        r#"
module app;
import std.http.codec.HttpWireResponseHead;
import std.http.codec.HttpDecodeStep;
flow head(value: HttpWireResponseHead) -> HttpWireResponseHead { return value; }
flow step(value: HttpDecodeStep<HttpWireResponseHead>) -> HttpDecodeStep<HttpWireResponseHead> { return value; }
"#,
    );
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "origin:a")).unwrap();
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    let head = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "head"),
    );
    let step = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "step"),
    );
    let graph = projector.finish().unwrap();
    let TypeInfo::Nominal {
        representation: Some(record),
        definition,
        ..
    } = graph.type_info(signature(&graph, head).output).unwrap()
    else {
        panic!("canonical std nominal record");
    };
    assert_eq!(definition.module(), ["std", "http", "codec"]);
    let TypeInfo::Record(fields) = graph.type_info(*record).unwrap() else {
        panic!("record shape")
    };
    let symbol = checked
        .std_registry
        .lookup_qualified(&["std", "http", "codec", "HttpWireResponseHead"])
        .unwrap();
    let etas_std::StdDecl::Type(decl) = &symbol.decl else {
        panic!("std type")
    };
    let Some(etas_std::StdType::Record(declared)) = &decl.representation else {
        panic!("registry record")
    };
    assert_eq!(
        fields.iter().map(|field| &field.name).collect::<Vec<_>>(),
        declared.iter().map(|field| &field.name).collect::<Vec<_>>()
    );
    let TypeInfo::Applied { constructor, .. } =
        graph.type_info(signature(&graph, step).output).unwrap()
    else {
        panic!("generic std enum")
    };
    let TypeInfo::Enum { variants, .. } = graph.type_info(*constructor).unwrap() else {
        panic!("enum payloads")
    };
    assert_eq!(
        variants
            .iter()
            .map(|variant| variant.name.as_str())
            .collect::<Vec<_>>(),
        ["NeedMore", "Complete", "Malformed"]
    );
    assert!(graph.type_fingerprint(step).is_ok());
}

#[test]
fn alias_declaration_remains_descriptive_without_retagging_its_target() {
    let checked = checked(
        "module app; alias Values<T> = Array<T>; flow use_values(value: Values<i32>) -> Values<i32> { return value; }",
    );
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "origin:a")).unwrap();
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    let symbol = checked
        .hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::TypeAlias(decl) => Some(decl.symbol),
            _ => None,
        })
        .unwrap();
    let alias = projector.project_type_declaration(symbol).unwrap();
    let use_values = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "use_values"),
    );
    let graph = projector.finish().unwrap();
    assert!(
        matches!(graph.type_info(alias), Some(TypeInfo::Alias { definition, parameters, .. }) if definition.declaration() == ["Values"] && parameters.len() == 1)
    );
    assert!(matches!(
        graph.type_info(signature(&graph, use_values).output),
        Some(TypeInfo::Builtin {
            constructor: BuiltinTypeInfo::Array,
            ..
        })
    ));
}

#[test]
fn descriptor_bounds_follow_resolved_reexports_wildcards_and_std_prelude() {
    for import in ["import app.bridge.API as Selected;", "import app.bridge.*;"] {
        let bound = if import.contains("Selected") {
            "Selected"
        } else {
            "API"
        };
        let consumer = format!(
            "module app.consumer; {import} flow use_bound<T ~ {bound}, I ~ Index>(value: T, index: I) -> T {{ return value; }}"
        );
        let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
        input.options.entry_policy = etas_frontend::EntryPolicy::Optional;
        input.sources = [
            "module app.bounds; public spec Allowed;",
            "module app.bridge; public import app.bounds.Allowed as API;",
            &consumer,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, text)| SourceInput {
            id: etas_core::SourceId(index as u32 + 1),
            path: None,
            text: text.into(),
            kind: etas_frontend::SourceKind::SourceProjectFile,
        })
        .collect();
        let output = Frontend.check_project(input);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let checked = output.checked.unwrap();
        let identities =
            DefinitionIdentityIndex::build(&checked, &origins(&checked, "origin:a")).unwrap();
        let mut projector =
            DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
        let root = project_flow(
            &mut projector,
            &checked,
            &identities,
            flow(&checked, "use_bound"),
        );
        let graph = projector.finish().unwrap();
        let signature = signature(&graph, root);
        let allowed = &graph.binder(signature.generics[0]).unwrap().bounds[0].definition;
        assert_eq!(allowed.module(), ["app", "bounds"]);
        assert_eq!(allowed.declaration(), ["Allowed"]);
        let index = &graph.binder(signature.generics[1]).unwrap().bounds[0].definition;
        assert_eq!(index.module(), ["std", "core"]);
        assert_eq!(index.declaration(), ["Index"]);
    }
}

#[test]
fn missing_checked_bound_facts_are_not_projected_as_an_unconstrained_binder() {
    let mut checked = checked("module app; spec Allowed; type Container<T ~ Allowed>;");
    let owner = origins(&checked, "origin:a");
    let data = std::sync::Arc::make_mut(&mut checked);
    std::sync::Arc::make_mut(&mut data.types)
        .type_param_bounds
        .clear();
    assert!(matches!(
        DefinitionIdentityIndex::build(&checked, &owner),
        Err(ProjectionError::Invalid(_))
    ));
}

#[test]
fn unresolved_std_named_types_cannot_become_opaque_successful_descriptors() {
    let mut checked = checked("module app; flow value() -> unit { return; }");
    let data = std::sync::Arc::make_mut(&mut checked);
    let unknown = std::sync::Arc::make_mut(&mut data.type_store).intern(etas_types::Type::Named(
        etas_types::NamedTypeRef {
            source_parameter: None,
            name: "std.forged.Unknown".into(),
        },
    ));
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "origin:a")).unwrap();
    let item = flow(&checked, "value");
    let HirItem::Flow(decl) = &checked.hir.items[item] else {
        panic!("flow")
    };
    let ItemSignature::Flow(signature) = &checked.types.item_signatures[&item] else {
        panic!("signature")
    };
    let mut malformed = signature.clone();
    malformed.output = unknown;
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    projector
        .project_signature(
            GenericOwner::Declaration(identities.symbol(decl.symbol).unwrap().clone()),
            &malformed,
            &[],
            None,
        )
        .unwrap();
    assert!(matches!(
        projector.finish(),
        Err(ProjectionError::MissingIdentity(_))
    ));
}

#[test]
fn private_import_aliases_in_separate_module_parts_keep_distinct_bindings() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
    input.options.entry_policy = etas_frontend::EntryPolicy::Optional;
    input.sources = [
        "module app.first; public spec Allowed;",
        "module app.second; public spec Allowed;",
        "module app.shared; import app.first.Allowed as Choice; flow first<T ~ Choice>(value: T) -> T { return value; }",
        "module app.shared; import app.second.Allowed as Choice; flow second<T ~ Choice>(value: T) -> T { return value; }",
    ].into_iter().enumerate().map(|(index, text)| SourceInput {
        id: etas_core::SourceId(index as u32 + 1), path: None, text: text.into(), kind: etas_frontend::SourceKind::SourceProjectFile,
    }).collect();
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "origin:a")).unwrap();
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    let first = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "first"),
    );
    let second = project_flow(
        &mut projector,
        &checked,
        &identities,
        flow(&checked, "second"),
    );
    let graph = projector.finish().unwrap();
    let first = &graph
        .binder(signature(&graph, first).generics[0])
        .unwrap()
        .bounds[0]
        .definition;
    let second = &graph
        .binder(signature(&graph, second).generics[0])
        .unwrap()
        .bounds[0]
        .definition;
    assert_eq!(first.module(), ["app", "first"]);
    assert_eq!(second.module(), ["app", "second"]);
    assert_ne!(first, second);
}
