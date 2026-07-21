# Repository Organization

## 1. Purpose

This document records the target Git repository organization for Etas.

Etas is now planned in three implementation phases:

```text
Phase 1: Frontend + typed HIR interpreter
Phase 2: AIR v0 + runtime execution model
Phase 3: FIR middle-end + agent workflow optimization
```

Repository boundaries must follow those phases and the ownership boundaries in
`agents/Progress.md`, not the raw number of Rust crates.

## 2. Target Organization Shape

Use a GitHub organization such as `etas-lang`.

Target component repositories:

| Repository | Required | Phase | Responsibility |
|---|---:|---:|---|
| `etas-core` | Strongly recommended | 1 | Shared source/span/diagnostic primitives, ids, arenas, generic algorithms, std declarations, and shared IR contract types |
| `etas-frontend` | Yes | 1 | Syntax, AST, HIR, name resolution, type/effect checking, frontend diagnostics |
| `etas-interpreter` | Yes | 1-2 | AST/HIR interpreter, Phase 1 typed-HIR execution, lightweight AST/HIR analysis and local optimization |
| `etas-optimizing` | Yes | 2-3 | HIR-to-AIR v0 lowering, AIR verification, FIR, FIR analyses, optimization, FIR-to-AIR lowering |
| `etas-runtime` | Yes | 2 | AIR interpreter, runtime authority, host adapters, providers, tools, memory, sandbox, trace, checkpoint, replay |
| `etas-ide` | Yes | 1-2 | LSP server, editor intelligence, VFS, incremental snapshots, editor extensions, graph previews |
| `etas` | Yes | 1-3 | User-facing CLI and distribution entry point that depends on component facades to provide commands |

Phase 1 implementation scope is limited to:

```text
etas-core
etas-frontend
etas-interpreter
etas
```

`etas-optimizing`, `etas-runtime`, and `etas-ide` remain target
repositories, but they are outside the Phase 1 implementation path.

Optional repositories:

| Repository | When | Responsibility |
|---|---|---|
| `etas-rfcs` | When design traffic grows | RFCs, accepted decisions, governance records |
| `etas-website` | When docs publishing starts | Website, docs site, examples gallery |

`etas-core` is listed as strongly recommended rather than merely
optional because runtime and IDE both need shared contracts without depending on
the frontend implementation. If the team keeps exactly four repositories for a
short period, put core crates inside `etas-frontend` only as a temporary
bootstrap measure.

## 3. Repository Contents

### 3.1 `etas-core`

```text
crates/
  etas_core
  etas_utils
  etas_cache
  etas_std
  etas_builtin
  etas_host
  etas_air        # contract-only AIR data model after Phase 2 sharing starts
```

Ownership rules:

- `etas_core` owns source files, spans, line indexes, diagnostics, ids, arenas,
  interners, and result helpers.
- `etas_utils` owns generic `fixpoint`, `graph`, and pass-pipeline
  algorithms/patterns.
- `etas_cache` owns generic artifact keys, fingerprints, dependency graph
  primitives, invalidation sets, memory artifact store, and disk artifact store.
  It does not own frontend artifact semantics.
- `etas_std` owns standard declarations, intrinsic descriptors, support
  signatures, and documentation metadata.
- `etas_builtin` owns reusable pure intrinsic kernels described by
  `etas_std`; interpreter and runtime adapt their own value models into this
  shared layer.
- `etas_host` owns shared host protocol values and reusable host adapters for
  models, tools, typed memory, console, authority, trace, and budget boundaries.
- Shared `etas_air` owns runtime-facing AIR data structures and serialization
  only; AIR builders, verifiers, and optimizers belong in `etas-optimizing`.

### 3.2 `etas-frontend`

```text
crates/
  etas_syntax
  etas_hir
  etas_types
  etas_effects
  etas_frontend
```

Ownership rules:

- `etas_syntax` owns lexer, parser, AST, syntax diagnostics, and AST dump.
- `etas_hir` owns HIR, symbols, scopes, source maps, name resolution, and HIR
  dump.
- `etas_types` owns type representation, type inference/checking, generic
  instantiation, checked signatures, and typed facts.
- `etas_effects` owns effect declarations, action-aware effect inference,
  policy-facing action facts, interpreter support classification, and frontend safety
  diagnostics.
- `etas_frontend` is the public facade from `.es` source to typed HIR.
- Command routing belongs in the real `etas` CLI via `etas_driver`; the
  frontend repository does not own a separate CLI crate.

### 3.3 `etas-interpreter`

```text
crates/
  etas_interpreter
  etas_ast_interpreter
  etas_hir_interpreter
  etas_value
  etas_env
  etas_builtin_adapter
  etas_hir_light_analysis
  etas_hir_light_opt
```

Ownership rules:

- Interpreter consumes AST/HIR/typed-HIR public outputs from `etas-frontend`.
- Interpreter executes AST/HIR, not AIR.
- Interpreter may do lightweight AST/HIR-local analysis and optimization.
- Interpreter must reject or defer behavior that requires AIR/runtime authority.

### 3.4 `etas-optimizing`

```text
crates/
  etas_lowering
  etas_air_builder
  etas_air_verify
  etas_fir
  etas_fir_analysis
  etas_fir_opt
```

Ownership rules:

- Phase 2 starts with direct typed-HIR to AIR v0 lowering.
- Phase 3 adds FIR as the analysis and transformation IR.
- FIR owns optimization-time CFG/DFG-like semantic graph models.
- AIR verification belongs here, but AIR interpretation does not.

### 3.5 `etas-runtime`

```text
crates/
  etas_runtime
  etas_host
  etas_provider_openai
  etas_tool_host
  etas_memory_store
  etas_sandbox
```

Ownership rules:

- Runtime consumes checked AIR and host policy.
- Runtime is the AIR interpreter.
- Runtime mediates model calls, tools, memory, approvals, budgets, checkpoints,
  trace, replay, retries, and sandboxing.
- Runtime must not depend on parser, HIR, type checker, effect checker, FIR, or
  `etas-interpreter`.

### 3.6 `etas-ide`

```text
crates/
  etas_intel
  etas_lsp
  etas_editor_graph
editors/
  vscode
  zed
```

Ownership rules:

- `etas_lsp` is only the protocol adapter.
- `etas_intel` owns editor-facing queries over frontend facts and in-memory
  snapshots.
- IDE support may preview AIR/FIR graphs, but it must not execute real runtime
  effects.

### 3.7 `etas`

```text
crates/
  etas_cli
  etas_cli_commands
  etas_integration_tests
```

Ownership rules:

- `etas` is the real user-facing command-line repository.
- It depends on component repositories' public facades to provide commands.
- It must not own compiler, interpreter, optimizing, runtime, or IDE internals.

## 4. Dependency Direction

Allowed repository dependency direction:

```text
etas-core

etas-frontend
  -> etas-core

etas-interpreter
  -> etas-core
  -> etas-frontend

etas-optimizing
  -> etas-core
  -> etas-frontend

etas-runtime
  -> etas-core

etas-ide
  -> etas-core
  -> etas-frontend
  -> etas-optimizing       # optional, preview-only

etas
  -> etas-core
  -> etas-frontend
  -> etas-interpreter
  -> etas-optimizing       # Phase 2+
  -> etas-runtime          # Phase 2+
  -> etas-ide              # editor/LSP command surface only
```

Forbidden:

```text
etas-runtime -> etas-frontend
etas-runtime -> etas-optimizing
etas-runtime -> etas-interpreter
etas-interpreter -> etas-optimizing
etas-interpreter -> etas-runtime
etas-frontend -> etas-optimizing
etas-frontend -> etas-runtime
etas-optimizing -> etas-runtime
etas-ide -> etas-runtime     # except through an explicit dry-run/mock API
```

The runtime does not ask the compiler how to execute source-shaped programs.
The interpreter executes AST/HIR for the early source-shaped path. The
optimizing repository produces checked AIR contracts. The runtime executes AIR
under host authority.

## 5. Migration Order

Recommended migration sequence from the current `etas-frontend` repository:

1. Keep current Phase 1 work in `etas-frontend`.
2. Split `etas-core` when `etas_core`, `etas_utils`, and `etas_std`
   need to be consumed by both frontend and another repository.
3. Create `etas-interpreter` for AST/HIR interpretation once the Phase 1
   typed-HIR execution path needs separate ownership from frontend checking.
4. Move `etas_lsp` and `etas_intel` into `etas-ide` once LSP behavior
   becomes a real deliverable.
5. Create `etas-optimizing` at Phase 2 start for typed-HIR to AIR v0 lowering and
   AIR verification.
6. Create `etas-runtime` at Phase 2 start for AIR interpretation and host
   authority.
7. Create `etas` as the user-facing CLI once commands need to compose more
   than the frontend repository.
8. Expand `etas-optimizing` in Phase 3 with FIR, analyses, and optimization.

Do not split a crate merely because it exists. Split when a component has a
clear API, an owner, CI in isolation, and a release or integration cadence that
differs from the surrounding repository.

## 6. CI And Release Policy

Every component repository needs local checks:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Cross-repo integration must additionally test:

- frontend produces typed HIR accepted by optimizing;
- interpreter executes supported AST/HIR fixtures and rejects unsupported
  runtime-only effects explicitly;
- optimizing emits AIR accepted by runtime;
- runtime executes checked AIR with mock host adapters;
- IDE diagnostics match frontend diagnostics for the same source;
- no forbidden repository dependency appears in `Cargo.toml`.

Release policy:

- Release component repositories independently only after their public API is
  documented.
- Keep all versions pre-1.0 while cross-repo contracts are unstable.
- Runtime releases must declare compatible AIR contract versions.
- Interpreter releases must declare compatible frontend AST/HIR contract
  versions.
- IDE releases must declare compatible frontend diagnostic/query versions.

## 7. Current Recommendation

Current state:

- Phase 1 is in progress.
- `etas-frontend` already exists and contains bootstrap crates.
- `agents/Progress.md` records that repository splitting should follow stable
  ownership and API boundaries, not crate count.

Architect recommendation:

- Treat the current repository as `etas-frontend`.
- Keep core crates there only temporarily.
- Do not add interpreter, FIR, or real runtime ownership to `etas-frontend`.
- Plan `etas-core`, `etas-interpreter`, `etas-ide`, `etas-optimizing`,
  `etas-runtime`, and `etas` as separate repositories before Phase 2
  implementation starts.
