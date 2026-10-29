use std::collections::BTreeMap;

use etas_code::{
    CodeFingerprint, PackageKey,
    descriptor::{DescriptorLimits, PrimitiveInfo, TypeInfo},
};
use etas_frontend::{
    CheckedProgram, EntryPolicy, Frontend, ProjectInput, SourceInput, SourceKind,
    code::{
        definition::{DefinitionCatalog, StaticReflectionView},
        descriptor::ProjectionError,
        identity::{DefinitionOrigins, SourceModuleOrigin},
    },
};
use etas_hir::{HirExprId, HirReflectTarget};

fn check(sources: &[&str]) -> CheckedProgram {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
    input.options.entry_policy = EntryPolicy::Optional;
    input.sources = sources
        .iter()
        .enumerate()
        .map(|(index, source)| SourceInput {
            id: etas_core::SourceId(index as u32 + 1),
            path: None,
            text: (*source).into(),
            kind: SourceKind::SourceProjectFile,
        })
        .collect();
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.unwrap()
}

fn origins(checked: &CheckedProgram) -> DefinitionOrigins {
    let key = |value: &str| {
        PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
            "etas.test.catalog-origin.v1",
            value.as_bytes(),
        ))
    };
    DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(checked, key("locked:app@1"))
            .unwrap(),
        standard_package: key("locked:std@1"),
        external_definitions: BTreeMap::new(),
    }
}

fn catalog(checked: &CheckedProgram) -> DefinitionCatalog {
    DefinitionCatalog::build(
        checked.clone(),
        &origins(checked),
        DescriptorLimits::default(),
    )
    .unwrap()
}

