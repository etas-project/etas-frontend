use etas_frontend::{Frontend, ProjectInput, SourceInput};

const SOURCE: &str = r#"
spec Control<Live> {
    flow root(self) -> string ![];
    flow start(self, root: string) -> string ![];
    flow attach(self) -> Live ![];
}
type Handle = { id: string };
type Host = {};
impl Host ~ Control<Handle> {
    flow root(self) -> string ![] { return "."; }
    flow start(self, root: string) -> string ![] { return root; }
    flow attach(self) -> Handle ![] { return Handle { id = "process" }; }
}
flow check<Live, C ~ Control<Live>>(control: C) -> string ![] {
    return control.start(control.root());
}
flow live<Live, C ~ Control<Live>>(control: C) -> Live ![] {
    return control.attach();
}
flow main(args: Array<string>) -> i32 ![] {
    let value = check(Host {});
    let process = live(Host {});
    let id = process.id;
    return 0;
}
"#;

#[test]
fn native_witness_infers_spec_arguments_from_the_concrete_control() {
    let output =
        Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(SOURCE)));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let live_arguments = checked
        .types
        .generic_instantiations
        .values()
        .flat_map(|fact| &fact.type_bindings)
        .filter(|(name, _)| name == "Live")
        .map(|(_, ty)| checked.type_store.get(*ty))
        .collect::<Vec<_>>();
    assert_eq!(live_arguments.len(), 2);
    assert!(
        live_arguments.iter().all(|ty| matches!(ty,
            Some(etas_types::Type::Nominal(name)) if name.name.rsplit('.').next() == Some("Handle")
        )),
        "{live_arguments:?}"
    );
}
