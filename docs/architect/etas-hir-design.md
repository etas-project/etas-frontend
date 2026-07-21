# Etas HIR Design

## 1. Purpose

This document defines the first implementation design for `etas_hir`.

HIR means High-level Intermediate Representation. In Etas, HIR is the resolved
high-level source representation between parsed syntax and type/effect checking.

```text
source text
  -> etas_syntax AST
  -> etas_hir lowering and name resolution
  -> type/effect checking over HIR
  -> checked HIR
      -> Phase 1 HIR interpreter
      -> later FIR/AIR lowering
```

HIR must carry enough semantic information for type checking, effect checking,
abstract interpretation, direct Phase 1 interpretation, and later lowering
while still preserving source mappings back to `etas_syntax`.

## 2. Boundary

`etas_hir` owns:

- module structure after parsing;
- scope tree;
- symbol table;
- imports and name resolution;
- resolved references from syntax names to symbols;
- high-level source-shaped HIR nodes;
- source mapping from HIR nodes to syntax spans;
- name-resolution diagnostics;
- HIR dump support.

`etas_hir` does not own:

- lexing or parsing;
- token-level trivia;
- source AST definitions;
- type inference;
- effect inference;
- action-grant or trace-spec checking;
- trace-spec validation beyond resolving names;
- explicit control-flow graph construction;
- explicit data-flow graph construction;
- FIR graph construction;
- AIR graph construction;
- runtime execution.

HIR is not an LLVM-like IR. It should not eagerly build basic blocks, phi nodes,
or explicit control/data edges. That belongs in FIR. AIR is generated later as
the executable runtime representation.

## 3. Crate Layout

Recommended file layout:

```text
crates/etas_hir/
  Cargo.toml
  src/
    lib.rs
    db.rs
    ids.rs

    hir/
      mod.rs
      program.rs
      module.rs
      item.rs
      ty.rs
      expr.rs
      stmt.rs
      pattern.rs

    lower/
      mod.rs
      program.rs
      imports.rs
      item.rs
      ty.rs
      expr.rs
      stmt.rs
      pattern.rs

    symbol/
      mod.rs
      table.rs
      symbol.rs
      visibility.rs

    scope/
      mod.rs
      tree.rs
      lookup.rs

    resolve/
      mod.rs
      imports.rs
      paths.rs
      actions.rs

    source_map/
      mod.rs

    diagnostic/
      mod.rs
      codes.rs

    dump/
      mod.rs
      text.rs
```

Module responsibilities:

| Module | Responsibility |
|---|---|
| `ids` | Stable HIR ids such as `HirItemId`, `HirExprId`, `SymbolId`, and `ScopeId` |
| `hir` | HIR data model only |
| `lower` | AST-to-HIR lowering |
| `symbol` | Symbols, symbol kinds, visibility, and symbol tables |
| `scope` | Scope tree and lexical lookup support |
| `resolve` | Import, path, and effect-action resolution |
| `source_map` | HIR-to-syntax span and node mapping |
| `diagnostic` | HIR/name-resolution diagnostic codes and builders |
| `dump` | Deterministic HIR dump for golden tests and debugging |

Keep the internal dependency direction disciplined:

```text
hir      -> ids
symbol   -> ids
scope    -> ids, symbol
resolve  -> hir, symbol, scope, diagnostic
lower    -> hir, symbol, scope, resolve, source_map, diagnostic
dump     -> hir, symbol, scope, source_map
```

The `hir/` module should not contain lowering or resolution algorithms. It is
the data model. Algorithms live in `lower/` and `resolve/`. This keeps HIR easy
to inspect, easy to dump, and stable enough for type/effect checking and AIR
lowering.

The crate depends on `etas_syntax` and shared core types once `etas_core`
exists:

```text
etas_hir -> etas_syntax
etas_hir -> etas_core
```

`etas_syntax` must not depend on `etas_hir`.

## 4. Conceptual Role

AST answers:

```text
What source shape did the user write?
```

HIR answers:

```text
What declarations, scopes, and symbols do those source names refer to?
```

Example source:

```etas
flow BuildFeature(brief: FeatureBrief) -> DesignDoc {
  let prd = brief ~> ProductManager;
  return prd ~> Architect;
}
```

AST keeps names as text:

```text
Path("brief")
Path("ProductManager")
Path("Architect")
```

HIR resolves them:

```text
Path("brief")          -> SymbolId(param BuildFeature.brief)
Path("ProductManager") -> SymbolId(agent ProductManager)
Path("Architect")      -> SymbolId(agent Architect)
```

The HIR should remain source-shaped:

```text
Pipeline
Let
Return
Block
```

It should not lower the example into explicit AIR nodes yet.

## 5. Program Model

Recommended top-level model:

```rust
pub struct HirProgram {
    pub modules: Vec<HirModuleId>,
    pub symbols: SymbolTable,
    pub scopes: ScopeTree,
    pub items: HirItemArena,
    pub exprs: HirExprArena,
    pub stmts: HirStmtArena,
    pub pats: HirPatArena,
    pub types: HirTypeArena,
    pub source_map: SourceMap,
    pub diagnostics: Vec<Diagnostic>,
}
```

The arenas can be simple `Vec<T>`-backed id allocators in the first
implementation.

```rust
pub struct HirItemId(pub u32);
pub struct HirExprId(pub u32);
pub struct HirStmtId(pub u32);
pub struct HirPatId(pub u32);
pub struct HirTypeId(pub u32);
pub struct HirBlockId(pub u32);
```

Use ids internally so later type/effect facts can attach to stable HIR nodes.

### 5.1 Storage, Topology, and Query Layers

`HirProgram` is the storage layer. Its arenas are allowed to be flat because
Rust compiler data structures need stable ids, cheap cloning of ids, and
borrow-checker-friendly access. This storage choice must not leak into every
semantic pass as "scan every arena and rediscover structure".

HIR is organized in three layers:

```text
Storage Layer
  HirProgram
  HirItemArena
  HirExprArena
  HirBlockArena
  HirStmtArena
  HirTypeArena

Topology Layer
  HirTreeIndex
    owner maps
  ScopeTree
  SourceMap

Query / View Layer
  HirTreeView
  HirScopeView
  ModuleView
  ItemView
  BodyView
  BlockView
  ExprView
```

The storage layer owns HIR nodes. The topology layer records parent/owner and
semantic-child relations between typed ids. The query/view layer is the normal
entry point for frontend passes, type checking, effect checking, trace-spec
checking, abstract interpretation, direct HIR interpretation, IDE support, and
HIR dumps.

Architectural rule:

- arenas are implementation storage, not the public traversal model;
- business passes must not rediscover modules, items, bodies, blocks, or
  expressions by scanning the whole `HirProgram`;
- direct arena iteration is allowed only for arena validation, dump/debug
  generation, index construction, and narrowly scoped diagnostics;
- ordinary semantic passes must enter through `HirTreeView`, `HirScopeView`, or
  frontend pipeline `ForEach(...)` adapters built on those views.

### 5.2 HIR Tree and Scope View

HIR must expose a source-shaped semantic tree view even though the underlying
nodes are arena allocated:

```text
HirProgram
  Module
    Item
      Body
        Block
          Statement
          Expression
            child Expression
            child Block
            HandlerArm
            Pattern
```

