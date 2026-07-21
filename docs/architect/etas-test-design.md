# Etas Test Support Design

## 1. Purpose

This document defines the architecture of the dev-only `etas_test` crate and
the workspace fixture layout.

`etas_test` exists to make compiler, CLI, LSP, AIR, runtime, and safety tests
consistent. It should provide reusable test harness utilities, not production
behavior.

No production crate may depend on `etas_test` in normal builds.

## 2. Position In The Architecture

`etas_test` is outside the production dependency graph.

```text
workspace tests / crate tests
  -> etas_test
      - fixture loading
      - golden comparison
      - diagnostic assertions
      - batch pipeline test helpers
      - CLI smoke harness
      - LSP fake-client harness
      - mock host adapters
  -> Etas public crate APIs
```

Allowed dependency direction:

```text
crate tests
  dev-depend on etas_test

workspace integration tests
  depend on etas_test

etas_test
  may depend on Etas crates under test

production crates
  must not depend on etas_test
```

This keeps test convenience APIs from leaking into compiler, runtime, or host
design.

## 3. Responsibilities

`etas_test` owns:

- loading `.es` fixtures from stable workspace paths;
- normalizing line endings and paths for deterministic tests;
- comparing golden text and JSON artifacts;
- asserting diagnostics by code, severity, message fragment, span, labels, and
  suggestions;
- running batch compiler pipeline tests through `etas_driver`;
- creating test `SourceFile` values with stable `SourceId`s;
- providing CLI smoke helpers around `etas_cli`'s testable entrypoint;
- providing LSP protocol smoke helpers around `etas_lsp`;
- providing mock model, mock tool, mock memory, and mock approval host adapters;
- capturing runtime traces and budget events in deterministic tests.

`etas_test` does not own:

- AST, HIR, AIR, type, effect, or runtime data models;
- parser or checker semantics;
- production mock behavior;
- production config loading;
- provider-specific adapters;
- CI policy.

## 4. Crate Layout

Recommended file layout:

```text
crates/etas_test/
  Cargo.toml
  src/
    lib.rs
    fixture.rs
    golden.rs
    diagnostics.rs
    pipeline.rs
    cli.rs
    lsp.rs
    mock_host.rs
```

Module responsibilities:

| Module | Responsibility |
|---|---|
| `fixture` | Locate and load `.es`, `.json`, `.txt`, `.mmd`, and expected diagnostic fixtures |
| `golden` | Compare deterministic text/JSON output against checked-in expected files |
| `diagnostics` | Assertion helpers for diagnostic codes, spans, labels, notes, and suggestions |
| `pipeline` | High-level helpers for parse, lower, check, AIR export, graph export, and dry-run tests |
| `cli` | CLI smoke harness using `etas_cli`'s testable entrypoint |
| `lsp` | Fake LSP client/session helpers for protocol smoke tests |
| `mock_host` | Deterministic model/tool/memory/approval adapters for runtime tests |

The modules should be small adapters over public crate APIs. If a helper needs
access to private compiler internals, the compiler API boundary is probably not
designed well enough yet.

## 5. Fixture Layout

Recommended workspace fixture layout:

```text
tests/
  fixtures/
    syntax/
      positive/
      negative/
      golden/

    hir/
      golden/
      resolution/

    typecheck/
      positive/
      negative/

    effects/
      inference/
      policy/

    air/
      golden/
      graph/

    runtime/
      dry_run/
      trace/

    safety/
      approval/
      taint/
      sandbox/
      budget/
```

Fixture naming should make intent visible:

```text
pipeline_basic.es
pipeline_basic.ast.txt
pipeline_basic.hir.txt
pipeline_basic.air.json
pipeline_basic.diagnostics.json
```

Use `.es` for source inputs. Use explicit suffixes for expected artifacts:

| Suffix | Meaning |
|---|---|
| `.ast.txt` | Deterministic AST dump |
| `.hir.txt` | Deterministic HIR dump |
| `.air.json` | Deterministic AIR JSON |
| `.graph.mmd` | Mermaid graph output |
| `.graph.dot` | DOT graph output |
| `.diagnostics.json` | Expected diagnostics |
| `.trace.jsonl` | Expected trace event stream |

## 6. Golden Testing Rules

Golden tests should be deterministic and reviewable.

Rules:

1. Golden output must not contain absolute machine-local paths unless explicitly
   normalized.
2. Source ids, node ids, symbol ids, and graph ids must be stable for identical
   input.
3. JSON output should be pretty-printed for golden fixtures unless JSONL is
   intentionally required.
4. Golden updates should be explicit. A test should not silently rewrite
   expected files by default.
5. Human-oriented dumps may evolve, but changes should be reviewed as language
   behavior changes, not incidental formatting churn.

`etas_test::golden` may later support an opt-in environment variable such as:

```text
ETAS_UPDATE_GOLDEN=1
```

That update mode must remain explicit.

## 7. Diagnostic Assertions

Diagnostic tests should not rely only on full string comparison.

`etas_test::diagnostics` should support assertions by:

- stable diagnostic code;
- severity;
- primary span;
- line and column;
- message fragment;
- label message fragment;
- note fragment;
- suggestion replacement;
- applicability.

Full golden diagnostic JSON is still useful for end-to-end snapshots, but
targeted assertions make tests less brittle when wording improves.

## 8. Pipeline Harness

