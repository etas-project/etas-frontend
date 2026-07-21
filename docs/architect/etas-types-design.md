# Etas Type Checking Design

Status: `Draft`

Owner: `Architect`

Last updated: `2026-07-02`

## 1. Purpose

`etas_types` owns Etas value type representation, type inference, type
checking, generic instantiation, unification, assignability checks, spec
satisfaction, effect-row kind checking, and typed facts over checked HIR.

It consumes source-shaped HIR from `etas_hir` and standard declarations from
`etas_std`. It does not mutate HIR into an interpreter-specific structure.
Instead, it produces facts keyed by HIR ids and symbols.

```text
HirProgram
  + SymbolTable
  + SourceMap
  + StdRegistry
  -> TypeOutput
```

## 2. Boundary

`etas_types` owns:

- type model for values;
- transparent alias expansion and alias-cycle diagnostics;
- nominal type identity for all `type` declarations, including
  representation-backed and bodyless types;
- primitive type registry mapping from `etas_std`;
- generic type constructors and instantiation;
- kinded generic parameters, including `T ~ Spec` and `effect E`;
- type variables and substitutions;
- unification and occurs checks;
- assignability checks;
- type-spec, callable-spec, and trace-spec declaration environments;
- spec satisfaction constraints such as `S ~ ByteStream`, `I ~ Index`,
  `T ~ PromptEncode + Schema`, and `T ~ ResponseDecode`;
- effect-row variables in flow types, handler types, and generic signatures;
- branch type unification for `if` and `match`;
- value-type shaping for postfix `?`, including provisional `Result[T, E]`
  facts;
- flow, agent, tool, method, and support-flow signatures;
- `TypeFacts` and `SymbolTypeFact`;
- type diagnostics.

`etas_types` does not own:

- parsing or HIR construction;
- name resolution;
- effect inference;
- action-grant or trace-spec monitor validation;
- interpreter values or runtime values;
- AIR/FIR construction;
- generic fixpoint algorithms.

Generic lattices, transfers, worklists, fixpoint engines, and graph algorithms
come from `etas_utils`. Type-specific domains remain in `etas_types`.

## 3. Crate Layout

Recommended layout:

```text
crates/etas_types/
  src/
    lib.rs

    api/
      mod.rs
      request.rs
      output.rs

    ty/
      mod.rs
      id.rs
      primitive.rs
      nominal.rs
      row.rs
      scheme.rs
      display.rs

    facts/
      mod.rs
      signatures.rs
      symbol_facts.rs
      type_facts.rs
      expression_facts.rs
      specs.rs
      effect_facts.rs

    lower/
      mod.rs
      type_ref.rs
      effect_row.rs

    constraint/
      mod.rs
      ty.rs
      spec_obligation.rs
      validation.rs
      origin.rs

    solver/
      mod.rs
      unification.rs
      assignability.rs
      callable.rs
      field.rs
      index.rs
      spec_solver.rs
      effect_row.rs
      report.rs

    pipeline/
      mod.rs
      context.rs

      signature/
        mod.rs
        state.rs
        declare/
          nominal_types.rs
          aliases.rs
          specs.rs
          callables.rs
          effect_actions.rs
          imports.rs
        resolve/
          aliases.rs
          spec_closure.rs
          imports.rs
        validate/
          spec_impls.rs
          flow_spec_satisfaction.rs
          exported_contracts.rs
        materialize.rs

      body/
        mod.rs
        state.rs
        generate/
          entry.rs
          block.rs
          stmt.rs
          expr.rs
          pattern.rs
          literal.rs
          record.rs
          call.rs
          method_call.rs
          index.rs
          handler.rs
          perform.rs
          lambda.rs
          loop_stmt.rs
        solve.rs
        validate.rs
        materialize.rs

      finalize/
        mod.rs
        merge.rs
        remap.rs

    diagnostic/
      mod.rs
      codes.rs
      builder.rs
```

Layering:

- `ty` is the type data model.
- `facts` exposes the checked output contract. Final facts are written only by
  materialization passes.
- `lower` lowers HIR type and effect-row references into the internal type
  model. It must not inspect executable bodies.
- `constraint` defines type constraints, spec obligations, and validation
  requests. It is the data boundary between body generation, solving, and
  diagnostic validation.
- `solver` owns unification, assignability, callable application, field/index
  access, spec solving, and effect-row kind/compatibility solving.
- `pipeline/signature` builds declaration-level facts before any body is
  checked. It does not enter flow/tool/agent bodies.
- `pipeline/body` generates constraints from executable HIR bodies, solves them,
  validates diagnostics, and then materializes typed HIR facts.
