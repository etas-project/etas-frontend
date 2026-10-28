use etas_core::{DiagnosticCode, SourceFile, SourceId, TypeDiagnosticCode};
use etas_hir::{HirExpr, HirItem, ResolveResult};
use etas_types::{CheckedReflectionTarget, ItemSignature, Type, check_program};

fn check(text: &str) -> (etas_hir::HirProgram, etas_types::TypeOutput) {
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(81), None, text));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    let output = check_program(&hir);
    (hir, output)
}

#[test]
fn definition_descriptor_fields_are_read_only_and_not_executable_values() {
    for statement in [
        "var definition = ^Worker; definition.signature = None;",
        "var definition = ^Worker; definition.methods = [];",
        "var definition = ^Worker; definition.tools = [];",
        "var definition = ^Worker; definition.specs = [];",
        "var definition = ^Worker; definition.annotations = [];",
        "var annotation = match (^Worker).annotations.get(0) { Some(value) => value, None => { return; }, }; annotation.path = [];",
        "var signature = (^Worker.run).signature; signature.parameters = [];",
        "var signature = (^Worker.run).signature; signature.output = signature.output;",
    ] {
        let (_, output) = check(&format!(
            "agent Worker() -> i32 {{ return 1; }} flow main() -> unit {{ {statement} return; }}"
        ));
        assert!(
            output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("read-only")),
            "{statement}: {:?}",
            output.diagnostics
        );
    }
    let (_, output) = check(
        "agent Worker() -> i32 { return 1; } impl agent Worker { flow helper() -> i32 { return 2; } } flow main() -> unit { let method = match (^Worker).methods.get(0) { Some(method) => method, None => { return; }, }; method(); return; }",
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
            && diagnostic
                .message
                .contains("only be created by its registered backend operations")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn reflected_type_data_is_not_a_runtime_value_of_the_described_type() {
    let (_, output) = check(
        "agent Worker() -> i32 { return 1; } flow main() -> i32 { return (^Worker.run).signature.output; }",
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code
                == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn reflected_annotation_values_are_descriptions_not_the_described_runtime_values() {
    let (_, output) = check(
        "@custom(42) agent Worker; flow main() -> i32 { let annotation = match (^Worker).annotations.get(0) { Some(value) => value, None => { return 0; }, }; let argument = match annotation.arguments.get(0) { Some(value) => value, None => { return 0; }, }; return argument.value; }",
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code
                == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn reflected_spec_descriptions_do_not_prove_spec_satisfaction() {
    let (_, output) = check(
        r#"
spec Marker;
agent Worker;
impl agent Worker ~ Marker {}
flow use_bound<T ~ Marker>(value: T) -> unit { return; }
flow main() -> unit {
    let description = match (^Worker).specs.get(0) { Some(value) => value, None => { return; }, };
    use_bound(description);
    return;
}
"#,
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Marker")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn compiled_agent_inferred_binding_resolves_its_checked_method() {
    let (_, output) = check(
        r#"
import std.compiler.compile;
import std.compiler.CompileError;
agent Worker(value: i32) -> i32 { return value; }
flow main() -> i32 ![Error<CompileError>] {
    let current = compile(^Worker);
    return current.run(7);
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.agent_method_calls.len(), 1);
}

#[test]
fn compiled_agent_inferred_binding_selects_named_methods_by_component_identity() {
    let (_, output) = check(
        r#"
import std.compiler.compile;
import std.compiler.CompileError;
agent Worker;
impl agent Worker {
    flow run() -> i32 { return 1; }
    flow describe(value: string) -> string { return value; }
}
agent Other;
impl agent Other {
    flow describe(value: i32) -> bool { return true; }
}
flow main() -> string ![Error<CompileError>] {
    let current = compile(^Worker);
    return current.describe("worker");
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let selected = *output.facts.agent_method_calls.values().next().unwrap();
    let method = &output.facts.agent_methods[&selected];
    assert_eq!(method.name, "describe");
    assert!(
        matches!(output.store.get(method.component), Some(Type::Nominal(nominal)) if nominal.name == "Worker")
    );
}

#[test]
fn inferred_agent_method_call_preserves_argument_validation() {
    let (_, output) = check(
        r#"
import std.compiler.compile;
import std.compiler.CompileError;
agent Worker(value: i32) -> i32 { return value; }
flow main() -> i32 ![Error<CompileError>] {
    let current = compile(^Worker);
    return current.run("not an integer");
}
"#,
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
            && diagnostic.message.contains("call argument type")),
        "{:?}",
        output.diagnostics
    );
    assert!(output.facts.agent_method_calls.is_empty());
}

#[test]
fn inferred_agent_method_preserves_type_and_effect_generic_arguments() {
    for (row, valid) in [("![]", true), ("i32", false)] {
        let (_, output) = check(&format!(
            r#"
import std.compiler.compile;
import std.compiler.CompileError;
agent Worker;
impl agent Worker {{
    flow run() -> i32 {{ return 1; }}
    flow apply<T, effect E>(value: T, callback: T -> T ![E]) -> T ![E] {{ return callback(value); }}
}}
flow echo(value: string) -> string {{ return value; }}
flow main() -> string ![Error<CompileError>] {{
    let current = compile(^Worker);
    return current.apply<string, {row}>("value", echo);
}}
"#
        ));
        if valid {
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            let (expr, _) = output.facts.agent_method_calls.iter().next().unwrap();
            let generic = &output.facts.generic_instantiations[expr];
            assert_eq!(generic.type_bindings.len(), 1);
            assert_eq!(
                generic.effect_row_bindings,
                vec![(
                    "E".into(),
                    etas_types::EffectRowRef {
                        effects: Vec::new(),
                        tail: None
                    }
                )]
            );
        } else {
            assert!(
                output.diagnostics.iter().any(|diagnostic| diagnostic
                    .message
                    .contains("requires an effect-row argument")),
                "{:?}",
                output.diagnostics
            );
        }
    }
}

#[test]
fn qualified_std_calls_do_not_discard_effect_generic_arguments() {
    let (_, output) = check("flow main() -> string { return std.text.trim<![]>(\"value\"); }");
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code
                == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn bare_agent_reflection_preserves_nominal_identity_without_inventing_an_entry() {
    let (hir, output) = check(
        "import std.compiler.AgentDef; agent Worker; flow inspect() -> AgentDef<Worker> { return ^Worker; }",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let (item, agent) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Agent(agent) => Some((id, agent)),
            _ => None,
        })
        .unwrap();
    let ItemSignature::Agent(signature) = &output.facts.item_signatures[&item] else {
        panic!("agent")
    };
    assert!(signature.default_entry.is_none());
    assert!(
        matches!(output.store.get(signature.component), Some(Type::Nominal(nominal)) if nominal.name == "Worker")
    );
    let (&expr, fact) = output.facts.reflections.iter().next().unwrap();
    assert_eq!(
        fact.target,
        CheckedReflectionTarget::Agent {
            declaration: agent.symbol,
            component: signature.component
        }
    );
    assert_eq!(output.facts.expr_types[&expr], fact.descriptor_type);
    assert!(
        matches!(output.store.get(fact.descriptor_type), Some(Type::Applied { args, .. }) if args == &[signature.component])
    );
}

#[test]
fn flow_reflection_is_a_callable_descriptor_not_an_agent_or_function_value() {
    let (_, output) = check(
        "import std.compiler.CallableDef; flow normalize(x: i32) -> i32 { return x; } flow inspect() -> CallableDef { return ^normalize; }",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let fact = output.facts.reflections.values().next().unwrap();
    assert!(matches!(
        fact.target,
        CheckedReflectionTarget::Callable { .. }
    ));
    assert_eq!(
        Some(fact.descriptor_type),
        output.facts.known_std_types.code.callable_definition
    );
    assert!(
        matches!(output.store.get(fact.descriptor_type), Some(Type::Nominal(nominal)) if nominal.name == "std.compiler.CallableDef")
    );
}

#[test]
fn agent_default_entry_and_component_reflection_share_the_same_body_owner() {
    let (hir, output) = check(
        "agent Worker() -> Prompt { return Prompt.new(); } flow inspect() -> unit { let definition = ^Worker; let method = ^Worker.run; return; }",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let mut facts = output.facts.reflections.values();
    let agent = facts
        .find_map(|fact| match fact.target {
            CheckedReflectionTarget::Agent {
                declaration,
                component,
            } => Some((declaration, component)),
            _ => None,
        })
        .unwrap();
    let method = output
        .facts
        .reflections
        .values()
        .find_map(|fact| match fact.target {
            CheckedReflectionTarget::AgentDefaultEntry {
                declaration,
                component,
            } => Some((declaration, component)),
            _ => None,
        })
        .unwrap();
    assert_eq!(agent, method);
    assert_eq!(
        hir.items
            .iter()
            .filter(|(_, item)| matches!(item, HirItem::Agent(_)))
            .count(),
        1
    );
    assert!(
        output
            .facts
            .agent_signature(agent.1)
            .unwrap()
            .default_entry
            .is_some()
    );
}

#[test]
fn reflection_of_agent_values_keeps_the_binding_and_nominal_component() {
    let (hir, output) = check(
        "import std.compiler.AgentDef; agent Worker; flow inspect(input: Worker) -> AgentDef<Worker> { let current = input; return ^current; }",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let fact = output.facts.reflections.values().next().unwrap();
    let CheckedReflectionTarget::AgentValue {
        binding,
        component,
        method,
    } = fact.target
    else {
        panic!("agent value")
    };
    assert_eq!(hir.symbols.get(binding).unwrap().name, "current");
    assert!(method.is_none());
    assert!(
        output
            .facts
            .agent_signature(component)
            .unwrap()
            .default_entry
            .is_none()
    );
    let expr = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            HirExpr::Reflect(_) => Some(id),
            _ => None,
        })
        .unwrap();
    assert_eq!(output.facts.expr_types[&expr], fact.descriptor_type);
}

#[test]
fn rejects_wrong_agent_identity_nonagent_values_and_bare_entry_selection() {
    for text in [
        "import std.compiler.AgentDef; agent A; agent B; flow bad() -> AgentDef<B> { return ^A; }",
        "flow bad(n: i32) -> unit { let d = ^n; return; }",
        "agent A; flow bad() -> unit { let d = ^A.run; return; }",
        "flow f() -> unit { return; } flow bad() -> unit { let x = f; let d = ^x; return; }",
        "import std.compiler.AgentDef; agent A; flow bad() -> AgentDef<A> { return AgentDef<A> {}; }",
        "import std.compiler.AgentDef; agent A; flow bad() -> AgentDef<A> { return AgentDef<A>(); }",
        "import std.compiler.CallableDef; flow bad() -> CallableDef { return CallableDef(); }",
        "import std.compiler.CallableDef; flow f() -> unit { return; } flow bad() -> unit { let d = ^f; d(); }",
    ] {
        let (_, output) = check(text);
        assert!(!output.diagnostics.is_empty(), "accepted {text}");
    }
    let (_, output) = check("flow bad(n: i32) -> unit { let d = ^n; return; }");
    assert!(output.facts.reflections.is_empty());
    assert!(output.diagnostics.iter().any(|diagnostic| diagnostic.code
        == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
        && diagnostic.primary.span.source == SourceId(81)));
}

#[test]
fn agents_are_nominal_values_and_dynamic_calls_query_the_checked_default_entry() {
    let (hir, output) = check(
        "agent A() -> Prompt { return Prompt.new(); } flow invoke(value: A) -> Prompt { return value(); } flow alias() -> A { return A; }",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let source_expr = hir.exprs.iter().find_map(|(id, expr)| match expr { HirExpr::Path(path) if matches!(path.resolution, ResolveResult::Resolved(symbol) if hir.symbols.get(symbol).unwrap().name == "A") => Some(id), _ => None }).unwrap();
    assert!(
        matches!(output.store.get(output.facts.expr_types[&source_expr]), Some(Type::Nominal(nominal)) if nominal.name == "A")
    );
    let (_, rejected) = check(
        "agent A() -> Prompt { return Prompt.new(); } flow wrong() -> () -> Prompt { return A; }",
    );
    assert!(
        !rejected.diagnostics.is_empty(),
        "agent identity must not be erased to a flow type"
    );
}

#[test]
fn immutable_definition_edit_has_a_materialized_registry_call_abi() {
    let (_, output) = check(
        "import std.compiler.SyntaxBlock; agent Worker; flow edit(body: SyntaxBlock) -> unit { let d = ^Worker; let changed = d.with_body(body); let text = changed.body; return; }",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let fact = output.facts.std_method_calls.values().next().unwrap();
    assert_eq!(fact.qualified_path, ["std", "compiler", "with_body"]);
    assert_eq!(fact.intrinsic, etas_std::intrinsic::runtime::CODE_WITH_BODY);
    let Some(Type::Function(signature)) = output.store.get(fact.signature) else {
        panic!("method ABI")
    };
    let descriptor = output
        .facts
        .reflections
        .values()
        .next()
        .unwrap()
        .descriptor_type;
    assert_eq!(
        signature.input,
        [
            descriptor,
            output.facts.known_std_types.code.syntax_block.unwrap()
        ]
    );
    assert_eq!(signature.output, descriptor);
}

#[test]
fn definition_body_is_read_only_and_editing_rejects_string_and_wrong_nominal_identity() {
    for text in [
        "agent Worker; flow bad() -> unit { var d = ^Worker; d.body = None; return; }",
        "agent Worker; flow bad() -> unit { var holder = { d = ^Worker }; holder.d.body = None; return; }",
        "agent Worker; flow bad() -> unit { let d = ^Worker; let changed = d.with_body(\"return;\"); return; }",
        "import std.compiler.AgentDef; import std.compiler.SyntaxBlock; agent A; agent B; flow bad(body: SyntaxBlock) -> AgentDef<B> { return (^A).with_body(body); }",
    ] {
        let (_, output) = check(text);
        assert!(!output.diagnostics.is_empty(), "accepted {text}");
    }
}
