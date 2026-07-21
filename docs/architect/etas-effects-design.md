# Etas Effect Checking Design

Status: `Draft`

Owner: `Architect`

Last updated: `2026-07-02`

## 1. Purpose

`etas_effects` owns effect declaration checking, action-aware effect
inference, explicit handler/source-implementation elimination,
trace-spec-facing action facts, public effect contracts, and
interpreter-readiness facts over typed HIR.

The effect checker is an interprocedural abstract interpretation over typed
HIR. It is not a collection of local recursive checker helpers, and it must not
attach canned summaries to known names while treating unknown code as pure.

Input:

```text
HirProgram
  + TypeOutput
  + StdRegistry
  + dependency effect metadata
  -> EffectOutput
```

Output facts are consumed by `etas_frontend`, `etas_interpreter`, later
FIR/AIR lowering, package metadata generation, deployment checks, policy
admission, trace-spec monitors, tests, and IDE support.

The current PL SPEC has no source-level `Capability`, no `extern` keyword, and
no source-level `policy` block. Runtime authority is expressed through effect
actions, deployment grants, sandbox rules, limits, handlers, host services,
active trace specs, and trace evidence.

## 2. Boundary

`etas_effects` owns:

- effect tags, action signatures, action instances, extension relations, and
  row coverage;
- action-aware abstract domains and transfer functions;
- interprocedural effect summary solving over semantic bodies;
- explicit handler and source-implementation elimination;
- postfix `?` effect capture validation and facts;
- requested-action facts for trace-spec checks, grants, trace, replay, and runtime
  mediation;
- ordered action trace abstractions for trace-spec monitors;
- latent effects for first-class flow values;
- public/exported effect contracts;
- interpreter-readiness and host/orchestration requirement facts;
- effect diagnostics.

`etas_effects` does not own:

- type inference, unification, or HIR lowering;
- frontend session/cache/project orchestration;
- runtime authority enforcement;
- provider, tool, memory, filesystem, network, command, approval, checkpoint,
  sandbox, or trace execution;
- generic fixpoint, graph, automaton, or pipeline algorithms.

`etas_utils` owns generic lattice, transfer, worklist, fixpoint, graph,
automaton, and pipeline infrastructure. `etas_effects` owns only the
effect-specific domains, transfer semantics, projections, and diagnostics.

## 3. Dependency Direction

Allowed:

```text
etas_effects -> etas_core
etas_effects -> etas_utils
etas_effects -> etas_std
etas_effects -> etas_hir
etas_effects -> etas_types
etas_effects -> etas_hir_analysis
```

Forbidden:

```text
etas_effects -> etas_frontend
etas_effects -> etas_interpreter
etas_effects -> etas_runtime
etas_effects -> etas_air
etas_effects -> etas_cli
etas_effects -> etas_host
```

`etas_effects` may depend on `etas_hir_analysis` because effect inference is
an abstract interpretation over HIR. `etas_hir_analysis` must remain generic
and must not mention effect rows, policies, or host services.

## 4. Recommended Layout

```text
crates/etas_effects/
  src/
    lib.rs

    effect/
      mod.rs
      id.rs
      tag.rs
      action.rs
      row.rs
      extension.rs
      registry.rs
      coverage.rs

    infer/
      mod.rs
      domain.rs
      trace.rs
      state.rs
      transfer/
        mod.rs
        body.rs
        expr.rs
        stmt.rs
        call.rs
        perform.rs
        handler.rs
        try_expr.rs
        latent.rs
        std_intrinsic.rs
      collect.rs
      dependency.rs
      solve.rs

    trace_spec/
      mod.rs
      action_pattern.rs
      monitor.rs
      facts.rs

    pipeline/
      mod.rs
      context.rs
      artifacts.rs
      passes/
        mod.rs
        build_registry.rs
        collect_constraints.rs
        build_graph.rs
        solve_summaries.rs
        validate_contracts.rs
        materialize_facts.rs

    validate/
      mod.rs
      contracts.rs
      declarations.rs
      public_contracts.rs
      interpreter_support.rs

    facts/
      mod.rs
      effect_facts.rs
      action_facts.rs
      handler_facts.rs
      latent_facts.rs
      trace_spec_facts.rs
      contract_facts.rs
      support_facts.rs

    diagnostic/
      mod.rs
      codes.rs
      builder.rs
```

Layering:

- `effect` defines normalized tags, rows, actions, registry, and coverage.
- `infer` owns abstract domains, transfer functions, dependency graphs, and
  fixpoint solving.
