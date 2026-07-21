# Technical Selection

## 1. Purpose

This document records the initial technology choices for the Rust-based Etas
prototype.

Project architecture, workspace boundaries, implementation scope, and
engineering handoff are recorded separately in
`docs/architect/project-architecture.md`.

## 2. Primary Technology Choice

Use Rust as the implementation language.

Reasons:

- Etas is a language implementation with parser, checker, IR, analysis, and
  runtime components. Rust is a good fit for explicit data models and compiler
  pipelines.
- The runtime must enforce authority boundaries around tools, memory,
  approvals, traces, budgets, and sandbox profiles. Rust's ownership model and
  explicit error handling help keep these boundaries visible.
- AIR and trace data should be deterministic, serializable, and testable.
  Rust's type system is useful for keeping AIR node kinds, effects,
  capabilities, and values structured.
- The project needs a solid CLI and local runtime before it needs distributed
  execution or generated SDKs.

Recommended baseline:

```text
Rust stable
Cargo workspace
Edition 2024 unless a lower MSRV forces Edition 2021
rustfmt and clippy in CI once a repository baseline exists
```

## 3. Dependency Choices

Initial recommended dependencies:

| Area | Choice | Notes |
|---|---|---|
| CLI | `clap` | For `etas check`, `etas graph`, `etas run`, `etas repl`, and later commands |
| LSP server | `async-lsp`, `lsp-types` | For JSON-RPC/LSP transport, request lifecycle, cancellation, and protocol data types |
| Parser | `chumsky` | Good for fast grammar iteration and error recovery |
| Diagnostics | `ariadne` | Source-span diagnostics for parser/checker errors |
| Serialization | `serde`, `serde_json` | AIR, trace, and golden-test artifacts |
| Async runtime | `tokio` | Model calls, tool calls, approval wait, checkpoint I/O, and concurrent joins |
| HTTP client | `reqwest` | Hidden behind provider adapters |
| Structured logging | `tracing` | Internal Rust logs, not a substitute for Etas semantic traces |
| Temporary storage | file JSON / JSONL | Good enough for Phase 0 and Phase 1 |
| Durable local storage | `redb` later | Candidate for Phase 4 checkpoints, trace indexes, and memory versions |
| Lossless syntax tree | `rowan` later | Useful for LSP and formatting once syntax contracts stabilize |
| Incremental analysis | `salsa` later | Useful after syntax and type model stabilize |

Avoid committing to heavy frameworks before AIR and the checker stabilize.

## 4. Timing Guidance

Use immediately:

- `clap`;
- `serde`;
- `serde_json`;
- `chumsky`;
- `ariadne`;
- `tracing`.

Use immediately for the LSP slice:

- `async-lsp`;
- `lsp-types`.

Use when runtime I/O begins:

- `tokio`;
- `reqwest`.

Evaluate after the first checked AIR slice:

- `redb`;
- `rowan`;
- `salsa`.

## 5. Non-Goals For Initial Selection

The first executable slice should not select technology for:

- native code generation;
- distributed runtime scheduling;
- formatter implementation;
- persistent production database;
- OS-level or Wasm-level command sandboxing;
- provider-specific routing beyond a stable adapter interface.

The initial LSP server is in scope, but advanced LSP features are not. Defer:

- rename;
- formatting;
- code actions;
- full workspace indexing;
- protocol or flow graph visualizers inside editors.

These areas should remain behind abstractions until the core compiler, AIR, and
runtime boundaries are proven.