- `pipeline/finalize` merges body outputs and remaps type stores when the
  frontend schedules bodies independently.
- `diagnostic` builds rendering-neutral diagnostics.

There must be no `check/` module. A recursive `check_expr -> check_stmt ->
check_block` implementation is explicitly stale because it mixes traversal,
type relation solving, diagnostics, and fact materialization in one layer.

## 4. Dependency Direction

Allowed:

```text
etas_types -> etas_core
etas_types -> etas_utils
etas_types -> etas_std
etas_types -> etas_hir
```

Forbidden:

```text
etas_types -> etas_effects
etas_types -> etas_interpreter
etas_types -> etas_runtime
etas_types -> etas_air
etas_types -> etas_cli
```

The effect checker may depend on `etas_types`; the type checker should not
depend back on effect checking.

## 5. Type Model

The source-facing primitive names come from the PL design and `etas_std`:

```text
bool
i8 i16 i32 i64 i128 isize
u8 u16 u32 u64 u128 usize
f32 f64
char
string
bytes
unit
never
```

Recommended internal shape:

```rust
pub struct TypeId(pub u32);
pub struct TypeVarId(pub u32);

pub enum Type {
    Primitive(PrimitiveType),
    Var(TypeVarId),
    Array(TypeId),
    List(TypeId),
    Map { key: TypeId, value: TypeId },
    Set(TypeId),
    Range { index: TypeId },
    Slice(TypeId),
    Option(TypeId),
    Result { ok: TypeId, err: TypeId },
    Record(RecordType),
    Tuple(Vec<TypeId>),
    Enum(EnumTypeRef),
    Nominal(NominalTypeRef),
    Function(FlowType),
    Handler(HandlerType),
    Named(NamedTypeRef),
    Applied { constructor: TypeConstructorId, args: Vec<GenericArgId> },
    Trust { wrapper: TrustWrapper, inner: TypeId },
    Schema(TypeId),
    Prompt,
    PromptPart,
    Message(TypeId),
    Store { key: TypeId, value: TypeId },
    MemoryRegion(TypeId),
    ResourceHandle(ResourceHandleType),
    Agent(AgentType),
}

pub struct NominalTypeRef {
    pub constructor: TypeConstructorId,
    pub args: Vec<GenericArgId>,
}

pub struct FlowType {
    pub input: Vec<TypeId>,
    pub output: TypeId,
    pub effects: Option<EffectRowRef>,
}

pub struct HandlerType {
    pub handled: EffectRowRef,
    pub produced: HandlerProducedEffects,
    pub result: Option<TypeId>,
}

pub enum HandlerProducedEffects {
    Infer,
    Explicit(EffectRowRef),
}

pub struct AgentType {
    pub input: TypeId,
    pub output: TypeId,
    pub effects: Option<EffectRowRef>,
    pub context: Option<TypeId>,
    pub config: Option<TypeId>,
}

pub enum GenericArg {
    Type(TypeId),
    EffectRow(EffectRowRef),
}

pub struct TypeParam {
    pub name: String,
    pub kind: GenericParamKind,
    pub spec_bounds: Vec<SpecRef>,
}

pub enum GenericParamKind {
    Type,
    EffectRow,
}

pub enum EffectRowRef {
    Infer,
    Closed(EffectRowId),
    Open {
        base: EffectRowId,
        tail: EffectRowVarId,
    },
    Var(EffectRowVarId),
}

pub enum ResourceHandleType {
    MemoryRegion { schema: TypeId },
    HostActionGrant { action_pattern: String },
    ExternalTool { signature: TypeId },
    Other { name: String, args: Vec<TypeId> },
}
```

`unit` and `never` are represented through `Primitive(PrimitiveType::Unit)` and
`Primitive(PrimitiveType::Never)`, not separate source-level type families.

### 5.1 Alias And Nominal Type Identity

The latest language SPEC makes `type` nominal by default:

```etas
alias A = B;    // transparent abbreviation
type A = B;     // nominal type with representation B
type A;         // bodyless nominal type
```

`alias` declarations do not create a `Type` value with its own identity. The
type checker expands aliases when canonicalizing type refs, while retaining
alias facts for diagnostics, hover, package metadata, and source navigation.
Alias expansion must detect cycles and report them on the alias chain.

Every `type` declaration creates a distinct nominal constructor:

- `type UserId = string;` is not assignable to `string` without an explicit
  constructor, accessor, or conversion flow;
- `type ProjectId = string;` is not assignable from `UserId`;
- `type Review = { ... }` is a nominal record type, not a structural alias for
  an anonymous record;
