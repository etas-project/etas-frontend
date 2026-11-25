use etas_code::{PackageKey, descriptor::DescriptorLimits};
use etas_frontend::{CheckedProgram, EntryPolicy, Frontend, ProjectInput, SourceInput};
use etas_hir::HirItem;
use etas_types::ItemSignature;

fn checked(source: &str) -> CheckedProgram {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    input.environment.local_source_package =
        Some(PackageKey::from_name_and_version("app", "1.0.0").unwrap());
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.unwrap()
}

fn schema(source: &str) -> etas_code::CodeFingerprint {
    let program = checked(source);
    let item = program
        .items
        .iter()
        .find_map(|(item, declaration)| match declaration {
            HirItem::Flow(flow) if program.symbols.get(flow.symbol).unwrap().name == "sample" => {
                Some(item)
            }
            _ => None,
        })
        .unwrap();
    let ItemSignature::Flow(signature) = &program.types.item_signatures[&item] else {
        panic!("sample signature")
    };
    let described = program
        .describe_storage_layout(signature.params[0], DescriptorLimits::default())
        .unwrap();
    // A recursive tree is represented once, rather than unrolling its depth.
    assert!(described.layout.nodes.len() < 16);
    described.layout.schema
}

#[test]
fn schema_survives_recompilation_and_tracks_representation_changes() {
    let before = schema(
        r#"
        module app;
        public alias Small = { left: i32, right: bool };
        public type Item<T> = { value: T, ready: bool };
        public enum Tree<T> { Leaf(T), Branch(Tree<T>, Tree<T>), }
        flow sample(value: { item: Item<Small>, tree: Tree<Small> }) -> unit ![] { return; }
    "#,
    );
    let recompiled = schema(
        r#"
        module app;
        public enum Tree<T> { Branch(Tree<T>, Tree<T>), Leaf(T), }
        public type Item<T> = { ready: bool, value: T };
        flow sample(value: { tree: Tree<{ right: bool, left: i32 }>, item: Item<{ right: bool, left: i32 }> }) -> unit ![] {
            let unrelated = "new function body";
            return;
        }
    "#,
    );
    let changed = schema(
        r#"
        module app;
        public type Item<T> = { ready: i64, value: T };
        public enum Tree<T> { Leaf(T), Branch(Tree<T>, Tree<T>), }
        flow sample(value: { item: Item<{ left: i32, right: bool }>, tree: Tree<{ left: i32, right: bool }> }) -> unit ![] { return; }
    "#,
    );
    assert_eq!(before, recompiled);
    assert_ne!(before, changed);
}
