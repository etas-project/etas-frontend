# Phase 1 Frontend Design

## 1. Purpose

Phase 1 `etas-frontend` owns the source-language implementation up to checked
HIR. It parses `.es`, resolves names, checks types/effects, and produces a
`CheckedProgram` contract consumed by `etas-interpreter`.

It does not interpret programs and does not execute runtime effects.

The language SPEC in `docs/design/` is the source of truth for frontend
semantics. Architecture and implementation must follow the SPEC's module,
import, package, entry-point, type, and effect rules rather than deriving
language behavior from the current implementation shape.

## 2. Crates

```text
etas-frontend/
  crates/
    etas_syntax/
    etas_hir/
    etas_types/
    etas_effects/
    etas_frontend/
```

The frontend repository is the only Phase 1 owner of source-language semantics.
The interpreter may consume frontend outputs, but it must not repair or invent
frontend facts. If a program cannot be resolved, typed, or classified by
effects, it should not reach the interpreter as a successful `CheckedProgram`.

### 2.1 `etas_syntax`

Owns:

- lexer, tokens, trivia, comments;
- parser and recovery;
- AST node definitions;
- spec declarations, spec satisfactions, type-parameter bounds, and `effect E`
  effect-row parameters as source syntax;
- syntax diagnostics;
- AST dump.

Output:

```text
SourceFile -> Parse<AstProgram>
SourceSet -> ParsedSource*
```

### 2.2 `etas_hir`

Owns:

- AST to HIR lowering;
- symbol table;
- scopes;
- name and path resolution;
- HIR nodes and symbols for specs, spec impls, marker impls, type parameters,
  and effect-row parameters;
- source map from HIR ids to syntax spans;
- HIR diagnostics and HIR dump.

Output:

```text
ParsedSource + ModuleIndex -> HirProgram + HirSourceMap + HirDiagnostics
ModuleIndex + UnitTree -> Project HirProgram
```

### 2.3 `etas_types`

Owns:

- type representation for values;
- type inference and type checking over HIR;
- generic instantiation, unification, and assignability checks;
- spec environment and spec satisfaction, including `Index`, `ByteStream`,
  `PromptEncode`, `Schema`, `ResponseDecode`, and `Limit`;
- effect-row kind checking for `effect E` parameters and row-polymorphic flow
  types;
- checked flow, agent, tool, method, and support-flow signatures;
- typed expression, pattern, symbol, and item facts;
- type diagnostics.

Output should be stored as facts keyed by HIR ids rather than mutating HIR into
an interpreter-specific representation.

Detailed design is recorded in `docs/architect/etas-types-design.md`.

### 2.4 `etas_effects`

Owns:

- effect declaration checking;
- effect row inference over typed HIR;
- effect-row polymorphic summary instantiation using typed generic facts;
- requested-action, policy, residual-check, limit, and support facts;
- Phase 1 interpreter execution-requirement classification;
- diagnostics for behavior that is statically invalid or missing required
  declarations for checked-HIR interpretation.

It should not enforce host authority. It only decides whether a checked program
is statically valid and records what action footprints, explicit handler facts,
host services, grants, limits, checkpoint support, and interpreter orchestration
features are required.

Detailed design is recorded in `docs/architect/etas-effects-design.md`.

### 2.5 `etas_frontend`

Frontend crate used by `etas`, `etas-interpreter`, `etas_intel`, tests, and
future build tooling.

The primary public entry point is `FrontendSession`. The older `Frontend`
facade may remain as a thin convenience wrapper for parser dumps, smoke tests,
and simple one-shot calls, but CLI, watch, IDE, and future daemon code should
create and configure a session explicitly.

Recommended public API shape:

```rust
pub struct Frontend;

impl Frontend {
    pub fn parse(&self, input: SourceInput) -> ParseOutput;
    pub fn lower(&self, parsed: &ParseOutput) -> HirOutput;
    pub fn check(&self, input: SourceInput) -> CheckOutput;
    pub fn check_project(&self, input: ProjectInput) -> ProjectOutput;
}

pub struct FrontendSession;

impl FrontendSession {
    pub fn with_options(options: FrontendSessionOptions) -> Result<Self, FrontendSessionError>;
    pub fn open_project(&mut self, input: ProjectInput) -> Result<ProjectSessionId, FrontendSessionError>;
    pub fn apply_changes(
        &mut self,
        project: ProjectSessionId,
        changes: ProjectChangeSet,
    ) -> Result<ChangeSummary, FrontendSessionError>;
    pub fn check(
        &mut self,
        project: ProjectSessionId,
        request: CheckRequest,
    ) -> Result<CheckResponse, FrontendSessionError>;
    pub fn snapshot(&self, project: ProjectSessionId) -> Option<ProjectSemanticSnapshot>;
}

pub struct ProjectInput {
    pub package_root: PathBuf,
    pub manifest: Option<ProjectManifestInput>,
    pub sources: Vec<SourceInput>,
    pub entry: ProjectEntry,
}

pub struct ProjectEntry {
    pub module: Option<ModulePath>,
    pub flow: String,
}

pub struct CheckOutput {
    pub checked: Option<CheckedProgram>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct ProjectOutput {
    pub checked: Option<CheckedProject>,
    pub diagnostics: Vec<Diagnostic>,
    pub sources: Option<SourceSet>,
    pub parsed_sources: Vec<ParsedSource>,
    pub modules: Option<ModuleIndex>,
    pub units: Option<UnitTree>,
    pub hir: Option<HirOutput>,
    pub types: Option<TypeOutput>,
    pub effects: Option<EffectOutput>,
}

pub struct CheckedProgram {
    pub sources: SourceBundle,
    pub hir: HirProgram,
    pub symbols: SymbolTable,
    pub scopes: ScopeTree,
    pub source_map: HirSourceMap,
    pub types: TypeFacts,
    pub effects: EffectFacts,
    pub interpreter_support: InterpreterSupportFacts,
    pub entry: Option<FlowId>,
}

pub type CheckedProject = CheckedProgram;
```

The exact Rust names can evolve, but other repositories should obtain checked
frontend output through `FrontendSession` or a wrapper that delegates to it.
They must not bypass the session to run private passes directly.

`check(SourceInput)` is a convenience API for tests, editor scratch buffers, and
single-file smoke checks. It must be implemented by wrapping the source in a
single-file `ProjectInput`. It must not define different semantics from
project-level session checking.

### 2.6 Command Surface

The frontend repository does not own a CLI. The temporary bootstrap command
surface has been removed now that the real `etas` CLI routes `check`,
`dump ast`, and `dump hir` through
`etas_driver -> etas_frontend::FrontendSession`.

Frontend debugging entry points should be unit/integration tests or the real
`etas` CLI. No production command should bypass `FrontendSession` or
`etas_driver`.

## 3. Dependency Direction

```text
etas_syntax
  -> etas_core

etas_hir
  -> etas_core
  -> etas_syntax

etas_types
  -> etas_core
  -> etas_utils
  -> etas_std
  -> etas_hir

etas_effects
  -> etas_core
  -> etas_utils
  -> etas_std
  -> etas_hir
  -> etas_types

etas_frontend
  -> etas_core
  -> etas_std
  -> etas_syntax
  -> etas_hir
  -> etas_types
  -> etas_effects
```

Forbidden:

