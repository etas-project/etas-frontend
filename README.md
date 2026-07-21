# Etas Frontend

The compiler frontend for the Etas programming language.

This repository turns an explicit project source set and a resolved project
environment into checked HIR, diagnostics, semantic snapshots, and package
metadata. It owns language syntax and static semantics. It does not download
packages, interpret manifests or lockfiles, select host adapters, or execute a
program.

## Workspace Crates

| Crate | Responsibility |
|---|---|
| `etas_syntax` | Tokens, lexer, parser, AST, trivia/comments, syntax diagnostics, and AST dumps |
| `etas_hir` | HIR arenas, symbols, scopes, source maps, structured tree indexes/views, lowering, and traversal |
| `etas_hir_analysis` | Reusable intra/interprocedural HIR analysis context, semantic units, domains, and traversal integration |
| `etas_types` | Signature constraints, body constraints, solvers, nominal/alias/spec facts, and materialized type facts |
| `etas_effects` | Effect/action domains, handler semantics, interprocedural inference, trace-spec checking, and effect facts |
| `etas_frontend` | Project protocol, pass orchestration, `FrontendSession`, incremental invalidation, cache integration, snapshots, and checked outputs |

## Compilation Pipeline

The project pipeline is explicit and pass-managed. Its current phase order is:

```text
ProjectInput + ProjectEnvironmentInput
  -> build source set
  -> parse each SourceFile
  -> build ModuleIndex and ModuleCatalog
  -> resolve import targets without depending on HIR
  -> build semantic unit tree and import graph
  -> detect cycles and compute module order/affected modules
  -> predeclare symbols
  -> normalize imports and lower each ModulePart
  -> finalize project HIR and apply resolved imports
  -> resolve paths and the selected entry
  -> compute entry reachability
  -> collect/solve/materialize type facts
  -> infer/materialize effect and requested-action facts
  -> validate loop progress, interpreter support, and entry contract
  -> build CheckedProject and optional ProjectSemanticSnapshot
```

Project, source file, module part, item/body, block, and expression identities
remain distinct. Multiple source files may contribute `ModulePart` values to
one semantic module. HIR arenas are the canonical storage; `HirTreeIndex`,
`HirTreeView`, typed views, and visitors provide owner-aware structured access
without duplicating HIR data.

## Public Protocol

The main library entry point is `etas_frontend::FrontendSession`.

The frontend accepts:

- `ProjectInput`, including source files or caller-provided overlays;
- source-root and entry information supplied by the caller;
- `ProjectEnvironmentInput`, including resolved package identities, import
  roots, external package metadata, standard metadata, and fingerprints;
- check options controlling incremental reuse, disk artifacts, reachability,
  and semantic snapshot detail.

It returns diagnostics whether or not a checked project can be produced. A
successful static check can return `CheckedProject`; callers that request IDE
data can also receive `ProjectSemanticSnapshot` with syntax/HIR/source-map,
symbol, path, type, and effect information.

The frontend must not:

- search registries, clone Git repositories, or resolve versions;
- own `etas.toml`, `etas.lock`, or `.etas/packages` layout semantics;
- configure models, tools, memory providers, or network authority;
- execute checked HIR or lower it to AIR;
- own editor documents, cursors, debounce policy, or LSP transport state.

`etas_driver` and `etas_package` assemble the project environment for the CLI.
`etas_intel` owns editor overlays and the IDE's latest/last-good snapshot
policy while reusing the same frontend session protocol.

## Incremental Compilation

`FrontendSession` is an in-process compiler session, not a standalone server.
It owns revisions, dependency invalidation, in-memory artifacts, and optional
disk artifact reads/writes. Both CLI and IDE callers use the same API:

- one-shot CLI checks can disable incremental and disk reuse;
- repeated CLI or build operations can enable them;
- `etas_intel` keeps a long-lived session and applies changed overlays;
- the generic disk cache remains cross-process, while live session state is
  process-local.

Only stable, expensive, fingerprinted artifacts should be persisted. Open
documents, transient AST navigation state, active diagnostics, and editor
snapshot ownership remain in memory.

## Standard and Package Metadata

`etas_std` declarations are consumed from `etas-core`; the frontend does not
hardcode standard imports or synthesize fake source bodies. External packages
are consumed through versioned `.etasmeta` artifacts that preserve public
nominal identities, aliases, specs, signatures, effects, requested actions,
handlers, and source-independent definition identities.

Source and metadata modules share one module identity/catalog model. Import
resolution must distinguish exact modules, child modules, and exported items;
prefix matching is not a valid substitute.

## Build and Verify

The workspace requires Rust `1.85` or newer and access to
[`etas-core`](https://github.com/etas-project/etas-core). Committed dependencies
use Git; the top-level `etas` checkout documents sibling patch configuration.

```bash
cargo build --workspace
```

Standard verification:

```bash
cargo fmt --all -- --check
cargo test --workspace --offline
cargo clippy --workspace --offline -- -D warnings
```

Focused checks:

```bash
cargo test -p etas_syntax --offline
cargo test -p etas_hir --offline
cargo test -p etas_hir_analysis --offline
cargo test -p etas_types --offline
cargo test -p etas_effects --offline
cargo test -p etas_frontend --offline
```

Use the user-facing CLI for end-to-end project checks:

```bash
etas check path/to/main.es
etas check --all path/to/project
etas dump hir path/to/project --symbols --scopes --source-map
etas effects path/to/project
```

## Architecture Documents

- [Phase 1 frontend architecture](docs/architect/phase1-frontend-design.md)
- [Syntax and AST](docs/architect/etas-syntax-design.md)
- [HIR and structured views](docs/architect/etas-hir-design.md)
- [Type system](docs/architect/etas-types-design.md)
- [Effect and action analysis](docs/architect/etas-effects-design.md)
- [Standard package integration](docs/architect/etas-std-design.md)
- [Test architecture](docs/architect/etas-test-design.md)
- [Repository organization](docs/architect/repository-organization.md)

The normative language specification is maintained under
[`etas/docs/design`](https://github.com/etas-project/etas/tree/main/docs/design).

## License

Etas Frontend is distributed under the terms of both the
[MIT License](LICENSE-MIT) and the
[Apache License (Version 2.0)](LICENSE-APACHE). You may choose either license.