The view must be deterministic, owner-aware, and stable enough for incremental
compilation. It does not own HIR nodes; it indexes ids already stored in
`HirProgram`.

```rust
pub struct HirTreeIndex {
    pub module_items: Map<HirModuleId, Vec<HirItemId>>,
    pub item_module: Map<HirItemId, HirModuleId>,
    pub item_body: Map<HirItemId, HirBodyRef>,
    pub block_owner: Map<HirBlockId, HirOwner>,
    pub stmt_block: Map<HirStmtId, HirBlockId>,
    pub expr_owner: Map<HirExprId, HirOwner>,
    pub handler_arm_owner: Map<HirHandlerArmId, HirOwner>,
    pub pat_owner: Map<HirPatId, HirOwner>,
    pub scope_children: Map<ScopeId, Vec<ScopeId>>,
}

pub enum HirBodyRef {
    Flow(HirItemId, HirBlockId),
    Tool(HirItemId, HirBlockId),
    Agent(HirItemId, HirBlockId),
    Lambda(HirExprId, HirBlockId),
    HandlerArm(HirHandlerArmId, HirBlockId),
    TraceSpecExpr(HirItemId, HirSpecExprId),
    TopLevelLet(HirItemId, HirExprId),
}

pub enum HirOwner {
    Module(HirModuleId),
    Item(HirItemId),
    Body(HirBodyRef),
    Block(HirBlockId),
    Stmt(HirStmtId),
    Expr(HirExprId),
    HandlerArm(HirHandlerArmId),
}
```

The query API should make structured traversal explicit:

```rust
impl HirTreeView<'_> {
    pub fn modules(&self) -> impl Iterator<Item = ModuleView<'_>>;
    pub fn module(&self, id: HirModuleId) -> Option<ModuleView<'_>>;
    pub fn item(&self, id: HirItemId) -> Option<ItemView<'_>>;
    pub fn body(&self, id: HirBodyRef) -> Option<BodyView<'_>>;
    pub fn block(&self, id: HirBlockId) -> Option<BlockView<'_>>;
    pub fn expr(&self, id: HirExprId) -> Option<ExprView<'_>>;
    pub fn owner_of_expr(&self, id: HirExprId) -> Option<HirOwner>;
    pub fn owner_of_block(&self, id: HirBlockId) -> Option<HirOwner>;
    pub fn source_span(&self, node: HirNodeRef) -> Option<Span>;
}

impl ModuleView<'_> {
    pub fn items(&self) -> &[HirItemId];
}

impl ItemView<'_> {
    pub fn body(&self) -> Option<HirBodyRef>;
    pub fn scope(&self) -> ScopeId;
}

impl BodyView<'_> {
    pub fn root_block(&self) -> Option<HirBlockId>;
    pub fn root_expr(&self) -> Option<HirExprId>;
    pub fn owner_item(&self) -> Option<HirItemId>;
}

impl BlockView<'_> {
    pub fn statements(&self) -> &[HirStmtId];
    pub fn final_expr(&self) -> Option<HirExprId>;
    pub fn child_blocks(&self) -> impl Iterator<Item = HirBlockId>;
    pub fn child_exprs(&self) -> impl Iterator<Item = HirExprId>;
}

impl ExprView<'_> {
    pub fn child_exprs(&self) -> impl Iterator<Item = HirExprId>;
    pub fn child_blocks(&self) -> impl Iterator<Item = HirBlockId>;
    pub fn handler_arms(&self) -> impl Iterator<Item = HirHandlerArmId>;
}
```

The tree index must satisfy these invariants:

- every module listed in `HirProgram.modules` has a `ModuleView`;
- every item belongs to exactly one module;
- every executable/analyzable body has exactly one `HirBodyRef`;
- every block has exactly one owner;
- every statement belongs to exactly one block;
- every expression has exactly one syntactic or semantic owner, except
  explicitly synthetic recovery nodes;
- every scope appears in `scope_children` under its parent unless it is a root
  scope;
- traversal order is source order where possible and otherwise stable id order;
- no semantic pass is required to scan the entire `exprs`, `stmts`, `blocks`, or
  `items` arena to find its work units.

### 5.3 HIR Traversal Helpers

`etas_hir` should provide reusable traversal helpers built on `HirTreeView`:

```rust
pub trait HirVisitor {
    fn enter_module(&mut self, module: ModuleView<'_>) {}
    fn exit_module(&mut self, module: ModuleView<'_>) {}
    fn enter_item(&mut self, item: ItemView<'_>) {}
    fn exit_item(&mut self, item: ItemView<'_>) {}
    fn enter_body(&mut self, body: BodyView<'_>) {}
    fn exit_body(&mut self, body: BodyView<'_>) {}
    fn enter_block(&mut self, block: BlockView<'_>) {}
    fn exit_block(&mut self, block: BlockView<'_>) {}
    fn enter_expr(&mut self, expr: ExprView<'_>) {}
    fn exit_expr(&mut self, expr: ExprView<'_>) {}
}

pub fn walk_module(view: HirTreeView<'_>, module: HirModuleId, visitor: &mut impl HirVisitor);
pub fn walk_item(view: HirTreeView<'_>, item: HirItemId, visitor: &mut impl HirVisitor);
pub fn walk_body(view: HirTreeView<'_>, body: HirBodyRef, visitor: &mut impl HirVisitor);
pub fn walk_block(view: HirTreeView<'_>, block: HirBlockId, visitor: &mut impl HirVisitor);
```

Passes that need only one body should use `walk_body`. Project passes should
ask the frontend pass manager for affected `HirBodyRef`s or `HirModuleId`s,
then traverse those subtrees. This keeps incremental recomputation and
diagnostics unit-aware.

## 6. Symbols

HIR must distinguish declaration kinds.

```rust
pub struct SymbolId(pub u32);

pub struct Symbol {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub visibility: Visibility,
    pub defining_module: HirModuleId,
    pub defining_item: Option<HirItemId>,
    pub def: SymbolDef,
    pub declared_type: Option<HirTypeId>,
    pub definition_span: Span,
}
```

```rust
pub enum SymbolKind {
    Module,
    TypeAlias,
    Type,
    Enum,
    EnumVariant,
    Spec,
    Flow,
    Agent,
    Tool,
    ResourceHandle,
    SupportType,
    Protocol,
    Effect,
    EffectAction,
    Param,
    Local,
    Field,
    TypeParam,
    EffectParam,
    Import,
    StdPreludeAlias,
}
```

`SymbolKind` is only the coarse category. Type checking, abstract
interpretation, and direct interpretation need a symbol to recover the
declaration or binding that introduced it. HIR should therefore keep an explicit
definition payload:

```rust
pub enum SymbolDef {
    Module {
        module: HirModuleId,
    },
    Item {
        item: HirItemId,
    },
    TopLevelLet {
        item: HirItemId,
        ty: Option<HirTypeId>,
        initializer: HirExprId,
        classification: TopLevelLetClassification,
    },
    Param {
        owner: HirItemId,
        param_index: u32,
        pattern: Option<HirPatId>,
        ty: Option<HirTypeId>,
    },
    Local {
        binding: HirStmtId,
        pattern: HirPatId,
        ty: Option<HirTypeId>,
        initializer: Option<HirExprId>,
    },
    PatternBinding {
        owner: PatternBindingOwner,
        pattern: HirPatId,
        ty: Option<HirTypeId>,
        initializer: Option<HirExprId>,
    },
    Field {
        owner: HirItemId,
        field_index: u32,
        ty: HirTypeId,
    },
    TypeParam {
        owner: HirItemId,
        param_index: u32,
    },
    EffectParam {
        owner: HirItemId,
        param_index: u32,
    },
    EnumVariant {
        enum_item: HirItemId,
        variant_index: u32,
    },
    EffectAction {
        owner: HirItemId,
        action_index: u32,
    },
    ImportAlias {
        path: Vec<String>,
        origin: ImportAliasOrigin,
    },
    Synthetic {
        reason: SyntheticSymbolReason,
    },
    Error,
}

pub enum ImportAliasOrigin {
    SourceImport,
    StdPrelude,
}

pub enum PatternBindingOwner {
    MatchArm { expr: HirExprId },
    Handler { expr: HirExprId },
    For { stmt: HirStmtId },
}

pub enum TopLevelLetClassification {
    Unknown,
    Const,
    ResourceHandle(ResourceKind),
    Invalid,
}

pub enum ResourceKind {
    MemoryRegion,
    ExternalTool,
    HostActionGrant,
    Other,
}
```

Important invariants:

- every flow/agent/tool parameter symbol must store its lowered parameter type
  in `SymbolDef::Param.ty` or `Symbol.declared_type`;
- local binding symbols should point back to the `let`/`var` statement and
  pattern that introduced them;
- pattern binding symbols should point back to the match arm, handler arm, or
  loop that introduced them;
- item symbols should point to their `HirItemId`;
- field and enum variant symbols should point to their owning type item and
  ordinal;
- symbols introduced by recovery or desugaring must be marked `Synthetic` or
  `Error`, not silently look like source declarations.

This avoids forcing later crates to rediscover declaration structure by walking
all HIR items or guessing from `SymbolKind`.

`SymbolKind` and `SymbolDef` together should be precise enough for:

- hover;
- go-to-definition;
- completion;
- type checking;
- effect checking;
- direct HIR interpretation;
- abstract interpretation;
- FIR/AIR lowering;
- user-facing diagnostics.

## 7. Scopes

HIR owns scope construction.

```rust
pub struct ScopeId(pub u32);

pub struct Scope {
    pub id: ScopeId,
    pub parent: Option<ScopeId>,
    pub owner: ScopeOwner,
    pub symbols: Vec<SymbolId>,
    pub span: Span,
}
```

```rust
pub enum ScopeOwner {
    Module(HirModuleId),
    Item(HirItemId),
    Block(HirBlockId),
    Lambda(HirExprId),
    Handler(HirExprId),
    MatchArm(HirExprId),
}
```

Scopes should cover:

- module items;
- type parameters;
- flow parameters;
- local `let` and `var` bindings;
- lambda parameters;
- block-local bindings;
- match arm patterns;
- handler arm patterns.

`ScopeTree` is a lexical binding and lookup structure. It is not the complete
HIR traversal API. A scope owner can point at a module, item, block, lambda,
handler, or match arm, but semantic passes should use `HirTreeView` to walk HIR
nodes and `HirScopeView` only when they need enclosing-scope, child-scope, or
name-lookup information.

```rust
pub struct HirScopeView<'hir> {
    pub tree: &'hir ScopeTree,
    pub hir_tree: &'hir HirTreeIndex,
}

impl HirScopeView<'_> {
    pub fn parent(&self, scope: ScopeId) -> Option<ScopeId>;
    pub fn children(&self, scope: ScopeId) -> &[ScopeId];
    pub fn owner(&self, scope: ScopeId) -> ScopeOwner;
    pub fn enclosing_scope(&self, node: HirNodeRef) -> Option<ScopeId>;
    pub fn lookup(&self, scope: ScopeId, name: &str) -> ResolveResult;
}
```

`HirScopeView` must be built from `ScopeTree` plus `HirTreeIndex`, not by
rescanning all symbols or HIR nodes. This gives type checking, effect
checking, policy checking, IDE queries, and interpreter diagnostics a uniform
way to move between source-shaped HIR structure and lexical scopes.

Name lookup should produce a resolved reference or a diagnostic:

```rust
pub enum ResolveResult {
    Resolved(SymbolId),
    PartiallyResolved(PartialResolution),
    Unresolved,
    Ambiguous(Vec<SymbolId>),
}

pub struct PartialResolution {
    pub resolved_prefix: Option<SymbolId>,
    pub resolved_segments: u32,
    pub remaining: Vec<String>,
    pub reason: PartialResolutionReason,
}

pub enum PartialResolutionReason {
    MemberRequiresTypeChecking,
    ModuleMemberMissing,
    UnsupportedPathShape,
    PackageResolverRequired,
}
```

`PartiallyResolved` is not a successful reference to the full path. It records
that a prefix was meaningful while the remaining segments still need type
checking, module/package resolution, or a better diagnostic.

## 8. Modules and Imports

The syntax layer preserves import source shape as `ast::ImportTree`. HIR must
normalize that shape into data that is convenient for scope construction, name
resolution, diagnostics, and later public API metadata.

HIR must support project-level module graphs. A single `HirProgram` may contain
many `HirModule`s from different source files plus virtual standard-library or
dependency modules represented through resolver metadata. Single-file lowering
is only a convenience mode; it must not be the semantic model for package
compilation.

```rust
pub struct HirModule {
    pub id: HirModuleId,
    pub name: Option<ResolvedPath>,
    pub imports: Vec<HirImport>,
    pub items: Vec<HirItemId>,
    pub scope: ScopeId,
    pub span: Span,
}

pub struct HirImport {
    pub source: HirImportSource,
    pub kind: HirImportKind,
    pub target: ResolvedPath,
    pub binding: Option<HirImportBinding>,
    pub visibility: Visibility,
    pub span: Span,
}

pub enum HirImportKind {
    Single,
    GroupMember,
    Wildcard,
}

pub enum HirImportSource {
    Single {
        path_span: Span,
    },
    GroupMember {
        prefix: Vec<PathSegment>,
        member: PathSegment,
        group_span: Span,
    },
    Wildcard {
        prefix: Vec<PathSegment>,
        star_span: Span,
    },
}

pub struct HirImportBinding {
    pub symbol: SymbolId,
    pub local_name: String,
    pub local_name_span: Span,
    pub is_alias: bool,
}
```

HIR should preserve enough source import shape for diagnostics while also
storing the normalized target path used by name resolution:

- module declaration;
- package and module identity;
- named item imports;
- optional alias;
- grouped imports;
- wildcard imports;
- `public import` re-exports;
- qualified path lookup.

Import normalization rules:

- `import std.io;` becomes one `HirImportKind::Single` whose effective local
  binding is `io`.
- `import std.io as io;` becomes one `Single` whose binding is marked
  `is_alias = true`.
- `import std.io.println;` becomes one `Single` whose effective local binding is
  `println`.
- `import std.io.println as log;` becomes one `Single` whose binding is `log`
  and whose target remains `std.io.println`.
- `import std.io.{print, println};` becomes two `GroupMember` imports, both
  carrying the shared prefix `std.io` and their member-local spans.