- `type TcpStream;` is a bodyless nominal type whose values can only be
  produced by trusted std/package/runtime APIs.

Anonymous record type expressions remain structural shapes where the source
language permits them, for example in inline parameter annotations. Named type
declarations are nominal regardless of whether their representation is a
primitive, record, collection, or support type.

Recommended declaration facts:

```rust
pub enum TypeDeclFact {
    Alias {
        symbol: SymbolId,
        params: Vec<TypeParam>,
        target: TypeId,
    },
    Nominal {
        symbol: SymbolId,
        constructor: TypeConstructorId,
        params: Vec<TypeParam>,
        representation: Option<TypeId>,
    },
}
```

Equality and assignability must compare nominal constructors by identity before
looking at representation. Representation is used for checked construction,
destruction, schema derivation, host-value encoding, and explicit conversion;
it is not used for implicit unification with the nominal type.

Collection types must match the PL SPEC:

- `[a, b, c]` has type `Array[T]`.
- `[a; b; c]` and `a :: xs` have type `List[T]`.
- `{ key => value }` has type `Map[K, V]`.
- `#{a, b, c}` has type `Set[T]`.
- `[start, end)` and `(start, end]` have type `Range[I]`.
- `xs[start, end)` and `xs(start, end]` have type `Slice[T]` for sequence-like
  inputs.

`Array`, `List`, `Map`, `Set`, `Range`, and `Slice` are first-class collection
types in `etas_types`. Other collection names from the standard vocabulary,
such as `Deque`, `Queue`, `Stack`, `PriorityQueue`, `OrderedMap`, and
`OrderedSet`, may be represented as named/applied standard types until the
compiler needs specialized typing rules for them.

`EffectRowRef` is a type-position reference to an effect row annotation. It can
name a closed row, an open row with a tail variable, or a row variable declared
by `effect E`. The effect semantics are checked by `etas_effects`;
`etas_types` preserves and validates the shape needed for flow types, handler
types, generic instantiation, and facts consumed by effect analysis.

For `FlowType.effects`, `None` means "effect row to be inferred" in inferred or
local positions. It does not mean pure. An explicit source annotation of `![]`
must be preserved as an empty closed row. This distinction is required for
anonymous/local flow values: creating a flow value has no immediate effect, but
the value carries a latent effect row that `etas_effects` solves and realizes
when the value is called.

Examples:

```etas
flow twice[T, effect E](f: () -> T ![E]) -> (T, T) ![E]
flow log_then[T, effect E](x: T, f: T -> T ![E]) -> T ![Console.stdout_write, E]
```

The type checker must record the `E` parameter as an effect-row variable and keep
the row tail in type facts. It must not flatten `![Console.stdout_write, E]` into
a string, and it must not replace `E` with `Unknown` or an empty row. Call-site
instantiation creates row substitutions that `etas_effects` uses when it
instantiates the callee summary.

Handler type expressions are value types for first-class handler values:

```text
![H]             -> HandlerType { handled = H, produced = Infer, result = None }
![H for R]       -> HandlerType { handled = H, produced = Infer, result = R }
![H => E]        -> HandlerType { handled = H, produced = Explicit(E), result = None }
![H => E for R]  -> HandlerType { handled = H, produced = Explicit(E), result = R }
```

`etas_types` owns the shape: handler arms must type check as blocks, `resume`
payloads must match the performed action return type, and non-resumable actions
such as `Error[E].raise(...) -> never` must not allow `resume`. `etas_effects`
owns the produced effect row and the effect subtraction when a handler is
applied by `handle`.

Postfix `?` is not a `Result` unwrap operator. `etas_types` must type its value
shape without pretending to know the captured effect:

```rust
pub struct TryExprTypeFact {
    pub expr: HirExprId,
    pub inner_type: TypeId,
    pub result_type: TypeId,
    pub target_error: TryTargetError,
}

pub enum TryTargetError {
    FromExpectedResult(TypeId),
    Inferred(TypeId),
    Ambiguous,
}
```

Typing rule:

```text
operand value type = T
target error type = E
try expression type = Result[T, E]
```

The operand can be any typed expression. For a block operand, the inner `T` is
the block result type, so `{ ...; expr }?` is typed as `Result[T, E]`.
Semicolons do not belong to `?`; they are handled by statement/block typing
before the try expression's value shape is recorded.

When an expected type is `Result[T, E]`, the checker uses that `E`. Without an
expected result type, the checker may create an inference variable for `E`, but
it must not invent ad-hoc error unions. The effect checker owns validation that
the operand actually may raise `Error[E]`, that a single target error type is
unambiguous, and that all required error conversions exist.