```text
etas-frontend -> etas-interpreter
etas-frontend -> etas
etas-frontend -> etas-runtime
etas-frontend -> etas-optimizing
```

## 4. Interpreter Contract

The interpreter receives only checked frontend output. It must not reparse
source, rebuild scopes, or redo type/effect checking.

`CheckedProgram` requirements:

- stable HIR ids;
- source spans for diagnostics;
- resolved symbol references;
- typed expression and pattern facts;
- symbol type facts for params, locals, fields, flows, agents, and tools;
- checked flow signatures;
- effect, requirement, and interpreter support facts sufficient to reject
  unsupported Phase 1 behavior;
- entry-flow metadata.

Recommended output pipeline:

```text
ProjectInput
  -> SourceSet
  -> ParsedSource*
  -> ModuleIndex
  -> UnitTree
  -> HirOutput
  -> TypeOutput
  -> EffectOutput
  -> CheckedProject
```

The facade should preserve intermediate outputs for CLI dump commands without
forcing the CLI to call private crate APIs.

## 5. Project-Level Parsing And Compilation

The frontend must compile projects as package/module graphs, not as a loop over
independent files.

The current SPEC requires:

- `module` and `import` paths are logical module paths, not raw filesystem
  paths;
- a package resolver maps logical paths to source files by convention;
- `module foo.bar;` maps to `src/foo/bar.es` or `src/foo/bar/mod.es`;
- a file's `module` declaration must match the logical path chosen by the
  resolver;
- if both file forms define the same module, the package is ambiguous;
- Etas modules are logical modules and may be assembled from multiple source
  files as `ModulePart`s;
- relative filesystem imports such as `import "./writer.es";` are not part of
  the language;
- `public import` re-exports imported public names;
- wildcard imports only import public names and ambiguous wildcard-provided
  names are rejected at use sites.

### 5.1 Project Data Model

Project-level frontend state must separate source files, parsed AST, logical
modules, and semantic pass units. Files are parse and diagnostic containers;
modules are semantic containers.

```rust
pub struct SourceSet {
    pub package_root: PathBuf,
    pub source_root: PathBuf,
    pub files: Vec<SourceFile>,
}

pub struct SourceFile {
    pub id: SourceId,
    pub path: Option<PathBuf>,
    pub text: Arc<str>,
    pub line_index: LineIndex,
    pub kind: SourceKind,
}

pub enum SourceKind {
    PackageFile,
    SingleFileInput,
    VirtualStd,
    VirtualGenerated,
    LspOverlay,
}

pub struct ModulePath {
    pub segments: Vec<String>,
}

pub struct ParsedSource {
    pub source: SourceId,
    pub parse: Parse<ast::Program>,
    pub declared_module: Option<ModulePath>,
    pub imports: Vec<AstImportRef>,
    pub items: Vec<AstItemRef>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct ModuleIndex {
    pub modules: Arena<ModuleId, ModuleInfo>,
    pub parts: Arena<ModulePartId, ModulePart>,
    pub by_path: HashMap<ModulePath, ModuleId>,
    pub by_source: HashMap<SourceId, ModuleId>,
}

pub struct ModuleInfo {
    pub id: ModuleId,
    pub path: ModulePath,
    pub parts: Vec<ModulePartId>,
    pub origin: ModuleOrigin,
    pub visibility_exports: ExportTable,
    pub span: Span,
}

pub enum ModuleOrigin {
    Source,
    VirtualStd(ModulePath),
    Dependency {
        package: PackageId,
        module: ModulePath,
    },
}

pub struct ModulePart {
    pub id: ModulePartId,
    pub module: ModuleId,
    pub source: SourceId,
    pub declared_module_span: Option<Span>,
    pub imports: Vec<AstImportRef>,
    pub items: Vec<AstItemRef>,
}

pub struct AstItemRef {
    pub source: SourceId,
    pub module_part: Option<ModulePartId>,
    pub index: usize,
    pub kind: AstItemKind,
    pub span: Span,
}

pub struct AstImportRef {
    pub source: SourceId,
    pub module_part: Option<ModulePartId>,
    pub index: usize,
    pub span: Span,
}
```

`ParsedSource` preserves file-shaped AST order. `ModulePart` groups parsed
items into a logical module while keeping the original `SourceId` and span.
`ModuleInfo.parts` is the ordered set of source parts that contribute to the
same logical module.

Module merging rules:

- multiple source files may declare the same `module` path and contribute
  distinct `ModulePart`s to one `ModuleInfo`;
- file path conventions still determine which logical module a source file is
  allowed to declare;
- `src/foo/bar.es` and `src/foo/bar/mod.es` are ambiguous only when both are
  discovered as canonical roots for the same logical module path;
- imports are `ModulePart`-local by default: an import in one source file does
  not implicitly become available in another source file that shares the same
  logical module;
- top-level declarations are merged into the module export/signature namespace;
- duplicate top-level item names in the same logical module are diagnostics,
  even when the duplicates come from different source files;
- public top-level items and `public import` re-exports are aggregated into the
  module export table.

Pass scheduling and project diagnostics use a separate unit tree:

```rust
pub struct UnitTree {
    pub root: UnitId,
    pub nodes: Arena<UnitId, UnitNode>,
    pub by_target: HashMap<UnitTarget, UnitId>,
}

pub struct UnitNode {
    pub id: UnitId,
    pub kind: UnitKind,
    pub parent: Option<UnitId>,
    pub children: Vec<UnitId>,
    pub target: UnitTarget,
    pub source: Option<SourceId>,
    pub span: Option<Span>,
}

pub enum UnitKind {
    Project,
    SourceFile,
    Module,
    ModulePart,
    Item,
    Body,
    Block,
    Expression,
}

pub enum UnitTarget {
    Project(ProjectId),
    Source(SourceId),
    Module(ModuleId),
    ModulePart(ModulePartId),
    AstItem(AstItemRef),
    AstBody(AstBodyRef),
    HirModule(HirModuleId),
    HirItem(HirItemId),
    HirBlock(HirBlockId),
    HirExpr(HirExprId),
}
```

The semantic hierarchy is:

```text
Project -> Module -> ModulePart -> Item -> Body -> Block -> Expression
```

`SourceFile` is not the semantic parent of `Module`; it is the source location
owner. Every `ModulePart`, AST ref, and HIR source-map entry must retain enough
identity to report diagnostics as `SourceId + Span`.

`UnitTree` is not the AST and is not HIR. It is the compiler's semantic unit
index for pass scheduling, artifact caching, incremental invalidation, and
diagnostic anchoring. AST remains source-shaped; HIR remains arena/id-shaped.
The unit tree connects both shapes to a stable semantic hierarchy. HIR's arena
storage must be accessed through `HirTreeView` / `HirScopeView` when building
or consuming HIR semantic units; business passes must not rediscover bodies or
expressions by scanning the whole HIR arena set.

Example source:

```etas
module app.main;

import std.io.println;

flow main(args: Array[string]) -> i32 ![Error[IOError]] {
    let x = 1;
    if x > 0 {
        println("ok");
    }
    return 0;
}
```

The AST preserves file order:

```text
Program
  ModuleDecl(app.main)
  ImportDecl(std.io.println)
  Item::Flow(main)
    Block
      Stmt::Let
      Stmt::If
      Stmt::Return
```

HIR stores lowered ids:

```text
HirProgram
  HirModuleId(app.main)
  HirItemId(flow main)
  HirBlockId(flow body)
  HirExprId(x > 0)
  HirBlockId(if body)
  HirExprId(println("ok"))
```

HIR view exposes those ids as a structured tree:

```text
HirTreeView
  Module(app.main)
    Item(flow main)
      Body(flow main)
        Block(flow body)
          Stmt(let x)
          Stmt(if)
            Expr(x > 0)
            Block(if body)
              Stmt(expr println("ok"))
          Stmt(return 0)
```

The semantic unit tree provides the pass and diagnostic hierarchy:

```text
Project
  Module(app.main)
    ModulePart(source=app/main.es)
      Item(flow main)
        Body(flow main body)
          Block(flow body)
            Expression(x > 0)
            Block(if body)
              Expression(println("ok"))
```

Passes should use the unit tree for their execution granularity:

| Pass shape | Unit granularity |
|---|---|
| Source parsing | `SourceFile` |
| Module index building | `Project` |
| Import resolution and cycle detection | `Project` |
| Module HIR lowering | `Module` / `ModulePart` |
| Flow signature collection | `Item` |
| Type and effect body checking | `Body` |
| Local expression facts | `Expression` |

When a source edit changes only `println("ok")`, incremental invalidation can
walk from `Expression` to `Block`, `Body`, `Item`, and `Module` while keeping
unaffected modules and import graph artifacts valid. When a diagnostic is
reported on that call, renderers resolve `UnitNode.source + UnitNode.span`
through `SourceFile.line_index`.

### 5.2 Module Providers And Resolver

Module resolution should be isolated behind provider/resolver interfaces. Do
not scatter path mapping rules across the CLI, HIR lowering, type checker, and
tests.

```rust
pub trait ModuleProvider {
    fn resolve_module(&self, path: &ModulePath) -> ModuleResolution;
    fn exports(&self, module: ModuleId) -> ExportTable;
}

pub struct ProjectModuleResolver {
    providers: Vec<Box<dyn ModuleProvider>>,
}
```

Phase 1 providers:

```text
SourceModuleProvider
  current package .es files under the source root

StdModuleProvider
  virtual modules declared by etas_std

DependencyModuleProvider
  interface only at first; full package graph can land later
```

Resolution order:

1. current package source modules;
2. standard-library virtual modules;
3. dependency package modules when dependency metadata is available.

The resolver must return structured results for:

- resolved source module;
- resolved virtual std module;
- missing module;
- duplicate module;
- path/file mismatch;
- ambiguous file mapping;
- private item imported from another module;
- missing exported item;
- wildcard ambiguity.

### 5.3 Project Pass Pipeline

The frontend owns one project-first pipeline. Single-file checking is a wrapper
that creates a one-file `ProjectInput`; it must not run a separate semantic
pipeline.

The pipeline is organized by semantic unit granularity. Project passes see the
full project context. Adapter steps such as `ForEach(SourceFile)`,
`ForEach(ModulePart)`, and `ForEach(Body)` run a child pipeline over matching
units. `SourceFile` units are available from `SourceSet` before `UnitTree`
exists; `ModulePart`, `Item`, `Body`, `Block`, and `Expression` units come from
`UnitTree`.

```text
FrontendSession.check [Project]
  source/
    BuildSourceSetPass [Project]
    ForEach(SourceFile)
      ParseSourceFilePass [SourceFile]

  module/
    BuildModuleIndexPass [Project]
    BuildUnitTreePass [Project]

  graph/
    BuildImportGraphPass [Project]
    DetectImportCyclesPass [Project]
    ComputeModuleTopoOrderPass [Project]
    ComputeAffectedModulesPass [Project, incremental only]

  hir/
    PredeclareProjectSymbolsPass [Project]
    ForEach(ModulePart, topo order)
      NormalizeModuleImportsPass [ModulePart]
      LowerModuleItemsPass [ModulePart]
    ResolveImportsPass [Project]
    ResolvePathsPass [Project]

  analysis/
    BuildSignatureFactsPass [Project -> etas_types::SignaturePipeline]
    ForEach(Body)
      TypeCheckBodyPass [Body -> etas_types::BodyPipeline]
    RunEffectPipelinePass [Project]
    VerifyInterpreterSupportPass [Project]

  output/
    ResolveEntryPointPass [Project]
    BuildCheckedProjectPass [Project]
```

Required artifacts:

| Artifact | Owner | Meaning |
|---|---|
| `SourceSet` | source stage | Normalized input files, virtual std files, generated files, and LSP overlays |
| `ParsedSourceSet` | source stage | `ParsedSource` for every `SourceFile` |
| `ModuleIndex` | module stage | Logical modules, module parts, source-to-module mapping, duplicate and mismatch diagnostics |
| `UnitTree` | module stage | `Project -> Module -> ModulePart -> Item -> Body -> Block -> Expression` units |
| `ImportGraph` | graph stage | Module import edges, reverse import edges, and import source spans |
| `ModuleTopoOrder` | graph stage | Topological module order after cycle checks |
| `AffectedModuleSet` | graph stage | Incremental recompilation frontier when a previous snapshot exists |
| `HirProgram` | HIR stage | Project HIR containing every source, virtual, and dependency module needed for checking |
| `ResolvedImports` | HIR stage | Explicit imports, wildcard imports, aliases, re-exports, visibility decisions, and import diagnostics |
| `ResolvedPaths` | HIR stage | Non-type-directed lexical/module path resolution facts |
| `SignatureFacts` | analysis stage | Flow, agent, tool, effect-action, type, enum, and protocol signatures |
| `TypeFacts` | analysis stage | Expression, statement, pattern, symbol, and item type facts |
| `EffectFacts` | analysis stage | Expression, statement, item, requirement, and interpreter support effect facts |
	| `ProjectEntryFact` | output stage | Resolved entry flow and entry diagnostics |
	| `CheckedProject` | output stage | Final cross-repository frontend contract |

### 5.3.1 Incremental Session And Cache Boundary

The frontend should expose a stateful compiler session for CLI, watch mode,
`etas_intel`, and future daemon use. Convenience one-shot APIs may wrap the
same session model, but they are not the primary integration boundary.

Recommended public model:

```rust
pub struct FrontendSession;

impl FrontendSession {
    pub fn with_options(options: FrontendSessionOptions) -> Result<Self, FrontendSessionError>;
    pub fn open_project(&mut self, input: ProjectInput) -> Result<ProjectSessionId, FrontendSessionError>;
    pub fn apply_changes(
        &mut self,
        project: ProjectSessionId,
        changes: ProjectChangeSet,
    ) -> Result<ChangeSummary, FrontendSessionError>;
    pub fn check(
        &mut self,
        project: ProjectSessionId,
        request: CheckRequest,
    ) -> Result<CheckResponse, FrontendSessionError>;
    pub fn snapshot(&self, project: ProjectSessionId) -> Option<ProjectSemanticSnapshot>;
    pub fn source_set(&self, project: ProjectSessionId) -> Option<&SourceSet>;
    pub fn artifact_manifest(&self, project: ProjectSessionId) -> Option<&FrontendArtifactManifest>;
}

pub struct FrontendSessionOptions {
    pub cache: FrontendCacheConfig,
}

pub enum FrontendCacheConfig {
    MemoryOnly,
    Disk {
        root: PathBuf,
        access: DiskCacheAccess,
        policy: FrontendDiskCachePolicy,
    },
}

pub enum DiskCacheAccess {
    Disabled,
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

pub struct CheckRequest {
    pub mode: CheckMode,
    pub scope: CheckScope,
    pub cache_access: CacheAccess,
}

pub enum CheckMode {
    FullProject,
    Incremental,
}

pub enum CheckScope {
    FullProject,
    EntryReachable,
}

pub enum CacheAccess {
    Default,
    MemoryOnly,
    ReadOnly,
    WriteOnly,
    ReadWrite,
}
```

