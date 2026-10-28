use etas_code::syntax::{
    CodeSyntaxOperations, ParseBodyFailure, SyntaxOperationError, SyntaxResource,
};
use etas_core::memory::MemoryFootprint;
use etas_frontend::code::service::{NativeCodeSyntaxOperations, NativeSyntaxLimits};

#[test]
fn code_syntax_ref_holds_native_ast_and_formats_without_checking_names() {
    let backend = NativeCodeSyntaxOperations::default();
    let syntax = backend
        .parse_body("let answer = unknown_name(42); return answer;")
        .unwrap();
    let native = backend.resolve(&syntax).unwrap();
    let etas_syntax::SourceBodyView::Block(block) = native.body().unwrap() else {
        panic!("fragment block")
    };
    assert_eq!(block.stmts.len(), 2);
    let text = backend.format(&syntax).unwrap();
    assert!(text.contains("unknown_name(42)"));
    assert!(text.contains("return answer;"));
    let reparsed = backend.parse_body(&text).unwrap();
    assert_eq!(backend.format(&reparsed).unwrap(), text);
}

#[test]
fn retained_views_share_the_source_file_program_and_retention_budget() {
    use etas_code::CodeFingerprint;
    use etas_core::{SourceFile, SourceId};
    use etas_syntax::ProgramBodySelection;
    use std::sync::Arc;
    let text = format!(
        "flow first() -> i32 {{ /* {} */ return 1; }} flow second() -> i32 {{ return 2; }}",
        "x".repeat(32_000)
    );
    let source = Arc::new(SourceFile::new(SourceId(51), None, text));
    let parsed = Arc::new(etas_syntax::parse_program((*source).clone()));
    let baseline = MemoryFootprint::of(&(source.clone(), parsed.clone())).estimated_bytes();
    let backend = NativeCodeSyntaxOperations::new(NativeSyntaxLimits {
        max_retained_bytes: baseline + 8192,
        ..NativeSyntaxLimits::default()
    });
    let mut bodies = Vec::new();
    for item in 0..2 {
        let owner = CodeFingerprint::of_bytes("etas.test.retained-owner.v1", &[item as u8]);
        bodies.push(
            backend
                .retain_source_body(
                    owner,
                    source.clone(),
                    parsed.clone(),
                    ProgramBodySelection {
                        item,
                        impl_member: None,
                    },
                )
                .unwrap(),
        );
    }
    let first = backend.resolve(&bodies[0]).unwrap();
    let second = backend.resolve(&bodies[1]).unwrap();
    assert!(std::ptr::eq(first.source(), second.source()));
    assert!(std::ptr::eq(first.source(), &*source));
    assert!(backend.format(&bodies[0]).unwrap().contains("return 1;"));
    assert_eq!(backend.format(&bodies[1]).unwrap(), "return 2;");
}

#[test]
fn code_syntax_is_not_recovered_from_a_foreign_digest() {
    let first = NativeCodeSyntaxOperations::default();
    let second = NativeCodeSyntaxOperations::default();
    let left = first.parse_body("return 42;").unwrap();
    let right = second.parse_body("return 42;").unwrap();
    assert_eq!(left.identity(), right.identity());
    assert_eq!(
        second.format(&left),
        Err(SyntaxOperationError::ForeignSyntaxObject)
    );
    assert_eq!(
        first.format(&right),
        Err(SyntaxOperationError::ForeignSyntaxObject)
    );
    assert_eq!(first.format(&left).unwrap(), "return 42;");
}

#[test]
fn code_syntax_parsing_errors_have_original_body_offsets() {
    let backend = NativeCodeSyntaxOperations::default();
    let source = "let = ;";
    let Err(ParseBodyFailure::InvalidSyntax(error)) = backend.parse_body(source) else {
        panic!("invalid source must not publish a syntax object")
    };
    assert!(!error.diagnostics.is_empty());
    assert!(error.diagnostics.iter().all(|diagnostic| {
        let location = diagnostic.location.as_ref().expect("real body location");
        diagnostic.code.starts_with("syntax::")
            && location.byte_start <= location.byte_end
            && location.byte_end <= source.len()
    }));
    assert!(
        error
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.location.as_ref().unwrap().byte_start > 0)
    );
}

#[test]
fn code_syntax_leases_share_backing_and_release_object_quota() {
    let backend = NativeCodeSyntaxOperations::new(NativeSyntaxLimits {
        max_retained_objects: 1,
        ..NativeSyntaxLimits::default()
    });
    let reference = backend.parse_body("return 42;").unwrap();
    let copies = vec![reference.clone(); 100];
    let base = MemoryFootprint::of(&reference).estimated_bytes();
    let shared = MemoryFootprint::of(&copies).estimated_bytes();
    assert!(shared <= base + (copies.capacity() * size_of_val(&reference)) as u64);
    let same = backend.parse_body("return 42;").unwrap();
    assert_eq!(
        MemoryFootprint::of(&(reference.clone(), same)).estimated_bytes(),
        base
    );
    let native = backend.resolve(&reference).unwrap();
    assert_eq!(
        MemoryFootprint::of(&(reference.clone(), native.clone())).estimated_bytes(),
        base
    );
    drop(reference);
    drop(copies);
    assert!(matches!(
        backend.parse_body("return 43;"),
        Err(ParseBodyFailure::Operation(
            SyntaxOperationError::ResourceLimit {
                resource: SyntaxResource::RetainedObjects,
                limit: 1
            }
        ))
    ));
    assert_eq!(native.source().text(), "return 42;");
    drop(native);
    assert!(backend.parse_body("return 43;").is_ok());
}

