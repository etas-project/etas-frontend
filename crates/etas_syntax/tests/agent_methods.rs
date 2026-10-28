use etas_syntax::{SourceFile, SourceId, ast, parse_program};

#[test]
fn agent_implementations_preserve_owner_specs_generics_and_method_visibility() {
    for target in ["Worker", "Worker ~ Stage<string, string>"] {
        let source = format!(
            "impl agent {target} {{ public flow echo<T, effect E>(value: T) -> T ![E] {{ return value; }} }}"
        );
        let parsed = parse_program(SourceFile::new(SourceId(71), None, source));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let ast::Item::Impl(decl) = &parsed.value.items[0].item else {
            panic!("agent impl");
        };
        let ast::ImplTarget::Agent {
            specs,
            target: owner,
            ..
        } = &decl.target
        else {
            panic!("explicit agent owner");
        };
        assert_eq!(owner.segments[0].text, "Worker");
        assert_eq!(specs.len(), usize::from(target.contains('~')));
        let ast::ImplItem::Flow(method) = &decl.items[0] else {
            panic!("ordinary flow method");
        };
        assert_eq!(method.type_params.len(), 2);
        assert_eq!(method.visibility, ast::Visibility::Public);
    }
}

#[test]
fn agent_implementation_does_not_accept_effect_action_members() {
    let parsed = parse_program(SourceFile::new(
        SourceId(71),
        None,
        "impl agent Worker { action request() -> unit; }",
    ));
    assert!(!parsed.diagnostics.is_empty());
}
