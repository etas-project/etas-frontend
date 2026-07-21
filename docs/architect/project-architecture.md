# Project Architecture

## 1. Purpose

This document records the initial project architecture for the Rust-based Etas
prototype.

Concrete dependency choices are recorded separately in
`docs/architect/technical-selection.md`.

CLI module design is recorded separately in
`docs/architect/etas-cli-design.md`.

Test support architecture is recorded separately in
`docs/architect/etas-test-design.md`.

Core foundation design is recorded separately in
`docs/architect/etas-core-design.md`.

Generic utility algorithm design is recorded separately in
`etas-core/docs/architect/etas-utils-design.md`.

Standard library architecture is recorded separately in
`docs/architect/etas-std-design.md`.

Repository and GitHub organization split guidance is recorded separately in
`docs/architect/repository-organization.md`.

## 2. Architecture Position

Etas should be staged as:

```text
Phase 1: Frontend + typed HIR interpreter
Phase 2: AIR v0 + runtime execution model
Phase 3: FIR middle-end + agent workflow optimization
```

The first implementation should not target native code generation. Phase 1
should focus on frontend correctness, typed HIR, and local deterministic
execution. Phase 2 should add AIR v0 and the runtime authority boundary. Phase 3
should add FIR only after AIR execution semantics are stable enough to optimize.

Etas's main complexity is not CPU execution. It is the checked mediation of:

- model calls;
- tool dispatch;
- effect checking;
- action-boundary and deployment-grant mediation;
- policy enforcement;
- approval gates;
- runtime-scoped effect handlers;
- checkpoint and resume;
- trace logging;
- memory versioning;
- prompt construction and taint checks;
- token, context, cost, and time budgets.

These concerns belong behind a runtime authority boundary. Interpreting checked
AIR lets the runtime mediate every meaningful operation.

## 3. Compiler and Runtime Pipeline

The Phase 1 implementation pipeline should follow:

```text
project source set
  -> parse every .es source
  -> module index and package/module resolution
  -> project HIR
  -> project type/effect facts
  -> typed HIR interpreter
```

The Phase 2 implementation pipeline should follow:

```text
.es source
  -> typed HIR
  -> AIR v0
  -> AIR interpreter / runtime
```

The Phase 3 implementation pipeline should follow:

```text
.es source
  -> typed HIR
  -> FIR
  -> optimized FIR
  -> AIR
  -> runtime
```

The runtime sequence should follow:

1. Parse and type-check the program.
2. Resolve the entry flow `main(args: Array[string]) -> i32`.
3. Summarize reachable effects, capabilities, policies, limits, and handlers
   from `main`.
4. In Phase 1, execute `main` through the typed HIR interpreter.
5. In Phase 2+, verify effects and capabilities against host policy.
6. In Phase 2, lower `main` to AIR v0.
7. In Phase 3, lower through FIR before AIR.
8. Execute AIR through the runtime.
9. Emit trace events, metrics, checkpoints, and the final process status code.

`main` is the program entry flow. It may call, select, compose, and invoke flow
values internally, but it returns an `i32` status code. The runtime does not
first evaluate `main` to obtain a second entry flow.

## 3.1 Project-Level Compilation

Etas source compilation is project-level, not single-file. A single `.es`
file may be accepted as a convenience source set, but it must go through the
same project pipeline as multi-file packages.

The frontend/driver compilation unit is:

```text
Package root
  -> Source root
  -> SourceSet
  -> ParsedSourceSet
  -> ModuleIndex
  -> UnitTree
  -> HirProgram with HirModule*
  -> TypeFacts / EffectFacts
  -> CheckedProject
```

Required architecture rules:

- module and import paths are logical module paths from the language SPEC;
- package/source discovery and file path mapping happen before HIR import
  resolution;
- the module resolver maps `foo.bar` to `src/foo/bar.es` or
  `src/foo/bar/mod.es`;
- module declarations must match resolved logical paths;
- multiple source files may contribute `ModulePart`s to the same logical module;
- ambiguous `bar.es` versus `bar/mod.es` canonical mappings are rejected;
- duplicate top-level item names inside one logical module are rejected;
- `public import` contributes re-export metadata;
- wildcard imports only expose public names and are checked for ambiguity at
  use sites;