- `trace_spec` owns action patterns and monitor compilation/projection for
  `spec ...: trace` declarations and inline trace-spec conformances.
- `pipeline` orchestrates passes; it does not contain effect semantics.
- `validate` checks solved summaries against contracts and language rules.
- `facts` exposes the stable output contract.
- `diagnostic` builds rendering-neutral diagnostics.

The old structure where `check/expr.rs`, `check/stmt.rs`,
`check/handler.rs`, and `check/item.rs` recursively compute summaries is stale.
Those files may temporarily exist during migration, but they must become thin
validation/projection helpers or be removed. Summary computation belongs in
`infer/transfer`.

## 5. Effect And Action Model

Core roots from the SPEC:

```text
Agentic
Network
FileIO
Command
Memory
Secret
Time
Human
Error[E]
```

Standard tags and actions, such as `Console.stdout_write`,
`Memory.read[R]`, `Command.run[S]`, `Approval.request`, and
`Agentic.infer[A]`,
are registry descriptors. `Approval` is a standard tag extending `Human`, not a
core root. `Console` extends `FileIO`; there is no `ConsoleIO`. Use
`Memory.read[R]` and `Memory.write[R]`; there is no `MemoryRead` or
`MemoryWrite`.

The accepted EDK-facing standard substrate adds more standard registry
descriptors. The effect checker must treat them exactly like other registry
entries, not as hardcoded strings:

| Standard API | Action owner | Root / relation | Notes |
|---|---|---|---|
| `std.net.tcp.connect` | `Net.tcp_connect[host, port]` | `Network` | TCP connect, timeout, cancellation metadata |
| `std.stream.read/read_until_limit/write_all/flush/close` | `Stream.*[stream]` | origin-indexed stream support | Byte stream operations over typed handles; provenance determines broad authority coverage |
| `std.tls.connect` | `Tls.handshake[server_name]` | `Network` | TLS session and certificate validation |
| `std.fs.read_bytes/write_bytes/list/stat/atomic_replace` | `Fs.*[path]` | `FileIO` | Project-root-scoped filesystem substrate |
| `std.secret.read` | `Secret.read[K]` | `Secret` | Host-mediated opaque secret access |
| `std.crypto.hmac_sha256[K]` and other opaque-secret operations | `Secret.use[K]` | `Secret` | Non-revealing operation over secret material |
| `std.browser.protocol.*` | `Browser.attach/send/recv/screenshot/close[...]` | `Network` | Browser protocol/session substrate with narrower browser policy targets |

Pure helpers such as `std.http.codec.*`, `std.codec.text.*`, and deterministic
public `std.crypto.*` functions over ordinary `bytes` do not add requested
actions unless a future operation uses randomness or another host boundary.
Secret-backed crypto is not pure; it records `Secret.use[K]`.

Internal model:

```rust
pub enum Effect {
    Tag(EffectTagId),
    Action(ActionRef),
    AppliedAction(ActionInstanceRef),
    Applied { tag: EffectTagId, args: Vec<TypeId> },
    Error(TypeId),
    Var(EffectVarId),
}

pub struct ActionRef {
    pub tag: EffectTagId,
    pub action: EffectActionId,
}

pub struct ActionInstanceRef {
    pub action: ActionRef,
    pub args: Vec<EffectArgRef>,
}

pub struct EffectRow {
    pub effects: EffectSet,
    pub open: Option<EffectVarId>,
}

pub struct EffectActionSig {
    pub id: EffectActionId,
    pub owner: EffectTagId,
    pub name: String,
    pub effect_args: Vec<EffectActionArgKind>,
    pub params: Vec<TypeId>,
    pub output: TypeId,
    pub returns_never: bool,
    pub resumable: bool,
    pub runtime_requirement: Option<RuntimeRequirementReason>,
    pub high_impact_ack: bool,
}
```

Effect rows are first-class semantic rows. A closed row has `open = None`; an
effect-polymorphic row such as `![Console.stdout_write, E]` has concrete entries
plus an `open` tail variable introduced by a generic parameter `effect E`.
`EffectVarId` must be linked back to the typed generic parameter fact produced
by `etas_types`.

Examples:

```etas
flow twice[T, effect E](f: () -> T ![E]) -> (T, T) ![E]
flow log_then[T, effect E](x: T, f: T -> T ![E]) -> T ![Console.stdout_write, E]
```

At a call site, `etas_effects` receives a typed generic-instantiation fact. It
substitutes concrete effect rows for `E` before appending the callee summary.
`E` is not an unknown action, not a broad tag, and not an empty row.

Action effect arguments are semantic arguments, not parser fallbacks:

