# Etas CLI Design

## 1. Purpose

This document defines the architecture of the `etas_cli` module.

`etas_cli` is the user-facing command surface for Etas. It should stay thin:
it owns command parsing, configuration discovery, terminal rendering, machine
readable output, workspace input selection, and exit-code policy.

It must not own parser logic, AST definitions, HIR lowering, type checking,
effect checking, AIR construction, runtime authority checks, or provider/tool
execution semantics.

Batch compilation and dry-run orchestration belong in `etas_driver`.

## 2. Position In The Architecture

The CLI path should be:

```text
terminal / script / CI
  -> etas_cli
      - command parsing
      - config and workspace discovery
      - output formatting
      - exit code mapping
  -> etas_driver
      - batch compile sessions
      - check / dump / graph / run orchestration
      - diagnostic aggregation
      - artifact production
  -> compiler, analysis, runtime, and host crates
```

The LSP path should stay separate:

```text
editor / LSP client
  -> etas_lsp
  -> etas_intel
  -> compiler and analysis crates
```

`etas_cli` and `etas_lsp` should not depend on each other.

`etas_cli` may expose `etas lsp` as an installation convenience, but the LSP
protocol implementation still belongs to `etas_lsp`.

## 3. Design Principles

1. Keep command modules declarative.
   - Arguments, defaults, and command names belong in `etas_cli`.
   - Semantic behavior belongs in `etas_driver` or lower crates.
2. Keep output stable.
   - Human output may improve over time.
   - JSON output should be treated as a tool-facing contract once published.
3. Keep diagnostics unified.
   - CLI diagnostics should render the shared diagnostic model.
   - CLI must not invent its own error representation for compiler errors.
4. Keep side effects explicit.
   - `etas run` should default to dry-run during early development.
   - Real model calls, tool calls, file writes, command execution, and network
     effects must require an explicit runtime authorization path.
5. Keep command names composable.
   - Prefer command groups such as `etas dump ast` over a growing list of
     unrelated top-level verbs.

## 4. Command Shape

Recommended command set:

```text
etas check <input...>
etas dump ast <file>
etas dump hir <file>
etas dump air <file>
etas graph <file>
etas effects <file>
etas policy <file>
etas run <file>
etas replay <trace>
etas resume <checkpoint-id>
etas watch <input...>
etas repl
etas lsp
```

Initial command set:

```text
etas check <input...>
etas dump ast <file>
etas dump hir <input...>
etas dump air <file>
etas graph <file>
etas run <file> --dry-run
etas lsp
```

The older shape `etas export-air <file>` may be kept as a compatibility alias
only if early scripts already depend on it. The canonical form should be
`etas dump air <file>`.

## 5. Global Options

Global options should be available on every command:

```text
--workspace <path>
--config <path>
--no-config
--format human|json|jsonl
--color auto|always|never
--quiet
--verbose
--log-level trace|debug|info|warn|error
--cache auto|off|read-only|write-only|read-write
--cache-root <path>
```

Defaults:

```text
--format human
--color auto
--cache auto
```

Output conventions:

- Diagnostics go to stderr by default.
- Primary command artifacts go to stdout by default.
- `--output <path>` writes artifacts to a file when a command supports it.
- JSON and JSONL output must be deterministic enough for tests and tooling.
- `--cache auto` lets the driver choose the workspace default cache behavior.
- `--cache off` forces a memory-only frontend session.
- `--cache-root <path>` overrides the workspace cache root for commands that
  use disk-backed frontend artifacts.

## 6. Core Commands

### 6.1 `etas check`

Purpose:

```text
Parse, lower, resolve, type-check, effect-check, policy-check, and report
diagnostics without executing the program.
```

Shape:

```text
etas check <input...>
etas check --workspace .
etas check --format json
```

Initial options:

```text
--all
--format human|json|jsonl
```

`etas check` is a project-level command. It must not check each input file as
an independent single-file program.

Input behavior:

```text
etas check file.es
  -> build a one-source project input for convenience

etas check file1.es file2.es
  -> build one explicit source-set project

etas check --workspace .
  -> discover package root and source root from etas.toml

etas check --workspace . --all
  -> discover all source files in the package source root and check one project
```

The CLI may own path normalization and workspace discovery, but semantic
compilation belongs to `etas_driver` / `etas_frontend`. The CLI should pass a
project input and render the returned project diagnostics. It should not loop
over files and call the single-file frontend API.

The driver must create an `etas_frontend::FrontendSession` explicitly:

```text
load project input
  -> choose FrontendSessionOptions from CLI/config
  -> open FrontendSession
  -> open_project
  -> check(CheckMode::FullProject, CheckScope::FullProject, CacheAccess::Default)
  -> render project diagnostics
```

`Frontend::check_project` is only a convenience wrapper for tests and simple
one-shot callers. The real CLI path must not rely on it because CLI commands
need to control cache root, cache access, check mode, and later snapshot/delta
handling.