#[test]
fn inference_descriptors_retain_the_selected_default_or_auxiliary_callable() {
    use etas_code::descriptor::StaticEffectArgInfo;
    let checked = check(&[r#"
module app;
agent Worker() -> string { return perform infer<string>(Prompt.new()); }
impl agent Worker { flow explain() -> string { return perform infer<string>(Prompt.new()); } }
flow inspect() -> unit { let first = ^Worker.run; let second = ^Worker.explain; return; }
"#]);
    let catalog = catalog(&checked);
    let views = reflections(&checked);
    let mut selected = Vec::new();
    for (expr, _) in views {
        let StaticReflectionView::Callable(callable) = catalog.static_reflection(expr).unwrap()
        else {
            panic!("callable");
        };
        let row = catalog
            .descriptors()
            .effect_row(callable.analysis.effects.requested.unwrap())
            .unwrap();
        assert_eq!(row.effects.len(), 1);
        let [
            StaticEffectArgInfo::Callable(id),
            StaticEffectArgInfo::Type(_),
        ] = row.effects[0].arguments.as_slice()
        else {
            panic!("canonical method/output selectors: {row:?}");
        };
        assert_eq!(id, &callable.id);
        selected.push(id.clone());
    }
    assert_eq!(selected.len(), 2);
    assert_ne!(selected[0], selected[1]);
}

fn reflections(checked: &CheckedProgram) -> Vec<(HirExprId, String)> {
    checked
        .hir
        .exprs
        .iter()
        .filter_map(|(id, expr)| match expr {
            etas_hir::HirExpr::Reflect(reflection) => {
                let path = reflection
                    .target
                    .root_path()
                    .segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .collect::<Vec<_>>()
                    .join(".");
                let path = match &reflection.target {
                    HirReflectTarget::Path(_) => path,
                    HirReflectTarget::SpecMethod { spec, method, .. } => format!(
                        "{path}::{}.{}",
                        spec.segments
                            .iter()
                            .map(|segment| segment.name.as_str())
                            .collect::<Vec<_>>()
                            .join("."),
                        method
                    ),
                };
                Some((id, path))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn primary_agent_and_run_share_one_catalog_callable_and_body_owner() {
    for declaration in [
        "agent Worker(value: i32) -> i32 { return value + 1; }",
        "agent Worker; impl agent Worker { public flow run(value: i32) -> i32 { return value + 1; } }",
    ] {
        let source = format!(
            "module app; {declaration} flow inspect() -> unit {{ let a = ^Worker; let b = ^Worker.run; return; }}"
        );
        let checked = check(&[&source]);
        assert!(
            checked.entry.is_none(),
            "catalog construction must not require a synthetic main"
        );
        let catalog = catalog(&checked);
        let views = reflections(&checked);
        let agent_expr = views.iter().find(|(_, path)| path == "Worker").unwrap().0;
        let run_expr = views
            .iter()
            .find(|(_, path)| path == "Worker.run")
            .unwrap()
            .0;
        let StaticReflectionView::Agent(agent) = catalog.static_reflection(agent_expr).unwrap()
        else {
            panic!("agent")
        };
        let StaticReflectionView::Callable(run) = catalog.static_reflection(run_expr).unwrap()
        else {
            panic!("run")
        };
        assert_eq!(agent.default_entry.as_ref(), Some(&run.id));
        assert!(agent.methods.is_empty());
        assert_eq!(catalog.callable(&run.id).unwrap().signature, run.signature);
        let owner = catalog.item_owner(&run.id).unwrap();
        assert_eq!(
            checked
                .types
                .agent_methods
                .values()
                .filter(|method| method.body == owner)
                .count(),
            1
        );
    }
}

#[test]
fn bare_agent_keeps_absent_default_and_accessible_additional_methods() {
    let checked = check(&[r#"
module app;
agent Worker;
impl agent Worker { flow explain(text: string) -> string { return text; } }
flow inspect() -> unit { let a = ^Worker; let b = ^Worker.explain; return; }
"#]);
    let catalog = catalog(&checked);
    let views = reflections(&checked);
    let agent_expr = views.iter().find(|(_, path)| path == "Worker").unwrap().0;
    let StaticReflectionView::Agent(agent) = catalog.static_reflection(agent_expr).unwrap() else {
        panic!("agent")
    };
    assert!(agent.default_entry.is_none());
    assert_eq!(agent.methods.len(), 1);
    let method = catalog.callable(&agent.methods[0]).unwrap();
    let TypeInfo::Function(signature) = catalog.descriptors().type_info(method.signature).unwrap()
    else {
        panic!("actual method signature")
    };
    assert_eq!(
        catalog.descriptors().type_info(signature.output),
        Some(&TypeInfo::Primitive(PrimitiveInfo::String))
    );
}

#[test]
fn agent_method_views_respect_the_checked_reflecting_scope() {
    let checked = check(&[
        r#"module app.lib;
public agent Worker;
impl agent Worker {
    public flow run() -> i32 { return 42; }
    private flow secret() -> i32 { return 1; }
    public flow explain() -> i32 { return 2; }
}
flow inspect() -> unit { let a = ^Worker; return; }
"#,
        r#"module app.main; import app.lib.Worker;
flow main() -> unit { let a = ^Worker; return; }
"#,
    ]);
    let catalog = catalog(&checked);
    let mut counts = Vec::new();
    for (expression, _) in reflections(&checked) {
        let StaticReflectionView::Agent(agent) = catalog.static_reflection(expression).unwrap()
        else {
            panic!("agent")
        };
        let names = agent
            .methods
            .iter()
            .map(|id| id.as_method().unwrap().name())
            .collect::<Vec<_>>();
        assert!(names.contains(&"explain"));
        assert!(!names.contains(&"run"));
        counts.push(names.len());
    }
    counts.sort();
    assert_eq!(counts, [1, 2]);
}

#[test]
fn spec_selected_same_named_methods_keep_implementation_and_spec_identity() {
    let checked = check(&[r#"
module app;
spec Stage<I, O>: callable I => O ![];
agent Worker;
impl agent Worker ~ Stage<i32, i32> { flow run(value: i32) -> i32 { return value; } }
impl agent Worker ~ Stage<string, string> { flow run(value: string) -> string { return value; } }
flow inspect() -> unit {
    let a = ^Worker::Stage<i32, i32>.run;
    let b = ^Worker::Stage<string, string>.run;
    let whole = ^Worker;
    return;
}
"#]);
    let catalog = catalog(&checked);
    let mut selected = Vec::new();
    for (expr, _) in reflections(&checked) {
        match catalog.static_reflection(expr).unwrap() {
            StaticReflectionView::Callable(callable) => selected.push(callable.id.clone()),
            StaticReflectionView::Agent(agent) => {
                assert!(agent.default_entry.is_none());
                assert_eq!(agent.methods.len(), 2);
            }
        }
    }
    assert_eq!(selected.len(), 2);
    assert_ne!(selected[0], selected[1]);
    assert_ne!(
        catalog.item_owner(&selected[0]),
        catalog.item_owner(&selected[1])
    );
    for selected in selected {
        let method = selected.as_method().unwrap();
        assert_eq!(method.name(), "run");
        assert_eq!(
            method
                .implementation()
                .selected_spec()
                .unwrap()
                .declaration(),
            ["Stage"]
        );
    }
}

#[test]
fn callable_identity_is_stable_across_body_edits_and_arena_reordering() {
    let build = |prefix: &str, body: &str| {
        let source = format!(
            "module app; {prefix} agent Worker; impl agent Worker {{ public flow run(value: i32) -> i32 {{ {body} }} }} flow inspect() -> unit {{ let a = ^Worker.run; return; }}"
        );
        let checked = check(&[&source]);
        let catalog = catalog(&checked);
        let expression = reflections(&checked)[0].0;
        let StaticReflectionView::Callable(view) = catalog.static_reflection(expression).unwrap()
        else {
            panic!("callable")
        };
        view.id.clone()
    };
    let original = build("", "return value;");
    let edited = build("type Unrelated;", "return value + 1;");
    assert_eq!(original, edited);
    assert_eq!(original.fingerprint(), edited.fingerprint());
}

#[test]
fn dynamic_agent_reflection_is_not_substituted_with_the_source_baseline() {
    let checked = check(&[r#"
module app;
agent Worker() -> i32 { return 42; }
flow inspect() -> unit { let current = Worker; let definition = ^current; return; }
"#]);
    let catalog = catalog(&checked);
    let expression = reflections(&checked)[0].0;
    assert!(matches!(
        catalog.static_reflection(expression),
        Err(ProjectionError::RuntimeRevisionRequired)
    ));
}

#[test]
fn runtime_reflection_rejects_an_agent_item_with_a_different_nominal_identity() {
    let checked = check(&[r#"
module app;
agent Worker() -> i32 { return 1; }
agent Other() -> i32 { return 1; }
flow inspect() -> unit { let current = Worker; let definition = ^current; return; }
"#]);
    let catalog = catalog(&checked);
    let expression = reflections(&checked)[0].0;
    let other = checked
        .hir
        .items
        .iter()
        .find_map(|(item, value)| match value {
            etas_hir::HirItem::Agent(agent)
                if checked.symbols.get(agent.symbol).unwrap().name == "Other" =>
            {
                Some(item)
            }
            _ => None,
        })
        .unwrap();
    assert!(matches!(
        catalog.agent_value_reflection(expression, other),
        Err(ProjectionError::Invalid(_))
    ));
}

#[test]
fn catalog_projects_inferred_callable_outputs_instead_of_declaration_placeholders() {
    let checked = check(&[r#"
module app;
agent Worker;
impl agent Worker { public flow run(value: i32) { return value + 1; } }
flow normalize(value: string) { return value; }
flow inspect() -> unit { let a = ^Worker.run; let b = ^normalize; return; }
"#]);
    let catalog = catalog(&checked);
    for (expression, path) in reflections(&checked) {
        let StaticReflectionView::Callable(callable) =
            catalog.static_reflection(expression).unwrap()
        else {
            panic!("callable")
        };
        let TypeInfo::Function(signature) =
            catalog.descriptors().type_info(callable.signature).unwrap()
        else {
            panic!("signature")
        };
        let primitive = if path == "normalize" {
            PrimitiveInfo::String
        } else {
            PrimitiveInfo::I32
        };
        assert_eq!(
            catalog.descriptors().type_info(signature.output),
            Some(&TypeInfo::Primitive(primitive))
        );
        assert!(signature.declared_output.is_none());
    }
}

#[test]
fn frozen_revision_uses_the_catalog_default_method_identity() {
    use etas_code::definition::{
        BaseRevisionInput, DefinitionSnapshot, FrozenDefinition, FrozenDefinitionInput,
    };
    let checked = check(&[
        "module app; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let a = ^Worker; let b = ^Worker.run; return; }",
    ]);
    let catalog = catalog(&checked);
    let views = reflections(&checked);
    let expr = views.iter().find(|(_, name)| name == "Worker").unwrap().0;
    let StaticReflectionView::Agent(agent) = catalog.static_reflection(expr).unwrap() else {
        panic!("agent")
    };
    let callable = agent.default_entry.unwrap();
    let entry = catalog.callable(&callable).unwrap();
    let fingerprint = CodeFingerprint::of_bytes("etas.test.catalog-snapshot.v1", b"checked");
    let frozen = FrozenDefinition::new(FrozenDefinitionInput {
        identity: agent.identity,
        descriptors: catalog.descriptors().clone(),
        default_entry: Some(entry.signature),
        default_callable: Some(callable.clone()),
        environment: fingerprint,
        contract_fingerprint: fingerprint,
        contract: std::sync::Arc::new(()),
        compilation: fingerprint,
    })
    .unwrap();
    assert_eq!(frozen.default_callable(), Some(&callable));
    let definition = DefinitionSnapshot::<_, ()>::from_base(
        frozen,
        BaseRevisionInput {
            body: None,
            content: fingerprint,
            parent: None,
            analysis_configuration: fingerprint,
            summaries: None,
        },
    )
    .unwrap();
    assert_eq!(definition.revision().inputs().definition, callable);
}

#[test]
fn resolved_header_aliases_do_not_change_method_identity() {
    let build = |header: &str, selected: &str| {
        let source = format!(
            "module app; {header} agent Worker; impl agent Worker ~ {selected} {{ flow run(value: i32) -> i32 {{ return value; }} }} flow inspect() -> unit {{ let a = ^Worker::{selected}.run; return; }}"
        );
        let checked = check(&[
            "module api; public spec Stage<I, O>: callable I => O ![];",
            &source,
        ]);
        let catalog = catalog(&checked);
        let StaticReflectionView::Callable(callable) = catalog
            .static_reflection(reflections(&checked)[0].0)
            .unwrap()
        else {
            panic!("callable")
        };
        callable.id.clone()
    };
    let direct = build("import api.Stage;", "Stage<i32, i32>");
    let aliased = build(
        "import api.Stage as S; alias Number = i32;",
        "S<Number, Number>",
    );
    assert_eq!(direct, aliased);
}

#[test]
fn inconsistent_checked_default_method_facts_are_rejected() {
    let mut checked = check(&["module app; agent Worker() -> i32 { return 1; }"]);
    let facts = std::sync::Arc::make_mut(&mut std::sync::Arc::make_mut(&mut checked).types);
    facts.agent_methods.values_mut().next().unwrap().component = etas_types::TypeId(u32::MAX);
    let origins = DefinitionOrigins {
        source_modules: checked
            .hir
            .modules
            .iter()
            .map(|module| {
                (
                    *module,
                    SourceModuleOrigin::declared(
                        &checked,
                        *module,
                        PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
                            "etas.test.origin.v1",
                            b"app",
                        )),
                    )
                    .unwrap(),
                )
            })
            .collect(),
        standard_package: PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
            "etas.test.origin.v1",
            b"std",
        )),
        external_definitions: BTreeMap::new(),
    };
    assert!(matches!(
        DefinitionCatalog::build(checked, &origins, DescriptorLimits::default()),
        Err(ProjectionError::Invalid(_))
    ));
}

#[test]
fn shared_catalog_references_account_for_one_checked_graph() {
    let checked = check(&[
        "module app; agent Worker() -> i32 { return 42; } flow inspect() -> unit { let a = ^Worker; let b = ^Worker.run; return; }",
    ]);
    let catalog = std::sync::Arc::new(catalog(&checked));
    let bytes = |count| {
        let references = vec![catalog.clone(); count];
        let bytes = etas_core::memory::MemoryFootprint::of(&references).estimated_bytes();
        bytes
            - (references.capacity() * std::mem::size_of::<std::sync::Arc<DefinitionCatalog>>())
                as u64
    };
    assert_eq!(bytes(1), bytes(10));
    assert_eq!(bytes(1), bytes(100));
}

#[test]
fn callable_analysis_separates_known_empty_effects_from_requested_actions() {
    use etas_code::descriptor::{DeterminismInfo, EffectTargetInfo, TraceInfo};
    let checked = check(&[r#"
module app;
import std.io.println;
flow writes() -> unit { println("handled")?; return; }
flow inspect() -> unit { let d = ^writes; return; }
"#]);
    let catalog = catalog(&checked);
    let StaticReflectionView::Callable(callable) = catalog
        .static_reflection(reflections(&checked)[0].0)
        .unwrap()
    else {
        panic!("callable")
    };
    let analysis = &callable.analysis;
    let graph = catalog.descriptors();
    assert!(analysis.effects.declared_bound.is_none());
    let escaping = graph
        .effect_row(analysis.effects.escaping.unwrap())
        .unwrap();
    assert!(escaping.effects.is_empty());
    assert!(escaping.tail.is_none());
    let requested = graph
        .effect_row(analysis.effects.requested.unwrap())
        .unwrap();
    let default_handled = graph
        .effect_row(analysis.effects.default_handled.unwrap())
        .unwrap();
    assert_eq!(requested, default_handled);
    assert!(matches!(requested.effects.as_slice(), [effect]
        if matches!(&effect.target, EffectTargetInfo::Action { effect, name }
            if effect.declaration() == ["Console"] && name == "stdout_write")));
    assert!(analysis.determinism.is_some());
    assert_ne!(
        analysis.determinism,
        Some(DeterminismInfo::NonDeterministic)
    );
    assert!(matches!(
        graph.trace(analysis.trace.unwrap()),
        Some(TraceInfo::Event(_))
    ));
}

#[test]
fn missing_analysis_stays_unavailable_instead_of_becoming_empty() {
    let mut checked = check(&[
        "module app; flow idle() -> unit { return; } flow inspect() -> unit { let d = ^idle; return; }",
    ]);
    std::sync::Arc::make_mut(&mut std::sync::Arc::make_mut(&mut checked).effects)
        .item_effects
        .clear();
    let catalog = catalog(&checked);
    let StaticReflectionView::Callable(callable) = catalog
        .static_reflection(reflections(&checked)[0].0)
        .unwrap()
    else {
        panic!("callable")
    };
    assert!(callable.analysis.effects.escaping.is_none());
    assert!(callable.analysis.effects.requested.is_none());
    assert!(callable.analysis.effects.default_handled.is_none());
    assert!(callable.analysis.determinism.is_none());
    assert!(callable.analysis.trace.is_none());
}

#[test]
fn descriptor_keeps_open_row_binders_and_each_parameter_call_position() {
    use etas_code::descriptor::{GenericParameterKind, TraceInfo};
    let checked = check(&[r#"
module app;
flow twice<effect E>(f: () -> unit ![E]) -> unit ![E] { f(); f(); return; }
flow inspect() -> unit { let d = ^twice; return; }
"#]);
    let catalog = catalog(&checked);
    let StaticReflectionView::Callable(callable) = catalog
        .static_reflection(reflections(&checked)[0].0)
        .unwrap()
    else {
        panic!("callable")
    };
    let graph = catalog.descriptors();
    for row in [
        callable.analysis.effects.declared_bound,
        callable.analysis.effects.escaping,
    ] {
        let binder = graph
            .binder(graph.effect_row(row.unwrap()).unwrap().tail.unwrap())
            .unwrap();
        assert_eq!(binder.name, "E");
        assert_eq!(binder.kind, GenericParameterKind::EffectRow);
    }
    let TraceInfo::Seq(parts) = graph.trace(callable.analysis.trace.unwrap()).unwrap() else {
        panic!("both parameter calls must remain in sequence")
    };
    assert_eq!(parts.len(), 2);
    for part in parts {
        let TraceInfo::ParameterCall(call) = graph.trace(*part).unwrap() else {
            panic!("checked parameter call")
        };
        assert_eq!(call.callable, callable.id);
        assert_eq!(call.position, 0);
    }
}

#[test]
fn descriptor_rejects_a_foreign_same_name_callback_parameter_identity() {
    let mut checked = check(&[r#"
module app;
flow invoke(f: () -> unit ![]) -> unit ![] { f(); return; }
flow unrelated(f: () -> unit ![]) -> unit ![] { f(); return; }
flow inspect() -> unit { let d = ^invoke; return; }
"#]);
    let flow = |name: &str| {
        checked
            .hir
            .items
            .iter()
            .find_map(|(item, data)| {
                let etas_hir::HirItem::Flow(flow) = data else {
                    return None;
                };
                (checked.hir.symbols.get(flow.symbol).unwrap().name == name)
                    .then_some((item, flow.params[0]))
            })
            .unwrap()
    };
    let (invoke, _) = flow("invoke");
    let (_, foreign_parameter) = flow("unrelated");
    let origin = origins(&checked);
    let effects = std::sync::Arc::make_mut(&mut std::sync::Arc::make_mut(&mut checked).effects);
    let summary = effects.item_effects.get_mut(&invoke).unwrap();
    let etas_effects::ActionTraceDomain::ParameterCall { target, .. } = &mut summary.action_trace
    else {
        panic!("symbolic callback call expected");
    };
    *target = etas_effects::ParameterCallTarget::Native(foreign_parameter);
    let result = DefinitionCatalog::build(checked, &origin, DescriptorLimits::default());
    assert!(
        matches!(result, Err(ProjectionError::Invalid(reason)) if reason.contains("signature parameter"))
    );
}

#[test]
fn trace_descriptor_fingerprint_is_span_independent_but_order_sensitive() {
    let build = |prefix: &str, body: &str| {
        let checked = check(&[&format!(
            "module app; import std.io.println; import std.io.eprintln; {prefix} flow writes() -> unit {{ {body} return; }} flow inspect() -> unit {{ let d = ^writes; return; }}"
        )]);
        let catalog = catalog(&checked);
        let StaticReflectionView::Callable(callable) = catalog
            .static_reflection(reflections(&checked)[0].0)
            .unwrap()
        else {
            panic!("callable")
        };
        catalog
            .descriptors()
            .trace_fingerprint(callable.analysis.trace.unwrap())
            .unwrap()
    };
    let original = build("", "println(\"a\")?; eprintln(\"b\")?;");
    assert_eq!(
        original,
        build("type Unrelated;\n\n", "println(\"a\")?; eprintln(\"b\")?;")
    );
    assert_ne!(original, build("", "eprintln(\"b\")?; println(\"a\")?;"));
}

#[test]
fn action_summary_selectors_refer_to_the_checked_callable_type_binder() {
    use etas_code::descriptor::{GenericOwner, StaticEffectArgInfo};
    let checked = check(&[r#"
module app;
effect Probe { action read<T>(value: T) -> unit; }
flow read<T>(value: T) -> unit ![Probe.read<T>] { perform Probe.read<T>(value); return; }
flow inspect() -> unit { let d = ^read; return; }
"#]);
    let catalog = catalog(&checked);
    let StaticReflectionView::Callable(callable) = catalog
        .static_reflection(reflections(&checked)[0].0)
        .unwrap()
    else {
        panic!("callable")
    };
    let graph = catalog.descriptors();
    let requested = graph
        .effect_row(callable.analysis.effects.requested.unwrap())
        .unwrap();
    assert_eq!(requested.effects.len(), 1);
    let [StaticEffectArgInfo::Type(ty)] = requested.effects[0].arguments.as_slice() else {
        panic!("typed selector")
    };
    let Some(TypeInfo::Parameter(parameter)) = graph.type_info(*ty) else {
        panic!("checked generic parameter")
    };
    let binder = graph.binder(*parameter).unwrap();
    assert_eq!(binder.name, "T");
    assert_eq!(
        binder.owner,
        GenericOwner::Declaration(callable.id.as_declaration().unwrap().clone())
    );
}

#[test]
fn unknown_summary_row_variable_is_not_projected_as_a_closed_empty_row() {
    let mut checked = check(&[
        "module app; flow idle() -> unit { return; } flow inspect() -> unit { let d = ^idle; return; }",
    ]);
    let facts = std::sync::Arc::make_mut(&mut std::sync::Arc::make_mut(&mut checked).effects);
    for summary in facts.item_effects.values_mut() {
        summary.requested_actions.open = Some(etas_effects::effect_var_id_from_name("Undeclared"));
    }
    assert!(matches!(
        DefinitionCatalog::build(
            checked.clone(),
            &origins(&checked),
            DescriptorLimits::default()
        ),
        Err(ProjectionError::Invalid(_))
    ));
}
