<!-- Historical recovery snapshot, archived 2026-11-21. Not a current specification, implementation status, or engineering task assignment. The current language SPEC and main implementation take precedence. -->

# Reflection And Checked Code Generation Architecture

Status: Architecture target aligned with SPEC chapter 18; not an implementation acceptance report.
Owner: Architect
Updated: 2026-11-03

## 1. Language Contract And Implementation Baseline

Read the complete [SPEC chapter 18](../../../etas/docs/design/18-reflection-and-checked-code-generation.md)
together with chapters 03, 05, 06, 09, 14 and 15. Chapter 18 explicitly identifies
this as a proposal, not current implementation support.

The two typed source paths are:

```text
^current : AgentDef<A>
  -> std.compiler.parse_body(text) : Result<SyntaxBlock, ParseError>
  -> definition.with_body(syntax) : AgentDef<A>
  -> std.compiler.compile(edited) : A ![Error<CompileError>]
  -> ordinary invocation of A at the new revision

^current : CallableDef<F>
  -> std.compiler.parse_body(text) : Result<SyntaxBlock, ParseError>
  -> definition.with_body(syntax) : CallableDef<F>
  -> std.compiler.compile(edited) : F ![Error<CompileError>]
  -> ordinary invocation of F at the new revision
```

compile returns the existing nominal A or ordinary callable type F, not a
CompiledAgent, CompiledFlow, draft-stage or version-indexed source type. F
includes the escaping-effect contract but does not by itself describe the full
requested-action/temporal contract. No eval(string), new revision operator or
mandatory public run-code wrapper is introduced.

Syntax, checked reflection facts, retained definitions and interpreter code
values now have implementation paths. Their existence is not evidence that the
complete source contract is accepted. This document specifies invariants and
work boundaries, rather than freezing an old implementation-progress inventory.
Shared contracts and Host protocol are defined in
[etas-code-design](../../../etas-core/docs/architect/etas-code-design.md).

## 2. Source Contract Matrix

| Target | Result of reflection | Editing and compilation |
| --- | --- | --- |
| Agent declaration or value at a selected revision | `AgentDef<A>` | with_body/check/compile; compile returns A |
| Ordinary flow declaration or value referring to a definition/revision | `CallableDef<F>` | with_body/check/compile; compile returns F |
| Direct agent method, including default or spec-selected method | `CallableDef` | Read-only inspection; not detached or installed as a free flow |
| Heterogeneous descriptor collection | `Array<CallableDef>` | Read-only erased views; no compile overload |

F is inferred from checked ordinary callable typing, including type/effect
bindings and the escaping row. An expected output type or runtime SignatureInfo
cannot choose F. Generic reflection follows ordinary flow instantiation; it
does not introduce first-class polymorphic schemes or quote arbitrary lambdas.

CallableDef<F> may project to bare CallableDef when that view is expected.
This is a specific, one-way support-type projection, not general generic-argument
omission, structural subtyping or implicit erasure of arbitrary types. There is
no inverse cast, descriptor-record constructor, or runtime-signature route back
to F. Erased descriptors are rejected by with_body/check/compile even when an
expected result type is supplied. Both forms retain revision/dependency fields.

A bare agent has signature=None and body=None, but can expose identity and
accessible methods. Editing does not invent a default signature; checking and
compilation reject an attempted replacement without that frozen entry contract.

SPEC now defines frozen compatibility, source retention/export, edited-summary
None values, reports/errors, resource accounting and source restoration. These
are implementation requirements rather than unresolved design gates. Binary
export/load is outside the public compiler surface. Do not implement guessed
Code.load/Code.parse actions, fake reports or automatic source export.

## 3. Ownership And Target Layout

Keep reflection in the existing syntax/HIR/type/effect/frontend crates. Use the
single shared-contract crate etas_code in etas-core. Do not add a second parser,
checker, policy engine or compiler service binary.