- `import std.io.{println as log, read_line,};` becomes two `GroupMember`
  imports. The trailing comma is not represented in HIR.
- `import std.io.*;` becomes one `Wildcard` import source. It does not allocate
  symbols for every exported member during lowering.
- `public import ...;` carries `Visibility::Public` and is later treated as a
  re-export.

Symbol rules:

- explicit single imports and explicit grouped members allocate one
  `SymbolKind::Import` symbol for the effective local name, with
  `ImportAliasOrigin::SourceImport`;
- `SymbolDef::ImportAlias.path` stores the target path segments, even when the
  local name is not written with `as`;
- standard-library prelude names are resolved lazily. A prelude name allocates a
  `SymbolKind::StdPreludeAlias` with `ImportAliasOrigin::StdPrelude` only when
  source code actually references that name. Unused prelude entries must not be
  materialized into the module symbol table, and prelude aliases must not be
  inserted into `HirModule.imports`;
- wildcard imports do not allocate member symbols during lowering;
- duplicate explicit local import names are diagnostics in HIR/name resolution;
- wildcard ambiguity is diagnosed when a name use is resolved, not when the
  wildcard import is parsed.

Pass boundary:

- parsing grouped imports, aliases, trailing commas, and wildcard imports is
  the responsibility of `etas_syntax`;
- `HirLowerPass` or the HIR lowering entry point normalizes `ast::ImportTree`
  into `HirImport`;
- `ImportResolutionPass` or `NameResolutionPass` resolves module/item targets,
  handles package resolver requirements, expands wildcard visibility rules, and
  reports unresolved or ambiguous imports;
- there should be no separate passes named after grammar details such as
  grouped import or trailing comma.

Package resolution starts from `etas.toml` metadata and dependency lockfiles.
Module and import paths are logical paths, not raw filesystem paths. The default
resolver may map `foo.bar` to `src/foo/bar.es` or `src/foo/bar/mod.es`, but HIR
should store the resolved package/module/item identity, not the chosen file path
as the semantic reference.
HIR should keep resolved paths explicit so later type/effect checking, public
API metadata generation, LSP, and AIR lowering do not need to rediscover package
or module identity from raw dotted strings.

### 8.1 Project-Level Lowering

Project HIR lowering must lower all project modules into one `HirProgram`.

Recommended entry points:

```rust
pub fn lower_project(input: HirProjectInput) -> HirProgram;

pub struct HirProjectInput {
    pub parsed_modules: Vec<ParsedModule>,
    pub module_index: ModuleIndex,
}

pub struct ParsedModule {
    pub module_id: ModuleId,
    pub source_id: SourceId,
    pub ast: ast::Program,
}
```

`lower_program(ast::Program)` may remain for unit tests and scratch buffers, but
it should wrap a one-module `HirProjectInput`. Production frontend code should
use project lowering.

Required project lowering order:

1. allocate all `HirModuleId`s from the `ModuleIndex`;
2. allocate all module scopes;
3. insert module symbols for every source and virtual module;
4. predeclare every top-level item symbol in every source module before any
   item body is lowered;
5. store visibility on item and import symbols;
6. lower all import trees into `HirImport` entries;
7. lower item bodies after predeclaration so cross-file and mutually recursive
   references are not source-order dependent;
8. run import and path resolution using the project `ModuleResolver`.

The HIR crate may own the low-level lowering algorithm, but package discovery,
manifest parsing, source collection, and file-system path mapping belong to the
frontend/driver layer that constructs `ModuleIndex`.

### 8.2 Module And Export Identity

HIR should distinguish source names from resolved identities:

```rust
pub struct ModuleId(pub u32);
pub struct PackageId(pub u32);

pub enum ModuleOrigin {
    Source { source: SourceId },
    StdVirtual,
    Dependency { package: PackageId },
}

pub struct Export {
    pub name: String,
    pub symbol: SymbolId,
    pub visibility: Visibility,
    pub source: ExportSource,
}

pub enum ExportSource {
    Declared,
    ReExported { import: HirImportId },
}
```

The exact type names can evolve, but HIR facts must be rich enough to answer:

- which module owns this symbol;
- whether the symbol is public;
- whether a public name was declared locally or re-exported;
- which import introduced a local binding;
- which package/module/item identity an import resolved to.

### 8.3 Import Resolution Against Project Modules

Import resolution should be a project-aware name-resolution phase:

```text
current package modules
  + standard-library virtual modules
  + dependency modules
  -> export tables
  -> local import bindings
  -> resolved paths
```

Rules:

- explicit single imports and grouped import members bind one local name;
- explicit imports may reference only exported/public items from another module;
- importing a module itself binds the module name or alias;
- wildcard imports contribute public exported names lazily;
- public imports create re-export metadata for package API generation;
- local declarations and explicit imports take priority over wildcard imports;
- multiple wildcard providers for the same unqualified name are ambiguous when
  the name is used;
- missing modules and missing exported items produce diagnostics at the import
  source span.

## 9. Resolved References

HIR should use resolved references instead of raw syntax paths where possible.

```rust
pub struct ResolvedPath {
    pub syntax_path: ast::Path,
    pub segments: Vec<PathSegment>,
    pub resolution: ResolveResult,
    pub span: Span,
}

pub struct PathSegment {
    pub name: String,
    pub span: Span,
}

pub struct ResolvedSymbol {
    pub symbol: SymbolId,
    pub span: Span,
}
```

For unresolved code, keep the original syntax path and attach diagnostics. HIR
must still be buildable for incomplete programs so LSP can continue working.

Qualified path resolution must distinguish these cases:

```text
Resolved
  std.text.len -> SymbolId(flow std.text.len)

PartiallyResolved(MemberRequiresTypeChecking)
  paper.authors.len
  prefix: SymbolId(local paper)
  remaining: ["authors", "len"]

PartiallyResolved(ModuleMemberMissing)
  std.text.missing
  prefix: SymbolId(module std.text)
  remaining: ["missing"]

Unresolved
  completely_unknown.name

Ambiguous
  name exists in multiple visible scopes/imports
```

This distinction is required because field access, method calls, associated
items, and package/module paths are not all the same operation. HIR resolves
lexical/module prefixes; `etas_types` can later finish type-directed member
resolution using the recorded prefix and remaining segments.

## 10. HIR Items

```rust
pub enum HirItem {
    TypeAlias(HirTypeAliasDecl),
    Type(HirTypeDecl),
    Enum(HirEnumDecl),
    Spec(HirSpecDecl),
    Impl(HirImplDecl),
    Effect(HirEffectDecl),
    Tool(HirToolDecl),
    Agent(HirAgentDecl),
    Protocol(HirProtocolDecl),
    Flow(HirFlowDecl),
    TopLevelLet(HirTopLevelLetDecl),
    Error { span: Span },
}
```

`TypeAlias` and `Type` are separate HIR item kinds. `alias A = B;` is a
transparent source abbreviation; it must be recorded for imports, exports,
diagnostics, package metadata, and source navigation, but it must not allocate a
new nominal type identity. `type A = B;` and `type A;` both allocate a nominal
type constructor. A named record declaration such as `type Review = { ... }`
is nominal even though its representation is a record shape.

