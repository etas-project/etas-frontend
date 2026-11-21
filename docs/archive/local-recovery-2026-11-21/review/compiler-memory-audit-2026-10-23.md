<!-- Historical recovery snapshot, archived 2026-11-21. Not a current specification, implementation status, or engineering task assignment. The current language SPEC and main implementation take precedence. -->

# Compiler Memory And Performance Audit

Review started: 2026-10-23. Completed: 2026-10-24 (Asia/Hong_Kong). Owner: Architect. No compiler implementation changes were made.

## Result

The current frontend has confirmed superlinear memory growth on small, valid projects. The principal causes are body artifacts containing project-wide facts, followed by multiple owned copies of those artifacts in output and cache. Whole-project HIR index reconstruction inside per-unit abstract interpretation adds significant allocation churn and CPU cost.

The controlled 128-helper input is 9,142 bytes / 514 lines, yet a successful debug CLI check reaches **341.6 MiB peak RSS** with disk cache and profiling disabled. This does not require external packages, networking, agent execution, a long-lived IDE or large literals.

No unbounded leak was demonstrated: three repeated identical checks return to the same retained live-heap level after each response is dropped, and dropping the session releases almost all tracked allocations. Duplicate ownership, excessive retention and allocation churn must be distinguished from a leak.

## Baseline And Method

- CLI: `9f2e63f67ee9cc160a69ce79500d3d529d34217f`.
- Frontend: `5d7b3d76cb9d634c69676d17ed49c30057968b36`.
- Core: `28814d6fe29cbdb2622d7a238608e9d0ed4238d8`.
- macOS / arm64. Rebuilt the current CLI with `cargo build -q -p etas_cli --offline`.
- Normal debug binary; no release benchmark or claim about release latency is made.
- Measurements ran serially. CLI samples use `/usr/bin/time -l`; on this platform its maximum resident set size is in bytes.
- Primary CLI samples use `check --all --cache off --no-config`; they type/effect-check all source bodies without executing them.
- Single samples establish a reproducible scale problem, not a statistically rigorous throughput comparison. Small-case wall times include startup/page-fault noise.
- An external Rust probe uses the current frontend public session API, local path patches for all Core crates, and a System allocator wrapper that counts requested live heap bytes, peak live bytes and cumulative allocated bytes. These counters exclude allocator overhead, RSS and untracked native allocations.
- Component clone costs measure the additional live bytes of one owned clone; they are evidence of retained structure size, not serialized size.
- Probe fixture source, Cargo manifest and source are under `/tmp/etas-compile-memory-audit-2026-10-23/`. The probe changes no compiler source. The full measurement data is [available here](/Users/hrtan/Projects/aplas/etas-frontend/docs/review/compiler-memory-audit-2026-10-23.json).

## CLI Measurements

All helper functions have the same small body; a separate main calls one helper. Helper counts below therefore have one additional body for main.

| Helper flows | Source bytes | Lines | Wall time | Peak RSS |
| --- | ---: | ---: | ---: | ---: |
| 1 | 125 | 6 | 0.09 s | 24.3 MiB |
| 16 | 1,190 | 66 | 0.06 s | 34.6 MiB |
| 32 | 2,326 | 130 | 0.22 s | 55.1 MiB |
| 64 | 4,598 | 258 | 0.44 s | 119.6 MiB |
| 128 | 9,142 | 514 | 1.65 s | 341.6 MiB |

The existing `compiler/projects/positive/multi_agent_system` fixture also succeeds: 6 source files, 276 lines / 7,687 source bytes, **128.3 MiB peak RSS**, 0.36 s. No model or Host execution is involved.

A separate 128-helper run with profiling enabled reaches 341.7 MiB, so the high memory use does not depend on the profiler.

Reproducer shape:

```etas
module main;
public flow f_0000(x: i32) -> i32 {
    let y = x + 0;
    return y;
}
// Repeat independent helpers with distinct names.
flow main() -> i32 { return f_0000(1); }
```

Command, from the etas repository:

```sh
/usr/bin/time -l target/debug/etas check --all --cache off --no-config \
  /tmp/etas-compile-memory-audit-2026-10-23/case_128/main.es
```

## Findings

### 1. P1: Effect Body Artifacts Retain Project-Wide Facts

[RunEffectPipelinePass](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_frontend/src/passes/analysis/effect_check.rs:108) clones the complete EffectOutput for every body and calls [local_effect_output](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_frontend/src/passes/analysis/effect_check/body_artifacts.rs:52).

That projection filters some item/symbol/support maps, but leaves project-wide expression facts, statement facts, unit facts, public contracts and other fields in each body artifact. Retaining a few map entries also does not shrink cloned HashMap backing capacity.

Measured ownership counts:

| Data | 65 bodies | 129 bodies |
| --- | ---: | ---: |
| Project expression-effect facts | 259 | 515 |
| Sum of expression-effect facts in body outputs | 16,835 = 259 x 65 | 66,435 = 515 x 129 |
| Project unit-effect facts | 65 | 129 |
| Sum of unit-effect facts in body outputs | 4,225 | 16,641 |
| Project public contracts | 64 | 128 |
| Sum of public contracts in body outputs | 4,160 | 16,512 |

One global EffectOutput clone costs 471,104 bytes at 65 bodies; cloning all body outputs costs **30,728,392 bytes**. At 129 bodies those costs are 942,144 and **121,749,832 bytes** respectively.

This is a confirmed O(B x E) ownership problem, where B is body count and E is project effect-fact size. The session then clones these outputs into its cache.

Required correction:
- Materialize body-owned effect facts directly from the project result using validated body ownership.
- A body artifact may contain its own expressions, statements, unit summary and needed contract references; it must not embed every other body's facts.
- Keep shared registry/common information once. Establish explicit ownership for nested lambdas/handler units so they are not accidentally duplicated or discarded.
- Construct local maps directly. Cloning a global map and retaining a subset still incurs global transient copying and may retain global capacity.
- Test both semantic isolation and physical capacity/bytes, including a body change that must not reuse another body's stale facts.
- Preserve real requested/escaping effect analysis and all validation; skipping effects is not the fix.

### 2. P1: Type Body Outputs Copy The Global Signature Seed

[TypeCheckBodyPass](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_frontend/src/passes/analysis/type_check_body.rs:55) clones Signature TypeOutput for every body. [check_body_with_seed_and_std_registry](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_types/src/pipeline/mod.rs:47) consumes the cloned TypeStore/facts, reconstructs an interner and symbol index, and returns a complete TypeOutput again.

[FinalizeTypeFactsPass](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_frontend/src/passes/analysis/type_check_finalize.rs:24) then clones all body outputs into a Vec while the original body map stays alive. [merge_outputs](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_types/src/pipeline/finalize/merge.rs:1) remaps and merges them.

The 65-body project has 65 item signatures but stores **4,225** copies across body outputs; the 129-body project has 129 signatures but stores **16,641**. Cloning all body type outputs costs 7,077,224 and 22,782,824 bytes respectively. Local type inference is small in these fixtures.

Required correction:
- Use one immutable revision-owned SignatureEnvironment and a shared base type universe.
- Each body produces a BodyTypeDelta: body-local facts, checked instantiations/diagnostics and local inferred type additions.
- Define how local TypeIds are relocated into the final type universe and into persisted artifacts. Sharing a Vec behind Arc without resolving ID ownership is insufficient.
- Aggregate by moving or borrowing deltas; do not clone the entire map into a second Vec.
- Keep the IDE/session API and invalidation dependencies backed by immutable revision data.
- The frontend architecture already describes body TypeFacts deltas; this implementation does not meet that granularity.

### 3. P1: Output, Cache And Snapshots Own Duplicate Semantic Graphs

[BuildCheckedProjectPass](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_frontend/src/passes/output/checked_project.rs:93) clones HIR, type/effect facts and related data into CheckedProject. It also clones symbols/scopes/source_map separately even though HirProgram embeds them.