Given only `r: Result[T, E]`, `r?` is invalid under the current PL SPEC unless
the expression producing `r` also has a capturable `Error[E]` effect. That
diagnostic belongs to the combined type/effect facts: `etas_types` records the
candidate value shape, and `etas_effects` accepts or rejects the capture.

Agents use the internal constructor described by the SPEC:

```text
Agent[Input, Output, Effects, Context, Config]
```

`Effects` is an effect-row argument, not an ordinary value type argument. The
internal constructor must therefore use `GenericArg::EffectRow` for that
position, and generic agent APIs must reject a value type where an effect row is
expected.

The source declaration remains `Annotation* agent Name(input: I) -> O { ... }`.
Users should not have to write the full internal constructor in ordinary source.
Type checking should:

- type the single input parameter and output schema type;
- require the agent body to produce `Prompt`;
- validate compiler-known annotation metadata such as `@model`, `@tools`,
  `@limits`, `@trace`, and `@deprecated`;
- classify `Writer.run(input)` and `input ~> Writer` as agent invocation only
  after resolving the receiver/stage to an agent symbol;
- keep `Prompt` and `Message[T]` distinct: `Prompt` is a model-call input
  package, while `Message[T]` is a typed communication/session value.

Persistent memory is typed through ordinary standard support types. Schema
abbreviations that should not introduce a new type identity use `alias`:

```etas
alias ProjectMemorySchema = MemoryRegion[{
    Papers: Store[PaperId, PaperRecord],
    Decisions: Store[TraceId, DecisionRecord],
}];

let ProjectMemory =
    std.memory.region[ProjectMemorySchema](
        stable_id = "project_memory",
        store = "project-main"
    );
```

`MemoryRegion[S]` and `Store[K, V]` are type constructors supplied by
`etas_std`. They are not source-level declaration kinds. The top-level `let`
that binds `ProjectMemory` is checked as a resource handle: the initializer
must resolve to a compiler-known standard resource constructor, and its type is
recorded as `ResourceHandle(MemoryRegion { schema })`.

Using `type ProjectMemorySchema = MemoryRegion[...]` would instead create a
nominal wrapper around the memory-region representation. That is valid only if
the program intentionally wants a distinct type identity and provides explicit
construction/conversion where required.

`let` and `var` have different scopes:

- block-scoped `let` creates an immutable local binding;
- block-scoped `var` creates a mutable local binding;
- top-level `let` creates an immutable module item and must be either a
  compile-time deterministic/effect-free constant or a compiler-known runtime
  resource handle;
- top-level `var` is rejected.

## 6. Type Pipeline

Type checking is a pass pipeline. It must not be implemented as a recursive
checker that walks HIR, solves relations, emits diagnostics, and writes final
facts in the same call stack.

```text
SignaturePipeline
  -> BodyPipeline
  -> FinalizePipeline
```

### 6.1 Signature Pipeline

The signature pipeline is declaration-level. It reads item headers, type
declarations, spec declarations, effect declarations, import bindings, and
dependency metadata. It does not enter executable bodies.

```text
SignaturePipeline
  declare declarations
  resolve aliases / imports / spec closure
  validate impl and exported contracts
  materialize signature facts
```

It produces:

- alias facts and alias-cycle diagnostics;
- nominal type constructor facts for every `type` declaration;
- enum constructor facts;
- flow, agent, tool, top-level-let, and impl method signatures;
- spec signatures, superspec closure, type-param bounds, and impl facts;
- effect tag and effect action signatures;
- std/source/external symbol facts.

The signature pipeline is separated from body checking because bodies can be
recursive and cross-reference later items:

```etas
flow a(x: i32) -> i32 { b(x) }
flow b(x: i32) -> i32 { a(x) }
```

Both body checks require both signatures to exist before either body is
checked.

### 6.2 Body Pipeline

The body pipeline is expression-level and body-local. It enters flow bodies,
tool bodies, agent bodies, handler arms, lambda bodies, and top-level-let
initializers. It must be organized as:

```text
BodyPipeline
  generate constraints
  solve type constraints
  solve spec obligations
  validate diagnostics
  materialize facts
```

`generate` is not a renamed `check`. It is a constraint generator. It may:

- allocate fresh type variables;
- lower contextual expected types;
- record provisional expression, statement, pattern, and symbol types;
- emit type constraints;
- emit spec obligations;
- emit validation requests.

It must not:

- write final `TypeFacts`;
- push final diagnostics;
- call assignability and immediately branch on success/failure;
- infer nominal compatibility from runtime value shape;
- execute or inspect effect summaries.