The session is a library object owned by the caller. `etas_frontend` does not
own a background process, file watcher, LSP server, daemon event loop, or editor
VFS.

`CheckMode` controls pipeline scheduling. `CheckScope` controls the semantic
body boundary for the current command. `FullProject` validates every project
body; `EntryReachable` validates the entry item closure and emits
`RuntimeSourceRequirements` for dependency source overlays. `CacheAccess`
controls whether this particular check may read or write reusable artifacts.
These must remain separate: a full project check may still read body-scoped
facts from disk, and an incremental IDE check may deliberately stay memory-only
to avoid blocking the editor on disk writes.

```text
etas check
  -> create FrontendSession with CLI-selected cache config
  -> check once with CheckMode::FullProject + CheckScope::FullProject
  -> drop session

etas run
  -> create FrontendSession with metadata-only dependency environment
  -> check once with CheckMode::FullProject + CheckScope::EntryReachable
  -> driver materializes RuntimeSourceRequirements into dependency overlays
  -> apply_changes with DependencyOverlayChange
  -> incremental checks with CheckScope::EntryReachable until no new overlay is needed
  -> execute checked entry

etas watch
  -> create FrontendSession once
  -> first FullProject check with CheckScope::FullProject
  -> apply_changes on file events
  -> Incremental checks while watch runs

etas_intel
  -> owns FrontendSession while the editor workspace is open
  -> applies VFS overlay changes
  -> runs Incremental checks for editor feedback
```

`Frontend::check_project(ProjectInput)` may remain as a thin convenience wrapper
around `FrontendSession` for tests and simple one-shot callers. It must not be
the primary path for CLI, watch, `etas_intel`, or a future build daemon,
because those callers must control cache configuration, check mode, and
snapshot/delta consumption explicitly.

Incremental protocol types belong to `etas_frontend` because they describe
frontend semantic change:

```rust
pub struct ProjectChangeSet {
    pub revision: ProjectRevision,
    pub source_changes: Vec<SourceChange>,
    pub manifest_change: Option<ManifestChange>,
    pub dependency_changes: Vec<DependencyChange>,
    pub option_changes: Vec<CompilerOptionChange>,
}

pub enum SourceChange {
    Add(SourceInput),
    Remove(SourceId),
    Replace { source: SourceId, version: SourceVersion, text: String },
    Edit { source: SourceId, version: SourceVersion, edits: Vec<TextEdit> },
}
```

`ProjectSemanticSnapshot` is the checked compiler view at a revision. It is not
just HIR:

```rust
pub struct ProjectSemanticSnapshot {
    pub revision: ProjectRevision,
    pub sources: SourceSet,
    pub modules: ModuleIndex,
    pub units: UnitTree,
    pub import_graph: ImportGraph,
    pub resolved_imports: ResolvedImports,
    pub resolved_paths: ResolvedPaths,
    pub hir: HirProgram,
    pub source_map: SourceMap,
    pub symbols: SymbolTable,
    pub scopes: ScopeTree,
    pub types: TypeFacts,
    pub type_store: TypeStore,
    pub effects: EffectFacts,
    pub diagnostics: DiagnosticSet,
}
```

`ProjectSemanticDelta` describes how a caller can refresh its own secondary
indexes:

```rust
pub struct ProjectSemanticDelta {
    pub changed_sources: Vec<SourceId>,
    pub affected_modules: Vec<ModuleId>,
    pub affected_items: Vec<HirItemId>,
    pub affected_bodies: Vec<BodyId>,
    pub invalidated_artifacts: ArtifactSet,
    pub replaced_diagnostics: Vec<SourceId>,
}
```

`etas_frontend` owns frontend artifact semantics:

- artifact kinds such as `ParsedSource`, `ModuleIndex`, `ImportGraph`,
  `HirModule`, `HirBody`, `SignatureFacts`, `TypeFacts`, `EffectFacts`,
  `Diagnostics`, and `CheckedProject`;
- dependency meaning between those artifacts;
- invalidation decisions;
- pass scheduling over semantic units.

Generic cache infrastructure belongs to `etas_cache` in `etas-core`:

- artifact keys and fingerprints;
- revision and dependency graph primitives;
- memory artifact store;
- disk artifact store;
- cache policy and eviction primitives.

`etas_cache` must not define frontend artifact kinds or understand HIR/type
semantics. `etas_frontend` maps its own semantic artifacts onto generic
`etas_cache` keys.

`etas_cache` should provide both memory and disk stores. `etas_frontend`
should depend only on the generic store interface. CLI, watch, `etas_intel`,
tests, and future daemon code choose cache behavior through
`FrontendSessionOptions` and per-check `CacheAccess`. Disk artifacts remain
optional, versioned, fingerprinted, and fully rebuildable; they are never the
canonical source of program truth.

### 5.3.2 Persistence Policy And `etas_intel`

Disk persistence is a selective optimization, not a goal by itself. The
frontend must not persist every artifact that can be serialized. Persisting too
much turns `.etas/cache/` into a second frontend database with duplicated
source text, stale semantic state, larger schema migration surface, more SQLite
lock pressure, slower GC, and higher risk of using obsolete data.

The persistence decision belongs to `etas_frontend` because only the frontend
knows artifact semantics. `etas_cache` stores generic bytes and metadata; it
must not decide that `SourceSet`, `HirProgram`, `TypeFacts`, or
`Diagnostics` are worth persisting.

Recommended policy shape:

```rust
pub enum PersistenceClass {
    MemoryOnly,
    DiskMetadataOnly,
    DiskPayload {
        priority: CachePriority,
        max_payload_bytes: u64,
    },
}

pub enum CachePriority {
    Low,
    Normal,
    High,
}

pub fn persistence_class(key: &FrontendArtifactKey) -> PersistenceClass;
```

The policy must be artifact-key based, not Rust-type based. A type such as
`TypeOutput` may be worth persisting at body granularity, while an aggregate
project-level `TypeFacts` payload is usually not. A type such as `SourceSet`
may be useful in memory but should not become a disk payload.

Default disk payload allowlist:

| Artifact | Persistence | Reason |
|---|---|---|
| `ArtifactManifest` | `DiskPayload` | Small recovery index for validating persisted artifact relationships |
| `BodyArtifactReuseIndex` | `DiskPayload` | Small index that maps stable body identity to reusable expensive body facts |
| item-scoped `SignatureFacts` / `ItemSignature` | `DiskPayload` | Small, stable, reused by many body checks |
| body-scoped `TypeFacts` / `TypeOutput` | `DiskPayload` | Expensive to recompute and naturally body-granular |
| body-scoped `EffectFacts` / `EffectOutput` | `DiskPayload` | Expensive to recompute and naturally body-granular |

