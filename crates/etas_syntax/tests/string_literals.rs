use etas_syntax::{
    SourceFile,
    ast::{Expr, Literal},
    parse_body_contents,
};

fn string_value(source: &str) -> String {
    let parsed = parse_body_contents(SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let Some(Expr::Literal(Literal::String { value, .. })) = parsed.value.final_expr.as_deref()
    else {
        panic!("expected string literal: {source}");
    };
    value.clone()
}

#[test]
fn escaped_backslash_is_consumed_before_following_escape_letter() {
    let value = string_value(r#""\\n""#);
    assert_eq!(value.as_bytes(), [b'\\', b'n']);
}

#[test]
fn string_escapes_are_decoded_once_from_left_to_right() {
    for (source, expected) in [
        (r#""\n""#, "\n"),
        (r#""\r""#, "\r"),
        (r#""\t""#, "\t"),
        (r#""\"""#, "\""),
        (r#""\\""#, "\\"),
        (r#""\q""#, r"\q"),
    ] {
        assert_eq!(string_value(source), expected, "{source}");
    }
}