### 6.3 Body Pipeline State

Recommended state boundary:

```rust
pub struct BodyPipelineState {
    pub item: HirItemId,
    pub expected_return: Option<TypeId>,
    pub provisional: ProvisionalFacts,
    pub constraints: Vec<TypeConstraint>,
    pub spec_obligations: Vec<SpecObligation>,
    pub validations: Vec<ValidationRequest>,
    pub solver_report: SolverReport,
}

pub struct ProvisionalFacts {
    pub expr_types: Map<HirExprId, TypeId>,
    pub stmt_types: Map<HirStmtId, TypeId>,
    pub pat_types: Map<HirPatId, TypeId>,
    pub symbol_types: Map<SymbolId, SymbolTypeFact>,
    pub item_signatures: Map<HirItemId, ItemSignature>,
    pub try_facts: Map<HirExprId, TryExprTypeFact>,
    pub index_facts: Map<HirExprId, CheckedIndexKind>,
    pub slice_facts: Map<HirExprId, CheckedSliceKind>,
    pub expr_memory_places: Map<HirExprId, MemoryPlaceType>,
}
```

Only `MaterializeBodyFactsPass` may merge `ProvisionalFacts` into final
`TypeFacts`.

### 6.4 Constraint Shape

Recommended constraint shape:

```rust
pub enum TypeConstraint {
    Equal {
        lhs: TypeId,
        rhs: TypeId,
        origin: ConstraintOrigin,
    },
    Assignable {
        from: TypeId,
        to: TypeId,
        origin: ConstraintOrigin,
        reason: AssignabilityReason,
    },
    Callable {
        callee: TypeId,
        args: Vec<TypeId>,
        output: TypeId,
        origin: ConstraintOrigin,
    },
    FieldAccess {
        base: TypeId,
        field: String,
        output: TypeId,
        origin: ConstraintOrigin,
    },
    IndexAccess {
        base: TypeId,
        index: TypeId,
        output: TypeId,
        origin: ConstraintOrigin,
    },
    TryOperand {
        operand: TypeId,
        output: TypeId,
        error: TypeId,
        origin: ConstraintOrigin,
    },
    EffectRowWellKinded {
        row: EffectRowRef,
        origin: ConstraintOrigin,
    },
    EffectRowAssignable {
        from: EffectRowRef,
        to: EffectRowRef,
        origin: ConstraintOrigin,
    },
}
```

Spec constraints should be represented as `SpecObligation` values rather than
hidden inside ad hoc call checking:

```rust
pub struct SpecObligation {
    pub subject: TypeId,
    pub spec_ref: SpecRef,
    pub origin: ConstraintOrigin,
}
```

Validation requests capture diagnostics that require solved types:

```rust
pub enum ValidationRequest {
    CallableArity { callee: TypeId, actual: usize, origin: ConstraintOrigin },
    ReturnType { actual: TypeId, expected: TypeId, origin: ConstraintOrigin },
    BranchJoin { lhs: TypeId, rhs: TypeId, origin: ConstraintOrigin },
    HandlerResult { actual: TypeId, expected: TypeId, origin: ConstraintOrigin },
    AgentPromptBody { actual: TypeId, origin: ConstraintOrigin },
}
```

`Equal` and `Assignable` work over canonicalized aliases but must not
structurally expand nominal types. For example, after alias expansion
`JsonText` may compare as `string`, but `UserId` remains `Nominal(UserId)` even
when its representation is `string`. A failed implicit assignment between a
nominal type and its representation should produce a targeted diagnostic
suggesting an explicit constructor or conversion flow rather than a generic
structural mismatch.

### 6.5 Constraint Generation Examples

For:

```etas
let id: UserId = "abc";
```

generation records:

```text
literal "abc" -> string
annotation UserId -> Nominal(UserId)
Assignable { from: string, to: UserId }
pattern id has expected UserId
```

The diagnostic is produced later by validation from the solver report.

For:

```etas
if cond { a } else { b }
```

generation records:

```text
Assignable { from: type(cond), to: bool }
Assignable { from: type(a), to: result }
Assignable { from: type(b), to: result }
```

For:

```etas
f(a, b)
```

generation records:

```text
Callable {
  callee: type(f),
  args: [type(a), type(b)],
  output: fresh,
}
```

Callable solving handles source flow signatures, std descriptors, external
metadata signatures, generic instantiation, nominal constructors, and wrong
arity reporting. The generator must not duplicate that logic.

### 6.6 Solver Responsibilities

The solver layer owns:

- unification and occurs checks;
- transparent alias expansion;
- nominal identity preservation;
- assignability;
- callable application and generic instantiation;
- field and index access solving;
- spec-obligation solving;
- effect-row kind and signature-level row compatibility.

Solver output is a `SolverReport` with successes, substitutions, inferred
types, and failures. It should not write final diagnostics directly. Validation
turns solver failures and validation requests into `Diagnostic`s.

`Index` is a standard compiler-owned marker spec, not a general implicit numeric
conversion. It is satisfied only by concrete integer types approved for sequence
indexing. Recommended members are `usize` plus concrete signed and unsigned
integer widths after the checker proves the indexed operation can use a
bounds-checked runtime conversion. `bool`, `char`, floats, `string`, records,
lists, maps, and ordinary user types do not satisfy `Index`. User-authored
`impl MyType ~ Index` should be rejected unless the SPEC later explicitly allows
user-defined index semantics. Compatibility parsing of `impl Index for MyType`
may exist, but it must normalize to the same checked impl fact and still be
rejected for `Index`.

`ByteStream` is also a marker spec, but it is not a concrete type. Standard
opaque stream handles such as `TcpStream`, `TlsStream`, `FileStream`, and
`BrowserStream` satisfy `ByteStream` through standard impl facts. Therefore:

```etas
std.stream.read_until_limit[S ~ ByteStream](stream: S, ...)
```

accepts `TlsStream` by spec satisfaction. It must not require `TlsStream` to be
assignable to a concrete `ByteStream` type, because the language does not define
that subtype relation.

Behavioral specs such as `PromptEncode`, `Schema`, and `ResponseDecode` require
complete checked impls or compiler-derived impl facts. Method lookup against a
spec bound must produce a resolved impl method or a typed spec-dispatch fact;
the interpreter must not later guess a method by string name.

Callable specs are computation-shape specs, not type specs. A callable spec
records input type, output type, and an optional effect-row constraint:

```etas
public spec Stage<I, O, effect E>: callable I => O ![E];
public spec Pure<I, O>: callable I => O ![];
public spec ReportWriter: callable Brief => Draft ![WriterEffects];
```

When a flow, agent, or tool declares `~ SpecName`, `etas_types` must infer or
check the spec arguments from the callable signature. `Pure` requires an
explicit empty effect row; a callable spec that omits `![...]` imposes no
effect constraint. Callable specs must never be satisfied by nominal types, and
type specs must never be satisfied by flows, agents, or tools.

Trace specs are also `spec` declarations, but they constrain requested-action
traces rather than value types or callable signatures:

```etas
spec SafeHttp: trace =
    +EdkHttp.request<_>
    & -Secret.read<_>
    & (Approval.request >> EdkHttp.request<_>);
```

`etas_types` owns trace-spec name resolution, kind checking, and static
argument typing. It does not run trace monitors. It must reject applying
trace-spec operators such as `+`, `-`, `>>`, or `<<` to type/callable specs,
and it must reject declaration conformance when a nominal type tries to satisfy
a trace spec or a flow/tool/agent tries to satisfy a type spec.

`EffectRowAssignable` is limited to type-level flow compatibility and declared
signature checking. It does not solve concrete effect summaries. Inferred body
effects, requested actions, handler subtraction, `?` capture, and public contract
validation remain in `etas_effects`.

Collection literal checking must support expected-type driven checking:

```rust
pub enum ExpectedType {
    Known(TypeId),
    Unknown,
}

pub fn generate_expr_constraints(
    ctx: &mut BodyConstraintGenerator,
    expr: HirExprId,
    expected: ExpectedType,
) -> TypeId;
```

The generator may keep a simpler internal API, but the semantics must support:

- `[]` resolving to `Array[T]` under expected `Array[T]`;
- `[]` resolving to `List[T]` under expected `List[T]`;
- `{}` resolving to record or map only under an expected type;
- non-empty array/list/map/set literals unifying element, key, and value types;
- range endpoints sharing a concrete index-compatible integer type;
- no silent default from an ambiguous empty literal to `List`, `Array`, `Map`, or
  record.

Index expression rules:

- `Array[T][I]` requires `I ~ Index` and returns `T`;
- `Slice[T][I]` requires `I ~ Index` and returns `T`;
- `bytes[I]` requires `I ~ Index` and returns `u8`;
- `Map[K, V][K]` uses the map key type and returns the map lookup result; it
  does not use the `Index` constraint;
- indexing remains bounds-checked and should produce diagnostics or typed error
  behavior according to the checked operation selected by lowering;
- no arithmetic or assignment context may use `Index` as permission for broad
  integer coercions.