Each item should store:

- its declaring symbol;
- source span;
- child scopes when applicable;
- links to lowered type/expression/block ids.

Example:

```rust
pub struct HirTopLevelLetDecl {
    pub symbol: SymbolId,
    pub visibility: Visibility,
    pub type_annotation: Option<HirTypeId>,
    pub value: HirExprId,
    pub classification: TopLevelLetClassification,
    pub span: Span,
}
```

Type declarations preserve the alias/nominal split:

```rust
pub struct HirTypeAliasDecl {
    pub symbol: SymbolId,
    pub visibility: Visibility,
    pub type_params: Vec<SymbolId>,
    pub target: HirTypeId,
    pub span: Span,
}

pub struct HirTypeDecl {
    pub symbol: SymbolId,
    pub visibility: Visibility,
    pub type_params: Vec<SymbolId>,
    pub body: HirTypeDeclBody,
    pub span: Span,
}

pub enum HirTypeDeclBody {
    Bodyless,
    Representation(HirTypeId),
}
```

Lowering must not normalize `type A = B;` into an alias and must not erase
`alias A = B;` before HIR is built. Alias expansion is a type-checker operation
performed through alias facts with cycle diagnostics; nominal identity is a
signature fact owned by `etas_types`.

Top-level `let` is lowered as an item because it participates in module
exports, imports, path resolution, and project-level facts. It is distinct from
block-scoped `HirStmt::Let`. Later semantic passes must classify it as either
a deterministic/effect-free constant or a compiler-known resource handle. A
persistent memory region is a `ResourceHandle(MemoryRegion)` introduced by a
top-level `let` whose value resolves to a standard constructor such as
`std.memory.region[Schema](...)`; there is no `HirMemoryDecl`.

Effect declarations preserve the tag/action split from the SPEC:

Spec declarations preserve static evidence constraints from the SPEC. HIR must
distinguish type specs, callable specs, and trace specs because they have
different targets and different checking rules:

```rust
pub struct HirSpecDecl {
    pub symbol: SymbolId,
    pub type_params: Vec<SymbolId>,
    pub kind: HirSpecKind,
    pub span: Span,
}

pub enum HirSpecKind {
    TypeSpec {
        entails: Vec<HirSpecRef>,
        items: Vec<HirSpecItem>,
    },
    CallableSpec {
        input: HirTypeId,
        output: HirTypeId,
        effects: HirCallableSpecEffects,
    },
    TraceSpec {
        expr: HirSpecExprId,
    },
}

pub enum HirSpecItem {
    FlowSignature(HirSpecFlowSig),
    Error { span: Span },
}

pub enum HirCallableSpecEffects {
    Unconstrained,
    Closed(HirEffectRow),
}

pub struct HirSpecFlowSig {
    pub symbol: SymbolId,
    pub type_params: Vec<SymbolId>,
    pub params: Vec<SymbolId>,
    pub return_type: HirTypeId,
    pub declared_effects: Option<HirEffectRow>,
    pub span: Span,
}
```

Marker type specs such as `ByteStream` and `Index` have no items. Behavioral
type specs such as `PromptEncode`, `Schema`, and `ResponseDecode` keep required
flow signatures in HIR. Flow specs such as `Pure[I, O] I => O ![]` keep a
callable shape rather than method items. Spec satisfaction is a separate
type-checker judgment; HIR must not model a spec as a concrete supertype or
insert implicit casts.

```rust
pub struct HirEffectDecl {
    pub symbol: SymbolId,
    pub type_params: Vec<SymbolId>,
    pub extends: Option<HirEffectRef>,
    pub actions: Vec<HirEffectActionDecl>,
    pub span: Span,
}

pub struct HirEffectActionDecl {
    pub symbol: SymbolId,
    pub type_params: Vec<SymbolId>,
    pub params: Vec<SymbolId>,
    pub output_type: HirTypeId,
    pub span: Span,
}

pub struct HirImplDecl {
    pub target: HirImplTarget,
    pub items: Vec<HirImplItem>,
    pub span: Span,
}

pub enum HirImplTarget {
    Inherent { ty: HirTypeId },
    Effect { effect: HirEffectRef },
    SpecSatisfaction {
        self_type: HirTypeId,
        specs: Vec<HirSpecRef>,
    },
    Error,
}

pub struct HirSpecRef {
    pub path: ResolvedPath,
    pub args: Vec<HirTypeId>,
    pub span: Span,
}

pub enum HirImplItem {
    Flow(HirFlowDecl),
    Action(HirEffectActionDecl),
    Error { span: Span },
}
```

HIR may preserve `effect Name { action ... }`, `impl EffectName { action ... }`,
inherent type impls, canonical `impl TypeName ~ SpecName { ... }`, and
compatibility `impl SpecName for TypeName { ... }`, but semantic checking must
kind-check the impl target. Both spec impl spellings lower to one
spec-satisfaction model; the spelling is retained only for diagnostics,
formatting, and migration tooling. Type impls may contain only flow methods.
Spec impls may contain only the spec-required flow methods and no extra members.
Marker spec impls have no items. Effect impls may contain only action
signatures with no Etas body. Action symbols should use
`SymbolDef::EffectAction` so `perform` and handler arms can resolve to a stable
action identity.

```rust
pub struct HirFlowDecl {
    pub symbol: SymbolId,
    pub type_params: Vec<SymbolId>,
    pub params: Vec<SymbolId>,
    pub return_type: Option<HirTypeId>,
    pub effects: Option<HirEffectRow>,
    pub conformances: Vec<HirDeclarationConformance>,
    pub body: HirFlowBody,
    pub scope: ScopeId,
    pub span: Span,
}

pub struct HirDeclarationConformance {
    pub target: HirDeclarationConformanceTarget,
    pub span: Span,
}

pub enum HirDeclarationConformanceTarget {
    Path(HirSpecRef),
    InlineTraceSpec(HirSpecExpr),
    Error { span: Span },
}
```

Flow, agent, and tool declarations may satisfy callable specs and trace specs
directly with declaration conformances such as
`flow Normalize(...) -> string ~ Pure { ... }`,
`flow Save(...) ~ SafeWriteTrace { ... }`, or
`flow Save(...) ~ (+Workspace.write<\"reports/**\"> & -Command.run<_>) { ... }`.
These conformances must be preserved in HIR and checked by `etas_types` and
`etas_effects` against the declared or inferred callable signature and
requested-action trace. They are not top-level `impl` items, and obsolete
`follows` or `policy { ... }` source syntax lowers only to an error conformance
for recovery.

`return_type` and `declared_effects` remain optional at HIR. Type/effect
checking infers them when omitted. `declared_effects` is lowered from the source
effect suffix after the output type, not from a declaration conformance.

Agent declarations keep the same high-level shape as source syntax: a typed
first-class agent item, item annotations that carry model/runtime metadata, and
an optional body that builds the model-call prompt. Annotation names such as
`model`, `tools`, `limits`, `trace`, and `optimization` are preserved as
structured HIR annotations, not config rows or declaration conformances.

HIR must preserve the SPEC boundary: an agent is one model inference boundary.
Its body is a context harness and prompt assembly block whose checked result is
`Prompt`; it is not a general orchestration body. Multi-step orchestration,
approval gates, durable writes, and post-output validation stay in `flow`.