- CLI, tests, LSP, and future build tooling should share the same project
  frontend/driver APIs instead of building separate per-file behavior.

## 4. Multi-Repository Layout

The project should move from "one workspace containing every crate" to several
Git repositories, each with its own internal Cargo workspace. Repository
boundaries should match the three implementation phases and the independent
developer-experience surface.

Minimum required component repositories:

| Repository | Phase pressure | Responsibility |
|---|---:|---|
| `etas-frontend` | 1 | Source parsing, AST, HIR, name resolution, type/effect checking, frontend diagnostics |
| `etas-optimizing` | 2-3 | AIR v0 construction and verification in Phase 2; FIR, analysis, optimization, and FIR-to-AIR lowering in Phase 3 |
| `etas-interpreter` | 1-2 | AST/HIR interpreter, Phase 1 typed-HIR execution, lightweight AST/HIR analysis and local optimization |
| `etas-runtime` | 2 | AIR interpreter, runtime authority, host adapters, provider/tool/memory/sandbox integrations, budgets, trace/checkpoint/replay |
| `etas-ide` | 1-2 | LSP server, editor intelligence, VFS, incremental query cache, editor extensions, graph previews |
| `etas` | 1-3 | User-facing CLI and distribution entry point that depends on component facades to provide commands |

Phase 1 implementation scope is limited to:

```text
etas-core
etas-frontend
etas-interpreter
etas
```

`etas-optimizing`, `etas-runtime`, and `etas-ide` remain target
repositories, but they are outside the Phase 1 implementation path.

Strongly recommended shared repository:

| Repository | Responsibility |
|---|---|
| `etas-core` | Semantics-light shared contracts: source/span/diagnostics, ids, arenas, generic algorithms, standard-library declarations, and stable IR contract types consumed by more than one component |

The core repository is not bureaucracy. Without it, either runtime must depend
on frontend for shared types, or frontend must own runtime-facing AIR contracts.
Both directions are wrong. If the team insists on exactly four repositories
temporarily, keep core crates inside `etas-frontend` only until
`etas-runtime` starts consuming them, then split `etas-core` first.

Recommended crate ownership:

| Repository | Crates |
|---|---|
| `etas-core` | `etas_core`, `etas_utils`, `etas_cache`, `etas_std`, `etas_builtin`, `etas_host`, contract-only `etas_air` types once AIR is shared |
| `etas-frontend` | `etas_syntax`, `etas_hir`, `etas_types`, `etas_effects`, `etas_frontend`, frontend tests |
| `etas-optimizing` | `etas_fir`, `etas_fir_analysis`, `etas_fir_opt`, `etas_lowering`, `etas_air_builder`, `etas_air_verify`, optimizing tests |
| `etas-interpreter` | `etas_interpreter`, `etas_ast_interpreter`, `etas_hir_interpreter`, `etas_hir_light_analysis`, `etas_hir_light_opt`, interpreter tests |
| `etas-runtime` | `etas_runtime`, runtime-specific provider/tool/memory/sandbox backend wiring, runtime tests |
| `etas-ide` | `etas_intel`, `etas_lsp`, editor extensions, editor graph/trace preview support, IDE smoke tests |
| `etas` | `etas_cli`, release packaging, examples, cross-repository integration tests |

Current crate relocation guidance:

| Current crate | Target repository | Note |
|---|---|---|
| `etas_core` | `etas-core` | Shared primitives; not frontend-owned long term |
| `etas_utils` | `etas-core` | Generic `fixpoint`, `graph`, and pass-pipeline algorithms/patterns |
| `etas_syntax` | `etas-frontend` | Lexer, parser, AST, AST dump |
| `etas_hir` | `etas-frontend` | HIR, symbols, scopes, source map, name resolution |
| `etas_types` | `etas-frontend` | Type representation, inference/checking, checked signatures, typed facts |
| `etas_effects` | `etas-frontend` initially | Effect declarations, action-aware inference, policy-facing action facts, interpreter support classification; later exposes checked effect facts to optimizing/runtime |
| `etas_builtin` | `etas-core` | Shared pure intrinsic kernels used by interpreter and future runtime |
| `etas_air` | split contract from builder | AIR data model belongs in shared contracts once runtime consumes it; AIR construction belongs in `etas-optimizing` |
| `etas_lsp` | `etas-ide` | Protocol adapter only |
| `etas_intel` | `etas-ide` | Editor query service over frontend facts |
| `etas_cli` | `etas` | True user-facing CLI; routes through `etas_driver`, not frontend-local bootstrap commands |
| `etas_tests` | split by repository | Each repository owns local tests; an optional integration repository owns cross-component acceptance tests |