```rust
pub enum EffectActionArgKind {
    Type,
    MemoryPlace,
    ValuePath { ty: String },
    StringPattern,
}
```

Examples. `Web` and `Workspace` in these examples are package/project-defined
effects that extend `Network` or `FileIO`; they are not built into
`std.effects`:

```etas
![Network]
![Web.search]
![Web.fetch["github.com"]]
![EdkWorkspace.write["reports/**"]]
![Memory.read[ProjectMemory.Papers]]
![Command.run[DefaultCommandSandbox]]
![Error[IOError]]
```

Bare action arguments such as `DefaultCommandSandbox` are modeled according to
the action signature. `Error[IOError]` uses a type argument. `Memory.read[R]`
uses a memory-place argument. `Command.run[DefaultCommandSandbox]` uses a
value/path argument. This classification belongs to registry/type-directed
lowering, not to a string-name fallback.

The same rule applies to substrate action arguments:

- `Net.tcp_connect[host, port]` uses value/path or literal-stable endpoint
  arguments resolved from the typed call;
- `Stream.read[stream]` uses a typed stream-handle argument;
- `Tls.handshake[server_name]` uses a host/server-name argument;
- `Fs.write[path]` uses a workspace path or path-scope argument;
- `Secret.read[key]` uses a secret-key argument;
- `Browser.send[session]` uses a browser-session argument.

If the action argument cannot be resolved to the declared kind, the checker
must emit a diagnostic. It must not reinterpret an unresolved argument as a
different kind to keep analysis moving.

## 6. Abstract Domains

The primary analysis domain is a product domain. `ActionSet` is a powerset
domain, but effect inference needs more than a single set.

```rust
pub type ActionSet = BTreeSet<Effect>;

pub struct EffectSummary {
    pub escaping_effects: EffectRow,
    pub requested_actions: ActionSet,
    pub default_actions: ActionSet,
    pub action_trace: ActionTraceDomain,
    pub residual_checks: ResidualCheckSet,
    pub trace_spec_obligations: RequirementSet,
    pub requirements: RequirementSet,
    pub determinism: Determinism,
    pub support: InterpreterSupport,
}
```

Meaning:

- `escaping_effects`: computation effects that remain visible to the caller
  after `?`, explicit handlers, and source-visible package implementations are
  applied. For a generic flow, this row may still contain declared row variables
  such as `E`; callers see an instantiated row after generic substitution.
- `requested_actions`: all action footprints requested by the computation.
  These are never erased by `?`, explicit handlers, or source-level package
  implementations.
- `default_actions`: the subset of requested actions handled by standard/runtime
  substrate wrappers or by explicit source-level package handlers. This is a fact
  about already-selected code, not a runtime fallback table for escaping actions.
- `action_trace`: an abstract event trace for trace-spec monitors.
- `residual_checks`: checks deferred to runtime because exact path, account,
  tenant, model, URL, sandbox, or budget data is not statically known.
- `requirements` and `support`: host/interpreter features needed by Phase 1.

`EffectSummary` implements `etas_utils::JoinSemiLattice` and
`etas_hir_analysis::AbstractDomain`.

Join behavior:

- escaping rows join by row union after normalization;
- action sets join by union after action-instance canonicalization;
- default actions must be a subset of requested actions after normalization;
- residual checks and trace-spec obligations join by stable identity;
- determinism joins toward less local/more runtime-mediated;
- support joins by required services/features and joins to `Rejected` only for
  hard static language failures.

No method on `EffectSummary` may materialize output facts or perform validation.
Materialization and validation are separate pipeline stages.

## 7. Action Trace Domain

Trace-spec checking cannot be based only on unordered sets. `+A` and `-A`
often work over action sets, but `A >> B`, `A << B`, limits, and future
temporal trace constraints need ordered events.

```rust
pub enum ActionTraceDomain {
    Empty,
    Event(ActionEvent),
    Seq(Vec<ActionTraceDomain>),
    Choice(Vec<ActionTraceDomain>),
    Repeat(Box<ActionTraceDomain>),
    UnknownOrder(ActionSet),
}

pub struct ActionEvent {
    pub action: Effect,
    pub span: Span,
    pub source: ActionEventSource,
}
```

Transfer rules:

- sequential statements use `Seq`;
- `if` and `match` use `Choice`;
- loops, retry blocks, and recursive summaries use `Repeat`, with widening to
  `UnknownOrder(ActionSet)` when the trace cannot remain finite;
- calls splice the callee trace after effect-argument instantiation;
- explicit handler/source-implementation elimination never removes the event
  from the trace;