The checker should record typed index facts so HIR consumers do not redo type
reasoning:

```rust
pub enum CheckedIndexKind {
    Sequence {
        base: TypeId,
        index: TypeId,
        output: TypeId,
    },
    MapLookup {
        key: TypeId,
        value: TypeId,
    },
}
```

Slice expression rules:

- `Array[T][I, I)` and `Array[T](I, I]` require both bounds to satisfy `Index`
  and return `Slice[T]`;
- `Slice[T][I, I)` and `Slice[T](I, I]` return `Slice[T]`;
- `bytes[I, I)` and `bytes(I, I]` return `bytes` or a dedicated bytes-slice
  representation if the std design later introduces one;
- `Range[I][I, I)` and `Range[I](I, I]` return `Range[I]`;
- negative index and negative slice bounds are rejected in the MVP when known
  statically, and must be checked at runtime when values are dynamic.

```rust
pub enum CheckedSliceKind {
    Sequence {
        base: TypeId,
        bound: TypeId,
        output: TypeId,
    },
    Range {
        index: TypeId,
    },
}
```

Unification, substitutions, and occurs checks belong in `etas_types`, not in
`etas_utils`.

## 7. Reusing `etas_utils`

`etas_utils` should provide generic algorithms only:

```text
PartialOrder
JoinSemiLattice
Transfer
Worklist
FixpointSolver
GraphView
SCC
TopologicalSort
```

`etas_types` can use these for:

- recursive flow signature stabilization;
- transparent alias expansion guards and alias-cycle diagnostics;
- recursive nominal representation validation without structural unification of
  distinct nominal constructors;
- assignability and spec-satisfaction closure;
- type narrowing facts across source-shaped control flow;
- constraint propagation where a worklist is useful.

It should not force every type operation through a fixpoint solver. Unification
and ordinary expression checking remain type-specific algorithms.

Example domain owned by `etas_types`:

```rust
pub struct TypeNarrowingDomain {
    pub symbol_types: Map<SymbolId, TypeId>,
    pub pattern_facts: PatternFacts,
}
```

This domain may implement generic `etas_utils::fixpoint` specs, but the type
semantics and facts stay in `etas_types`.

## 8. HIR Checking Rules

Key rules:

- all HIR path references must be resolved or explicitly marked unresolved
  before successful type checking;
- partially resolved paths may be completed by type-directed member lookup;
- every parameter symbol must produce a `SymbolTypeFact`;
- every local binding symbol must produce a `SymbolTypeFact`;
- `HirExpr::If` and `HirExpr::Match` in expression position must unify branch
  result types;
- `HirStmt::If` and `HirStmt::Match` discard values after checking branch
  bodies;
- only `HirBlock.final_expr` contributes to block result type;
- collection literal nodes must preserve SPEC semantics: comma bracket arrays,
  semicolon bracket lists, list cons, maps, sets, ranges, and slices are checked
  as distinct forms;
- ambiguous empty collection literals must be resolved from expected type or
  rejected with a diagnostic that asks for a type annotation;
- type-spec, callable-spec, and trace-spec declarations must create spec
  symbols and spec signature facts with an explicit spec kind;
- marker type-spec impls must create satisfaction facts only;
- behavioral spec impls must implement every required spec flow with compatible
  generic parameters, input/output types, and effect rows;
- calls to generic APIs with `S ~ ByteStream` or `T ~ PromptEncode + Schema` must
  solve spec constraints before producing a successful type fact;
- `effect E` parameters must be kind-checked as effect-row variables and may
  appear only in effect-row positions;
- flow, agent, and tool declarations with `~ CallableSpec` clauses must produce
  callable-spec satisfaction facts after checking callable input, output, and
  effect-row constraints;
- flow, agent, and tool declarations with `~ TraceSpec` clauses must produce
  trace-spec conformance facts after checking trace-spec kind and action-pattern
  static arguments;
- `never` can coerce into an expected type after aborting or raising a
  non-returning error;
- trust wrappers such as `Trusted[T]`, `Untrusted[T]`, and `Secret[T]` are
  ordinary type constructors with safety meaning consumed by later analysis and
  effect/trace-spec checks.

## 9. Output Contract

Recommended output:

```rust
pub struct TypeOutput {
    pub facts: TypeFacts,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct TypeFacts {
    pub expr_types: Map<HirExprId, TypeId>,
    pub stmt_types: Map<HirStmtId, TypeId>,
    pub pattern_types: Map<HirPatId, TypeId>,
    pub type_refs: Map<HirTypeId, TypeId>,
    pub symbol_types: Map<SymbolId, SymbolTypeFact>,
    pub item_signatures: Map<HirItemId, ItemSignature>,
    pub spec_facts: SpecFacts,
    pub generic_instantiations: Map<HirExprId, GenericInstantiationFact>,
    pub index_facts: Map<HirExprId, CheckedIndexKind>,
    pub slice_facts: Map<HirExprId, CheckedSliceKind>,
}

pub enum SymbolTypeFact {
    Param { ty: TypeId },
    Local { ty: TypeId, mutable: bool },
    Field { ty: TypeId },
    Flow { signature: FlowSignature },
    Agent { signature: AgentSignature },
    Tool { signature: ToolSignature },
    TypeAlias { target: TypeId },
    NominalType {
        constructor: TypeConstructorId,
        representation: Option<TypeId>,
    },
    Effect { tag: EffectTagRef },
    Error,
}

pub struct SpecFacts {
    pub specs: Map<SymbolId, SpecSignature>,
    pub impls: Vec<SpecImplFact>,
    pub satisfied: Vec<SpecSatisfactionFact>,
    pub callable_satisfied: Vec<CallableSpecSatisfactionFact>,
    pub trace_conformances: Vec<TraceSpecConformanceFact>,
}

pub struct SpecSignature {
    pub kind: SpecKind,
    pub type_params: Vec<SymbolId>,
}

pub enum SpecKind {
    TypeSpec {
        entails: Vec<SpecRef>,
        methods: Vec<SpecMethodSignature>,
    },
    CallableSpec {
        input: TypeId,
        output: TypeId,
        effects: CallableSpecEffectConstraint,
    },
    TraceSpec {
        expr: TraceSpecExprId,
    },
}

pub struct GenericInstantiationFact {
    pub callee: HirExprId,
    pub type_args: Vec<TypeId>,
    pub effect_row_args: Vec<EffectRowRef>,
    pub spec_obligations: Vec<SpecObligation>,
}
```

`TypeFacts` is consumed by:

- `etas_effects`;
- `etas_frontend` when building `CheckedProgram`;
- `etas_interpreter`;
- later FIR/AIR lowering;
- diagnostics, dumps, tests, and IDE support.

Package metadata must serialize public spec signatures, type-spec impl facts,
callable-spec satisfaction facts, and trace-spec conformance facts. A downstream
consumer must be able to check `S ~ ByteStream`, `R ~ Within<ReportsRoot>`,
`flow F ~ Pure`, or `flow F ~ SafeHttp` from dependency metadata without
re-analyzing dependency source. Metadata must preserve spec kind; it must not
collapse specs into ordinary named types or string-only support constraints.

## 10. Diagnostics

Required diagnostic classes:

- unknown type;
- unresolved or unsupported type-directed member path;
- duplicate type parameter;
- arity mismatch for generic type constructors;
- using an effect-row parameter in a value-type position;
- using a value-type parameter in an effect-row position;
- unsatisfied spec bound;
- incomplete spec impl;
- extra member in spec impl;
- duplicate or incoherent spec impl;
- type mismatch;
- non-callable callee;
- wrong argument count;
- missing field;
- duplicate field;
- branch type mismatch;
- non-exhaustive or impossible pattern where type checking can know it;
- invalid `impl` target kind;
- trust wrapper misuse where it is type-level rather than trace-authority-level.

Diagnostics must use `etas_core::Diagnostic`, HIR source origins, and stable
diagnostic codes.

## 11. Test Direction

Tests should cover:

- primitive type names and concrete numeric widths;
- generic instantiation for `Array`, `List`, `Map`, `Set`, `Range`, `Slice`,
  `Option`, and `Result`;
- generic instantiation for `S ~ ByteStream` standard stream APIs;
- effect-row polymorphic flows such as `twice[T, effect E]` and
  `log_then[T, effect E]`;
- type-spec declarations, marker type-spec impls, behavioral type-spec impls,
  callable-spec declarations, trace-spec declarations, flow/agent/tool spec satisfaction, compatibility
  `impl Spec for Type` compatibility parsing, and negative spec-bound diagnostics;
- contextual typing for `[]` and `{}`;
- index facts for `Array`, `Slice`, `bytes`, and `Map`;
- slice facts for `Array`, `Slice`, `bytes`, and `Range`;
- record, enum, tuple, and field access checking;
- flow call checking;
- method and type-directed member lookup;
- `if` and `match` expression unification;
- statement-position value discard;
- symbol-to-type facts for params, locals, fields, flows, agents, and tools;
- diagnostics for unresolved types and mismatches;
- type facts consumed by a checked-HIR interpreter fixture.