Phase ownership:

| Phase | Owning repositories |
|---|---|
| Phase 1: Frontend + typed HIR interpreter | `etas-core`, `etas-frontend`, `etas-interpreter`, early `etas-ide`, `etas` |
| Phase 2: AIR v0 + runtime execution model | `etas-core`, `etas-frontend`, `etas-optimizing`, `etas-runtime`, `etas-ide`, `etas` |
| Phase 3: FIR middle-end + agent workflow optimization | `etas-optimizing` becomes the primary owner of analysis and transformation before AIR |

The Phase 1 typed-HIR interpreter is a bootstrap execution path owned by
`etas-interpreter`. It consumes AST/HIR/typed-HIR structures produced by
`etas-frontend`. It does not execute AIR. The runtime repository is the AIR
interpreter and host-authority boundary.

## 5. Dependency Direction

Repository dependency direction must stay one-way.

```text
etas-core
  has no dependency on other Etas repositories

etas-frontend
  -> etas-core

etas-optimizing
  -> etas-core
  -> etas-frontend

etas-runtime
  -> etas-core
  consumes checked AIR contract types
  owns AIR interpretation, host authority, and runtime backends
  must not depend on etas-frontend, etas-optimizing, or etas-interpreter

etas-interpreter
  -> etas-core
  -> etas-frontend
  consumes AST/HIR/typed-HIR frontend outputs
  must not depend on etas-optimizing or etas-runtime

etas-ide
  -> etas-core
  -> etas-frontend
  may depend on etas-optimizing for AIR/FIR preview metadata
  must not depend on etas-interpreter or etas-runtime for real effect execution

etas
  -> etas-core
  -> etas-frontend
  -> etas-interpreter
  -> etas-optimizing   # Phase 2+
  -> etas-runtime      # Phase 2+
  -> etas-ide          # editor/LSP command surface only
  user-facing CLI and release wrapper only
```

Layer responsibilities:

| Layer | Repository | Responsibility |
|---|---|---|
| Core | `etas-core` | Shared primitives, generic algorithms, std declarations, and stable cross-repo contracts |
| Frontend | `etas-frontend` | Parse, resolve, type/effect check, and produce typed HIR |
| Optimizing | `etas-optimizing` | Lower typed HIR to AIR v0 in Phase 2; introduce FIR analysis/optimization in Phase 3 |
| Interpreter | `etas-interpreter` | Execute AST/HIR/typed HIR, run lightweight AST/HIR-local analysis and optimization |
| Runtime | `etas-runtime` | Execute checked AIR and mediate every external effect through host authority |
| IDE support | `etas-ide` | LSP/editor protocol, incremental snapshots, frontend-backed code intelligence |
| User CLI | `etas` | User-facing command surface and distribution entry point |

Important boundary rules:

- `etas-interpreter` consumes AST/HIR/typed-HIR frontend outputs. It is allowed
  to depend on frontend public APIs because AST/HIR is its execution domain.
- `etas-interpreter` must not consume AIR, execute AIR, or mediate real
  external effects. Effectful behavior that requires AIR/runtime authority must
  be rejected or routed through the later `etas` CLI pipeline.
- `etas-runtime` consumes checked AIR and host policy. It is the AIR
  interpreter and external-effect authority boundary.
- `etas-optimizing` may depend on frontend output types, but the frontend must not
  depend on FIR or middle-end optimization.
- `etas-interpreter` may do lightweight AST/HIR-local analysis and
  optimization, such as constant folding for pure local expressions,
  deterministic branch pruning with already-known constants, simple dead local
  elimination, HIR evaluation-plan normalization, and interpreter-only cache
  planning. It must not replace `etas-optimizing` as the home for HIR-to-AIR
  lowering, FIR, whole-flow rewrites, policy proofs, prompt/context
  optimization, or cross-flow analysis.
- `etas-ide` is an adapter and query service. It may cache and query frontend
  facts, but it must not own compiler semantics.
