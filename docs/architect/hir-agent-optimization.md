# Typed-HIR agent optimization passes

The frontend runs a conservative, semantics-preserving subset of the agent optimization patterns after type, effect, reachability, and loop analysis. `ProjectCompileOptions::optimization_level` selects the monotonic O0–O3 pass schedule and records matches and rewrites in `CheckedProject::optimization_report`.

The survey examples are used as workload models, not as instructions. The implemented HIR-local forms are:

| Pass | Survey workload represented by the Etas fixtures | Exact HIR-local rewrite |
|---|---|---|
| `AgentEffectLegality` | authorization, hooks, cancellation, event and tool/model effects | materialize per-expression owner, symbol-read and mutable-version dependencies, then classify only deterministic, local expressions with empty request/action/residual/order facts as movable |
| `PrepareVersionedRequest` | mini-swe retry payload preparation, OpenManus tool schemas, cline model/tool mapping | hoist pure loop-invariant leading preparations out of known-nonempty literal loops |
| `VersionedPreparationCSE` | Codex context delta, aider RepoMap, opencode model snapshot reuse | common structurally equal pure immutable preparation expressions within one block |
| `SuffixDemandHistory` | latest CWD, recent tool result, retry error and checkpoint demand | replace a complete literal history scan whose only result is the last value with the last value |
| `FuseProjectionScans` | adjacent message, event, metadata and tool projections | fuse equal bounded literal scans with disjoint string reductions |
| `SynthesizeScopedIndex` | MCP/tool/model/request ID and pending-interrupt lookup | build a compile-time literal-key index and lower a full static membership scan to its proven result |
| `LowerOwnedSnapshotCOW` | history, prompt, config and manifest snapshots | lower a typed immutable `Array`/`List` extension by an empty collection to the original snapshot, eliminating the owned copy/allocation |
| `FuseSerializationBoundary` | prompt, schema, trace, checkpoint and RPC encode/decode boundaries | remove the typed standard-library `JsonValue -> stringify -> parse` round trip, including checked `Result.unwrap` nodes, while preserving the original structured value |
| `BatchStorageSemijoin` | N+1 key validation and batched storage probes | combine adjacent independent membership probes over the same bounded literal snapshot into one scan |
| `ScheduleBoundedPipeline` | bounded file, model, tool, trace and persistence stages | combine adjacent `BoundedPipelineStage`-marked pure stages over the same literal worklist when their mutable accumulators are disjoint |
| `LowerResourceRegion` | model clients, connection pools, HTTP sessions and tool hosts | move a pure `ReusableResourceFactory`-marked acquisition with version-stable arguments to the outer region of a known-nonempty loop |
| `PruneUnusedQualityContext` | repo map, chat history, retrieval, memory and policy context candidates | remove an unconsumed pure `QualityContext`-marked boundary exactly |
| `ApplyQualityContextBudgetPolicy` | explicitly budgeted context candidates | when `quality_context_token_budget` is set, retain only the most recent units of its literal context argument; reported separately as a policy transform |

## Implemented pass reference

### Shared legality analysis

#### `AgentEffectLegalityPass`

This is the prerequisite analysis for every optimization group, not a rewrite by
itself. For each checked expression it records:

- the owning HIR item;
- symbols read by the expression;
- mutable symbols whose versions affect the result;
- whether the expression has no escaping, requested, default, handled, or
  residual effects;
- whether evaluation is deterministic and has an empty action trace;
- whether moving the expression can preserve observable order.

The transform passes use these facts through `is_movable` and
`is_version_stable`. Missing effect facts, non-local interpreter support,
mutable-version dependencies, handlers, inference, residual checks, or trace
obligations make an expression ineligible. The analysis therefore fails closed
instead of guessing purity from source spelling.

### O1: local exact rewrites

#### `LowerOwnedSnapshotCowPass`

Pattern: agent state often creates an immutable history, configuration, tool
registry, or checkpoint snapshot and then extends it with an empty collection.

Rewrite:

```text
snapshot.extend([])  ->  snapshot
```

The pass accepts only checked standard-library `array_extend` or `list_extend`
intrinsics, an empty collection argument, a path or literal receiver, and a
movable expression. It removes the redundant collection operation and its
temporary nodes while preserving the original snapshot value and identity.

#### `FuseSerializationBoundaryPass`

Pattern: a typed JSON value crosses an internal boundary by being serialized
and immediately parsed again even though no external byte or text consumer
observes the wire representation.

Rewrite:

```text
parse(stringify(value).unwrap()).unwrap()  ->  value
```

The matcher requires the resolved `std.json.stringify`, `std.json.parse`, and
checked `Result.unwrap` intrinsics. The original value must be a movable path.
Aliases with the same spelling, partial encode/decode chains, and observable
wire boundaries are not rewritten.

#### `SuffixDemandHistoryPass`

Pattern: an agent scans a complete bounded literal history only to overwrite
the same variable with every element and return the most recent item.

Rewrite:

```text
var latest = fallback;
for item in [old_1, old_2, current] limit Iterations(3) {
    latest = item;
}
```

becomes an exact assignment of the last literal element. The loop must cover
the whole non-empty literal sequence, contain only the overwrite, and have no
effectful or order-sensitive work. This models latest-message, latest-CWD,
retry-error, checkpoint, and provider-snapshot demand.

#### `PruneUnusedQualityContextPass`

Pattern: context collection produces a repo map, history, retrieval pack,
memory recall, or diagnostic trace that is never consumed by the selected
answer path.

Rewrite: remove the unused `let` statement whose value is a pure call marked
with the `QualityContext` optimization contract. The binding must have no
reads, and the call must be movable. This is dead-context elimination, not
context truncation, so returned values and model inputs remain unchanged.

### O2: reuse, fusion, and static lookup

#### `PrepareVersionedRequestPass`

Pattern: retry, model, tool, provider, and serializer loops rebuild the same
version-stable request preparation for every iteration.

Rewrite: move the leading invariant `let` bindings from a loop body to the
parent block immediately before the loop. The iterable must be a known
non-empty literal collection. Hoisted expressions cannot read the loop binding
or mutable locals, contain control/effect forms, or acquire a reusable resource.
Dependent invariant bindings may be hoisted together in source order.

#### `VersionedPreparationCsePass`

Pattern: one block prepares the same context delta, repo map, model snapshot,
route plan, schema, or serializer configuration more than once for the same
version.

Rewrite:

```text
let first = prepare("revision-7");
let replay = prepare("revision-7");
```

becomes one preparation plus a reference to `first`. Equality is checked over
the HIR structure, resolved symbols, generic arguments, named argument order,
literals, and supported collection/binary forms. Both expression trees must be
version-stable and free of mutable or effectful dependencies.

#### `FuseProjectionScansPass`

Pattern: adjacent message, trace, tool, policy, or workspace projections scan
the same bounded snapshot into two independent accumulators.

Rewrite: retain the first loop and append the second projection assignment to
its body, removing the second traversal. Both loops must have equal literal
inputs and effective iteration limits, exactly one string-reduction assignment,
and different target variables. The reduction expression and iterable must be
movable.

#### `BatchStorageSemijoinPass`

Pattern: adjacent authorization or storage membership probes independently scan
the same bounded key snapshot for two literal keys.

Rewrite: fuse the two full scans into one traversal while retaining both
literal comparisons and their distinct boolean result variables. The pass
requires equal literal inputs, equal covering limits, simple `target = true`
hit bodies, and movable comparison work. It does not combine arbitrary storage
effects or host calls.

#### `SynthesizeScopedIndexPass`

Pattern: a scoped MCP, tool, model, provider, resource, or workspace lookup
performs a full literal scan for a literal key whose presence is known at
compile time.

Rewrite: build a temporary compiler-side set of literal keys and replace a
proven-hit loop with the loop's `target = true` assignment. The loop must cover
the complete literal sequence and have no other body behavior. Dynamic probes,
partial scans, and absent-key cases remain untouched.

### O3: scheduling and lifetime changes

#### `ScheduleBoundedPipelinePass`

Pattern: two adjacent pure stages independently traverse the same bounded job,
tool, file, retrieval, trace, or persistence worklist.

Rewrite: fuse the second stage assignment into the first traversal. Each stage
callee must carry the `BoundedPipelineStage` contract, take its own accumulator
and the current item, and depend on no mutable version other than its own
accumulator. Inputs and limits must match and target accumulators must be
disjoint.

The pass changes scheduling shape, so it is O3 even though checked legality
facts prove the result and effect order are unchanged.

#### `LowerResourceRegionPass`

Pattern: every iteration reacquires an identical model client, connection pool,
HTTP session, cache handle, tool host, trace sink, or vector store.

Rewrite: move one `let resource = acquire(...)` before a known-nonempty loop.
The factory must carry the `ReusableResourceFactory` contract, use no generic
arguments, and receive only version-stable arguments. The acquisition itself
must be version-stable. This deliberately excludes resources whose authority,
lifetime, or arguments vary by iteration.

### Explicit context policy

#### `ApplyQualityContextBudgetPolicyPass`

This transform is not part of O0-O3 because it can change model input and
therefore program results. It runs only when
`quality_context_token_budget` is explicitly configured.

For a `QualityContext`-marked call with a literal string argument, it retains
the most recent whitespace-delimited input units up to the configured budget.
The implementation currently measures lexical input units, not provider-specific
tokenizer tokens. Its rewrites are recorded as policy transforms rather than
semantics-preserving optimizations.

## Optimization schedule

| Level or option | Newly enabled work |
|---|---|
| O0 | no optional HIR optimization |
| O1 | legality analysis, owned-snapshot COW, serialization fusion, suffix demand, unused-context pruning |
| O2 | O1 plus versioned preparation hoisting/CSE, projection fusion, storage semijoin batching, scoped-index synthesis |
| O3 | O2 plus bounded-pipeline scheduling and resource-region lowering |
| explicit context budget | context-budget policy after the selected optimization schedule |