- missing ordered facts must fail closed for trace specs that require order.

`UnknownOrder` is not a success fallback. It is a conservative abstraction. If
a trace spec depends on order and the checker only has `UnknownOrder`, the
trace-spec result is incomplete/rejected with a diagnostic.

## 8. Transfer Semantics

Transfer functions live in `infer/transfer/*`. They are the only place where
effect summaries are computed.

Core transfer rules:

```text
literal/path/type construction
  -> identity

record/tuple/array/list/set/map construction
  -> sequentially transfer child expressions

block
  -> sequentially transfer statements, then final expression

if/match
  -> transfer condition/scrutinee, then join branch choices

loop/retry
  -> transfer body through fixpoint/widening, add limit requirements

perform A(args)
  -> transfer args
  -> requested_actions += A
  -> action_trace += Event(A)
  -> requirements += runtime_requirement(A)
  -> escaping_effects += public_effect_of(A)
     unless an enclosing explicit handler handles A

ordinary call f(args)
  -> transfer callee and args
  -> instantiate solved summary(f) using type/effect generic substitutions
  -> append instantiated summary

first-class flow call
  -> transfer callee and args
  -> instantiate latent summary from value type/facts, including row variables

lambda / anonymous flow
  -> no immediate effect
  -> body summary is recorded as latent effect

handler { arms }
  -> creates a handler value fact
  -> arm bodies are analyzed as handler semantic bodies
  -> does not execute handled computation

handle body with a handler argument
  -> body_summary = transfer(body)
  -> handler_summary = transfer(handler expression or inline arms)
  -> escaping_effects =
       subtract(body_summary.escaping_effects, handler.handled)
       + handler.produced
       + handler_summary.escaping_effects
  -> requested_actions =
       body_summary.requested_actions
       + handler_summary.requested_actions
  -> action_trace =
       body_summary.action_trace
       + handler_summary.action_trace

e?
  -> transfer operand
  -> require exactly one checked capturable Error[E]
  -> remove captured Error[E] from escaping_effects
  -> preserve requested_actions and action_trace

std intrinsic call
  -> use StdRegistry descriptor
  -> instantiate generic spec/effect parameters if present
  -> add public escaping effects and requested action metadata
```

For standard substrate wrappers, the descriptor contributes both public
escaping effects and requested action metadata:

```text
std.net.tcp.connect(...)
  -> escaping_effects  += Error[NetworkError]
  -> requested_actions += Net.tcp_connect[host, port]

std.stream.read(...)
  -> escaping_effects  += Error[StreamError]
  -> requested_actions += Stream.read[stream]

std.tls.connect(...)
  -> escaping_effects  += Error[TlsError]
  -> requested_actions += Tls.handshake[server_name]

std.fs.write_bytes(...)
  -> escaping_effects  += Error[IOError]
  -> requested_actions += Fs.write[path]

std.secret.read(...)
  -> escaping_effects  += Error[SecretError]
  -> requested_actions += Secret.read[K]

std.crypto.hmac_sha256[K](...)
  -> escaping_effects  += Error[CryptoError]
  -> requested_actions += Secret.use[K]
```

Pure standard helpers such as `std.http.codec.encode_request`,
`std.codec.text.utf8_decode`, `std.crypto.sha256`, and
`std.crypto.constant_time_eq` only transfer their child expressions and
value-level `Result` shapes. They do not introduce host support requirements.
`std.http.codec` operates on std-owned `HttpWire*` values; EDK-level
`HttpRequest` / `HttpResponse` records must be translated before entering the
codec.

Stream transfer must preserve handle provenance. `Stream.read[s]` and
`Stream.write[s]` are concrete requested actions, while broad coverage is
derived from the origin of `s`: TCP/TLS-origin streams remain covered by
`Network`, and future file-origin streams remain covered by `FileIO`. If
provenance is unknown, trace-spec validation must either emit a residual check
or fail closed for trace specs that rely on broad authority coverage.

Sequence and branch are different operations. A block cannot be implemented as
plain set union once trace-spec traces exist:

```rust
domain.seq_assign(next);      // preserves order
domain.join_branch(other);    // merges alternatives
```

## 9. Interprocedural Solving

The solver operates over semantic bodies, not source files:

```rust
pub enum EffectSemanticBody {
    Item(HirItemId),
    HandlerArm {
        owner: HirItemId,
        handler_expr: HirExprId,
        arm_index: u32,
        body: HirBlockId,
    },
    AnonymousFlow {
        owner: HirItemId,
        value: HirExprId,
        body: EffectAnonymousFlowBody,
    },
    FirstClassFlowCall {
        owner: HirItemId,
        call: HirExprId,
    },
}
```