```text
etas_syntax/src/
  grammar/reflection.rs       # caret target grammar and precedence
  grammar/body_fragment.rs    # body contents to ordinary ast::Block
  ast/reflection.rs           # dedicated ReflectExpr and target syntax
  format/                    # deterministic AST body rendering

etas_hir/src/
  hir/reflection.rs           # reflective reference, not a Call
  lower/reflection.rs         # source target to HIR reference
  resolve/reflection.rs       # stable source/default/impl-method identity

etas_types/src/
  constraint/reflection.rs    # definition target, kind and generic obligations
  pipeline/body/collect/reflection.rs
  solver/reflection.rs        # resolve typed targets; enforce visibility/kind
  facts/reflection.rs         # checked target family, descriptor view, A or F

etas_frontend/src/code/
  mod.rs
  identity/                  # stable definition/revision and local-ID remapping
  definition/
    collect.rs               # primary agent + flow + additional method catalog
    environment.rs           # frozen lexical/import/dependency bindings
    retention.rs             # source visibility/availability and leases
    store.rs                 # immutable syntax/base/draft objects
  descriptor/
    signature.rs
    behavior.rs
    annotation.rs
    project.rs               # project checked facts -> neutral descriptor graph
  draft/
    overlay.rs               # replace canonical agent-default or free-flow body
    revision.rs              # changed source/dependency/summary invalidation
  contract/
    validation.rs            # callable interface and behavior compatibility
    preservation.rs          # frozen caller-visible obligations
    latent/                  # higher-order and frozen-call origin validation
  pipeline/
    context.rs
    run.rs                   # normal frontend passes plus contract validation
    passes/
      resolve_target.rs
      build_descriptors.rs
      freeze_environment.rs
      validate_contract.rs
      emit_candidate.rs
  service/
    syntax.rs                # CodeSyntaxOperations implementation
    compiler.rs              # candidate check/compile request facade
    candidate.rs             # CandidatePublisher port, no engine dependency
  diagnostic/
    project.rs               # structured diagnostics with generated source keys
  artifact/
    metadata.rs
    validate.rs
```

These are target responsibility boundaries, not a request to create empty
folders. Use smaller cohesive files where the implementation needs them.
Metadata conversion in code/descriptor consumes existing TypeFacts/EffectFacts;
it must not infer effects again.

etas_driver::compiler composes the frontend service with the supplied
CandidatePublisher. The interpreter implements that port over its normal
preparation pipeline. Driver must not import interpreter, CLI must not implement
a private checker, and Host/Core must not import frontend.

## 4. Syntax, HIR And Binding

Caret is a dedicated reflection expression, not an arithmetic unary operator.
Member selection binds more tightly:

```text
^Generator.run    -> Reflect(selected method)
(^Generator).body -> field of Reflect(agent)
^candidate       -> reflect the implementation carried by candidate
```

Use ordinary resolved path/member/spec-selection machinery for the permitted
target forms. A call expression is not a reflection target:
^Generator.run(input) must be rejected without executing or quoting it.
Arbitrary expressions and live frames are not made reflectable. Grammar
recovery may retain an invalid target node solely for diagnostics.

Parsing body text uses the normal expression/statement/block grammar and must
consume EOF. Input excludes enclosing braces. Implement a body-fragment entry
point; do not wrap text in a dummy flow and then guess shifted spans.
Invalid syntax returns Err(ParseError), even if the parser produced recovery AST.
A SyntaxBlock owns a retained immutable structured AST/source reference.
parse/format/parse should preserve normalized structure, not exact comments.

HIR preserves a Reflect node, target evaluation/reference semantics and origin.
CheckedReflectionFact records:
- static declaration or allowed agent/ordinary-flow value binding;
- stable definition/method/spec identity and original source body slot;
- generic specialization, expected nominal A or ordinary callable F;
- typed agent/typed flow/erased method descriptor family and view projection;
- access decision and body availability;
- source/revision provenance.

Reflection contributes metadata/retention dependencies, not a callee-body
execution edge. It must not import every reflected helper into the caller's
effect row. Binding a path is not invoking it.

For edited code, build a structured body overlay in the target's definition
environment. Preserve original resolved declarations, generic binders, permitted
imports, type/spec identities and locked package graph. Generated names resolve
there, not in unrelated caller locals. Existing private lexical dependencies
may be used only under the established definition environment and visibility
rules; a draft cannot gain new access by spelling a name.

Generated source receives its own immutable source key/SourceId within a
revision namespace. Diagnostics identify that draft, exact span/line/column,
base definition and related original locations. Do not attribute every generated
error to SourceId(0), the compile call site or a fabricated span.

## 5. Definition And Descriptor Projection

AgentDef<A>.signature/body/effects/determinism/trace describe the default
invocation directly and its transitive calls. They do not union every auxiliary
method. If lowering represents the default invocation by a run method, keep one
CallableDefinitionId/body slot, shared by ^A and ^A.run.