`etas run` is the exception that intentionally uses a narrower frontend scope:
the driver opens one session with metadata-only dependency inputs, runs an
entry-reachable planning check, materializes the returned runtime source
requirements as dependency overlay sources, applies those changes to the same
session, and repeats incremental `EntryReachable` checks until no more runtime
source is needed. The CLI still does not decide dependency reachability or
prune package metadata; it only chooses the command policy and delegates
semantic planning to the driver/frontend boundary.

`--all` is not optional in the architecture. It may be implemented
incrementally, but the target behavior is package source discovery and
project-level checking.

### 6.1.1 `etas watch`

Purpose:

```text
Run a long-lived frontend session, watch workspace/source changes, and report
incremental diagnostics without restarting the compiler for every edit.
```

Shape:

```text
etas watch <input...>
etas watch --workspace . --all
etas watch --format jsonl
```

`etas watch` may be deferred until the compiler session model and file watching
infrastructure are stable. When implemented, it should not repeatedly spawn
`etas check`; it should own one `FrontendSession`, call `apply_changes` for
file events, and run
`check(CheckMode::Incremental, CheckScope::FullProject, CacheAccess::Default)`.
The command is useful as an incremental compiler validation path and as a
non-IDE fast-feedback mode, but it is not required for the first Phase 1 user
workflow.

### 6.2 `etas dump ast`

Purpose:

```text
Expose the parsed AST for parser development, language design review, and
golden tests.
```

Shape:

```text
etas dump ast app.es
etas dump ast app.es --tokens
etas dump ast app.es --spans
etas dump ast app.es --diagnostics
etas dump ast app.es --format text
etas dump ast app.es --format json
```

The AST dump must not include HIR symbols, inferred types, effect rows, AIR
nodes, or runtime data.

### 6.3 `etas dump hir`

Purpose:

```text
Expose lowered HIR, resolved symbols, scopes, and source mappings.
```

Shape:

```text
etas dump hir app.es
etas dump hir app.es --symbols
etas dump hir app.es --scopes
etas dump hir app.es --source-map
etas dump hir app.es --format text
etas dump hir app.es --format json
```

This command is the main debug surface for name resolution and HIR lowering.

### 6.4 `etas dump air`

Purpose:

```text
Expose AIR after type/effect checking and lowering.
```

Shape:

```text
etas dump air app.es
etas dump air app.es --flow main
etas dump air app.es --output air.json
etas dump air app.es --format json
```

AIR dump should be deterministic and suitable for golden tests.

### 6.5 `etas graph`

Purpose:

```text
Render source-level or AIR-level graph views for flows, pipelines, effects, and
runtime plans.
```

Shape:

```text
etas graph app.es
etas graph app.es --flow main
etas graph app.es --kind air
etas graph app.es --format mermaid
etas graph app.es --format dot
etas graph app.es --output graph.mmd
```

Initial options:

```text
--flow <name>
--kind source|air|effects
--format text|mermaid|dot|json
--output <path>
```

This command should help validate `|` stage composition and `~>` pipeline
application visually.

### 6.6 `etas effects`

Purpose:

```text
Show inferred and declared effect rows for flows, agents, tools, handlers, and
effect actions.
```

Shape:

```text
etas effects app.es
etas effects app.es --flow main
etas effects app.es --format json
```

This command may be deferred until effect inference has enough surface area.

### 6.7 `etas policy`

Purpose:

```text
Explain policy decisions, approval requirements, requested actions, missing
action grants, residual runtime checks, and denied operations for the entry flow
or a named analysis target.
```

Shape:

```text
etas policy app.es
etas policy app.es --flow main
etas policy app.es --format json
```

This command is useful for CI and for auditing high-impact agentic workflows.

### 6.8 `etas run`

Purpose:

```text
Compile and execute the program entry flow through the Etas runtime.
```

Shape:

```text
etas run app.es
etas run app.es --dry-run
etas run app.es --trace-out trace.json
etas run app.es --checkpoint-dir .etas/checkpoints
etas run app.es -- arg1 arg2
```

Initial options:

```text
--flow <name>
--dry-run
--allow-effects
--trace-out <path>
--checkpoint-dir <path>
--budget-tokens <n>
--budget-cost <amount>
--budget-time <duration>
```

Early implementations should require `--dry-run` or default to dry-run. Real
side effects should require an explicit authorization path such as
`--allow-effects`, and the runtime must still enforce policy.

The user entry ABI is `main(args: Array[string]) -> i32`. Positional arguments
after `--` are CLI arguments for `main`, not compiler input files, and the CLI
passes them to `etas_interpreter` as an `Array[string]`. The CLI must not
construct a `List[string]` for entry arguments.

### 6.9 `etas replay`

Purpose:

```text
Replay a recorded trace for debugging and deterministic investigation.
```

Shape:

```text
etas replay trace.json
etas replay trace.json --until <event-id>
```

This command can be deferred until trace schemas stabilize.

### 6.10 `etas resume`

Purpose:

```text
Resume a checkpointed runtime session.
```

Shape:

```text
etas resume <checkpoint-id>
etas resume <checkpoint-id> --checkpoint-dir .etas/checkpoints
```

This command belongs after checkpoint persistence has a stable runtime model.

### 6.11 `etas repl`

