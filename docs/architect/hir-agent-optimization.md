# Typed-HIR agent optimization passes

The frontend runs a conservative, semantics-preserving subset of the agent optimization patterns after type, effect, reachability, and loop analysis. The pass pipeline is enabled by `ProjectCompileOptions::optimize_hir` and records matches and rewrites in `CheckedProject::optimization_report`.

The survey examples are used as workload models, not as instructions. The implemented HIR-local forms are:

| Pass | Survey workload represented by the Etas fixtures | Exact HIR-local rewrite |
|---|---|---|
| `AgentEffectLegality` | authorization, hooks, cancellation, event and tool/model effects | classify only deterministic, local expressions with empty request/action/residual facts as movable |
| `PrepareVersionedRequest` | mini-swe retry payload preparation, OpenManus tool schemas, cline model/tool mapping | hoist pure loop-invariant leading preparations out of known-nonempty literal loops |
| `VersionedPreparationCSE` | Codex context delta, aider RepoMap, opencode model snapshot reuse | common structurally equal pure immutable preparation expressions within one block |
| `SuffixDemandHistory` | latest CWD, recent tool result, retry error and checkpoint demand | replace a complete literal history scan whose only result is the last value with the last value |
| `FuseProjectionScans` | adjacent message, event, metadata and tool projections | fuse equal bounded literal scans with disjoint string reductions |
| `SynthesizeScopedIndex` | MCP/tool/model/request ID and pending-interrupt lookup | build a compile-time literal-key index and lower a full static membership scan to its proven result |
| `LowerOwnedSnapshotCOW` | history, prompt, config and manifest snapshots | remove an owned snapshot wrapper only when its body is proven to return the immutable input unchanged |
| `FuseSerializationBoundary` | prompt, schema, trace, checkpoint and RPC encode/decode boundaries | remove an encode/decode boundary only when its body is proven to preserve the value exactly |
| `BatchStorageSemijoin` | N+1 key validation and batched storage probes | combine adjacent independent membership probes over the same bounded literal snapshot into one scan |
| `ScheduleBoundedPipeline` | bounded file, model, tool, trace and persistence stages | combine adjacent pure stages over the same literal worklist when their mutable accumulators are disjoint |
| `LowerResourceRegion` | model clients, connection pools, HTTP sessions and tool hosts | move a pure static resource descriptor acquisition to the outer region of a known-nonempty loop |
| `QualityAwareContextPolicy` | repo map, chat history, retrieval, memory and policy context candidates | remove a pure candidate only when typed-HIR use analysis proves there is no consumer |

The default pipeline does not perform an approximate P3 context change. A pass that intentionally changes model-input tokens must use a separate explicit policy option and be evaluated for task success and fact recall with a live model; it must not be presented as an exact compiler rewrite.

## Stable facts across rewrites

HIR arena IDs remain stable. Removed nodes are explicitly marked optimized out so tree ownership validation still rejects accidental orphans. When an expression root changes shape, its stable type and empty effect summary remain valid, while method, generic-call, intrinsic, reflection, indexing, and other old-shape facts are invalidated before later preparation and backend passes consume them.

## Tests

Each pass has at least ten Etas programs under `crates/etas_frontend/tests/fixtures/optimization`. Frontend integration tests assert pass ownership and rewrite counts. The interpreter repository compiles the performance programs with optimization disabled and enabled, checks equal results, and measures deterministic steps, allocations, and elapsed time.