Pipeline:

```text
CollectConstraintsPass
  -> collect semantic bodies, declared rows, effect-row parameters, call-site
     row substitutions, handler arms, latent bodies, trace-spec regions, tool
     boundaries, and dependency edges

BuildGraphPass
  -> build EffectDependencyGraph

SolveSummariesPass
  -> SCC condensation
  -> topological order
  -> FixpointEngine over EffectSolutionState for each SCC
  -> instantiate row-polymorphic callee summaries through typed substitutions
```

Solution state:

```rust
pub struct EffectSolutionState {
    pub summaries: BTreeMap<EffectSemanticBody, EffectSummary>,
    pub diagnostics: Vec<Diagnostic>,
}
```

For each SCC:

```text
initial summaries = bottom
repeat:
  for body in component:
    output = EffectBodyInterpreter::analyze(body, current summaries)
    summaries[body] join= output
until no change
```

The solver must use `etas_utils::FixpointEngine`. It must inspect the returned
`ConvergenceStatus`. If the iteration limit is reached, the affected summaries
are invalid/rejected and a blocking diagnostic is emitted. Ignoring convergence
status is a bug.

Worklist solving may be used when the dependency graph provides precise
dependents. Private SCC/toposort/worklist implementations are forbidden.

## 10. Registry And Coverage

`EffectRegistry` is the only source of truth for effect/action identity:

```rust
pub struct EffectRegistry {
    pub tags: Map<EffectTagId, EffectTagDecl>,
    pub names: Map<QualifiedName, EffectTagId>,
    pub actions: Map<EffectActionId, EffectActionSig>,
    pub actions_by_owner: Map<EffectTagId, Vec<EffectActionId>>,
    pub extensions: EffectExtensionGraph,
    pub memory_places: MemoryPlaceGraph,
}
```

Inputs:

```text
StdRegistry
  + HIR effect/action declarations
  + TypeFacts
  + SpecFacts
  + GenericInstantiationFacts
  + dependency public effect metadata
  -> EffectRegistry
```

Coverage rules:

- exact refs cover themselves;
- parent tags cover child tags through extension closure;
- a tag covers actions owned by itself or descendants;
- broad action refs cover matching action instances;
- applied action refs compare effect arguments according to argument kind;
- `Memory.read[ProjectMemory]` covers
  `Memory.read[ProjectMemory.Papers]`;
- `Memory.read[R]` never covers `Memory.write[R]`;
- `Error[E]` covers only normalized matching error type unless an explicit
  checked conversion is present.

No checker may special-case identity by string names such as `ConsoleIO`,
`MemoryRead`, `MemoryWrite`, or `Approval` as a core root.

## 11. Standard Library Action Metadata

`etas_std` must distinguish public escaping effects from requested action
metadata. This is a registry-level contract, not a source stub trick.

Recommended declaration shape:

```rust
pub struct FlowDecl {
    pub name: String,
    pub params: Vec<StdType>,
    pub output: StdType,
    pub public_effects: Vec<String>,
    pub requested_actions: Vec<String>,
}
```

Example:

```text
std.io.println:
  public_effects    = [Error[IOError]]
  requested_actions = [Console.stdout_write]
  default_actions   = [Console.stdout_write]

std.fs.write_bytes:
  public_effects    = [Error[IOError]]
  requested_actions = [Fs.write[path]]

std.http.codec.encode_request:
  public_effects    = []
  requested_actions = []
```

Therefore:

```text
std.io.println("hello")
  -> escaping_effects  = { Error[IOError] }
  -> requested_actions = { Console.stdout_write }
  -> default_actions   = { Console.stdout_write }

std.io.println("hello")?
  -> escaping_effects  = {}
  -> requested_actions = { Console.stdout_write }
  -> default_actions   = { Console.stdout_write }
```

The old design where `std.io.println` publicly declared
`![Console.stdout_write, Error[IOError]]` and then later removed
`Console.stdout_write` from `escaping_effects` is obsolete. Standard wrappers
must expose their public row and action footprint separately from the start.

## 12. Explicit Handler And Source Implementation Elimination

Elimination is transfer semantics, not an ad hoc summary cleanup.

Explicit handler:

```text
handle body with H
  requested_actions(body) remain visible
  handled actions/effects may be removed from escaping_effects(body)
  produced effects from H are added
  handler arm requested actions remain visible
  handler dispatch does not grant authority
```

Source-level package implementation:

