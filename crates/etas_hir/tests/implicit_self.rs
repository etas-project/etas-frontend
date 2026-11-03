use etas_core::{SourceFile, SourceId};
use etas_hir::{
    HirImplItem, HirItem, HirSpecItem, HirTreeView, HirType, ResolveResult, SymbolDef,
    lower_program,
};

#[test]
fn implicit_receiver_binds_spec_self_and_concrete_impl_owner_without_a_public_type_argument() {
    let parsed = etas_syntax::parse_program(SourceFile::new(
        SourceId(1),
        None,
        r#"
spec Encoder { flow encode(self) -> i32; }
type Packet = { number: i32 };
impl Packet ~ Encoder { flow encode(self) -> i32 { return self.number; } }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    let view = HirTreeView::try_new(&hir).expect("method body has a real item owner");
    let spec = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Spec(spec) => Some(spec),
            _ => None,
        })
        .unwrap();
    assert!(spec.type_params.is_empty());
    let HirSpecItem::FlowSignature(signature) = &spec.items[0] else {
        panic!("method signature");
    };
    let receiver = signature
        .receiver_type_param
        .expect("implicit method-local Self binder");
    assert_eq!(signature.type_params, vec![receiver]);
    let SymbolDef::TypeParam { bounds, .. } = &hir.symbols.get(receiver).unwrap().def else {
        panic!("receiver type parameter");
    };
    assert!(
        matches!(bounds[0].path.resolution, ResolveResult::Resolved(symbol) if symbol == spec.symbol)
    );
    let parameter = hir.symbols.get(signature.params[0]).unwrap();
    assert!(
        matches!(&hir.types[parameter.declared_type.unwrap()], HirType::Path { path, .. } if path.resolution == ResolveResult::Resolved(receiver))
    );
    let implementation = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Impl(implementation) => Some(implementation),
            _ => None,
        })
        .unwrap();
    let HirImplItem::Callable { item } = implementation.items[0] else {
        panic!("normalized method item");
    };
    assert_eq!(view.item(item).unwrap().bodies().count(), 1);
    let HirItem::Flow(flow) = &hir.items[item] else {
        panic!("executable flow method");
    };
    let parameter = hir.symbols.get(flow.params[0]).unwrap();
    let HirType::Path { path, .. } = &hir.types[parameter.declared_type.unwrap()] else {
        panic!("concrete receiver type");
    };
    let ResolveResult::Resolved(owner) = path.resolution else {
        panic!("resolved nominal owner");
    };
    assert_eq!(hir.symbols.get(owner).unwrap().name, "Packet");
}