The generated default method is a callable reference to the canonical source
body owner, not a second HirFlow body owning the same blocks. For an explicit
impl method, its existing body owner remains canonical. Record the default
invocation mapping and let tree/unit collectors schedule that body once; do not
duplicate blocks or introduce a forwarding method that invokes the agent again.

AgentDef.methods is Array<CallableDef>: accessible additional impl methods with
their selected spec and independent signatures/summaries. Direct ^A.run and
^A.helper also return this read-only erased view. They are not independently
editable copies of an agent body or an array of executable functions with one
synthetic signature. Default-body editing uses AgentDef<A>.

An ordinary flow has a canonical CallableId/body owner and a typed
CallableDef<F> snapshot. Its signature is always present. Projecting to the
erased view shares the same immutable descriptor graph, revision, dependency
environment and optional body, but removes source-level edit/compile access.
AgentDef.signature remains optional; type erasure never invents a signature.

SignatureInfo preserves type/effect binders, parameter names, resolved input/
output types and optional original annotations. TypeInfo and SpecInfo are
interned descriptive graphs; no descriptor acts as a source type or proof.

Project annotation arguments through existing checked constant evaluation.
Retain unknown user annotations as static metadata without changing checking,
effects or authority. ToolRef lists model-visible declarations, not grants.

Project EffectSummary separately as declared_bound, escaping, requested and
default_handled; preserve row variables and parameterized action identities.
TraceSummary references existing branching/order/temporal analysis. Early
analysis failure yields explicit unavailable information, not ![] or
Deterministic. with_body preserves declared_bound but exposes current inferred
escaping/requested/default_handled, determinism and trace as None. Internally
retained baseline summaries belong only to the authoritative frozen contract.
Dependent auxiliary summaries are invalidated too; check returns new analysis
without mutating the draft, and ^candidate describes the compiled revision.

A missing body is None. It is neither an empty executable block nor a request to
look up source files during pure reflection.

Retain bodies only through the owning manifest's [reflection].retain_bodies;
export_bodies applies only to public owned items and implies retention. Both
default to empty. Validate canonical item selections; a consumer or re-export
cannot expose a dependency's hidden body. The same policy applies to agent and
flow revisions and is inherited by their compiled successors.

## 6. Compilation Pipeline

The same immutable base/environment is used for check and compile. Candidate
compilation invokes the existing FrontendSession pipeline, including normal
imports, signatures, body constraints/solver/facts, effects, handlers, spec/
trace checks and interpreter support. The compiler validates the selected
agent-default or ordinary-flow body against its authoritative frozen contract;
it does not silently widen the interface or recompile callers to permit a change.
Entry validation is for the candidate definition; no synthetic public main is
required and arbitrary original main must not execute.

The new passes surround and consume normal compiler artifacts:

| Pass | Scope | Requires | Produces |
| --- | --- | --- | --- |
| ResolveReflectionTargetsPass | Project/body fact integration | HIR + resolved paths + checked type/member facts | CheckedReflectionTargets |
| BuildDefinitionDescriptorsPass | Project | checked signatures/types/effects/trace/annotations and stable IDs | DefinitionDescriptorGraph |
| FreezeDefinitionEnvironmentPass | Definition | canonical bindings + locked dependency/config identities + target A/F | FrozenDefinitionEnvironment + frozen callable contract reference |
| ValidateCandidateContractPass | Candidate project | edited overlay + final checked facts + authoritative frozen contract | ContractCheckResult + identified dynamic proof obligations |
| ValidateResidualBackendSupportPass | Candidate/project execution target | VERIFIED_RESIDUAL_PLANS + neutral target capabilities/config | RESIDUAL_BACKEND_SUPPORT + rejected/guarded report projection |
| EmitCheckedCandidatePass | Candidate project | successful normal pipeline + contract result + VERIFIED_RESIDUAL_PLANS + RESIDUAL_BACKEND_SUPPORT | immutable checked candidate and plans for CandidatePublisher |

Reflection target typing itself stays in etas_types's collect/solve/materialize
pipeline. BuildDefinitionDescriptorsPass runs only when the relevant analysis
artifacts are valid; partial diagnostics may expose unavailable fields.