Default metadata-only candidates:

| Artifact data | Persistence | Reason |
|---|---|---|
| source fingerprint summary | `DiskMetadataOnly` | Store source id/path/kind/text hash, not source text |
| module/import/export fingerprint summary | `DiskMetadataOnly` | Useful for invalidation decisions without persisting full graph structures |
| artifact dependency graph and reuse stats | `DiskMetadataOnly` | Needed for validation, invalidation, GC, and tuning |

Default memory-only or recomputed artifacts:

| Artifact | Persistence | Reason |
|---|---|---|
| `SourceSet` | `MemoryOnly` | Source files and editor overlays are canonical; persisting source text duplicates the project |
| `ParsedSource` / `ParsedSourceSet` / AST | `MemoryOnly` | Large, parser-schema-sensitive, and usually cheaper than type/effect checking |
| `HirProgram`, `HirModule`, `HirBody` payloads | `MemoryOnly` | Large id-heavy structures; rebuild from source and current lowering rules |
| complete `ModuleIndex`, `UnitTree`, `ImportGraph`, `ModuleTopoOrder` payloads | `MemoryOnly` | Recompute in the live session; persist only compact fingerprints if needed |
| `AffectedModuleSet` | `MemoryOnly` | Request-local scheduling result |
| `ResolvedImports`, `ResolvedPaths` | `MemoryOnly` by default | Keep in live snapshot; promote only if profiling proves disk read wins |
| `Diagnostics` | `MemoryOnly` | User-facing output derived from current source and facts |
| `ProjectEntryFact` | `MemoryOnly` | Small and cheap to recompute |
| `CheckedProject` | `MemoryOnly` | Aggregate frontend contract; rebuild from source, HIR, and facts |
| aggregate project-level `TypeFacts`, `EffectFacts`, `SignatureFacts` | `MemoryOnly` | Avoid duplicating item/body-scoped facts in large project payloads |

`etas_intel` support must come from the live frontend contract, not from
persisting the entire frontend state. The intended IDE data path is:

```text
etas_intel VFS / open documents
  -> FrontendSession with in-memory overlays
  -> ProjectSemanticSnapshot
  -> ProjectSemanticDelta
  -> etas_intel secondary editor indexes
  -> etas_lsp protocol conversion
```

`ProjectSemanticSnapshot` must expose enough in-memory data for IDE queries:

- source text and line indexes, including unsaved editor overlays;
- AST/HIR source mapping and reverse lookup hooks;
- module index, import graph, resolved imports, and resolved paths;
- HIR, symbols, scopes, definition targets, and visibility/export facts;
- type store, item signatures, expression types, and symbol type facts;
- effect summaries, requested-action facts, trace-spec/residual-check facts, and
  interpreter-support facts;
- diagnostics for the current revision.

`ProjectSemanticDelta` must let `etas_intel` update secondary indexes without
rebuilding the whole workspace:

- changed sources;
- affected modules;
- affected items;
- affected bodies;
- invalidated artifact keys;
- replaced diagnostics;
- enough stable ids for source map, symbol, type, and effect index refresh.

This design is sufficient for `etas_intel` because editor queries need the
current checked semantic view, including unsaved buffers. Disk cache only
accelerates cold start and expensive body-level recomputation. It must not be
the source of truth for hover, completion, definition, references, semantic
tokens, diagnostics, or graph previews.

Required cache budgeting and instrumentation:

- disk cache must support per-project and per-namespace byte budgets;
- disk payload writes must honor per-artifact `max_payload_bytes`;
- eviction should use priority plus `last_used_at`, not only artifact count;
- cache telemetry should record compute time, serialization time,
  deserialization time, compressed size, hit count, miss count, skipped writes,
  and evictions by artifact kind;
- an artifact kind should be promoted from `MemoryOnly` to `DiskPayload` only
  when measurement shows disk read plus validation is cheaper than recomputing.

### 5.3.3 Incremental Implementation Contract

The `etas_frontend` facade should contain explicit session, artifact, and
incremental layers instead of hiding this behavior inside ad hoc check helpers.

Recommended internal layout:

```text
crates/etas_frontend/src/
  session/
    mod.rs
    project.rs
    change.rs
    snapshot.rs
    delta.rs

  artifact/
    mod.rs
    kind.rs
    key.rs
    fingerprint.rs
    manifest.rs
    dependency.rs

  incremental/
    mod.rs
    dirty.rs
    invalidation.rs
    affected.rs
    reuse.rs

  pipeline/
    mod.rs
    passes.rs
    adapters.rs
    scheduler.rs
```

These modules remain inside `etas_frontend` because they know frontend
semantics. They may use `etas_cache` keys, stores, and dependency graph
helpers, but `etas_cache` must not import these modules.

Recommended project state:

```rust
pub struct FrontendProjectState {
    pub id: ProjectSessionId,
    pub revision: ProjectRevision,
    pub sources: SourceSet,
    pub manifest: FrontendArtifactManifest,
    pub last_snapshot: Option<ProjectSemanticSnapshot>,
    pub store: Box<dyn ArtifactStore>,
}

pub struct FrontendArtifactManifest {
    pub artifacts: Vec<FrontendArtifactRecord>,
    pub dependencies: FrontendArtifactDependencyGraph,
}

pub struct FrontendArtifactRecord {
    pub key: FrontendArtifactKey,
    pub fingerprint: ArtifactFingerprint,
    pub unit: FrontendUnitKey,
    pub diagnostics: Vec<DiagnosticId>,
}
```

`FrontendArtifactKey` is a frontend-owned typed wrapper that converts into the
generic `etas_cache::ArtifactKey`:

```rust
pub enum FrontendArtifactKind {
    ParsedSource,
    ModuleIndex,
    UnitTree,
    ImportGraph,
    ModuleTopoOrder,
    HirModule,
    HirBody,
    ResolvedImports,
    ResolvedPaths,
    SignatureFacts,
    TypeFacts,
    EffectFacts,
    Diagnostics,
    CheckedProject,
}
```

The key must include enough identity to avoid stale reuse:

```text
namespace = "frontend"
kind      = frontend artifact kind
unit      = project/source/module/module-part/item/body id as appropriate
fingerprint inputs =
  source text hash
  module declaration and import hash
  compiler option hash
  std registry version
  dependency artifact fingerprints
  frontend artifact schema version
```

`apply_changes` must only mutate source/session state and compute a change
summary. It must not silently run a full check.

Required `apply_changes` behavior:

```text
validate monotonically increasing ProjectRevision and SourceVersion
  -> update SourceSet entries and line indexes
  -> record changed/added/removed SourceId values
  -> map changed sources to dirty artifact roots
  -> invalidate direct source artifacts such as ParsedSource and Diagnostics
  -> leave semantic recomputation to check()
  -> return ChangeSummary
```

`check` owns recomputation. It should run the same pass pipeline for full and
incremental builds; incremental mode changes the selected unit sets and cache
reuse decisions, not the language semantics.

Required `check` behavior:

```text
read previous snapshot and artifact manifest
  -> run source/project discovery passes
  -> recompute parse artifacts for changed sources
  -> rebuild ModuleIndex, UnitTree, ImportGraph, and topo order when needed
  -> compute affected modules/items/bodies from dirty roots and reverse imports
  -> reuse valid artifacts whose fingerprints and dependency fingerprints match
  -> rerun project/module/body passes for affected semantic units
  -> aggregate unit-scoped deltas into TypeFacts, EffectFacts, and diagnostics
  -> build a new ProjectSemanticSnapshot
  -> emit ProjectSemanticDelta against the previous snapshot
  -> replace last_snapshot and artifact manifest atomically inside the session
```