[ProjectSemanticSnapshot::from_output](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_frontend/src/session/snapshot.rs:71) clones them again. [FrontendProjectState::check_with_pipeline](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_frontend/src/session/project.rs:158) builds this snapshot unconditionally on success, then clones it into last_good_snapshot and optionally into the response.

[store_project_artifacts](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_frontend/src/session/project.rs:382) separately clones parse outputs, HIR, body outputs, final facts and CheckedProject into memory storage. Parsed source data is stored both as the project parsed-source set and as individual parsed-source artifacts.

SnapshotDetailLevel::None only suppresses the response field; it does not suppress internal snapshot construction or last-good retention.

Measured at 65 bodies, with disk access and memory reuse disabled:

| Probe variant | Live heap after check | Peak live heap | After response drop |
| --- | ---: | ---: | ---: |
| Normal memory store, response snapshot None | 87,334,410 bytes | 89,070,098 bytes | 45,219,267 bytes |
| Diagnostic store discards writes, snapshot None | 44,678,707 bytes | 46,414,395 bytes | 2,554,332 bytes |
| Normal store, response snapshot Full | 89,070,098 bytes | 90,805,786 bytes | 45,219,267 bytes |

The discard store is a diagnostic experiment, not a proposed production cache replacement. All variants run the actual compiler and still construct its normal outputs.

Required correction:
- Establish immutable revision-owned semantic storage and cheap shared handles for CheckedProject/snapshot/cache views.
- Store body deltas once; make project-level views reference them.
- Keep last-good semantics for error recovery, but share the valid revision rather than deep-cloning its graph.
- Separate required one-shot output from incremental retention policy. MemoryArtifactReuse::Disabled currently disables reuse, not writes; disk `--cache off` does not disable the memory store.
- Release stage-only context when the next stable artifact has been constructed.
- Preserve source mapping and IDE query completeness; deleting information required by consumers is not an optimization.

### 4. P1: Abstract Interpretation Rebuilds Whole-Project HIR Index Per Unit

[HirAbstractInterpreter::new](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_hir_analysis/src/intraprocedural/interpreter/mod.rs:24) constructs a new HirAnalysisContext, which [builds HirTreeIndex for the entire program](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_hir_analysis/src/context.rs:12).

[SummarySolver::build_call_graph](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_hir_analysis/src/interprocedural/solver.rs:128) creates one interpreter per unit. [transfer_unit](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_hir_analysis/src/interprocedural/solver.rs:177) constructs another interpreter for each unit transfer, including subsequent fixpoint visits.

This is repeated by effect summary analysis, memory provenance and trace monitor analysis. There is already a frontend-finalized index, but this path does not receive it as shared context.

HirAnalysisContext::view also deep-clones its index. [HirTreeView](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_hir/src/view/tree.rs:15) owns a HirTreeIndex, whose [14 maps](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_hir/src/view/model.rs:8) are ordinary owned HashMaps/Vectors. Creating a view is not a cheap borrowed operation.

The 128-helper profile records:
- frontend.check: 1.476 s;
- effects.analyze_memory_provenance: 0.380 s;
- effects.solve_summaries: 0.380 s;
- effects.trace_spec.analyze_monitors: 0.378 s.

Together these three stages account for approximately 77% of the profiled frontend span. The source contains no memory accesses or user trace specs. The timings do not isolate the exact fraction spent constructing indexes, but the per-unit rebuild path is directly verified in code.

Required correction:
- Build/validate the index once per immutable HIR revision.
- Pass a borrowed or Arc-owned analysis context through the generic interprocedural driver and adapters.
- HirAbstractInterpreter instances should own per-analysis traversal/domain state, not reconstruct immutable whole-project structure.
- Make views borrow/share the validated index; tie validity to the HIR revision.
- Never bypass index validation or remove general abstract interpretation to obtain a faster test.

### 5. P2: Incremental Manifest Queries Clone The Whole Context

[current_artifact_manifest](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_frontend/src/project/context.rs:196) constructs a temporary ProjectOutput by cloning all available semantic outputs solely to generate a manifest.

