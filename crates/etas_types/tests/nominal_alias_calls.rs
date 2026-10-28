use etas_core::{SourceFile, SourceId};

fn check(source: &str) -> etas_types::TypeOutput {
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(1), None, source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    etas_types::check_program(&hir)
}

#[test]
fn transparent_alias_calls_use_the_nominal_constructors_immediate_input() {
    for source in [
        "type Value = i32; alias Public = Value; flow main() -> Public { return Public(42); }",
        "type Value = u8; alias Public = Value; flow main() -> Public { return Public(255); }",
        "type Inner = i32; type Outer = Inner; alias Public = Outer; flow main() -> Public { return Public(Inner(42)); }",
        "type Wrap<T> = { value: T }; alias Public<T> = Wrap<T>; flow main() -> Public<i32> { return Public<i32>({ value = 42 }); }",
        "type Wrap<T> = { value: T }; alias Public<T> = Wrap<T>; flow main() -> Public<i32> { return Public<i32>(abort(\"stopped\")); }",
        "type Wrap<T> = { value: T }; alias Public<T> = Wrap<T>; flow main() -> Public<i32> { return Public({ value = 42 }); }",
        "type Wrap<T> = { value: T }; alias Public<T> = Wrap<T>; flow main() -> Public<i32> { return Public { value = 42 }; }",
        "spec Allowed; type Value = i32; impl Value ~ Allowed; type Wrap<T ~ Allowed> = { value: T }; alias Public<T ~ Allowed> = Wrap<T>; flow main() -> Public<Value> { return Public<Value>({ value = Value(42) }); }",
        "spec Allowed; type Value = i32; impl Value ~ Allowed; type Wrap<T ~ Allowed> = { value: T }; alias Public<T ~ Allowed> = Wrap<T>; flow main() -> Public<Value> { return Public { value = Value(42) }; }",
    ] {
        let output = check(source);
        assert!(
            output.diagnostics.is_empty(),
            "{source}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn alias_calls_do_not_erase_nominal_identity_or_numeric_range_checks() {
    for source in [
        "type Value = i32; alias Public = Value; flow main() -> Public { return 42; }",
        "type Value = u8; alias Public = Value; flow main() -> Public { return Public(256); }",
        "type Inner = i32; type Outer = Inner; alias Public = Outer; flow main() -> Public { return Public(42); }",
        "type Value = i32; type Other = i32; alias Public = Value; flow main() -> Public { return Public(Other(42)); }",
        "type Wrap<T> = { value: T }; alias Public<T> = Wrap<T>; flow main() -> Public<i32> { return Public<i32>({ value = \"wrong\" }); }",
        "type Wrap<T> = { value: T }; alias Public<T> = Wrap<T>; flow main() -> Public<i32> { return Public<i32, string>({ value = 42 }); }",
        "type Value = i32; alias Public = Value; flow main() -> Public { return Public<![]>(42); }",
        "spec Allowed; type Wrap<T ~ Allowed> = { value: T }; alias Public<T ~ Allowed> = Wrap<T>; flow main() -> Public<i32> { return Public<i32>({ value = 42 }); }",
        "spec Allowed; type Wrap<T ~ Allowed> = { value: T }; alias Public<T ~ Allowed> = Wrap<T>; flow main() -> Public<i32> { return Public { value = 42 }; }",
    ] {
        let output = check(source);
        assert!(!output.diagnostics.is_empty(), "accepted {source}");
    }
}