Project-level graph artifacts must be rebuilt when any of these change:

- source file added or removed;
- module declaration changed;
- import declaration changed;
- visibility/export shape changed;
- package manifest changed;
- dependency or std registry version changed;
- compiler options affecting parse, name resolution, type checking, effects, or
  interpreter support changed.

Body-level artifacts may be reused when the module graph is unchanged and the
body fingerprint plus all dependency fingerprints still match. A body text edit
should invalidate that body, its enclosing item facts when the signature shape
changed, dependent body facts that reference changed signatures, and diagnostics
for the changed source. It should not invalidate unrelated modules merely
because they are in the same project.

Import graph changes are broader than local body edits. When an import edge is
added, removed, or retargeted, the frontend must recompute `ResolvedImports`,
`ResolvedPaths`, signatures that depend on imported names, and all affected
type/effect facts along reverse import edges.

`ProjectSemanticDelta` must be derived from the artifact invalidation and
replacement report, not guessed from changed files alone. This is what lets
`etas_intel` refresh only its affected editor indexes while keeping the
frontend independent from IDE code.

Diagnostics are source-owned outputs. Each check must replace diagnostics for
changed and affected sources as a set, remove diagnostics for deleted sources,
and preserve diagnostics for unaffected sources only when the producing
artifact was reused.

Pass responsibilities:

| Pass | Scope | Requires | Produces | Responsibility |
|---|---|---|---|---|
| `BuildSourceSetPass` | `Project` | `ProjectInput` | `SourceSet` | Normalize package, single-file, virtual std, generated, and LSP overlay inputs into stable `SourceFile`s with `SourceId`, path, text, line index, and source kind. |
| `ParseSourceFilePass` | `SourceFile` | `SourceSet` | `ParsedSource` | Parse one source file into source-shaped AST, collect syntax diagnostics, preserve imports/items in file order, and attach every AST ref to `SourceId + Span`. |
| `BuildModuleIndexPass` | `Project` | `ParsedSourceSet` | `ModuleIndex` | Group parsed sources by declared logical module path, create `ModuleInfo` and `ModulePart`, validate filesystem path/module declaration consistency, support multiple parts per logical module, and diagnose ambiguous canonical file mappings. |
| `BuildUnitTreePass` | `Project` | `SourceSet`, `ParsedSourceSet`, `ModuleIndex` | `UnitTree` | Build semantic units for project, source files, modules, module parts, items, bodies, blocks, and expressions. This is the scheduling and diagnostic hierarchy, not AST or HIR. |
| `BuildImportGraphPass` | `Project` | `ModuleIndex`, `ParsedSourceSet` | `ImportGraph` | Read module-level import declarations and build module import edges with reverse edges and source spans. It records unresolved edge candidates but does not bind local names. |
| `DetectImportCyclesPass` | `Project` | `ImportGraph` | diagnostics | Detect import cycles and report project-level diagnostics on the import spans that form the cycle. Later passes may continue in recovery mode, but affected modules are not considered fully checked. |
| `ComputeModuleTopoOrderPass` | `Project` | `ImportGraph` | `ModuleTopoOrder` | Produce deterministic topological order for module lowering and analysis. Cyclic modules are ordered only for recovery and must keep cycle diagnostics. |
| `ComputeAffectedModulesPass` | `Project` | previous snapshot, changed `SourceId`s, `ImportGraph` | `AffectedModuleSet` | Incremental-only pass. Walk reverse import edges and unit parents to determine which modules and bodies must be recomputed. Full builds may produce the all-modules set. |
| `PredeclareProjectSymbolsPass` | `Project` | `ModuleIndex`, `UnitTree` | HIR module shells, symbol seeds, export stubs | Allocate all `HirModuleId`s and module scopes, create module symbols, and predeclare every top-level item symbol, including aliases, nominal type declarations, specs, spec impls, marker impls, and effect declarations, before body lowering. This enables cross-file and recursive references. Alias and nominal type symbols must remain distinct. |
| `NormalizeModuleImportsPass` | `ModulePart` | `ParsedSource`, symbol seeds | HIR import entries | Normalize source import trees into explicit HIR import records. Grouped imports become member imports; wildcard imports remain wildcard import sources; visibility and aliases are preserved. |
| `LowerModuleItemsPass` | `ModulePart` | `ParsedSource`, symbol seeds, HIR module shells | partial `HirProgram`, source map updates | Lower AST items, bodies, blocks, expressions, patterns, and type references into HIR ids while preserving AST/HIR origin through source maps and unit targets. |
| `ResolveImportsPass` | `Project` | `ModuleIndex`, export stubs, HIR import entries, std/dependency providers | `ResolvedImports`, export tables | Resolve source, std, and dependency module imports; check public/private visibility; expand wildcard imports; record re-export metadata; diagnose missing modules/items and wildcard ambiguity. |
| `ResolvePathsPass` | `Project` | `HirProgram`, `ResolvedImports`, symbol tables | `ResolvedPaths` | Resolve lexical and module-qualified paths that do not require type-directed member lookup. It must preserve partially resolved paths for the type checker when the prefix is known but the member is type-directed. |
| `BuildSignatureFactsPass` | `Project` | `HirProgram`, `ResolvedPaths`, std/dependency metadata | `SignatureFacts` | Invoke `etas_types::SignaturePipeline`. This is a declaration-level pipeline: declare nominal types, aliases, type/callable/trace specs, callables, effect tags/actions, imports, std and external symbols; resolve aliases/import facts/spec closure; validate spec impls, callable-spec satisfaction, trace-spec kind checks, and exported contracts; then materialize signature facts. It must not enter flow/tool/agent bodies or compute expression facts. It must produce separate alias facts and nominal constructor facts; `type A = B` must not be lowered as a transparent alias. |
| `ValidateTopLevelLetPass` | `Project` | `HirProgram`, `SignatureFacts`, `ResolvedPaths` | top-level let facts, resource facts, diagnostics | Type-check top-level `let` initializers, reject top-level `var`, accept only compile-time deterministic/effect-free constants and compiler-known runtime resource constructors such as `std.memory.region[...]`, and classify resource handles without executing them. |
| `TypeCheckBodyPass` | `Body` | `HirProgram`, `SignatureFacts`, `ResolvedPaths` | `TypeFacts` delta | Invoke `etas_types::BodyPipeline` for one executable/analyzable body region: flow body, lambda body, handler arm body, inline trace-spec conformance expression, or agent prompt body. Internally it must generate body constraints, solve type constraints, solve spec obligations, validate diagnostics, and materialize facts. The frontend pass must not call a recursive checker, and the body generator must not write final facts or diagnostics directly. |
| `RunEffectPipelinePass` | `Project` | `HirProgram`, `SignatureFacts`, `TypeFacts`, `StdRegistry`, dependency metadata | `EffectOutput`, effect artifacts | Call `etas_effects::pipeline::RunEffectPipeline`, passing spec facts and generic instantiation facts from `TypeFacts`, then publish returned serializable/fingerprintable artifacts and final `EffectFacts` into `ProjectContext`. The frontend pass owns scheduling, cache/session integration, artifact publication, and diagnostic aggregation; it must not reimplement effect registry construction, constraint collection, dependency graph construction, row-polymorphic summary instantiation, summary solving, contract validation, or fact materialization. |
| `VerifyInterpreterSupportPass` | `Project` | `EffectFacts`, `ProjectEntryFact` optional | interpreter readiness facts and diagnostics | Aggregate effect support facts, distinguish static rejection from Phase 1 execution requirements, and record host/orchestration requirements for model calls, tools, authority, typed persistent-memory API access, retry, checkpoint, resume, workflow orchestration, and effect handlers. |
| `ResolveEntryPointPass` | `Project` | `ModuleIndex`, `HirProgram`, `SignatureFacts`, `TypeFacts`, `EffectFacts` | `ProjectEntryFact` | Resolve the selected entry flow, validate its signature and module, and record entry diagnostics. |
| `BuildCheckedProjectPass` | `Project` | all required artifacts, diagnostics | `CheckedProject` | Produce the final checked frontend contract only when blocking diagnostics are absent. It must include all source files, project HIR, source maps, type/effect facts, interpreter support facts, module index, and entry metadata. |