CheckReport.status is rejected, accepted or guarded, using the stable SPEC
string tags. Invalid source or unproved compatibility returns rejected from
Code.check. Code.compile rejects via CompileError(kind=rejected). Operational
compiler-service failure uses Error<CompileError> with kind=service for both
operations; authority denial, cancellation and budget exhaustion retain their
own enforcement semantics. ParseError and CompileError contain structured
diagnostics, not a substitute successful report.

Reports contain revision, dependencies, snapshot_fingerprint, available effects/
determinism/trace and runtime_checks. Locations name source and revision with
zero-based end-exclusive byte ranges; line/column is supplementary. Include
structured expected/actual, related locations and constraint/analysis paths
where relevant. Missing data is absent, never fabricated. `runtime_checks` is
descriptive, not executable evidence. `check` may return guarded only when every
remaining obligation has a validated plan and supported target binding; it does
not activate an invocation or request approval. `compile` must additionally
obtain engine-owned installation evidence before returning A/F. Neither a known
violation nor arbitrary unproved compatibility can be disguised as guarded.

compile performs the necessary validation even when check was not called.
A prior report is only reusable with matching code, contract, dependency,
retention/access-relevant inputs, compiler/backend/schema/config fingerprints
and verified provenance. A user-created "accepted" report cannot authorize emit.

Compiled output is prepared but not evaluated. This Phase 1 backend emits checked
HIR plus an engine preparation result, not AIR/native code by requirement.
The same public Code service can select another backend in the future.

Structured obligations, neutral plan schemas, pass artifacts and publication
gates are specified in
[Residual Checks And Runtime Trace Monitors](../../../etas-core/docs/architect/etas-residual-checks-design.md).
Follow the [residual implementation work list](residual-checks-implementation-worklist.md).
Do not parse `BTreeSet<String>` checks, treat a Host policy-client selection as
installation, or implement a reflection-only shadow monitor.