- `etas` is the only real user-facing CLI repository. It should route to
  repository-local facades instead of becoming a monolithic owner of
  compilation, interpretation, runtime authority, or IDE semantics.
- Test support splits by ownership: local fixture/golden tests live with the
  owning repository; cross-repo acceptance tests live in an integration gate.

## 6. LSP Architecture

Etas should ship a first-party LSP server as part of the early developer
experience.

External editors such as VS Code, Zed, Neovim, Helix, and other LSP clients
connect to `etas_lsp`. Etas does not build those editors.

```text
external editor / LSP client
  -> etas_lsp
      - initialize / shutdown
      - didOpen / didChange / didSave
      - hover / completion / definition / diagnostics routing
      - client capability negotiation
      - request cancellation
      - LSP type conversions
  -> etas_intel
      - virtual file snapshots
      - parse cache
      - HIR and symbol queries
      - type and effect diagnostics
      - optional AIR/FIR preview data after Phase 2/3
      - hover, completion, definition, symbols, semantic tokens
  -> etas-frontend facts
  -> optional etas-optimizing preview APIs
```

Initial LSP features:

1. Diagnostics:
   - parser diagnostics;
   - name resolution diagnostics;
   - type and effect diagnostics;
   - safety diagnostics for loop limits, prompt taint, approval dominance, and
     command sandbox requirements.
2. Document symbols for `type`, `enum`, `flow`, `agent`, `tool`,
   top-level `let` resource handles, `policy`, and `effect`.
3. Hover for inferred type, flow effect row, agent input/output schema, tool
   requirements, and policy references.
4. Go to definition for local symbols, imports, type refs, flows,
   agents, tools, resource handles, memory region support types, effects, and
   policies.
5. Completion for keywords, visible symbols, local bindings, effects, and
   common runtime support values.
6. Semantic tokens for declarations, type names, effects, capabilities,
   policies, agents, tools, and flows.

LSP commands may expose Etas-specific actions by phase:

```text
etas.showEffects
etas.explainDiagnostic
etas.exportAir       # Phase 2+
etas.showFlowGraph   # Phase 2+ AIR, Phase 3 FIR
etas.runDryRun       # Phase 2+ only through explicit dry-run/mock runtime API
```

Any LSP-triggered run command must default to dry-run behavior. The LSP server
must not execute real model calls, tools, file writes, commands, payments, or
network side effects without an explicit runtime authorization path.

## 7. Project File Tree

The target shape is multiple Git repositories. Each repository may still use a
Cargo workspace internally.

```text
etas-core/
  Cargo.toml
  crates/
    etas_core/
    etas_utils/
    etas_std/
    etas_builtin/          # shared pure builtin kernels
    etas_air/              # contract-only AIR data model once shared
  docs/
    architect/
      etas-core-design.md
      etas-utils-design.md
      etas-std-design.md
      etas-builtin-design.md

etas-frontend/
  Cargo.toml
  crates/
    etas_syntax/
    etas_hir/
    etas_types/
    etas_effects/
    etas_frontend/         # facade: source -> typed HIR
  examples/
  docs/
    design/
    architect/
      project-architecture.md
      etas-syntax-design.md
      etas-hir-design.md
      etas-types-design.md
      etas-effects-design.md
      etas-cli-design.md
  tests/
    fixtures/
      syntax/
      hir/
      typecheck/
      effects/

etas-optimizing/
  Cargo.toml
  crates/
    etas_lowering/         # Phase 2 direct typed-HIR -> AIR v0
    etas_air_builder/
    etas_air_verify/
    etas_fir/              # Phase 3
    etas_fir_analysis/     # Phase 3
    etas_fir_opt/          # Phase 3
  tests/
    fixtures/
      air/
      fir/
      optimization/

etas-interpreter/
  Cargo.toml
  crates/
    etas_interpreter/      # AST/HIR interpreter facade
    etas_ast_interpreter/  # optional early AST execution/debug path
    etas_hir_interpreter/  # Phase 1 typed-HIR execution
    etas_value/
    etas_env/
    etas_builtin_adapter/  # adapts interpreter values to core etas_builtin
    etas_hir_light_analysis/
    etas_hir_light_opt/    # lightweight AST/HIR-local optimization only
  tests/
    fixtures/
      ast_interpret/
      hir_interpret/
      hir_light_analysis/
      hir_light_opt/

etas-runtime/
  Cargo.toml
  crates/
    etas_runtime/
    etas_host/
    etas_provider_openai/
    etas_tool_host/
    etas_memory_store/
    etas_sandbox/
  tests/
    fixtures/
      dry_run/
      trace/
      checkpoint/
      replay/

etas-ide/
  Cargo.toml
  crates/
    etas_intel/
    etas_lsp/
    etas_editor_graph/
  editors/
    vscode/
    zed/
  tests/
    fixtures/
      lsp/
      completion/
      hover/
      semantic_tokens/

etas/
  README.md
  Cargo.toml
  crates/
    etas_cli/              # true user-facing CLI
    etas_cli_commands/
    etas_integration_tests/
  examples/
  docs/
    user/
    install/
    release/
  packaging/
    homebrew/
    cargo/
    github-release/
```