```rust
pub struct HirAgentDecl {
    pub symbol: SymbolId,
    pub params: Vec<SymbolId>,
    pub output_type: Option<HirTypeId>,
    pub effects: Option<HirEffectRow>,
    pub conformances: Vec<HirDeclarationConformance>,
    pub body: HirAgentBody,
    pub scope: ScopeId,
    pub span: Span,
}

pub enum HirAgentBody {
    Source { block: HirBlockId },
    Decl { span: Span },
    Error { span: Span },
}
```

Type checking verifies that the agent body produces `Prompt`, infers body
effects, validates compiler-known annotations such as `@model`, `@tools`, and
`@limits`, and records the internal `Agent[Input, Output, Effects, Context, Config]`
representation for downstream analysis.
The `Effects` position is an effect-row generic argument, not a `HirTypeId`.

Agent calls are represented in source/HIR as ordinary method calls such as
`Writer.run(topic)` or as pipeline stage applications whose stage type resolves
to an agent. HIR should not introduce a syntax-only `AgentCall` node. Later
type/effect checking classifies resolved calls and stages as agent calls, flow
calls, tool calls, support calls, or invalid calls.

## 11. HIR Types

HIR type refs should preserve arrow type structure and resolved names.

```rust
pub enum HirType {
    Handler {
        handled: HirEffectRow,
        produced: HirHandlerProducedEffects,
        result: Option<HirTypeId>,
        span: Span,
    },
    Arrow {
        effect: Option<HirEffectRow>,
        input: HirTypeId,
        output: HirTypeId,
        span: Span,
    },
    Primitive {
        kind: PrimitiveType,
        span: Span,
    },
    Path {
        path: ResolvedPath,
        args: Vec<HirGenericArg>,
        span: Span,
    },
    Record {
        fields: Vec<HirFieldDecl>,
        span: Span,
    },
    Tuple {
        elems: Vec<HirTypeId>,
        span: Span,
    },
    Error {
        span: Span,
    },
}
```

HIR keeps arrow types as arrow types. The type checker normalizes them to
`Flow[I, O, E]`.

Generic arguments are kinded. A source `Path[...]` argument list can contain
ordinary value types and, for flow/effect abstractions, effect-row arguments.
HIR should therefore avoid `Vec<HirTypeId>` for generic positions that may carry
`effect E`:

```rust
pub enum HirGenericArg {
    Type(HirTypeId),
    EffectRow(HirEffectRow),
    Error { span: Span },
}

pub struct HirEffectRow {
    pub effects: Vec<HirEffectRef>,
    pub tail: Option<SymbolId>, // an `effect E` parameter
    pub span: Span,
}
```

`![Console.stdout_write, E]` lowers to an effect row with a concrete effect
entry and a tail variable. The frontend must preserve the row variable so
`etas_types` can kind-check it and `etas_effects` can instantiate it at calls.

HIR keeps handler transformer types as handler types. `![H]`,
`![H => E]`, and `![H => E for R]` must not be normalized into flow types.
They describe reusable handler values whose handled action/effect set,
produced effect upper bound, and optional result type are consumed by
`etas_types`, `etas_effects`, and the interpreter.

```rust
pub enum HirHandlerProducedEffects {
    Infer,
    Explicit(HirEffectRow),
}
```

## 12. HIR Expressions

HIR expressions are source-shaped but resolved.

```rust
pub enum HirExpr {
    Literal(HirLiteral),
    Path(ResolvedPath),
    Record(HirRecordExpr),
    Tuple { elems: Vec<HirExprId>, span: Span },
    Array { elems: Vec<HirExprId>, span: Span },
    List { elems: Vec<HirExprId>, span: Span },
    ListCons {
        head: HirExprId,
        tail: HirExprId,
        span: Span,
    },
    EmptySequence { span: Span },
    Map {
        entries: Vec<HirMapEntry>,
        span: Span,
    },
    Set { elems: Vec<HirExprId>, span: Span },
    Range {
        start: HirExprId,
        end: HirExprId,
        bounds: HirRangeBounds,
        span: Span,
    },
    Call {
        callee: HirExprId,
        generic_args: Vec<HirGenericArg>,
        args: Vec<HirArg>,
        span: Span,
    },
    MethodCall {
        receiver: HirExprId,
        method: String,
        generic_args: Vec<HirGenericArg>,
        args: Vec<HirArg>,
        span: Span,
    },
    Perform {
        action: ResolvedActionRef,
        action_args: Vec<HirEffectArg>,
        args: Vec<HirArg>,
        span: Span,
    },
    Handle {
        body: HirExprId,
        handler: HirExprId,
        span: Span,
    },
    Handler {
        handlers: Vec<HirHandlerArmId>,
        span: Span,
    },
    StageCompose {
        stages: Vec<HirStage>,
        span: Span,
    },
    Pipeline {
        input: HirExprId,
        stages: Vec<HirStage>,
        span: Span,
    },
    Field {
        base: HirExprId,
        field: String,
        span: Span,
    },
    Index {
        base: HirExprId,
        index: HirExprId,
        span: Span,
    },
    Slice {
        base: HirExprId,
        start: HirExprId,
        end: HirExprId,
        bounds: HirRangeBounds,
        span: Span,
    },
    Try {
        expr: HirExprId,
        span: Span,
    },
    Unary {
        op: UnaryOp,
        expr: HirExprId,
        span: Span,
    },
    Binary {
        op: BinaryOp,
        lhs: HirExprId,
        rhs: HirExprId,
        span: Span,
    },
    If {
        cond: HirExprId,
        then_block: HirBlockId,
        else_branch: Option<HirElseBranch>,
        span: Span,
    },
    Match {
        scrutinee: HirExprId,
        arms: Vec<HirMatchArm>,
        span: Span,
    },
    Lambda {
        params: Vec<SymbolId>,
        body: HirLambdaBody,
        scope: ScopeId,
        span: Span,
    },
    Block(HirBlockId),
    Error { span: Span },
}
```

Collection HIR remains source-shaped but semantically explicit. Lowering must
not collapse `[a, b]`, `[a; b]`, `a :: xs`, `#{a}`, `{ k => v }`, and `[a, b)`
into a single generic container node. The type checker, interpreter, and later
lowering need the original collection kind for contextual typing, diagnostics,
and execution.

```rust
pub struct HirMapEntry {
    pub key: HirExprId,
    pub value: HirExprId,
    pub span: Span,
}

pub enum HirRangeBounds {
    ClosedOpen,
    OpenClosed,
}
```

`HirExpr::EmptySequence` represents source `[]`. It must remain unresolved until
type checking supplies context and records whether it is `Array[T]` or `List[T]`.
HIR lowering must not default `[]` to either collection kind.

`HirExpr::Index` and `HirExpr::Slice` preserve source-shaped indexing and
slicing operations. They must not encode `Index` constraint satisfaction
directly; that is a type-checking fact. After type checking, `TypeFacts` should
distinguish:

- sequence indexing such as `Array[T][I]`, `Slice[T][I]`, and `bytes[I]`,
  where `I` satisfies the standard compiler-owned `Index` spec;
- map lookup such as `Map[K, V][K]`, where the index expression is checked
  against key type `K` and does not use `Index`.