```text
package API body:
  perform A
  with an explicit source handler in the package implementation

effect summary:
  requested_actions += A
  action_trace += Event(A)
  handled_requested_actions/default_actions record A when it was handled inside this API
  handled A is removed from escaping_effects
  handler body requested actions remain visible
  handler body residual public effects are added
```

Package default behavior is ordinary source code selected by the package API.
Published package metadata records public API summary facts only:
`escaping_effects`, `requested_actions`, `handled_requested_actions` /
`default_actions`, runtime requirements, determinism, and latent flow summaries.
There is no package metadata fallback table that dispatches an escaped action to
a package handler. A direct `perform A` still escapes unless source code has an
explicit handler in scope. If an action escapes, the caller/application must
provide a handler; otherwise execution reports an unhandled action.
Standard/runtime substrate wrappers remain registry-driven host boundaries and
must not erase action trace events.

Facts:

```rust
pub struct HandlerValueFact {
    pub expr: HirExprId,
    pub handled: EffectRow,
    pub produced: EffectRow,
    pub result: Option<TypeId>,
    pub arms: Vec<HandlerArmFact>,
}

pub struct HandlerArmFact {
    pub action: ActionPattern,
    pub resumable: bool,
    pub resumes: ResumeSummary,
    pub arm_effects: EffectRow,
    pub arm_requested_actions: ActionSet,
}

pub struct HandleApplicationFact {
    pub expr: HirExprId,
    pub handler: HandlerValueRef,
    pub handled_actions: ActionSet,
    pub produced_effects: EffectRow,
    pub remaining_effects: EffectRow,
}
```

## 13. Trace-Spec Analysis

Trace specs constrain requested actions and ordered action traces. Effect rows
are positive upper-bound summaries; they do not contain `deny`.

Trace-spec-facing facts:

```rust
pub struct ActionPattern {
    pub target: Effect,
    pub source: ActionPatternSource,
}

pub enum TraceSpecClauseFact {
    Allow(ActionPattern),      // source `+A`
    Deny(ActionPattern),       // source `-A`
    RequireBefore {            // source `G >> A`
        guard: ActionPattern,
        target: ActionPattern,
    },
    RequireAfter {             // source `G << A`
        target: ActionPattern,
        obligation: ActionPattern,
    },
    Limit(LimitFact),
}
```

The current SPEC does not admit source-level `policy { ... }`, `allow`,
`deny`, `require`, `follows`, or policy `where` predicates. Obsolete syntax may
be recovered by `etas_syntax` for diagnostics, but it must not create policy
facts. The accepted source form is `spec Name: trace = SpecExpr;` or a
declaration conformance `flow f(...) ~ Name` / `flow f(...) ~ (SpecExpr)`.

Trace-spec monitors should reuse `etas_utils::automaton`. They consume
`ActionTraceDomain` or materialized ordered action facts. They must not infer
temporal order from an unordered `requested_actions` set.

If an expression has multiple possible materialized actions and no ordered
trace fact, temporal trace-spec checking must fail closed with a diagnostic
instead of choosing an arbitrary order.

## 14. Public Contracts

Exported flows, agents, tools, reusable handler values, and exported values
whose type contains a flow value must expose stable effect metadata.

```rust
pub struct PublicEffectContract {
    pub item: HirItemId,
    pub exported_name: QualifiedName,
    pub value_type: TypeId,
    pub declared: Option<EffectRow>,
    pub inferred_escaping: EffectRow,
    pub public_row: EffectRow,
    pub requested_actions: ActionSet,
    pub residual_checks: ResidualCheckSet,
    pub latent_flows: Vec<LatentEffectContract>,
    pub high_impact_ack: Option<EffectAcknowledgement>,
}
```

Rules:

- public rows are upper bounds for `escaping_effects`;
- requested actions must be covered by public/package/deployment metadata;
- generated metadata is an artifact, not a HIR/source mutation;
- downstream packages consume public effect metadata without reparsing source;
- missing public contract data is a blocking diagnostic, not an empty summary.

## 15. Tool And Agent Boundaries

`tool` declarations are model-callable boundaries. A `flow` is ordinary Etas
logic and cannot be exposed directly in an agent `tools = [...]` list.

Rules:

- source-bodied tools infer body effects and requested actions;
- bodyless `tool ...;` declarations must resolve through imports or
  std/package/precompiled provider metadata;
- bodyless tools require explicit public effects and requested-action metadata;
- unresolved bodyless tools are invalid and must not be treated as implicit
  host implementations;
- tool input/output types must be schema-encodable;
- a tool may call flows, other tools, bodyless tool signatures, or perform
  effect actions;
