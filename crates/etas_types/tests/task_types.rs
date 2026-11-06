use etas_core::{SourceFile, SourceId};
use etas_types::Type;

fn check(source: &str) -> (etas_hir::HirProgram, etas_types::TypeOutput) {
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(97), None, source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    let output = etas_types::check_program(&hir);
    (hir, output)
}

#[test]
fn task_std_signatures_keep_effect_rows_as_effect_rows() {
    let (_, output) = check(
        r#"
import std.task.{run, start, await, Scheduler};
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        let task = start(() => { return 42; });
        return await(task);
    });
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);

    let task = output
        .facts
        .symbol_types
        .values()
        .find_map(|fact| match fact {
            etas_types::SymbolTypeFact::Local { ty, .. }
                if matches!(output.store.get(*ty), Some(Type::Task(_))) =>
            {
                Some(*ty)
            }
            _ => None,
        })
        .expect("start result is a task handle");
    let Some(Type::Task(task)) = output.store.get(task) else {
        unreachable!()
    };
    assert!(task.effects.effects.is_empty());
    assert_eq!(task.effects.tail.as_deref(), Some("E"));
    assert!(matches!(
        output.store.get(task.output),
        Some(Type::Primitive(etas_types::PrimitiveType::I32))
    ));

    let obligation = output
        .facts
        .generic_instantiations
        .values()
        .flat_map(|fact| &fact.deferred_effect_row_obligations)
        .find(|obligation| obligation.param == "E")
        .expect("start has a deferred effect-row instantiation");
    assert!(obligation.fixed.effects.is_empty());
    assert!(obligation.fixed.tail.is_none());
    assert_eq!(output.facts.task_owners.len(), 1);
}

#[test]
fn scheduler_transfer_consumes_the_bound_continuation() {
    let (_, output) = check(
        r#"
import std.task.{run, start, await, Scheduler};
import std.task.scheduler.park;
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        let task = start(() => { return 7; });
        return handle { await(task) } with {
            Async.await(waited) as k => {
                park(scheduler, waited, k);
            }
        };
    });
}
"#,
    );

    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    assert_eq!(output.facts.continuation_bindings.len(), 1);
}

#[test]
fn continuation_cannot_be_stored_as_an_ordinary_value() {
    let (_, output) = check(
        r#"
effect Echo {
    action echo(value: i32) -> i32;
}
flow main() -> i32 {
    return handle { perform Echo.echo(1) } with {
        Echo.echo(value) as k => {
            let retained = k;
            resume value;
        }
    };
}
"#,
    );

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == etas_core::DiagnosticCode::Type(
                    etas_core::TypeDiagnosticCode::InvalidContinuationUse,
                )
        }),
        "{:#?}",
        output.diagnostics
    );
}

#[test]
fn continuation_cannot_be_captured_by_a_lambda() {
    let (_, output) = check(
        r#"
effect Echo {
    action echo(value: i32) -> i32;
}
flow main() -> i32 {
    return handle { perform Echo.echo(1) } with {
        Echo.echo(value) as k => {
            let delayed = () => { let retained = k; return; };
            resume value;
        }
    };
}
"#,
    );

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == etas_core::DiagnosticCode::Type(
                    etas_core::TypeDiagnosticCode::InvalidContinuationUse,
                )
        }),
        "{:#?}",
        output.diagnostics
    );
}

#[test]
fn start_requires_a_lexical_run_owner() {
    let (_, output) = check(
        r#"
import std.task.start;
flow main() -> unit {
    let task = start(() => { return 1; });
    return;
}
"#,
    );

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::InvalidTaskScope)
        }),
        "{:#?}",
        output.diagnostics
    );
}

#[test]
fn run_cannot_return_an_owned_task_handle() {
    let (_, output) = check(
        r#"
import std.task.{run, start, Scheduler};
flow main() -> unit {
    let leaked = run((scheduler: Scheduler) => {
        return start(() => { return 1; });
    });
    return;
}
"#,
    );

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::InvalidTaskScope)
        }),
        "{:#?}",
        output.diagnostics
    );
}
