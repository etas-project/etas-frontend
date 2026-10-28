use std::sync::atomic::{AtomicUsize, Ordering};

use etas_code::syntax::{CodeSyntaxOperations, ParseBodyFailure, SyntaxOperationError};
use etas_frontend::code::service::NativeCodeSyntaxOperations;
use etas_utils::{PipelineExecutionControl, PipelineInterruption, PipelineInterruptionKind};

struct StopAfter {
    checks: AtomicUsize,
    stop_at: usize,
    kind: PipelineInterruptionKind,
}

impl PipelineExecutionControl for StopAfter {
    fn checkpoint(&self) -> Result<(), PipelineInterruption> {
        if self.checks.fetch_add(1, Ordering::Relaxed) >= self.stop_at {
            Err(PipelineInterruption {
                kind: self.kind,
                reason: "syntax execution stopped".into(),
            })
        } else {
            Ok(())
        }
    }
}

#[test]
fn syntax_port_checks_execution_limits_even_when_the_exact_body_is_cached() {
    let backend = NativeCodeSyntaxOperations::default();
    let syntax = backend.parse_body("return 42;").unwrap();
    for kind in [
        PipelineInterruptionKind::Cancelled,
        PipelineInterruptionKind::ResourceLimit,
    ] {
        let control = StopAfter {
            checks: AtomicUsize::new(0),
            stop_at: 0,
            kind,
        };
        let error = backend
            .parse_body_controlled("return 42;", &control)
            .unwrap_err();
        assert!(
            matches!(error, ParseBodyFailure::Operation(SyntaxOperationError::Interrupted(error)) if error.kind == kind)
        );
        let error = backend
            .format_bounded_controlled(&syntax, 1024, &control)
            .unwrap_err();
        assert!(matches!(error, SyntaxOperationError::Interrupted(error) if error.kind == kind));
    }
    assert_eq!(backend.format(&syntax).unwrap(), "return 42;");
}

#[test]
fn native_syntax_port_preserves_interruption_from_inside_lexer_and_formatter() {
    let backend = NativeCodeSyntaxOperations::default();
    let text = "let value = 1;".repeat(128);
    let control = StopAfter {
        checks: AtomicUsize::new(0),
        stop_at: 64,
        kind: PipelineInterruptionKind::Cancelled,
    };
    assert!(matches!(
        backend.parse_body_controlled(&text, &control),
        Err(ParseBodyFailure::Operation(
            SyntaxOperationError::Interrupted(_)
        ))
    ));
    let syntax = backend.parse_body(&text).unwrap();
    let control = StopAfter {
        checks: AtomicUsize::new(0),
        stop_at: 64,
        kind: PipelineInterruptionKind::ResourceLimit,
    };
    assert!(matches!(
        backend.format_bounded_controlled(&syntax, 32 * 1024, &control),
        Err(SyntaxOperationError::Interrupted(_))
    ));
    assert_eq!(control.checks.load(Ordering::Relaxed), 65);
}