- a tool must not call an agent in Phase 1.

Agents request inference/model actions and may expose tool surfaces. The effect
checker records model/tool requested actions and interpreter-orchestration
requirements; the interpreter/runtime performs concrete host calls.

## 16. Interpreter Support Classification

Phase 1 executes checked HIR directly. The effect checker classifies every item
and entry flow by static validity and execution support:

```rust
pub enum InterpreterSupport {
    LocalOnly,
    RequiresHost(HostRequirementSet),
    RequiresInterpreterOrchestration(InterpreterOrchestrationRequirementSet),
    Rejected(FrontendRejectionReason),
}
```

Classification is derived from requested actions, residual checks, and control
features:

```text
Console.stdout_write       -> Console host service
Memory.read[R]             -> durable memory service
Memory.write[R]            -> durable memory service
Approval.request           -> approval/human interaction
Agentic.infer[A]           -> model/agent runtime provider
Command.run[S]             -> command sandbox
Net.tcp_connect[H, P]      -> network substrate service
Stream.read/write[S]       -> stream substrate service plus origin provenance
Tls.handshake[H]           -> TLS substrate service
Fs.read/write/list[P]      -> filesystem substrate service
Secret.read[K]             -> secret-store substrate service
Secret.use[K]              -> secret-backed non-revealing operation
Browser.attach/send/recv/screenshot/close[...] -> browser protocol substrate service
EdkWorkspace.write[P]      -> package action; a EDK API may handle it explicitly
                              with a package handler over Fs.*, otherwise an
                              escaping action requires an application handler
handler/resume             -> interpreter orchestration
checkpoint/retry/workflow  -> interpreter orchestration
```

Classification must be registry-driven. A package-defined action such as
`EdkWorkspace.write[P]` is not hardcoded as `Fs.write[P]`. If a package API
explicitly applies its handler, the handler body contributes the lower-level
`Fs.*` requested actions when compiling the EDK package itself. If the action
escapes, downstream code sees the public EDK action contract and must supply a
handler.

`Error[E]` is a control-plane escaping effect. It is not a host service by
itself.

The interpreter may repeat defensive checks, but it must not infer effects or
action footprints. If checked HIR has no materialized action fact for an action
boundary, execution fails closed.

## 17. Pipeline

`etas_effects` exposes one internal sub-pipeline:

```rust
pub struct EffectPipelineInput<'a> {
    pub hir: &'a HirProgram,
    pub types: &'a TypeOutput,
    pub dependency_metadata: Option<&'a DependencyEffectMetadata>,
}

pub struct EffectPipelineOutput {
    pub artifacts: EffectPipelineArtifacts,
    pub effects: EffectOutput,
}
```

Passes:

| Pass | Name | Responsibility |
|---|---|---|
| `BuildRegistryPass` | `effects.build_registry` | Build `EffectRegistry` from std descriptors, HIR declarations, spec facts, type facts, dependency metadata, standard defaults, and memory-place facts. |
| `CollectConstraintsPass` | `effects.collect_constraints` | Collect semantic bodies, declared rows, effect-row parameters, call-site row substitutions, calls, handler arms, latent bodies, tool/agent boundaries, trace-spec conformances, and dependency edges. It does not solve summaries. |
| `BuildGraphPass` | `effects.build_graph` | Build the semantic-body dependency graph. |
| `SolveSummariesPass` | `effects.solve_summaries` | Run interprocedural abstract interpretation over SCCs using `etas_utils::fixpoint`; instantiate row-polymorphic summaries through `TypeFacts::generic_instantiations`. |
| `ValidateContractsPass` | `effects.validate_contracts` | Validate solved summaries against declared rows, row-polymorphic public contracts, tool boundaries, handler bounds, high-impact action rules, and trace-spec monitor soundness. |
| `MaterializeFactsPass` | `effects.materialize_facts` | Project solved summaries into stable `EffectFacts`, action facts, handler facts, trace-spec facts, public contracts, and interpreter-support facts. |

Hard requirements:

- `etas_effects` must not depend on `etas_frontend`;
- cache, incremental scheduling, and artifact publication are owned by
  `etas_frontend` / `FrontendSession`;
- `EffectPipelineArtifacts` must be serializable/fingerprintable;
- `MaterializeFactsPass` is a facts-layer projection, not an abstract-domain
  method;
- partial/fallback summaries are forbidden.

## 18. Output Contract

