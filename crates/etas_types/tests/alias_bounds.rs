use etas_core::{SourceFile, SourceId};

fn check(source: &str) -> etas_types::TypeOutput {
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(1), None, source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    etas_types::check_program(&hir)
}

#[test]
fn alias_declaration_bounds_are_not_replaced_by_the_target_nominals_bounds() {
    for expression in [
        "Restricted<i32>({ value = 42 })",
        "Restricted<i32> { value = 42 }",
    ] {
        let source = format!(
            r#"
spec Allowed;
type Wrapper<T> = {{ value: T }};
alias Restricted<T ~ Allowed> = Wrapper<T>;
flow main() -> Wrapper<i32> {{ return {expression}; }}
"#
        );
        let output = check(&source);
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.message.contains("does not satisfy spec bound")),
            "unproved alias bound was accepted: {source}"
        );
    }
}

#[test]
fn an_unrelated_same_named_parameter_cannot_provide_spec_evidence() {
    let output = check(
        r#"
spec Allowed;
flow needs<T ~ Allowed>(value: T) -> T { return value; }
flow missing<T>(value: T) -> T { return needs(value); }
flow main() -> unit { return; }
"#,
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|d| d.message.contains("does not satisfy spec bound")),
        "another declaration's T supplied the missing evidence"
    );
}

#[test]
fn bounded_aliases_accept_explicit_evidence_and_preserve_their_transparent_target() {
    let output = check(
        r#"
spec Allowed;
type Value = i32;
impl Value ~ Allowed;
type Wrapper<T> = { value: T };
alias Restricted<T ~ Allowed> = Wrapper<T>;
flow main() -> Wrapper<Value> { return Restricted<Value> { value = Value(42) }; }
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn alias_bounds_apply_to_headers_and_annotations_without_a_constructor_call() {
    for use_site in [
        "flow unused(value: Restricted<i32>) -> unit { return; }",
        "flow unused<T>(value: Restricted<T>) -> unit { return; }",
        "flow unused(value: Wrapper<i32>) -> unit { let bad: Restricted<i32> = value; return; }",
        "alias Unbounded<T> = Restricted<T>;",
    ] {
        let source = format!(
            r#"
spec Allowed;
type Wrapper<T> = {{ value: T }};
alias Restricted<T ~ Allowed> = Wrapper<T>;
{use_site}
flow main() -> unit {{ return; }}
"#
        );
        let output = check(&source);
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.message.contains("does not satisfy spec bound")),
            "alias type formation ignored its bound: {source}"
        );
    }
}

#[test]
fn declaration_scoped_alias_bounds_accept_the_matching_generic_evidence() {
    let output = check(
        r#"
spec Allowed;
type Wrapper<T> = { value: T };
alias Restricted<T ~ Allowed> = Wrapper<T>;
alias Forward<T ~ Allowed> = Restricted<T>;
flow reuse<T ~ Allowed>(value: Restricted<T>) -> Forward<T> { return value; }
flow main() -> unit { return; }
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn another_calls_generic_instantiation_cannot_rebind_a_rigid_alias_argument() {
    let output = check(
        r#"
spec Allowed;
type Value = i32;
impl Value ~ Allowed;
type Wrapper<T> = { value: T };
alias Restricted<T ~ Allowed> = Wrapper<T>;
flow identity<T>(value: T) -> T { return value; }
flow invalid<T>(value: Wrapper<T>) -> unit {
    let restricted: Restricted<T> = value;
    let unrelated = identity(Value(42));
    return;
}
flow main() -> unit { return; }
"#,
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|d| d.message.contains("does not satisfy spec bound")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn alias_bound_arguments_are_substituted_without_using_another_parameters_evidence() {
    for (region, valid) in [("Root", true), ("Other", false)] {
        let source = format!(
            r#"
spec Within<R>;
type Root;
type Other;
type Value = i32;
impl Value ~ Within<Root>;
type Wrapper<T> = {{ value: T }};
alias Restricted<R, T ~ Within<R>> = Wrapper<T>;
flow main() -> Wrapper<Value> {{ return Restricted<{region}, Value> {{ value = Value(42) }}; }}
"#
        );
        let output = check(&source);
        if valid {
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        } else {
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.message.contains("does not satisfy spec bound")),
                "{:?}",
                output.diagnostics
            );
        }
    }
}