Optional governance and publishing repositories:

```text
etas-rfcs/
  rfcs/
  accepted/
  decisions/

etas-website/
  docs/
  examples/
  site/
```

`etas` is not an implementation monorepo. It is the user-facing distribution
and CLI repository. It may depend on other repositories' public facades to
provide commands, but it must not become the owner of compiler, interpreter,
runtime, optimizing, or IDE internals.

The integration test crate in `etas` is useful once cross-repository CI
exists, but it must test public behavior instead of reaching into private
module internals.

## 8. Phase 1 Scope

The first implementation-ready slice is Phase 1: frontend plus typed-HIR
interpreter. Phase 1 is an execution-boundary decision, not a reduced language
design. Shared data structures, registries, syntax models, HIR, and type/effect
facts should be shaped for the complete PL design, even when some execution
paths produce runtime-required diagnostics.

It should include:

1. `.es` parser and AST coverage for modules, imports, types, enums, flows,
   tools, agents, top-level `let`, policy blocks, effect actions, pipeline
   syntax, anonymous flows, and `main`. Persistent memory is covered as
   ordinary type declarations, standard support types, top-level resource
   handles, and API calls, not as a source-level `memory` item.
2. Type checking for PL primitive types, records, enums, lists, maps, sets,
   options, results, trust/provenance wrappers, prompt/message/schema support
   types, and arrow flow types such as `I -> O` and `I -> O ![E]`.
3. Flow determinism classification:
   - `flow` is the only user-defined callable declaration;
   - deterministic flows may lower to direct internal functions;
   - non-deterministic flows require runtime mediation;
   - agent calls request `Agentic.infer[A]` runtime action facts and Agentic
     runtime support, but do not add `Agentic` to ordinary escaping effect rows.
4. Effect declaration, effect extension, effect inference, and effect checking.
5. Tool boundary, requested-action, policy, and deployment-grant-facing facts.
6. Typed HIR model that stores resolved names, inferred types, effect facts,
   source maps, and diagnostics.
7. Typed-HIR interpreter for deterministic local execution only.
8. Explicit rejection diagnostics for Phase 2 runtime behavior that cannot run
   through the typed-HIR interpreter.
9. Execution-readiness checks before typed-HIR execution:
   - undeclared effect rejection;
   - agentic loop limit requirement;
   - command tools require sandbox profile, with default sandbox applied;
   - high-impact effect requires approval dominance where policy says so;
   - untrusted data cannot flow into trusted prompt channels.
10. CLI commands:
    - `etas check <file>`;
    - `etas dump ast <file>`;
    - `etas dump hir <file>`;
    - `etas run <file> --hir --dry-run` for supported deterministic programs;
    - explicit unsupported errors for AIR/runtime/FIR commands.
11. LSP server with diagnostics, document symbols, hover, definition,
    completion, and semantic tokens backed by `etas_intel`.
12. Dev-only `etas_test` helpers for fixture loading, golden comparisons,
    diagnostic assertions, CLI smoke tests, and mock host behavior.
13. `etas_core` primitives for source, span, diagnostic, id, arena, and
    interner infrastructure used across compiler, CLI, LSP, runtime, and tests.
14. `etas_utils` generic `fixpoint` framework, `graph` algorithms, and
    pass-pipeline orchestration available to type, effect, AIR, analysis,
    interpreter-planning, and runtime-preflight crates without embedding
    Etas-specific semantics.