- slicing such as `Array[T][I, I)`, `Slice[T][I, I)`, `bytes[I, I)`, and
  `Range[I][I, I)`, preserving bound inclusivity.

Interpreters, later lowering, and diagnostics should consume the checked index
or slice fact instead of re-inferring whether an operation is a sequence access,
map lookup, or slice from HIR shape alone.

`HirExpr::Try` represents postfix source `expr?`. It is source-shaped HIR for
the effect-to-value boundary defined by the PL SPEC:

```text
e  : T ! [Error[E], ...]
e? : Result[T, E] ! [...]
```

HIR lowering must not rewrite `e?` into a `match` on `Result`, an `unwrap`, a
call to a standard-library helper, or a synthetic `handle` block. It also must
not lower `e?` into `Error[E].raise(...)`. The node preserves the source
boundary so `etas_types` can record the `Result[T, E]` value shape and
`etas_effects` can validate and remove the captured `Error[E]` effect. A
value-level `Result[T, E]` operand is not sufficient for `?`; that invalid case
is diagnosed after type/effect facts are available.

The operand of `HirExpr::Try` may be any expression, including a `HirExpr::Block`.
This represents source such as `{ let text = fs.read(path); parse(text) }?`.
`HirExpr::Try` is not a statement terminator: semicolons belong to surrounding
statements, and a block's final `HirExpr::Try` participates in the block result
like any other final expression.

`HirExpr::Handle` represents `handle Expr with HandlerExpr`. The handled body is
an expression id, not only a block id, because the SPEC allows
`handle Search() with ChooseFirst` and `handle { ... } with { ... }`. A block
after `with` is lowered into a normal `HirExpr::Handler` and then used as the
handler expression. HIR does not retain a separate inline handler argument:

```rust
pub struct HirHandlerArm {
    pub action: ResolvedActionRef,
    pub action_args: Vec<HirEffectArg>,
    pub patterns: Vec<HirPatId>,
    pub body: HirBlockId,
    pub scope: ScopeId,
    pub span: Span,
}
```

```rust
pub enum HirEffectArg {
    Type(HirTypeId),
    MemoryPlace(ResolvedPath),
    ValuePath(ResolvedPath),
    Literal(HirLiteral),
    EffectRow(HirEffectRow),
    Error { span: Span },
}
```

`HirEffectArg` is still source-shaped. The final meaning is assigned by
registry/type-directed lowering in `etas_types` and `etas_effects`. The HIR
lowerer must not guess that a bare identifier is always a type, nor silently
reinterpret an unresolved argument to keep analysis moving.

`HirExpr::Handler` creates a first-class handler value. It is valid in ordinary
expression positions such as top-level immutable `let` initializers, parameters,
returns, and record fields. It does not execute the handled computation and does
not grant authority. Effect checking owns the handled action set, produced
effects, resume legality, and result-type compatibility.

`Pipeline` and `StageCompose` remain explicit. HIR does not decide whether a
stage is valid for a given input type; type/effect checking does that. HIR only
resolves what each stage expression refers to when possible.

```rust
pub struct HirStage {
    pub expr: HirExprId,
    pub limits: Vec<HirExprId>,
    pub span: Span,
}
```

## 13. HIR Statements and Blocks

```rust
pub struct HirBlock {
    pub id: HirBlockId,
    pub stmts: Vec<HirStmtId>,
    pub final_expr: Option<HirExprId>,
    pub scope: ScopeId,
    pub span: Span,
}

pub enum HirStmt {
    Let {
        pat: HirPatId,
        type_annotation: Option<HirTypeId>,
        value: HirExprId,
        span: Span,
    },
    Var {
        pat: HirPatId,
        type_annotation: Option<HirTypeId>,
        value: HirExprId,
        span: Span,
    },
    Assign {
        target: HirExprId,
        value: HirExprId,
        span: Span,
    },
    If(HirExprId),
    Match(HirExprId),
    For {
        pat: HirPatId,
        iter: HirExprId,
        limits: Vec<HirExprId>,
        body: HirBlockId,
        span: Span,
    },
    While {
        cond: HirExprId,
        limits: Vec<HirExprId>,
        body: HirBlockId,
        span: Span,
    },
    Retry {
        limits: Vec<HirExprId>,
        body: HirBlockId,
        span: Span,
    },
    Resume {
        value: Option<HirExprId>,
        span: Span,
    },
    Return {
        value: Option<HirExprId>,
        span: Span,
    },
    Break { span: Span },
    Continue { span: Span },
    Expr {
        expr: HirExprId,
        span: Span,
    },
    Error { span: Span },
}
```

The statement model should preserve source constructs. AIR lowering decides
whether expressions become explicit control/data graph nodes.

`HirStmt::Let` is a block-scoped immutable binding. It does not participate in
module exports or project import resolution. `HirStmt::Var` is also
block-scoped and mutable. A `var` at item position is invalid source and should
only survive as an error item/diagnostic during recovery.

`If` and `Match` intentionally exist as expressions and may also appear in
statement position through `HirStmt::If(HirExprId)` and
`HirStmt::Match(HirExprId)`. This preserves source shape without duplicating
the branch data model, but later phases must follow these rules:

- `HirStmt::If(expr)` and `HirStmt::Match(expr)` execute/evaluate the referenced
  expression for effects and control flow, then discard its value;
- expression-position `HirExpr::If` and `HirExpr::Match` must have branch result
  types unified by `etas_types`;
- statement-position `if`/`match` may type check as `unit` after discarding the
  expression value, but branch bodies are still checked normally;
- only `HirBlock.final_expr` contributes to a block result;
- a trailing `HirStmt::Expr`, `HirStmt::If`, or `HirStmt::Match` is still a
  statement and does not become the block result unless lowering placed it in
  `final_expr`;
- direct HIR interpretation must mirror this rule: statement-position
  expression values are dropped, while `final_expr` is returned as the block
  value.

This contract lets HIR stay source-shaped while preventing the interpreter from
accidentally returning the value of a statement-position conditional.

## 14. Patterns

Patterns introduce symbols.

```rust
pub enum HirPat {
    Binding {
        symbol: SymbolId,
        span: Span,
    },
    Wildcard {
        span: Span,
    },
    Literal(HirLiteral),
    Tuple {
        elems: Vec<HirPatId>,
        span: Span,
    },
    Record {
        path: Option<ResolvedPath>,
        fields: Vec<HirRecordPatField>,
        span: Span,
    },
    Variant {
        path: ResolvedPath,
        args: Vec<HirPatId>,
        span: Span,
    },
    Error {
        span: Span,
    },
}
```

Lowering from AST to HIR must add pattern bindings to the correct scope.

## 15. Source Mapping

HIR must preserve links back to syntax.