```rust
pub struct EffectOutput {
    pub facts: EffectFacts,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct EffectFacts {
    pub expr_effects: Map<HirExprId, EffectSummary>,
    pub stmt_effects: Map<HirStmtId, EffectSummary>,
    pub item_effects: Map<HirItemId, EffectSummary>,
    pub semantic_body_effects: Map<EffectSemanticBody, EffectSummary>,
    pub symbol_effects: Map<SymbolId, EffectSummary>,
    pub action_facts: ActionFacts,
    pub trace_spec_facts: TraceSpecFacts,
    pub handler_values: Map<HirExprId, HandlerValueFact>,
    pub handle_applications: Map<HirExprId, HandleApplicationFact>,
    pub try_captures: Map<HirExprId, TryCaptureFact>,
    pub latent_effects: Map<HirExprId, LatentEffectFact>,
    pub public_contracts: Vec<PublicEffectContract>,
    pub interpreter_support: InterpreterSupportFacts,
}
```

`EffectFacts` must contain enough source/HIR ids and spans for diagnostics,
dumps, IDE support, interpreter execution checks, and package metadata.

## 19. Implementation Migration

The current implementation must be migrated away from recursive effect checker
helpers. These old responsibilities must not continue computing authoritative
summaries:

- `check/block.rs::check_block`;
- `check/stmt.rs::check_stmt`;
- `check/expr.rs::check_expr`;
- `check/expr.rs::perform_expr`;
- `check/handler.rs::check_handle_expr`;
- `check/handler.rs::remove_handled_effects`;
- `check/handler.rs::remove_handled_runtime_support`;
- `check/item.rs::remove_default_actions_from_escaping`;
- `check/policy/analysis.rs::materialized_expr_actions`;
- `check/policy/analysis.rs::step_materialized_expr_actions`.

Allowed migration path:

1. Move effect computation to `infer/transfer/*`.
2. Keep old helpers only as temporary wrappers that call the new transfer layer.
3. Move contract validation into `validate/*`.
4. Move fact projection into `pipeline::MaterializeFactsPass` and `facts/*`.
5. Delete any code path that returns an empty/local summary because a callee,
   action signature, type fact, handler fact, or trace-spec target is missing.
6. Replace unordered policy/trace action stepping with ordered
   `ActionTraceDomain` or materialized ordered action facts.
7. Split standard flow metadata into public escaping effects and requested
   actions; do not rely on post-hoc default-action removal from public rows.

Any remaining code named `remove_default_actions_from_escaping` or
`materialized_expr_actions` is a migration smell unless it delegates to the new
domain/trace transfer and is not an authority source.

## 20. Diagnostics

Required blocking diagnostics include:

- unknown effect tag or action;
- duplicate or invalid action signature;
- invalid effect extension;
- invalid action argument kind;
- unresolved `perform`;
- missing standard registry or dependency effect metadata;
- missing callee summary or dependency metadata;
- fixpoint non-convergence;
- missing ordered action trace for temporal trace spec;
- requested action outside declared/public/tool boundary;
- inferred escaping effect outside declared row;
- handler arm matching unresolved/non-action target;
- handler produced effects outside explicit bound;
- illegal `resume` or non-resumable action resume;
- postfix `?` with no capturable `Error[E]`;
- postfix `?` with ambiguous error effect and no checked conversion;
- unresolved bodyless tool provider binding;
- high-impact action missing required acknowledgement;
- interpreter support missing for a materialized requested action.

Diagnostics must use `etas_core::Diagnostic`, stable diagnostic codes, and HIR
source origins.

## 21. Test Direction

Tests must cover:

- standard registry completeness for all SPEC effect tags/actions/defaults;
- user-defined effects/actions and extension coverage;
- action argument classification for `Error[E]`, `Memory.read[R]`,
  `Command.run[S]`, string patterns, and value paths;
- `std.io.println` public effect/action split;
- `std.io.println()?` preserving requested actions while removing
  `Error[IOError]`;
- explicit handler elimination preserving action traces;
- handler-produced effects and explicit produced-effect bounds;
- first-class handler values and `with <handler-value-expr>`;
- latent effects for lambda/anonymous flow values;
- ordinary calls, method calls, std intrinsic calls, tool calls, and agent calls;
- recursive and mutually recursive flow summaries through fixpoint;
- fixpoint iteration-limit diagnostics;
- trace-spec `+A` / `-A` over action sets;
- trace-spec `A >> B` / `A << B` over ordered traces;
- fail-closed diagnostics for unordered multi-action expressions under temporal
  trace specs;
- public contract generation and dependency metadata consumption;
- interpreter-support facts for host services and orchestration features;
- no source-level `Capability`, `extern`, `ConsoleIO`, `MemoryRead`, or
  `MemoryWrite` behavior.
