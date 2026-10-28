use std::sync::Arc;

use etas_core::memory::MemoryFootprint;
use etas_syntax::{
    AstDump, AstDumpWriter, BodyFormatError, BodyFormatOptions, BodySyntax, DumpOptions,
    SourceFile, SourceId,
};

fn parse(text: &str) -> Arc<BodySyntax> {
    BodySyntax::parse(SourceFile::new(
        SourceId(45),
        Some("generated/body.es".into()),
        text,
    ))
    .unwrap()
}

fn ast(body: &BodySyntax) -> String {
    let mut dump = AstDumpWriter::new(DumpOptions {
        include_spans: false,
        ..Default::default()
    });
    body.block().dump(&mut dump);
    dump.finish()
}

#[test]
fn formatting_round_trips_structure_and_is_idempotent() {
    for text in [
        "let x:i32=f(1,2); return x;",
        "let message=\"工作 // not a comment\"; /* removed */ return message;",
        "return if true { 1 } else { 2 };",
        "if true { return 1; } else if false { return 2; } return 3;",
        "let f=(x:i32)=>{ return x+1; }; f(41)",
        "let d=^A::Stage<i32>.run; (^A).body",
        "return Response.Failure { code=503, message=\"down\", };",
        "return match response { Response.Success(value) => value, Response.Failure {code: _, message} => 0, };",
        "return 1 :: [2,3];",
        "accepts_row<![Console.stdout_write]>(write_console);",
        "let result=handle { return perform Console.stdout_write(\"x\"); } with { Error<i32>.raise(error) => { finish error; } }; return result;",
        "let a=xs[0,2); let b=xs(0,2]; return a;",
        "return data ~> trim | uppercase;",
        "let values=#{1,2}; let names=[\"a\",\"b\"]; return { values=values, names=names };",
        "{ let n=-2; !false; n }",
        "",
        "// body\n/* empty */",
    ] {
        let original = parse(text);
        let formatted = original.format(BodyFormatOptions::default()).unwrap();
        let reparsed = parse(&formatted);
        assert_eq!(
            ast(&original),
            ast(&reparsed),
            "input: {text}\nformatted: {formatted}"
        );
        assert_eq!(
            formatted,
            reparsed.format(BodyFormatOptions::default()).unwrap(),
            "{text}"
        );
    }
}

#[test]
fn output_is_readable_and_keeps_original_literal_spelling() {
    let body = parse("let x:i32=f(0x2a,1_000);if true { return x; } return 0;");
    assert_eq!(
        body.format(BodyFormatOptions::default()).unwrap(),
        "let x: i32 = f(0x2a, 1_000);\nif true {\n    return x;\n}\nreturn 0;"
    );
    assert_eq!(
        body.source().path.as_ref().unwrap().to_str().unwrap(),
        "generated/body.es"
    );
    assert_eq!(body.tokens().source, SourceId(45));
    assert!(!body.block().stmts.is_empty());
}

#[test]
fn syntax_errors_never_create_a_successful_body_object() {
    for text in [
        "let x = ;",
        "return 1; }",
        "42 trailing",
        "^A.run(input)",
        "\"unterminated",
    ] {
        let error = BodySyntax::parse(SourceFile::new(SourceId(45), None, text)).unwrap_err();
        assert!(!error.diagnostics.is_empty());
        assert_eq!(error.source.text(), text);
        assert!(
            error
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.primary.span.source == SourceId(45))
        );
    }
}

#[test]
fn formatting_output_limit_is_enforced_without_truncating_success() {
    let body = parse("return 42;");
    assert_eq!(
        body.format(BodyFormatOptions {
            max_output_bytes: 10,
            ..Default::default()
        })
        .unwrap(),
        "return 42;"
    );
    assert_eq!(
        body.format(BodyFormatOptions {
            max_output_bytes: 9,
            ..Default::default()
        }),
        Err(BodyFormatError::OutputLimit { limit: 9 })
    );
    let nested = parse("if true { if true { return 42; } }");
    assert!(matches!(
        nested.format(BodyFormatOptions {
            indent_width: 255,
            max_output_bytes: 64
        }),
        Err(BodyFormatError::OutputLimit { .. })
    ));
    assert_eq!(
        parse("")
            .format(BodyFormatOptions {
                max_output_bytes: 0,
                ..Default::default()
            })
            .unwrap(),
        ""
    );
}

#[test]
fn body_references_share_ast_tokens_and_source_and_release_when_unpinned() {
    let body = parse("let text=\"shared source\"; return text;");
    let weak = Arc::downgrade(&body);
    let one = MemoryFootprint::of(&body);
    let refs = vec![body.clone(); 100];
    assert!(std::ptr::eq(body.block(), refs[99].block()));
    let many = MemoryFootprint::of(&refs);
    assert_eq!(many.allocations.len(), one.allocations.len() + 1);
    assert_eq!(
        many.estimated_bytes() - one.estimated_bytes(),
        (refs.capacity() * std::mem::size_of::<Arc<BodySyntax>>()) as u64
    );
    drop(body);
    assert!(weak.upgrade().is_some());
    drop(refs);
    assert!(weak.upgrade().is_none());
}
