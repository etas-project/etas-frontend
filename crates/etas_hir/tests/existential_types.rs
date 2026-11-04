use etas_core::{SourceFile, SourceId};
use etas_hir::{HirItem, HirType, ResolveResult, lower_program};

#[test]
fn existential_bounds_keep_resolved_spec_and_type_argument_identities() {
    let parsed = etas_syntax::parse_program(SourceFile::new(
        SourceId(1),
        None,
        r#"module main;
spec Value { flow read(self) -> i32; }
spec Within<R>;
type Root;
flow select(value: ? ~ Value + Within<Root>) -> ? ~ Value { return value; }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    let flow = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Flow(flow) => Some(flow),
            _ => None,
        })
        .unwrap();
    let ty = hir
        .symbols
        .get(flow.params[0])
        .unwrap()
        .declared_type
        .unwrap();
    let HirType::Existential { bounds, .. } = &hir.types[ty] else {
        panic!("existential HIR type");
    };
    assert_eq!(bounds.len(), 2);
    for (bound, name) in bounds.iter().zip(["Value", "Within"]) {
        let ResolveResult::Resolved(symbol) = bound.path.resolution else {
            panic!("resolved bound");
        };
        assert_eq!(hir.symbols.get(symbol).unwrap().name, name);
    }
    let HirType::Path { path, .. } = &hir.types[bounds[1].args[0]] else {
        panic!("bound type argument");
    };
    let ResolveResult::Resolved(root) = path.resolution else {
        panic!("resolved type argument");
    };
    assert_eq!(hir.symbols.get(root).unwrap().name, "Root");
}