```rust
pub struct SourceMap {
    pub item_sources: Map<HirItemId, HirOrigin>,
    pub expr_sources: Map<HirExprId, HirOrigin>,
    pub stmt_sources: Map<HirStmtId, HirOrigin>,
    pub pat_sources: Map<HirPatId, HirOrigin>,
    pub type_sources: Map<HirTypeId, HirOrigin>,
    pub symbol_sources: Map<SymbolId, HirOrigin>,
    pub syntax_to_hir: Map<SyntaxNodeId, Vec<HirNodeRef>>,
}

pub struct SyntaxNodeRef {
    pub id: SyntaxNodeId,
    pub span: Span,
    pub kind: SyntaxNodeKind,
    pub parent: Option<SyntaxNodeId>,
    pub child_index: u32,
}

pub enum HirOrigin {
    Direct(SyntaxNodeRef),
    Desugared {
        primary: SyntaxNodeRef,
        related: Vec<SyntaxNodeRef>,
        desugaring: DesugaringKind,
    },
    Synthetic {
        span: Span,
        reason: SyntheticOriginReason,
    },
    Error {
        span: Span,
    },
}

pub enum HirNodeRef {
    Item(HirItemId),
    Expr(HirExprId),
    Stmt(HirStmtId),
    Pat(HirPatId),
    Type(HirTypeId),
    Symbol(SymbolId),
}
```

The source map supports:

- diagnostics;
- hover;
- go-to-definition;
- find references;
- semantic tokens;
- AST/HIR dump correlation;
- AST/HIR roundtrip inspection;
- reverse lookup from syntax node to HIR nodes;
- AIR source span mapping.

HIR ids should also carry spans directly for common diagnostic paths, but the
source map is the durable bridge back to syntax.

`Span + SyntaxNodeKind` is enough for diagnostics but not enough for robust
AST/HIR correlation. `SyntaxNodeId` should be stable within one parse result
and deterministic for dumps. If `etas_syntax` does not yet expose durable node
ids, HIR lowering should assign parse-local node ids during lowering and record
the AST parent/child path. That is sufficient for deterministic dumps,
roundtrip debugging, and LSP correlation inside a single snapshot.

## 16. Diagnostics

HIR emits name-resolution and module diagnostics. It should reuse the shared
diagnostic model defined in `etas_core`.

Required diagnostic classes:

- unresolved name;
- ambiguous name;
- duplicate symbol in scope;
- invalid import path;
- import alias conflict;
- invalid `impl` target path shape;
- unresolved effect action path in `perform`;
- unresolved trace-spec/effect/tool/agent references where resolution is
  possible before type checking.

Diagnostics must include:

- primary span on the reference or declaration;
- secondary span on the previous definition for duplicates;
- help text where a likely import or symbol exists;
- suggestions where a typo candidate is reliable enough.

## 17. HIR Dump

`etas_hir` should provide deterministic HIR dump support.

Suggested API:

```rust
pub fn dump_hir(program: &HirProgram, options: HirDumpOptions) -> String;
```

Suggested dump contents:

- module tree;
- HIR tree view and owner links;
- symbol table;
- scopes and parent links;
- items;
- resolved references;
- HIR node ids;
- source spans;
- diagnostics when requested.

Suggested text shape:

```text
HirProgram
  Symbols
    s1 Agent ProductManager @10..82
    s2 Agent Architect @84..150
    s3 Flow BuildFeature @152..260
    s4 Param BuildFeature.brief @170..189
    s5 Local BuildFeature.prd @210..213

  Flow s3 BuildFeature
    params: s4
    return: TypeRef(DesignDoc)
    block b1
      Let s5
        Pipeline
          input PathResolved(s4)
          stage PathResolved(s1)
      Return
        Pipeline
          input PathResolved(s5)
          stage PathResolved(s2)
```

AST dump validates parsing. HIR dump validates resolution and high-level
semantic lowering.

## 18. Relationship to Type, Effects, FIR, and AIR

HIR should be rich enough to support later phases, but it should not perform
their work.

`etas_types` attaches type facts to HIR ids:

```text
HirExprId -> Type
HirTypeId -> Type
SymbolId  -> SymbolTypeFact
```

`SymbolTypeFact` should be able to reference the original symbol definition:

```text
Param symbol -> declared/inferred parameter type
Local symbol -> declared/inferred binding type
Field symbol -> field type
Flow symbol  -> checked flow signature
Agent symbol -> checked agent signature
Tool symbol  -> checked tool signature
```

The type checker should not need to recover a parameter type by walking a flow
body. The `SymbolDef` payload plus `SymbolTypeFact` should make symbol-to-type
queries direct.

`etas_effects` attaches effect facts to HIR ids and symbols:

```text
HirExprId -> EffectSet
HirItemId -> EffectSummary
```

Phase 1 direct interpretation consumes checked HIR, not raw HIR:

```text
HirProgram
  + TypeFacts
  + EffectFacts
  + interpreter support classification
  -> CheckedProgram
  -> etas-interpreter
```

The interpreter may use HIR node ids, symbol definitions, type facts, effect
facts, and source-map origins, but it must not redo name resolution or type
checking. Unsupported runtime behavior should be reported from frontend
classification and repeated defensively by the interpreter before execution.

`etas_analysis` should build FIR from checked HIR when analysis or
transformation needs explicit control-flow and data-flow structure. Temporary
views can still exist for narrow diagnostics, but the durable analysis IR is
FIR, not HIR.

`etas_lowering` lowers checked HIR to FIR, then optimized FIR to AIR:

```text
Hir Pipeline {
  input: brief
  stages: [ProductManager, Architect]
}

FIR:
  n1 = AgentCall(ProductManager, brief) -> prd
  n2 = AgentCall(Architect, prd) -> design
  control: n1 -> n2
  data: brief -> n1, prd -> n2

AIR:
  a1 = PromptBuild(ProductManager, brief) -> prompt0  // execute agent body/context harness
  a2 = AgentCall(ProductManager, prompt0, Schema[ProductRequirements]) -> raw0
  a3 = ValidateSchema(raw0, ProductRequirements) -> prd
  a4 = PromptBuild(Architect, prd) -> prompt1         // execute agent body/context harness
  a5 = AgentCall(Architect, prompt1, Schema[DesignDoc]) -> raw1
  a6 = ValidateSchema(raw1, DesignDoc) -> design
```

## 19. Implementation Scope

`etas_hir` should be designed for the complete semantic surface in the PL
design: source items, bindings, effects, policies, tools, agents, typed
persistent-memory support through resource handles, flows, handlers, and
pipeline forms. The first implementation can prioritize lowering and
resolution in dependency order, but the HIR architecture should be shaped for
the complete language.

The first `etas_hir` implementation should support:

- lowering from `etas_syntax::ast::Program`;
- module declarations and imports;
- item symbol creation;
- symbol definition payloads for items, params, locals, fields, type params,
  and enum variants;
- scopes for modules, flows, blocks, lambdas, match arms, and handlers;
- local and parameter binding;
- resolved paths where possible;
- partial qualified path resolution that distinguishes unresolved paths from
  resolved prefixes requiring type-directed member checking;
- unresolved/ambiguous/duplicate diagnostics;
- source map from HIR ids back to syntax origins and reverse syntax-to-HIR
  lookup for dump correlation;
- statement-position versus expression-position rules for `if` and `match`;
- HIR dump for golden tests;
- source-shaped HIR for types, expressions, statements, patterns, and item
  declarations.

The first implementation should not support:

- explicit CFG/DFG storage;
- FIR node creation;
- AIR node creation;
- type inference;
- effect inference;
- action-grant and trace-spec validation;
- trace-spec dominance/temporal checking;
- package-manager-grade module resolution;
- incremental salsa database unless and until the compiler pipeline needs it.