#[test]
fn code_syntax_limits_are_operational_errors_not_parse_errors() {
    let mut limits = NativeSyntaxLimits::default();
    limits.parse.max_source_bytes = 4;
    let backend = NativeCodeSyntaxOperations::new(limits);
    assert!(matches!(
        backend.parse_body("return 42;"),
        Err(ParseBodyFailure::Operation(
            SyntaxOperationError::ResourceLimit {
                resource: SyntaxResource::SourceBytes,
                limit: 4
            }
        ))
    ));
    let mut limits = NativeSyntaxLimits::default();
    limits.parse.max_tokens = 2;
    let backend = NativeCodeSyntaxOperations::new(limits);
    assert!(matches!(
        backend.parse_body("return 42;"),
        Err(ParseBodyFailure::Operation(
            SyntaxOperationError::ResourceLimit {
                resource: SyntaxResource::Tokens,
                limit: 2
            }
        ))
    ));
    let backend = NativeCodeSyntaxOperations::new(NativeSyntaxLimits {
        max_retained_bytes: 1,
        ..NativeSyntaxLimits::default()
    });
    assert!(matches!(
        backend.parse_body("return 42;"),
        Err(ParseBodyFailure::Operation(
            SyntaxOperationError::ResourceLimit {
                resource: SyntaxResource::RetainedBytes,
                ..
            }
        ))
    ));
    let backend = NativeCodeSyntaxOperations::new(NativeSyntaxLimits {
        max_formatted_bytes: 4,
        ..NativeSyntaxLimits::default()
    });
    let syntax = backend.parse_body("return 42;").unwrap();
    assert!(matches!(
        backend.format(&syntax),
        Err(SyntaxOperationError::ResourceLimit {
            resource: SyntaxResource::FormattedBytes,
            limit: 4
        })
    ));
}

#[test]
fn code_syntax_deep_inputs_fail_before_native_stack_overflow() {
    let backend = NativeCodeSyntaxOperations::default();
    let sources = [
        format!("return {}true;", "!".repeat(1000)),
        format!("return {}1{};", "(".repeat(1000), ")".repeat(1000)),
        format!("return x{};", ".call()".repeat(1000)),
        format!("return {}1;", "x => ".repeat(1000)),
        format!("return {}0;", "1 :: ".repeat(1000)),
        format!("let x: {}i32 = 0;", "() -> ".repeat(1000)),
        format!(
            "let x: {}i32{} = 0;",
            "Array<".repeat(1000),
            ">".repeat(1000)
        ),
        format!("let {}x{} = value;", "Some(".repeat(1000), ")".repeat(1000)),
        format!("{}return;", "if true {} else ".repeat(1000)),
        format!("return {};", vec!["1"; 1000].join(" + ")),
    ];
    for source in sources {
        assert!(
            matches!(
                backend.parse_body(&source),
                Err(ParseBodyFailure::Operation(
                    SyntaxOperationError::ResourceLimit {
                        resource: SyntaxResource::Nesting,
                        ..
                    }
                ))
            ),
            "source must terminate with a nesting error"
        );
    }
}

#[test]
fn code_syntax_cached_objects_do_not_bypass_stricter_invocation_limits() {
    let backend = NativeCodeSyntaxOperations::default();
    let original = backend.parse_body("return 42;").unwrap();
    for limits in [
        NativeSyntaxLimits {
            parse: etas_syntax::BodyParseLimits {
                max_tokens: 2,
                ..Default::default()
            },
            ..Default::default()
        },
        NativeSyntaxLimits {
            max_retained_objects: 0,
            ..Default::default()
        },
        NativeSyntaxLimits {
            max_retained_bytes: 0,
            ..Default::default()
        },
    ] {
        assert!(matches!(
            backend.parse_body_bounded("return 42;", limits),
            Err(ParseBodyFailure::Operation(
                SyntaxOperationError::ResourceLimit { .. }
            ))
        ));
    }
    assert_eq!(backend.format(&original).unwrap(), "return 42;");
    let repeated = backend.parse_body("return 42;").unwrap();
    assert_eq!(
        etas_core::memory::MemoryFootprint::of(&(original.clone(), repeated)).estimated_bytes(),
        etas_core::memory::MemoryFootprint::of(&original).estimated_bytes()
    );
}
