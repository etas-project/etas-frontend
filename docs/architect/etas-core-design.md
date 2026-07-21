# Etas Core Design

## 1. Purpose

This document defines the architecture of `etas_core`.

`etas_core` is the small, stable foundation crate shared by compiler, CLI,
LSP, runtime, host, and tests. It should contain primitives that are useful
everywhere and know nothing about Etas language semantics.

It must not become a `common` dumping ground.

## 2. Boundary

`etas_core` owns:

- source identity and source files;
- byte offsets, text ranges, spans, line and column conversion;
- the shared diagnostic data model;
- text edits and diagnostic suggestions;
- generic typed id helpers;
- generic arena storage;
- string interning primitives;
- small result/error accumulation helpers.

`etas_core` does not own:

- lexer, parser, tokens, grammar, or AST nodes;
- HIR nodes, scopes, symbols, or source maps;
- type, effect, action, policy, or AIR models;
- CFGs, DFGs, effect graphs, or AIR graphs;
- abstract domains;
- generic graph algorithms;
- fixpoint algorithms;
- runtime traces, checkpoints, model providers, or tool adapters;
- CLI, LSP, or test harness behavior.

If a type needs to know what a `flow`, `agent`, `effect`, `policy`, `HIR node`,
or `AIR node` means, it does not belong in `etas_core`.

## 3. Crate Layout

Recommended file layout:

```text
crates/etas_core/
  Cargo.toml
  src/
    lib.rs
    source.rs
    span.rs
    line_index.rs
    diagnostic.rs
    id.rs
    arena.rs
    interner.rs
    result.rs
```

Module responsibilities:

| Module | Responsibility |
|---|---|
| `source` | `SourceId`, `SourceFile`, source path and text ownership |
| `span` | `TextSize`, `TextRange`, `Span` |
| `line_index` | `LineIndex`, `LineCol`, line range lookup |
| `diagnostic` | Shared diagnostic envelope, severity, labels, notes, suggestions, text edits |
| `id` | Generic typed-id helper traits/macros |
| `arena` | Generic typed arena and index storage |
| `interner` | String interning and interned names |
| `result` | Shared `EtasResult` and error accumulation helpers if needed |

## 4. Diagnostic Model

The diagnostic envelope belongs in `etas_core`:

```rust
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub phase: DiagnosticPhase,
    pub severity: Severity,
    pub message: String,
    pub primary: DiagnosticSpan,
    pub labels: Vec<DiagnosticLabel>,
    pub notes: Vec<String>,
    pub help: Option<String>,
    pub suggestions: Vec<Suggestion>,
}
```

The stable envelope should be shared by syntax, HIR, type checking, effect
checking, analysis, CLI rendering, LSP conversion, and tests.

There are two acceptable designs for diagnostic codes:

1. Keep a single shared `DiagnosticCode` enum in `etas_core` with variants for
   syntax, name, type, effect, and analysis codes.
2. Keep `DiagnosticCode` as a compact structured code wrapper and let each
   crate define its own typed code enum with conversions.

The first implementation may use the simpler shared enum. If code ownership
starts to become noisy, move to the structured wrapper later.

## 5. Ids And Arenas

`etas_core` should provide generic infrastructure:

```rust
pub trait Idx {
    fn from_u32(value: u32) -> Self;
    fn into_usize(self) -> usize;
}

pub struct Arena<Id, T> {
    // typed storage
}
```

Specific semantic ids remain in their owner crates:

```text
HirExprId       -> etas_hir
SymbolId        -> etas_hir
AirNodeId       -> etas_air
TypeVarId       -> etas_types
EffectVarId     -> etas_effects
RuntimeTaskId   -> etas_runtime
```

The arena type may live in `etas_core`; the domain ids do not.

## 6. Source Mapping Boundary

`SourceId`, `SourceFile`, `TextSize`, `TextRange`, `Span`, `LineIndex`, and
`LineCol` belong in `etas_core`.

HIR source maps do not:

```text
HirExprId -> SyntaxNodeRef
HirStmtId -> SyntaxNodeRef
HirTypeId -> SyntaxNodeRef
```

Those mappings know HIR ids and syntax node kinds, so they belong in
`etas_hir`.

Likewise, AIR source maps belong in `etas_air` or `etas_lowering`, depending
on whether they are stored as part of AIR or emitted as lowering metadata.

## 7. Dependency Rules

Allowed:

```text
etas_syntax   -> etas_core
etas_std      -> etas_core
etas_hir      -> etas_core
etas_types    -> etas_core
etas_effects  -> etas_core
etas_air      -> etas_core
etas_analysis -> etas_core
etas_runtime  -> etas_core
etas_cli      -> etas_core
etas_lsp      -> etas_core
etas_test     -> etas_core
```

Forbidden:

```text
etas_core -> etas_syntax
etas_core -> etas_std
etas_core -> etas_hir
etas_core -> etas_types
etas_core -> etas_effects
etas_core -> etas_air
etas_core -> etas_analysis
etas_core -> etas_runtime
etas_core -> etas_cli
etas_core -> etas_lsp
etas_core -> etas_test
```

`etas_core` should have few external dependencies. Prefer the Rust standard
library unless a dependency provides a clear, stable benefit.

Generic algorithms such as graph traversal, strongly connected components,
topological sorting, dominator computation, lattice operations, worklists, and
fixpoint engines belong in `etas_utils`, not in `etas_core`.

## 8. Migration Direction

Current implementation has core-like primitives in `etas_syntax` and
`etas_hir`.

Move these from `etas_syntax` to `etas_core`:

- `SourceId`;
- `SourceFile`;
- `TextSize`;
- `TextRange`;
- `Span`;
- `LineIndex`;
- `LineCol`;
- shared diagnostic envelope;
- `Severity`;
- `DiagnosticLabel`;
- `Suggestion`;
- `TextEdit`;
- `Applicability`.

Move these from `etas_hir` to `etas_core`:

- generic `Arena`;
- generic typed-id helper machinery.

Keep these in `etas_hir`:

- `HirModuleId`;
- `HirItemId`;
- `HirExprId`;
- `HirStmtId`;
- `HirPatId`;
- `HirTypeId`;
- `ScopeId`;
- `SymbolId`;
- HIR source maps.

The migration should be mechanical and should not change language behavior.