Candidate source/overlay construction assigns an independent SourceRevisionKey
before type/effect analysis. Obligations use source/clause/binding-recipe
identities, not the current RevisionKey or FrozenDefinitionEnvironment, both of
which can depend on checked contracts. A replacement's frozen input contract is
already available, but does not replace the new source snapshot's identity.
After contract validation/final revision construction, EmitCheckedCandidatePass
validates separate plan-to-revision/contract bindings without rekeying obligations
or semantic plans. The ordinary output path follows the same protocol. See the
[source identity and hash dependency rules](../../../etas-core/docs/architect/etas-residual-checks-design.md#31-acyclic-source-and-obligation-identity).
No temporary/zero contract, original-summary substitution or additional check
cycle is allowed to manufacture an identity for earlier analysis stages.

### Frozen Compatibility Algorithm

Freeze callable input/output and generic/spec obligations together with escaping
upper bounds, requested-action scopes, trace assumptions/guarantees and promised
determinism. Do not regenerate that allowed contract from the edited body's
inference. Apply the same checks to A and F; equality of F is necessary at the
callable boundary but insufficient to prove behavioral replacement.

Reuse normal type/spec solving and effect interprocedural summaries. Check
conservative inclusion of candidate behavior in frozen allowed behavior for each
permitted abstract caller entry state, preserving normal/return/error/abort
outcomes and intermediate violations. Trace comparison uses the established
trace/monitor abstractions and shared automaton/graph/fixpoint infrastructure;
neither unordered action sets nor final monitor states alone are sufficient.
For example, send-then-approval cannot replace approval-then-send even with the
same action set. An unproved relation rejects, rather than accepting unknown.

Supported residual enforcement is part of the checked contract, not permission
to skip a failed inclusion check. Body-specific inlining, summaries and caches
also require revision/dependency validation; contract compatibility is not proof
that old optimized bodies remain valid. A public contract change is a separate
interface change with caller revalidation, outside with_body/compile.

### Static Contract Facts And Runtime Summary Handoff

This protocol implements SPEC 18 sections 5.1-5.2; it introduces no new source
type or effect. It applies equally to compiled agent entries and ordinary flows.
Keep three meanings separate:

| Fact | Meaning | Valid use |
| --- | --- | --- |
| S_base | Checked analysis of the original implementation | Describe that revision; establish an inferred contract when first freezing it |
| C_frozen | Authoritative caller-visible interface and allowed behavior | Statically analyze calls to any replacement proved compatible with it |
| S_new | Newly checked analysis of the candidate body | Validate the replacement and describe its specific executable revision |

The invariant is `C_after = C_before` and `S_new satisfies C_frozen`, not
`S_new == S_base`. Satisfaction means behavioral inclusion as specified above,
not fieldwise comparison of EffectSummary structs. When C_frozen was
established from S_base, sharing its immutable backing as a contract is valid.
Relabeling that old body analysis as the actual S_new is not valid. A frozen
behavior bound must be a sound abstract call transformer under the established
caller-side trace/effect rules, not just an unordered list of permitted actions.

The caller is checked before runtime-generated text is available. Its compile
result therefore carries a contract-backed latent origin, not a guessed future
revision or a promise that compilation will succeed:

```text
typed ^target / ^current
    -> descriptor origin + A/F + generic/row bindings + contract origin
with_body(syntax)
    -> same contract origin; edited current analysis is unavailable
compile(draft)
    -> on normal successful return: A/F with FrozenContract latent origin
candidate(args)
    -> apply C_frozen's conservative call transformer at this call site
```

Implement the following responsibilities in the existing fact/pipeline layers:

1. Types collect/solve/materialize a checked compile-result recipe containing
   the registry-resolved overload, descriptor input expression, target family,
   A/F and generic/row substitutions. Reference the descriptor's canonical or
   symbolic contract origin; do not store EffectSummary in TypeFacts or make
   types depend on effects. Existing facts may be extended; no parallel checker
   or mandatory new fact store is required.
2. The effects/frontend bridge resolves that origin to an authoritative frozen
   contract. Distinguish a `FrozenContract` latent source from a
   `CheckedRevision` body-summary source in the existing callable facts. Store a
   reference and substitutions, not a cloned global EffectOutput or fake S_new.
3. For a local original definition not yet summarized, retain a symbolic
   contract-of-definition dependency. The existing interprocedural SCC/worklist
   solves its base summary and dependent call transformers together. Freeze the
   final checked contract before publishing the checked project. Do not require
   a complete descriptor catalog before Effects can run, or freeze a provisional
   bottom summary. Imported/already-pinned contracts use validated artifacts.
4. These are analysis/contract dependencies, not executions. Constructing a
   draft or calling compile does not realize the target's latent actions. Only
   invoking the resulting A/F applies its frozen call transformer; compile
   itself contributes its ordinary Code/error/service behavior.
5. Propagate origins through ordinary assignments, projections, arguments,
   returned callables, higher-order invocations and wrappers. `compile(draft)?`
   puts the callable origin on the Result.Ok payload only, not Err or the
   Result container as an invocable value. Parameter/return templates preserve
   symbolic contract parameters and substitute them at checked call sites.
6. Branches retain all possible contract origins and conservatively join their
   call behavior, including trace choices and outcome distinctions. Recursion,
   loops and widening must preserve origins or an explicit unavailable state.
   Equal F does not imply equal contracts. Metadata must preserve these origin/
   substitution templates through cross-package replay, not recover them from
   a signature string or a display-name match.

An unresolved contract parameter may remain symbolic while normal higher-order
analysis can discharge it. A required but missing/untrusted/incomplete contract
must eventually produce an anchored incomplete-facts rejection. Never substitute
F's escaping row, S_base as a candidate-body result, empty requested actions,
determinism, an arbitrary branch or silent Top-as-permission. An unresolved
solver bottom is not proof of an empty contract.

At runtime the compiler obtains the immutable draft and C_frozen, derives S_new
using the normal frontend pipeline, proves compatibility and installs required
residual checks before CandidatePublisher returns a callable. Publication binds
the target/interface, contract identity, new revision, S_new and dependency/config
fingerprints. The returned provenance must match the descriptor contract used by
the caller; apply the same check to a custom handler's supplied checked callable.
Missing evidence rejects publication instead of returning the original flow.

The running caller keeps its immutable contract-based static facts. It does not
need to be rechecked or patched with S_new after a successful compile. The engine
executes the pinned new revision, and ^candidate exposes S_new. Future compiles
from ^candidate inherit C_frozen, not a silently narrower/wider contract rebuilt
from S_new. Reusing more precise revision-specific analysis requires its own
validated revision dependencies; it must not change the public replacement rule.

Required evidence includes fewer new requests within the same allowed contract,
an added handled request with unchanged escaping row being rejected, temporal
incompatibility, unchanged contract across successive revisions, generic helper
returns, Result.Ok extraction, mixed-origin branches, recursive discovery,
cross-package templates and forged custom-handler results. The same cases must
distinguish contract-backed caller facts from actual ^candidate analysis.

## 7. Effects And Service Boundaries

Pure operations:
- reading available retained definition descriptors with ^;
- parsing in-memory body text and formatting retained syntax;
- with_body and diff over retained snapshots.

These operations may perform bounded local computation but no hidden source
loading or dependency acquisition. Reuse normal constant/descriptor/syntax
operations; do not force frontend-dependent parsing into etas_builtin.

Effectful operations:
- Code.check / Code.compile, declared once in StdRegistry;
- ordinary authorized I/O when saved source and its environment are restored;
- the candidate's actual effects when it is subsequently invoked.

Both wrapper/action overload families are selected by the checked descriptor
type: AgentDef<A> -> A or CallableDef<F> -> F for compile, and CheckReport for
check. StdRegistry declares the overload set under each canonical action/API
identity; checked overload facts drive dispatch, not signature strings or an
Agent-only coercion. Erased CallableDef is never an overload input.

The std wrappers default-handle Code.check/Code.compile, record requested and
default-handled actions, and escape Error<CompileError>.
A custom handler's produced effects follow normal propagation. The postfix ?
captures that residual Error only and retains the requested action facts.
For Code.compile a custom handler may return a previously checked compatible
A/F, but the returned target must still have validated revision/frozen-contract
provenance. Merely sharing A or the arrow F is not a substitute for that check;
handlers cannot forge checked publication or grant new execution authority.

Checking/compilation do not execute or charge model/tool actions from the draft.
Later invocation uses A's or F's validated callable contract and current authority/
trace/handler/limit state. Compiler access is not permission to run the resulting
agent's or flow's effects. A pure F may still require an effectful compilation
request to construct it; its latent effects apply only when F is called.

Register compiler-service support through existing readiness classification.
There is no std.compiler.load or Code.load in this public surface. Source
restoration uses ordinary I/O followed by parse/with_body/compile.

## 8. Execution Revisions And Type Identity

A runtime agent or ordinary-flow value needs stable definition identity plus
an immutable executable revision reference. Agent values additionally carry
nominal A; flow values retain checked F, its generic instantiation and its
frozen behavior provenance. Normalize static declarations and compiled values
to revision-aware targets; do not key a dynamic fallback by a display string.

The interpreter-owned ExecutableRevision owns:
- shared checked program/prepared plan and selected agent method or free flow;
- checked target family and preserved A/F interface plus generic/row bindings;
- revision, authoritative contract and dependency/config fingerprints;
- a stable-to-local symbol/type mapping and interface bridge;
- leased retained definitions/syntax, validated residual plans and engine-owned
  installation evidence binding all required obligations to checked sites,
  revision/contract identities, provider versions and scope lifecycle hooks.

Preparation installs immutable factories/hooks, not one global mutable monitor.
Each invocation activates bounded scope-owned state. Ordinary checked code,
compiled agents and compiled flows use the same mediation and scope-exit path.
The publisher rejects missing/unsupported coverage before exposing A/F, including
custom compiler-handler results. Runtime predicates are enforcement, not new
handlers or permission to erase requested actions/escaping effects.

HIR item/type IDs are local to their checked program. Calls, returns and
higher-order values crossing candidate/base revisions need an explicit
RevisionTypeBridge based on stable nominal constructors, generic bindings and
checked layouts. Never interpret a candidate's TypeId under the caller's
TypeStore or accept compatible-looking runtime shapes as proof.

current = candidate changes one ordinary binding. Aliases and active frames keep
their original revision. ^current returns the selected implementation's snapshot.
New invocations resolve through the value's revision; an in-flight invocation
must not jump to the latest revision in a global map.

Calling F uses ordinary higher-order call dispatch and the pinned revision,
not a new public run-code operation. Its checked latent summary/provenance must
survive variable assignment, parameters, returns and revision bridges. Do not
derive requested actions/trace from F's escaping row or claim that the original
body's stale summary describes the new revision. Using its established frozen
contract as a static conservative boundary is required by the handoff protocol
above and is not that forbidden substitution. Method inspection does not create a
detached free-flow implementation, model tool or permission grant.

Checkpoint/replay preserve target family, revision/contract/dependency identities,
source/type relocation, pending compiler-operation state and residual plan/state
references. Historical monitor progress/register bindings, pending outcomes,
cursor and machine state are saved consistently. Restore validates plans/bounds
and current authority, invalidates old run-local approvals and their Approved/
armed readiness, and preserves other progress rather than resetting monitors.
Compiled A/F values cannot carry approval into another run through their revision
or checkpoint. Persistent reuse requires explicit application opt-in and verified
current bindings under the [run-scoped policy](../../../etas-core/docs/architect/etas-residual-checks-design.md#33-run-scoped-approval-and-explicit-reuse).
Engine-private validated snapshots are not a public binary code-loader API.
Missing/tampered code fails explicitly, never selects the original declaration
or a guessed F.

Public persistence saves source plus expected canonical target and frozen
contract/dependency/config fingerprints using ordinary authorized storage. On
restoration resolve the statically known typed target, validate the locked
surrounding definitions, parse, apply with_body and compile again under current
authority/limits. A body string alone is insufficient; stored summaries are not
proof. An erased CallableDef cannot be cast back using persisted SignatureInfo.
No implicit source/network access occurs during ^, and no std.compiler.load is
introduced. Recompilation required for restore belongs to an explicit restoration
operation, not a hidden fallback during ordinary invocation or reflection.

## 9. Memory, Incremental Reuse And Diagnostics

Code definitions/descriptors are shared projections of immutable revision data,
not copies of ProjectOutput or full maps for every reflected value. Drafts share
their frozen base and own a small overlay. Body edits invalidate body facts and
transitive affected facts under the existing dependency graph. Revalidating
revision-specific internal artifacts is not permission to recompile public
callers against a silently broadened contract.

The generic etas_cache layer stores bounded code artifacts by canonical keys;
frontend owns semantic fingerprints/invalidation. Retention counts must include
pinned revisions and avoid double-counting shared graphs. Evicting a derived
cache must not destroy code used by an active agent, flow or checkpoint.

Reuse current source/span/line-index diagnostics and add structured expected/
actual descriptors, constraint IDs, related call paths and source-fingerprint
references. Do not reconstruct these by parsing rendered message strings.
Abstract counterexample paths are labeled possible analysis paths, not concrete
execution witnesses.

Compiler work has bounded input/output bytes, syntax/descriptor nodes/depth,
dependency work, queue length, live revisions, deadline and cancellation.
The in-process worker uses existing managed operation ownership; child work
cannot become untracked when the caller's future is dropped.

Inherit the active caller limits and cancellation scope. WallTime includes queue
waiting and compiler execution; retries/cache misses/new revisions do not reset
the enclosing allowance. Pure parsing/formatting also count as execution work.
CPU/wall usage uses durations with explicit units and memory uses bytes. Service
CPU/memory ceilings are runtime configuration, not new source limit keywords or
model-token proxies. Tokens/ContextTokens charge actual model use; Cost reflects
actual applicable paid work. Preserve BudgetExceeded, cancellation and authority
outcomes instead of disguising them as reparable syntax errors or CompileError
service failures. Document and test how ceilings are enforced by each adapter.

## 10. Completion Evidence

Acceptance covers source -> syntax -> HIR -> type/effect facts -> reflection ->
draft -> compiler action -> checked candidate -> invocation, including package
metadata and checkpoint paths.

Required regressions include:
- precedence and invalid-call reflection without execution;
- ^A and ^A.run agreement plus separate additional spec methods;
- typed ^flow/^current, body edit, check/compile -> F and successive revisions;
- typed-to-erased projections, heterogeneous arrays, no erased-to-typed recovery,
  no method detachment and no arbitrary closure/frame reflection;
- ^candidate after two revisions and old aliases/in-flight calls;
- body absence/visibility and lexical binding collisions;
- parse errors, unresolved names, bad types, widened requested actions, trace/
  determinism violations and unavailable analysis;
- ordinary ? handling, custom handlers and denied compiler/candidate actions;
- stale reports/dependencies/config, forged descriptors/artifacts and local ID
  collisions across revisions;
- actual installation/enforcement of residual checks;
- frozen requested/trace compatibility when escaping rows are identical, including
  distinct caller entry states, intermediate violations and exit outcomes;
- source-bundle restoration with locked environment, missing body rejection and
  no public binary load or erased-signature cast;
- bounded deep/cyclic metadata, memory retention and cancellation races;
- a deterministic local revision example, followed by an optional real-model
  rewriting example. Model output never replaces compiler validation.

Evaluation, scoring, promotion, rollback/publication and state migration belong
to EDK/application composition. Compiler acceptance is not evidence of an
improved candidate.