`etas_test::pipeline` should call `etas_driver` for batch tests.

Recommended helper shape:

```rust
parse_fixture("syntax/positive/pipeline_basic.es")
lower_fixture("hir/resolution/pipeline_basic.es")
check_fixture("typecheck/positive/pipeline_basic.es")
check_project_fixture("compiler/positive/algorithms/binary_search.es")
dump_air_fixture("air/golden/pipeline_basic.es")
dry_run_fixture("runtime/dry_run/pipeline_basic.es")
```

The pipeline harness should not duplicate compiler orchestration. It should
exercise the same public batch path used by `etas_cli`.

Project-level fixture helpers must compile a source set, not one file at a
time. The existing acceptance fixtures under `etas_tests` should be treated as
the first project import-resolution corpus:

```text
fixtures/compiler/positive/algorithms/*.es
fixtures/compiler/negative/algorithms/*.es
fixtures/compiler/support/algorithms.es
```

Required project assertions:

- algorithm fixtures importing `tests.compiler.support.algorithms.{...}`
  resolve helpers from `compiler/support/algorithms.es`;
- only `public flow` helpers are importable from the support module;
- standard-library imports such as `std.collections.List`,
  `std.effects.Console`, and `std.io.{read_all, println}` resolve through
  the std declaration provider;
- missing modules, missing exported items, private imported items, duplicate
  modules, mismatched module declarations, and wildcard ambiguity are negative
  project fixtures.

The support module should not become a shadow standard library. It may contain
fixture-format helpers that are ordinary user code, while reusable primitive,
text, and collection operations belong to `etas_std` plus `etas_builtin`.

Recommended split for current algorithm helpers:

```text
standard library:
  Map[K, V].contains_key(key) -> bool
  string.len() / std.text.len(string) -> usize
  string.lines() / std.text.lines(string) -> List[string]
  std.text.join(List[string], separator) -> string
  concrete primitive formatting such as i32.to_string()
  concrete primitive parsing such as std.text.parse_i32(...)

fixture support or user code:
  parse_i32_list(input)    # fixture input convention
  join_i32_list(values, s) # fixture output convention until Display exists
  matrix_i32/repeat_i32/repeat_bool/slice/abs
```

If a positive fixture imports `parse_i32_list` or `join_i32_list`, the test is
checking project/module import support and ordinary flow execution. It should
not be used as evidence that those exact helper names belong in the standard
library.

## 9. CLI Harness

`etas_test::cli` should test CLI behavior through a testable API exposed by
`etas_cli`, not by shelling out for every case.

The CLI harness should verify:

- argument parsing;
- global option behavior;
- stdout/stderr separation;
- exit code mapping;
- diagnostic rendering;
- JSON output shape;
- smoke behavior for `check`, `dump ast`, `dump hir`, `dump air`, `graph`, and
  `run --dry-run`.

Process-spawn tests may still exist for final binary smoke coverage, but they
should be intentionally bounded and representative.

## 10. LSP Harness

`etas_test::lsp` should provide a fake LSP client around `etas_lsp`.

Initial protocol tests should cover:

- initialize;
- shutdown;
- didOpen;
- didChange;
- diagnostics publication;
- document symbols;
- hover;
- definition;
- completion;
- semantic tokens.

The LSP harness should use in-memory documents and deterministic responses. It
must not trigger real runtime side effects.

## 11. Mock Host

`etas_test::mock_host` should provide deterministic host components for runtime
and safety tests:

- mock model provider;
- mock tool registry;
- mock memory backend;
- mock approval provider;
- mock clock;
- mock budget tracker.

Mock host behavior should be explicit in fixtures. A runtime test should be
able to say which model responses, tool outputs, approvals, memory snapshots,
and budget limits are expected.

This is test infrastructure only. Production host abstractions still belong in
`etas_host`.

## 12. Test Categories

The baseline test matrix should include:

- syntax positive tests;
- syntax negative diagnostics;
- AST dump golden tests;
- HIR lowering and resolution tests;
- HIR dump golden tests;
- type checker positive and negative tests;
- effect declaration, action resolution, and effect inference tests;
- handler syntax/type/effect/interpreter tests covering `handler { ... }`,
  inline `with { ... }`, `with <handler-value-expr>`, `resume`, produced-effect bounds,
  and `handle` result compatibility;
- policy diagnostics;
- AIR JSON golden tests;
- graph output golden tests;
- runtime dry-run tests;
- trace capture tests;
- safety negative tests for approval, taint, sandbox, and budget rules;
- CLI smoke tests;
- LSP protocol smoke tests;
- `etas_intel` query tests.

Examples from the main Etas repository's `etas/docs/design/` directory may
be promoted into conformance fixtures only when the corresponding syntax and
semantic features are intentionally in scope.

## 13. Implementation Scope

The first `etas_test` implementation should provide:

- fixture loading;
- stable `SourceFile` construction;
- AST dump golden comparison;
- HIR dump golden comparison;
- diagnostic assertion helpers;
- `etas_driver` pipeline helpers for check and dump commands;
- CLI smoke harness for `check`, `dump ast`, `dump hir`, and `dump air`.

Deferred:

- full LSP fake client;
- mock model/tool/memory host;
- trace replay golden tests;
- checkpoint resume tests;
- budget simulation;
- property-based tests;
- benchmark harnesses.
