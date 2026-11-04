# Typed-HIR agent optimization passes

The frontend runs a conservative, semantics-preserving subset of the agent optimization patterns after type, effect, reachability, and loop analysis. The pass pipeline is enabled by `ProjectCompileOptions::optimize_hir` and records matches and rewrites in `CheckedProject::optimization_report`.

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
| `QualityAwareContextPolicy` | repo map, chat history, retrieval, memory and policy context candidates | remove an unconsumed pure `QualityContext`-marked boundary exactly; when `quality_context_token_budget` is explicitly set, retain only the most recent units of its literal context argument |

The default pipeline does not perform an approximate P3 context change. `ProjectCompileOptions::quality_context_token_budget` is a separate opt-in policy input, participates in the canonical compile fingerprint, and is intentionally not presented as a semantics-preserving rewrite. The interpreter and live-model evaluations report token reduction separately from answer/fact retention.

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
| `QualityAwareContextPolicy` | `OPT-045` exact unused-observation elimination plus the workbook's explicit P3 context-budget prototype |

## Stable facts across rewrites

HIR arena IDs remain stable. Removed nodes are explicitly marked optimized out so tree ownership validation still rejects accidental orphans. When an expression root changes shape, its stable type and empty effect summary remain valid, while method, generic-call, intrinsic, reflection, indexing, and other old-shape facts are invalidated before later preparation and backend passes consume them.

## Tests

Each pass has at least ten Etas programs under `crates/etas_frontend/tests/fixtures/optimization`. Frontend integration tests assert pass ownership and rewrite counts. The interpreter repository compiles exact-transform programs with optimization disabled and enabled, checks equal results, and measures deterministic steps, allocations, and elapsed time. The P3 budget fixtures instead assert a smaller returned model-input context and recent-suffix retention because the policy is explicitly semantics-changing.