Purpose:

```text
Interactive exploration of types, effects, graphs, and dry-run execution.
```

Initial REPL commands:

```text
:type <expr>
:effects <expr-or-flow>
:graph <flow>
:run --dry-run <flow>
:dump ast
:dump hir
```

`etas repl` should be treated as a client of the same driver APIs as the rest
of the CLI, not as a separate evaluator.

### 6.12 `etas lsp`

Purpose:

```text
Start the first-party LSP server from the main `etas` binary.
```

Shape:

```text
etas lsp
etas lsp --stdio
```

This command delegates to `etas_lsp`. Protocol state and LSP handlers remain
owned by `etas_lsp`.

## 7. Internal Module Layout

Recommended `etas_cli` layout:

```text
crates/etas_cli/
  Cargo.toml
  src/
    main.rs
    lib.rs

    args/
      mod.rs
      global.rs
      check.rs
      dump.rs
      graph.rs
      run.rs
      repl.rs
      lsp.rs

    command/
      mod.rs
      check.rs
      dump_ast.rs
      dump_hir.rs
      dump_air.rs
      graph.rs
      effects.rs
      policy.rs
      run.rs
      replay.rs
      resume.rs
      repl.rs
      lsp.rs

    output/
      mod.rs
      diagnostic.rs
      json.rs
      text.rs
      graph.rs

    config/
      mod.rs
      file.rs
      discovery.rs

    workspace/
      mod.rs
      input.rs
      files.rs

    exit.rs
    error.rs
```

Responsibilities:

| Area | Responsibility |
|---|---|
| `args` | `clap` data model, command names, flags, defaults, help text |
| `command` | One module per command; converts parsed args into driver calls |
| `output` | Human and machine rendering for diagnostics and artifacts |
| `config` | Config file discovery and CLI override merging |
| `workspace` | Input path normalization and workspace root discovery |
| `exit` | Stable exit-code mapping |
| `error` | CLI-local errors such as invalid flags or unreadable config |

`main.rs` should stay small:

```rust
fn main() {
    std::process::exit(etas_cli::run());
}
```

`lib.rs` should expose a testable `run_with(args, io)` style API so CLI smoke
tests do not need to spawn a process for every case.

## 8. Driver Boundary

`etas_driver` should expose session-backed APIs such as:

```rust
check(project_input, options) -> CheckReport
dump_ast(source_input, options) -> DumpArtifact
dump_hir(project_input, options) -> DumpArtifact
dump_air(input, options) -> DumpArtifact
graph(input, options) -> GraphArtifact
run(input, options) -> RunReport
replay(trace, options) -> ReplayReport
resume(checkpoint, options) -> RunReport
```

`etas_cli` should not build those reports manually. It should only pass inputs
and render results.

Driver input normalization should produce the same project compilation unit for
CLI, integration tests, and future CI/build tooling:

```rust
pub struct DriverProjectInput {
    pub package_root: PathBuf,
    pub source_root: PathBuf,
    pub sources: Vec<SourceInput>,
    pub entry: ProjectEntry,
}
```

The driver should create and own `FrontendSession` instances. Batch commands
such as `check`, `run`, `dump hir`, and `graph` may create short-lived sessions;
`watch` should keep one session alive across file events. The driver must not
call per-file `Frontend::check`, and it should not use `Frontend::check_project`
as the primary path.

Recommended driver flow:

```text
DriverProjectInput + DriverOptions
  -> FrontendSessionOptions
  -> FrontendSession::with_options
  -> open_project
  -> check(CheckRequest { mode, cache_access })
  -> command-specific report
```

This keeps the compiler pipeline reusable by:

- CLI commands;
- integration tests;
- golden-test harnesses;
- future CI adapters;
- future package/build tooling.

## 9. Diagnostics And Exit Codes

Exit codes:

```text
0  success
1  diagnostics contain errors
2  invalid CLI usage or config error
3  runtime failure or policy denial
4  internal compiler error
```

Diagnostic rendering:

- Human output should use source snippets, line and column labels, severity,
  stable diagnostic codes, notes, and suggestions.
- JSON output should use the shared diagnostic schema.
- Parser, HIR, type, effect, policy, and runtime diagnostics should preserve
  source spans whenever possible.

CLI-local errors should be clearly separated from compiler diagnostics. For
example, an invalid `--format` value is a CLI usage error; an undeclared effect
inside a `.es` file is a compiler diagnostic.

## 10. Implementation Scope

Initial implementation:

- `etas check <file>`;
- `etas dump ast <file>`;
- `etas dump hir <file>`;
- `etas dump air <file>`;
- `etas graph <file>`;
- `etas run <file> --dry-run`;
- `etas lsp`;
- human diagnostics;
- JSON diagnostics;
- stable exit codes.

Deferred:

- package manager commands;
- formatter commands;
- `etas new`;
- dependency fetching;
- real checkpoint resume;
- real trace replay;
- full watch mode;
- install/update commands;
- shell completion generation;
- editor extension management.

The CLI should leave room for these features, but it should not design around
them before the compiler, AIR, and runtime authority model are stable.