The sets are monotonic: a pass enabled at O1 remains enabled at O2 and O3.
`ProjectCompileOptions::optimization_level` and the independent context budget
both participate in the compile fingerprint.

The default pipeline does not perform an approximate P3 context change.
`ProjectCompileOptions::quality_context_token_budget` is a separate opt-in
policy input, participates in the canonical compile fingerprint, and is
intentionally not presented as a semantics-preserving rewrite. Interpreter
benchmarks report lexical model-input units; provider-tokenizer counts and
answer/fact-retention evaluation remain separate live-model work.

`@optimization` markers are explicit source contracts, not spelling heuristics. A marker only selects the intended workload role; checked type/effect facts, version dependencies, loop bounds, disjoint accumulators, and exact HIR shape still prove whether a rewrite is legal. Renaming a marked factory or stage therefore does not change optimization behavior.

## Survey provenance

The fixture families preserve the computational shape of concrete survey observations rather than copying their source language. Representative anchors are:

| Pass | Survey observation and source |
|---|---|
| `AgentEffectLegality` / `ScheduleBoundedPipeline` | `OPT-005`, mini-swe-agent effect-aware action scheduling |
| `PrepareVersionedRequest` | `OPT-006`, mini-swe-agent retry payload preparation |
| `VersionedPreparationCSE` | `OPT-020`, opencode session compaction preparation reuse |
| `SuffixDemandHistory` | `FULL-a4a777b4c850e2`, agent-zero bounded retention-history suffix |
| `FuseProjectionScans` | `FULL-d5cbce5e5fda8e`, ChatDev complementary message reductions |
| `SynthesizeScopedIndex` | `FULL-5723d4e7f08547`, ChatDev sealed-topology edge index |
| `LowerOwnedSnapshotCOW` | `OPT-026`, cline immutable history snapshot sharing |
| `FuseSerializationBoundary` | `FULL-2af3a5aa471885`, Codex single serialization for combined goal projections |
| `BatchStorageSemijoin` | `FULL-23ef735e150ad2`, Codex batched thread-item metadata hydration |
| `LowerResourceRegion` | `OPT-035`, ChatDev scoped runtime-resource reuse |
| `PruneUnusedQualityContext` / `ApplyQualityContextBudgetPolicy` | `OPT-045` exact unused-observation elimination plus the workbook's explicit P3 context-budget prototype |

## Stable facts across rewrites

HIR arena IDs remain stable. Removed nodes are explicitly marked optimized out so tree ownership validation still rejects accidental orphans. When an expression root changes shape, its stable type and empty effect summary remain valid, while method, generic-call, intrinsic, reflection, indexing, and other old-shape facts are invalidated before later preparation and backend passes consume them.

## Tests

Each pass has at least ten Etas programs under `crates/etas_frontend/tests/fixtures/optimization`. Frontend integration tests assert pass ownership and rewrite counts. The interpreter repository compiles and executes exact-transform programs at O0, O1, O2, and O3, checks equal results, and measures deterministic steps, allocations, and elapsed time. The budget fixtures are separate and assert a smaller returned model-input context and recent-suffix retention at every level because the policy is explicitly semantics-changing.

The current fixture set uses workflow-shaped programs rather than isolated
micro-expressions. Frontend coverage contains 130 programs and 4,250 effective
Etas lines; interpreter coverage contains 120 programs and 3,930 effective
lines. Every fixture has at least 30 non-blank, non-comment source lines. The
interpreter warms each optimization level and reports the median of five
measurement rounds.

The following reductions were generated on 2026-11-05 over ten programs per
pass. They compare the level that enables the pass with O0 and are evidence
from this fixture set, not performance guarantees for every workload or
machine.

| Pass | Interpreter steps | Allocations | Median elapsed time |
|---|---:|---:|---:|
| `LowerOwnedSnapshotCow` | 2.16% | 7.73% | 13.35% |
| `FuseSerializationBoundary` | 15.52% | 42.42% | 37.30% |
| `SuffixDemandHistory` | 22.16% | 22.38% | 20.11% |
| `PruneUnusedQualityContext` | 12.52% | 12.02% | 10.39% |
| `PrepareVersionedRequest` | 30.73% | 31.57% | 31.17% |
| `VersionedPreparationCse` | 10.43% | 11.00% | 10.77% |
| `FuseProjectionScans` | 8.22% | 11.06% | 8.47% |
| `BatchStorageSemijoin` | 7.77% | 10.35% | 9.06% |
| `SynthesizeScopedIndex` | 30.58% | 30.09% | 26.15% |
| `ScheduleBoundedPipeline` | 5.66% | 7.48% | 5.86% |
| `LowerResourceRegion` | 20.40% | 21.57% | 21.09% |

With an explicit budget of eight units, the separate context policy reduced the
measured model-input units by 75.76% and median elapsed time by 15.20%. This is
reported separately because the policy is intentionally semantics-changing.
