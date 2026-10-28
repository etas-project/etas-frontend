use etas_std::RuntimeTypeRole as Role;
use etas_types::{ItemSignature, Type, check_program};

#[test]
fn standard_value_facts_follow_resolved_aliases_and_checked_type_identity() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(7),
        None,
        r#"
module app.main;
import std.host.sandbox.{SandboxProfile, DefaultCommandSandbox as Profile};
flow profile() -> SandboxProfile { return Profile; }
flow logical() -> std.runtime.trace.TraceLabel { return Logical; }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let output = check_program(&etas_hir::lower_program(&parsed.value));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let known = &output.facts.known_std_types;
    assert_eq!(known.runtime_values.len(), 2);
    let sandbox = known
        .runtime_value(etas_std::StdValueRole::DefaultCommandSandbox)
        .unwrap();
    let trace = known
        .runtime_value(etas_std::StdValueRole::LogicalTraceLabel)
        .unwrap();
    assert_ne!(sandbox, trace);
    for ty in [sandbox, trace] {
        assert!(output.facts.item_signatures.values().any(|signature| matches!(signature, ItemSignature::Flow(signature) if signature.output == ty)));
    }
}

#[test]
fn session_and_limit_abi_facts_are_canonical_and_fully_typed() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(7),
        None,
        r#"
module app.main;
flow config() -> SessionConfig {
    return SessionConfig { id = SessionId("s"), context = LastTurns(8), retention = Days(90) };
}
flow limit() -> Limit { return Tokens(32); }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let output = check_program(&etas_hir::lower_program(&parsed.value));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let known = &output.facts.known_std_types;
    let session_id = known.runtime_type(Role::SessionId).unwrap();
    assert!(!known.opaque_values.contains(&session_id));
    assert!(
        !known
            .opaque_values
            .contains(&known.runtime_type(Role::SessionConfig).unwrap())
    );
    let Some(Type::Nominal(id)) = output.store.get(session_id) else {
        panic!("nominal id");
    };
    assert!(matches!(
        id.representation.and_then(|id| output.store.get(id)),
        Some(Type::Primitive(etas_types::PrimitiveType::String))
    ));
    for (role, names) in [
        (
            Role::ContextPolicy,
            &["All", "LastTurns", "SummaryPlusRecent"][..],
        ),
        (Role::RetentionPolicy, &["Forever", "Days"][..]),
        (
            Role::Limit,
            &[
                "Iterations",
                "Tokens",
                "ContextTokens",
                "Cost",
                "WallTime",
                "Attempts",
            ][..],
        ),
    ] {
        let ty = known.runtime_type(role).unwrap();
        assert!(matches!(output.store.get(ty), Some(Type::Enum(_))));
        let layout = &output.facts.enum_layouts[&ty];
        assert_eq!(
            layout
                .variants
                .iter()
                .map(|variant| variant.name.as_str())
                .collect::<Vec<_>>(),
            names
        );
    }
}

#[test]
fn native_runtime_identity_facts_survive_body_merge_and_applied_aliases() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(7),
        None,
        r#"
import std.collections.{Deque, Queue, Stack, PriorityQueue, OrderedMap, OrderedSet};
alias Work = Queue<i32>;
flow containers(a: Deque<i32>, b: Work, c: Stack<i32>, d: PriorityQueue<string, i32>, e: OrderedMap<i32, string>, f: OrderedSet<i32>) -> unit { return; }
type Command = string;
flow ordinary(value: Command) -> unit { return; }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    let output = check_program(&hir);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let known = &output.facts.known_std_types;
    assert_eq!(known.runtime_types.len(), 19);
    let signature = output
        .facts
        .item_signatures
        .values()
        .find_map(|signature| match signature {
            ItemSignature::Flow(signature) if signature.params.len() == 6 => Some(signature),
            _ => None,
        })
        .unwrap();
    for (ty, role) in signature.params.iter().zip([
        Role::Deque,
        Role::Queue,
        Role::Stack,
        Role::PriorityQueue,
        Role::OrderedMap,
        Role::OrderedSet,
    ]) {
        assert_eq!(known.runtime_role(&output.store, *ty), Some(role));
        let Type::Applied { constructor, args } = output.store.get(*ty).unwrap() else {
            panic!("applied native type")
        };
        assert_eq!(
            known.runtime_type(role),
            Some(etas_types::TypeId(constructor.0))
        );
        assert_eq!(args.len(), role.type_parameter_count());
    }
    let ordinary = output
        .facts
        .item_signatures
        .values()
        .find_map(|signature| match signature {
            ItemSignature::Flow(signature) if signature.params.len() == 1 => {
                Some(signature.params[0])
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(known.runtime_role(&output.store, ordinary), None);
    assert_ne!(known.runtime_type(Role::Command), Some(ordinary));
}