Effect inference is exposed to the frontend as one project pass. The internal
effect phases live in `etas_effects::pipeline` as `BuildRegistryPass`,
`CollectConstraintsPass`, `BuildGraphPass`, `SolveSummariesPass`,
`ValidateContractsPass`, and `MaterializeFactsPass`. This keeps effect semantics
inside `etas_effects` while allowing `etas_frontend` to own pass scheduling,
cache/session behavior, artifact publication, and final `CheckedProject`
assembly.

`SolveSummariesPass` is an interprocedural abstract interpretation over typed
HIR semantic bodies. The frontend must not implement a parallel effect checker,
body-local fallback summaries, or unordered action-set policy stepping. It only
passes `HirProgram`, `TypeOutput` including spec/generic-instantiation facts,
standard metadata, dependency metadata, and cache/session context into
`etas_effects`, then stores the returned artifacts and diagnostics.

Effect-row polymorphism is a cross-pass contract:

1. `etas_syntax` preserves `effect E` and row tails such as
   `![Console.stdout_write, E]`.
2. `etas_hir` lowers effect parameters to `SymbolKind::EffectParam` and keeps
   `HirGenericArg::EffectRow`.
3. `etas_types` kind-checks row parameters and records call-site row
   substitutions in generic instantiation facts.
4. `etas_effects` instantiates summaries through those substitutions before
   validation and materialization.

Alias/nominal typing is also a cross-pass contract:

1. `etas_syntax` parses `alias A = B;`, `type A = B;`, and `type A;` as
   distinct declaration shapes.
2. `etas_hir` lowers aliases and nominal type declarations to distinct HIR
   item kinds and symbols.
3. `etas_types` expands aliases transparently but assigns every `type`
   declaration a nominal constructor identity.
4. `etas_effects` and package metadata consume the checked `TypeFacts`; they
   must not re-expand nominal representations to decide public contracts.
5. `etas_interpreter` executes checked nominal constructors/conversions only
   through type facts and must not treat representation equality as implicit
   assignability.

Postfix `?` is a cross-pass contract, not a parser-only feature:

1. `etas_syntax` parses it as `TryExpr`.
2. `etas_hir` lowers it to `HirExpr::Try` without desugaring to a synthetic
   `handle`, `match`, `unwrap`, or std helper.
3. `etas_types` records `Result[T, E]` value-shape facts for the expression.
4. `etas_effects` validates that the operand raises a capturable `Error[E]`,
   removes that captured effect, and records `TryCaptureFact`.
5. `etas_interpreter` executes the checked capture fact and returns `Ok(v)` or
   `Err(e)` as a value.

`?` is only postfix. Prefix `?x` is invalid. It is an expression operator, not a
statement terminator; semicolons are owned by `let`, `return`, or expression
statements. If the source writes `{ let x = f(); g(x) }?`, the whole block is the
operand and the capture boundary covers the whole block summary.

The frontend must reject Rust-style `Result[T, E]` unwrapping with `?`. A
value-level `Result` alone is not enough; the operand must have a capturable
typed `Error[E]` effect.

Runtime-scoped handlers are also a cross-pass contract:

1. `etas_syntax` parses `handler { ... }` as a handler expression and parses
   `handle Expr with HandlerArg`, where `HandlerArg` is either an inline
   handler block or an ordinary expression.
2. `etas_hir` lowers handler literals to `HirExpr::Handler` and handle
   applications to `HirExpr::Handle { body, handler }`. HIR must not flatten a
   handler value into the handle body or rewrite it into ordinary calls.
3. `etas_types` checks handler transformer types such as
   `![Approval.request => Console.stdout_write for Report]`, action parameter
   patterns, resume payload types, and result compatibility.
4. `etas_effects` resolves action identities, infers produced effects for
   handler arms, records `HandlerValueFact` and `HandleApplicationFact`, removes
   only fully handled actions/effects from the handled body, and keeps all
   requested actions, trace-spec obligations, sandbox checks, approval checks, and
   limit requirements.
5. `etas_interpreter` consumes the checked handler facts. It may push a handler
   frame for the dynamic scope of `handle`, but it must not let a handler grant
   missing authority or mutate the active grant/trace-spec environment.

Effect declarations follow the same project-level signature path as other
items. `effect Name;`, `effect Name { action ...; }`, and
`impl EffectName { action ...; }` must all register action signatures before
body checking. `impl TypeName` and `impl EffectName` are kind-checked after name
resolution; parser recovery must not silently reinterpret one as the other.

The pass manager is project orchestration. It must not expose parser grammar
details such as grouped imports, aliases, or trailing commas as separate pass
boundaries. Those are syntax forms handled by parsing and import normalization.
The pass manager should expose semantic granularity and artifact dependencies,
not grammar productions.

Adapter semantics:

```rust
pub trait FrontendUnitProvider {
    fn units(&self, selector: UnitSelector, order: UnitOrder) -> Vec<UnitKey>;
    fn unit_target(&self, unit: UnitKey) -> UnitTarget;
}
```

`etas_utils::pipeline` owns the generic adapter mechanism. `etas_frontend`
owns the concrete unit kinds and how they map to `SourceSet` or `UnitTree`:

| Unit kind | Backing data | Available after | Typical order |
|---|---|---|---|
| `SourceFile` | `SourceSet.files` | `BuildSourceSetPass` | source discovery order |
| `Module` | `ModuleIndex.modules` and `UnitTree` | `BuildUnitTreePass` | module topo order |
| `ModulePart` | `ModuleIndex.parts` and `UnitTree` | `BuildUnitTreePass` | module topo order, then source order |
| `Item` | `UnitTree` + AST/HIR item refs | `BuildUnitTreePass` / HIR lowering | stable id order |
| `Body` | `UnitTree` + AST/HIR body refs | `BuildUnitTreePass` / HIR lowering | stable id order or affected-first |
| `Block` | `UnitTree` + HIR block refs | HIR lowering | stable id order |
| `Expression` | `UnitTree` + HIR expr refs | HIR lowering | stable id order |

Adapter execution rules:

- `ForEach(SourceFile)` runs before module grouping and must read from
  `SourceSet`, not `UnitTree`;
- `ForEach(ModulePart, topo order)` runs after `ModuleTopoOrder` and must visit
  module parts in dependency order so imported module shells and exports are
  available before dependents when possible;
- `ForEach(Body)` runs after signatures are built, because body checking depends
  on project-wide item signatures;
- child passes receive the current `UnitKey` through the pipeline pass context;
- child passes must check only the selected unit and emit unit-scoped artifact
  deltas;
- `ForEach(Module)`, `ForEach(Body)`, `ForEach(Block)`, and
  `ForEach(Expression)` must derive work units from `HirTreeView` and
  `HirScopeView`, not from ad hoc global scans over `HirProgram` arenas;
- parent project passes aggregate deltas into project-level artifacts such as
  `TypeFacts`, `EffectFacts`, and `CheckedProject`;
- incremental builds may replace `ForEach(Body)` with `ForEach(Affected Body)`
  while full builds use all body units.

Example body adapter expansion:

```text
ForEach(Body, stable id order)
  TypeCheckBodyPass
```

is recorded by instrumentation as if the scheduler ran:

```text
TypeCheckBodyPass @ Body(flow main)
TypeCheckBodyPass @ Body(inline trace-spec conformance)
```

This is intentionally different from a monolithic `TypeCheckProjectPass`.
Business passes should not rediscover every body by walking the whole HIR. The
pipeline adapter performs traversal so timing, invalidation, diagnostics, and
future parallelism can remain unit-aware.

Implementation boundary:

- HIR lowering builds `HirProgram` plus `HirTreeIndex` and `HirScopeView`;
- pipeline adapters consume those views to produce `UnitKey` streams;
- type/effect/trace-spec/interpreter passes receive the current unit and use
  `BodyView`, `BlockView`, or `ExprView` for local traversal;
- direct arena iteration is limited to HIR validation, view construction, dump,
  and deliberately global project passes.

### 5.4 Project HIR Lowering

Project HIR lowering must not lower one file at a time into isolated
`HirProgram`s.

Required lowering order:

1. allocate every `HirModuleId`;
2. create every module scope;
3. predeclare module symbols;
4. predeclare top-level item symbols for every module, preserving
   `public`/`private`;
5. lower import trees into HIR import entries without requiring all targets to
   be resolved yet;
6. lower item bodies;
7. run import and name resolution against the full module index and export
   tables.

This order is required so cross-file references and mutually recursive
declarations are not constrained by file order.

### 5.5 Import Semantics

Project import resolution must implement the SPEC:

```text
import tests.compiler.support.algorithms.{len}
  -> resolve current package module tests.compiler.support.algorithms
  -> check that len is public
  -> bind local name len

import std.io.{read_all, println}
  -> resolve std virtual/dependency module std.io
  -> check public exports
  -> bind local names read_all and println

public import std.prelude.*
  -> re-export the exact public names resolved from std.prelude
```

Name priority:

```text
local declarations
  > explicit imports
  > wildcard imports
```

If an unqualified name is provided by more than one wildcard import and no local
or explicit import overrides it, name resolution must report ambiguity.

### 5.6 Diagnostics And Source Mapping

Project-level diagnostics must remain source-specific:

- every diagnostic carries the original `SourceId` and span;
- CLI/LSP renderers map diagnostics back to the correct file;
- project outputs preserve `SourceBundle` for all parsed files;
- package/module resolution diagnostics point at the importing source span and
  may add notes for candidate files or exported names.

### 5.7 Acceptance Tests

Project-level tests should use the existing `etas_tests` compiler fixtures:

```text
fixtures/compiler/positive/algorithms/*.es
fixtures/compiler/support/algorithms.es
fixtures/compiler/negative/algorithms/*.es
```

Required acceptance cases:

- an algorithm fixture importing
  `tests.compiler.support.algorithms.{len}` resolves `len` to the public helper
  flow in `compiler/support/algorithms.es`;
- imports from `std.collections.List`, `std.effects.Console`, and
  `std.io.{read_all, println}` resolve through standard-library declaration
  providers;
- importing a private helper from another module is rejected;
- missing imported item is rejected;
- missing imported module is rejected;
- duplicate top-level item names inside one logical module are rejected;
- file path/module declaration mismatch is rejected;
- `src/foo/bar.es` plus `src/foo/bar/mod.es` for the same module is rejected;
- wildcard ambiguity is rejected at the use site;
- `public import` contributes re-export metadata.

## 6. `CheckedProgram` Ownership Rules

`CheckedProgram` is a cross-repository contract from `etas-frontend` to
`etas-interpreter`.

It should contain:

- immutable HIR program data;
- symbol and scope tables;
- source map;
- type facts keyed by HIR ids;
- symbol type facts keyed by `SymbolId`;
- effect facts keyed by flow/item/expression ids;
- requirement and interpreter-support facts;
- frontend diagnostics emitted before successful checking;
- enough entry metadata for `etas run`;
- all source files that participated in project compilation.

It should not contain:

- interpreter frames;
- runtime values;
- host adapters;
- AIR nodes;
- CLI rendering configuration.

## 7. Dump Interfaces

The frontend should expose deterministic dump APIs through `etas_frontend`:

```rust
dump_ast(parse_output, options) -> String
dump_hir(check_output, options) -> String
dump_diagnostics(diagnostics, options) -> DiagnosticDocument
```

These APIs allow the `etas` repository to implement user commands without
depending on `etas_syntax` or `etas_hir` private modules.

For project inputs, dump commands should be explicit about scope:

```text
dump ast <file>       -> parse one source file
dump hir <input...>   -> lower/check the project source set, then dump project HIR
```

`dump hir` should not silently dump only the first file when project inputs are
provided.

## 8. Phase 1 Execution-Requirement Policy

Frontend should classify checked-HIR execution requirements before
interpretation:

- agent inference boundaries, including `Agent.run(input)` and pipeline stages
  whose resolved stage type is an agent;
- tool calls requiring host authority;
- network, filesystem, command, and typed persistent-memory API boundaries;
- approval, secret, time, and other host-mediated authority categories;
- effect handlers and `resume`;
- retry and loops where limits are required;
- checkpoint and resume policies;
- workflow orchestration requirements for checked-HIR execution.

These are not ordinary static failures when the source declares the required
effects, capabilities, limits, handlers, and policies. Ordinary `etas check`
should report static correctness. Direct execution commands such as
`etas run` or `etas check --phase1` should verify host handler readiness,
authority grants, checkpoint store availability, and interpreter orchestration
support.

An `agent` declaration itself may be parsed, lowered, typed, and effect-checked
in Phase 1. The agent body is prompt/context construction and must type-check as
`Prompt`. Calling the agent is a checked-HIR inference boundary executed by the
interpreter through a supplied model handler.

The interpreter may repeat defensive checks, but frontend should produce the
primary static diagnostics and execution-requirement facts.

## 9. Test Direction

Frontend tests should include:

- syntax positive and recovery fixtures;
- AST dump golden tests;
- HIR lowering and symbol table tests;
- name resolution negative tests;
- project-level module index and import resolution tests;
- type checker positive/negative tests;
- effect restriction and execution-readiness tests for host/orchestration
  requirements;
- facade tests proving `CheckedProgram` is produced only when diagnostics allow
  interpretation.
