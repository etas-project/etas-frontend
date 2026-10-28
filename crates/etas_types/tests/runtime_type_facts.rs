use etas_std::RuntimeTypeRole as Role;
use etas_types::{ItemSignature, Type, check_program};

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
    assert_eq!(known.runtime_types.len(), 14);
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