It is called by [ReuseTypeBodyFactsPass](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_frontend/src/passes/analysis/type_reuse.rs:39) and [record_reused_effect_body_artifacts](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_frontend/src/passes/analysis/effect_check/body_artifacts.rs:17).

This cost is established statically; the main CLI scale experiment uses a cold FullProject request and does not quantify it. In a warm session it can overlap old snapshot/cache, incoming body artifacts and current context.

Required correction:
- Build the manifest from borrowed artifact metadata/fingerprints or an incremental manifest builder.
- Never build an owned ProjectOutput merely to inspect hashes/dependencies.
- Validate warm no-change checks and one-body edits separately; fewer passes do not prove lower memory.

### 6. P2: Memory Retention Has No Byte Budget Or Useful Memory Telemetry

[MemoryArtifactStore](/Users/hrtan/Projects/aplas/etas-core/crates/etas_cache/src/store/memory.rs:10) owns a HashMap of boxed artifacts with no eviction/byte accounting. Its get returns an owned clone. Frontend disk store also contains a memory store; disabling disk does not remove its RAM costs.

The existing [profile model](/Users/hrtan/Projects/aplas/etas-core/crates/etas_utils/src/profile/model.rs:4) records times and user counters, but current compiler wiring does not record per-stage heap/retained bytes, allocation churn or HIR index construction counts.

Required correction:
- Budget derived caches separately from mandatory live revision state.
- Expose retained bytes by artifact kind, allocation churn, revision count and index-build count.
- Record cold checks, warm checks, edits and session close. RSS alone cannot attribute ownership because the allocator may retain freed pages.
- A cache budget is a backstop; it must not replace the body-granularity fixes above.

## Allocation Versus Retention

At 65 bodies the probe allocates 168,932,992 cumulative bytes during the check and peaks at 89,070,098 live bytes. At 129 bodies these grow to 593,025,964 and 314,776,074 bytes.

Three identical 65-body full checks retain exactly 45,219,292 live bytes after each response drop. Dropping the session leaves approximately 1.9 KiB of tracked process allocations (including output/runtime globals). This experiment supports a stable but excessive working set; it does not prove an indefinitely growing leak.

The first experiment's main owned components are:
- all body effects: 30,728,392 bytes;
- all body types: 7,077,224 bytes;
- CheckedProject: 1,548,641 bytes;
- HIR plus tree index: 588,702 bytes;
- final project type output: 157,728 bytes.

The logical body artifact maps, rather than AST/HIR arena layout alone, dominate this small case.

## Engineering Order And Acceptance

1. Correct body artifact ownership for Effects and Types. Test that body facts contain no unrelated global facts and that total body payload grows with total source facts rather than body count times project size.
2. Share the validated HIR analysis context across all generic analysis visits; a new revision may rebuild once, repeated transfers must not.
3. Unify immutable ownership for output, CheckedProject, snapshots and reusable artifacts. Introduce an explicit one-shot retention policy without weakening last-good/error recovery semantics for persistent sessions.
4. Remove manifest-query clones; add byte budgets and instrumentation.
5. Re-run both real fixtures and controlled 16/32/64/128-body inputs in debug and release. Add more bodies only after the superlinear path is fixed.
6. Measure a warm no-change check, one-body edit, signature edit, failed edit retaining last-good state, response disposal and project/session close.
7. Record peak live heap, peak RSS, cumulative allocation, retained bytes by kind, fact counts and validated-index builds. Avoid a single machine-dependent wall-clock assertion.
8. Keep precise type/effect diagnostics, import/package identity, TypeId relocation, effect summary soundness and IDE source links unchanged.

These fixes belong to frontend artifact ownership and shared analysis infrastructure. Introducing AIR/bytecode alone would not remove them.

## Review Boundaries

Only temporary fixtures/probe files and this review report/evidence were created. Production source, language SPEC and existing tests were not edited. The CLI was rebuilt against the current local repositories. Existing dirty SPEC files and lockfiles were preserved. No commits, pushes or Issue state changes were made.