15. `etas_std` declaration registry for primitive types, containers, result
    and error support, prompt/message/session support, trust wrappers,
    requirements, limits, core effects, approval, checkpoint, trace, and
    standard intrinsic signatures.

Phase 2 adds:

- shared AIR contract types stable enough for runtime consumption;
- typed-HIR to AIR v0 lowering;
- AIR verification;
- AIR JSON dump;
- dry-run AIR runtime;
- runtime authority checks, trace emission, checkpoint metadata, and mock host
  adapters.

Phase 3 adds:

- FIR data model;
- FIR construction from typed HIR;
- FIR analyses for dataflow, effects, prompt taint, approval dominance, cost,
  typed persistent-memory API access, and workflow structure;
- selected FIR optimization passes;
- optimized FIR to AIR lowering.

## 9. Deferred Features

Do not implement these in Phase 1 unless they become unavoidable:

- native code generation;
- full protocol/session type checking;
- declarative validator solving;
- AIR runtime execution beyond explicit unsupported diagnostics;
- FIR construction or optimization;
- distributed runtime backend;
- durable checkpoint resume across process restart;
- formatter;
- advanced LSP features such as rename, code actions, full workspace indexing,
  and editor-embedded graph visualization;
- optimizer passes such as context slicing, prompt partial evaluation,
  retrieval hoisting, verifier insertion, and agent call caching;
- provider-specific model routing beyond a stable adapter interface;
- real OS-level or Wasm-level command sandbox implementation.

The first version should still define diagnostics and typed facts carefully so
Phase 2 can lower to AIR without rediscovering source semantics.

## 10. Runtime Authority Boundary

The runtime owns every external effect.

Every effectful operation must pass through:

```text
effect declared?
policy allowed?
action grant available?
approval satisfied if required?
budget still available?
sandbox profile active if Command?
trace event emitted?
```

The LLM is not trusted to enforce policy. An agent may propose actions, but only
the runtime may authorize tool calls, typed persistent-memory API writes,
command execution, network access, approval results, and checkpoint mutation.

## 11. Testing Direction

The technical architecture should be testable from the beginning.

Minimum test categories:

- syntax positive fixtures;
- syntax negative fixtures with diagnostics;
- deterministic AST dump golden tests;
- HIR lowering, symbol, scope, resolution, and source-map tests;
- deterministic HIR dump golden tests;
- type checker positive and negative tests;
- effect inference golden tests;
- AIR export golden tests;
- safety negative tests for missing approval, prompt taint, unbounded loops,
  unauthorized typed persistent-memory API access, and command sandbox
  omissions;
- CLI smoke tests;
- `etas_intel` query tests for diagnostics, hover, completion, definition,
  symbols, and semantic tokens;
- LSP protocol smoke tests for initialize, didOpen, didChange, diagnostics, and
  basic request/response handling;
- runtime dry-run tests with mock model, mock tool, mock memory, trace capture,
  and budget simulation.

Examples in `etas/docs/design/07-examples.md` should eventually become
conformance fixtures, but they should not all be treated as first-day
executable behavior.

## 12. Initial Engineering Handoff

The current Engineer-owned work should treat `etas-frontend` as the Phase 1
repository and avoid adding runtime or FIR ownership there.

```text
etas-frontend/
  crates/etas_syntax
  crates/etas_hir
  crates/etas_types
  crates/etas_effects
  crates/etas_frontend
```

Until `etas-core` is split, shared crates may remain in
`etas-frontend`, but they must keep core boundaries:

```text
temporary in etas-frontend/
  crates/etas_core
  crates/etas_utils
  crates/etas_std
  crates/etas_builtin
```

The first milestone is not "run a real agent" and not "dump optimized AIR."
The first milestone is:

```text
etas check examples/smoke.es
etas check --workspace . --all
etas dump ast examples/smoke.es
etas dump hir examples/smoke.es
etas run examples/smoke.es --hir --dry-run
etas lsp initialize/didOpen smoke test
```

with deterministic diagnostics, deterministic HIR output, and project-level
module/import resolution. AIR export, runtime dry-run, checkpoint/replay, and
FIR optimization are Phase 2/3 handoffs, not Phase 1 frontend acceptance
criteria.
