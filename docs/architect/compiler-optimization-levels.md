# Compiler Optimization Levels And Pass Scheduling

Owner: `Architect`

Status: `Proposed`

Last updated: `2026-11-05`

Related documents:

- [Phase 1 Frontend Design](phase1-frontend-design.md)
- [Typed-HIR Agent Optimization Passes](hir-agent-optimization.md)
- [Project Architecture](project-architecture.md)

## 1. Decision Summary

Etas exposes one compiler optimization-level option with four values:

```text
-O0  no optional optimization
-O1  cheap local typed-HIR simplification
-O2  balanced typed-HIR optimization (default)
-O3  aggressive typed-HIR scheduling and lifetime optimization
```

Optimization levels select a deterministic subset of ordinary
`etas_utils::Pass<ProjectContext>` implementations. The selected passes are
composed into the existing frontend `Pipeline<ProjectContext>` and executed by
the existing `PassManager<ProjectContext>`. Optimization levels must not create
a second optimizer runner, a second pass trait, or command-specific compiler
pipelines.

`ProjectCompileOptions::optimize_hir: bool` is replaced by the typed
`optimization_level: OptimizationLevel` option. `OptimizationLevel::O2` is the
default so that the new API preserves the current default, where guarded HIR
optimization is enabled.

All `-O0` through `-O3` profiles preserve language semantics. An option that
intentionally changes prompt/context selection, resource authority, effects,
or observable ordering is a separate policy option and is never enabled
implicitly by an optimization level.

## 2. Goals

- Give CLI and API callers one stable optimization-level contract.
- Reuse the existing project Pipeline, PassManager, pass descriptors,
  instrumentation, timing, interruption, and artifact ledger.
- Make the pass schedule for every level deterministic, inspectable, and
  testable.
- Keep the pass sets monotonic: every optimization enabled at a lower level is
  also enabled at every higher level.
- Keep correctness, validation, and backend-support passes independent of the
  selected optimization level.
- Include optimization level and independent policy inputs in compilation and
  cache fingerprints.
- Allow local HIR optimization to migrate to a future `etas-optimizing`
  pipeline without changing the CLI contract.

## 3. Non-Goals

- `-Os`, `-Oz`, profile-guided optimization, or target-specific tuning.
- Enabling approximate or semantics-changing context reduction at `-O3`.
- Giving source annotations permission to raise the compiler optimization
  level.
- Adding an optimizer-specific execution framework.
- Moving FIR or AIR optimization into `etas-frontend`. Those remain owned by
  `etas-optimizing` when that phase is implemented.

## 4. Semantic Contract

### 4.1 Mandatory Pipeline Versus Optional Optimization

The project pipeline is divided into mandatory compiler work and one optional
optimization group:

```text
FrontendSession.check
  -> source/module/graph/HIR construction
  -> type, effect, reachability, residual, and loop analysis
  -> frontend.hir_optimization(level, explicit policies)
  -> interpreter/backend support verification
  -> invocation/reflection preparation
  -> checked-project emission
```

The following work is mandatory at every optimization level:

- parsing, name and import resolution;
- type and effect checking;
- reachability and residual-plan validation;
- loop-progress diagnostics;
- tree ownership and checked-HIR validation;
- interpreter/backend support verification;
- entry-contract, invocation, and reflection preparation;
- checked-project construction.

`-O0` disables only optional optimization. It does not mean "skip analysis" and
does not expose unchecked or unnormalized HIR.

### 4.2 Semantics Preservation

Every pass selected by `OptimizationLevel` must preserve:

- returned values and raised errors;
- effect requests, actions, handler behavior, and trace obligations;
- deterministic ordering required by the checked effect facts;
- resource authority and externally observable resource lifetime;
- public reflection and metadata contracts;
- source-to-HIR identity required for diagnostics and debugging.

A pass must fail closed when its proof facts are absent. It must not treat a
missing type, effect, reachability, ownership, or version fact as permission to
rewrite.

### 4.3 Policy-Changing Transforms

`quality_context_token_budget` is not an optimization level. It is an explicit
policy input because truncating context can change model input and program
results. It remains independently fingerprinted and may be used with any
optimization level, including `-O0`.

The existing `QualityAwareContextPolicyPass` currently combines two different
contracts and must be split:

```text
PruneUnusedQualityContextPass
  exact, semantics-preserving, selected by O1+

ApplyQualityContextBudgetPolicyPass
  semantics-changing, selected only when quality_context_token_budget is set
```

The policy pass runs after the selected semantics-preserving optimization
groups. Reports and structured output must distinguish `optimization` from
`policy_transform`.

## 5. Public Configuration Contract

### 5.1 Frontend API

The canonical API type is a closed enum, not an integer stored throughout the
compiler:

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OptimizationLevel {
    O0,
    O1,
    #[default]
    O2,
    O3,
}

impl OptimizationLevel {
    pub const fn number(self) -> u8;
    pub const fn enables(self, minimum: Self) -> bool;
}

pub struct ProjectCompileOptions {
    pub optimization_level: OptimizationLevel,
    pub quality_context_token_budget: Option<usize>,
    // existing options...
}
```

Only CLI parsing converts text or an integer into the enum. Pipeline and pass
code use the enum and `enables`; they must not compare unvalidated raw numbers.

`CheckedProject::compile_options` retains the effective level. Optimization
reports also record it so an emitted checked program is self-describing.

### 5.2 CLI Contract

Compilation-producing commands accept:

```text
-O0
-O1
-O2
-O3
--opt-level 0
--opt-level 1
--opt-level 2
--opt-level 3
```

The short and long spellings are aliases for one argument. Repeating the option
with conflicting values is an error. Values outside `0..=3` are CLI usage
errors. `-O` without a value is an error.

The option applies to `check`, `run`, `dump hir`, and future `dump air` or build
commands because all of them consume compiler output. `dump ast` does not run
HIR optimization. `resume` consumes the optimization level embedded in its
prepared checked artifact and must not silently recompile it at a different
level.

`etas_cli` owns spelling and help text. `etas_driver` maps the parsed argument
to the frontend compile option. Neither layer owns pass selection.

The default is `-O2`. `-O3` remains opt-in until cross-repository benchmarks
show that its runtime improvement justifies its compile-time and memory cost.

### 5.3 Compatibility With `optimize_hir`

There must be one effective source of truth. Do not keep both public fields in
`ProjectCompileOptions`.

Migration mapping:

| Legacy value | New value |
|---|---|
| `optimize_hir = false` | `optimization_level = O0` |
| `optimize_hir = true` | `optimization_level = O2` |

If a compatibility helper is required for one release, expose a deprecated
builder such as `with_hir_optimization(bool)` that performs this mapping. It
must not add a second stored field. Serialized legacy inputs, if any, use the
same mapping during versioned decoding.

## 6. Optimization Levels And Pass Sets

### 6.1 Level Intent

| Level | Intent | Compile-time budget | Runtime/code-size tradeoff |
|---|---|---|---|
| `O0` | Debugging and unoptimized baseline | Minimum | No optional optimization |
| `O1` | Cheap, local, low-risk simplification | Low | Avoid unnecessary copies, round trips, and proven-dead local work |
| `O2` | Balanced production default | Medium | Add local CSE, hoisting, scan fusion, and batching |
| `O3` | Maximum proven runtime optimization | High | Permit broader scheduling and resource-region changes; code size and compile time may increase |

### 6.2 Pass Assignment

`AgentEffectLegalityPass` is a prerequisite analysis, not a user-visible
optimization. It runs once when at least one selected optimization or explicit
policy transform requires its facts.

| Group | Pass | Minimum level | Rationale |
|---|---|---:|---|
| prerequisite | `AgentEffectLegalityPass` | `O1` | Materializes legality and version dependencies shared by transforms |
| O1 local | `LowerOwnedSnapshotCowPass` | `O1` | Removes a proven redundant immutable copy |
| O1 local | `FuseSerializationBoundaryPass` | `O1` | Removes an exact typed JSON round trip |
| O1 local | `SuffixDemandHistoryPass` | `O1` | Replaces a complete static last-value scan exactly |
| O1 local | `PruneUnusedQualityContextPass` | `O1` | Removes an unconsumed pure context boundary; exact half of the current policy pass |
| O2 balanced | `PrepareVersionedRequestPass` | `O2` | Hoists proven loop-invariant request preparation |
| O2 balanced | `VersionedPreparationCsePass` | `O2` | Reuses structurally equal immutable preparation |
| O2 balanced | `FuseProjectionScansPass` | `O2` | Combines compatible bounded scans |
| O2 balanced | `BatchStorageSemijoinPass` | `O2` | Batches independent membership probes |
| O2 balanced | `SynthesizeScopedIndexPass` | `O2` | Resolves a complete bounded static membership scan |
| O3 aggressive | `ScheduleBoundedPipelinePass` | `O3` | Changes scheduling of proven independent bounded stages |
| O3 aggressive | `LowerResourceRegionPass` | `O3` | Expands a proven reusable resource lifetime and therefore needs the strongest resource checks |
| explicit policy | `ApplyQualityContextBudgetPolicyPass` | none | Runs only when the independent context-budget option is present |

The mapping is deliberately conservative. A future change to a minimum level
is an architecture and release-note change, not an incidental refactor.

### 6.3 Monotonicity

The effective pass sets are:

```text
O0 = {}
O1 = {legality, O1 local}
O2 = O1 + {O2 balanced}
O3 = O2 + {O3 aggressive}
```

An explicit policy transform is appended independently:

```text
effective schedule = optimization(level) + explicit_policy(options)
```

No pass is enabled at a lower level and then removed at a higher level. If two
passes become mutually exclusive, they must be represented as two strategies
behind one scheduled pass or the monotonic contract must be revised explicitly.

## 7. Pipeline And PassManager Design

### 7.1 Schedule Construction

Optimization level selection happens while constructing the Pipeline, not
inside each transform's `run` method.

```rust
pub struct OptimizationPlan {
    pub level: OptimizationLevel,
    pub quality_context_token_budget: Option<usize>,
}

fn optimization_pipeline(plan: &OptimizationPlan) -> Pipeline<ProjectContext> {
    let needs_legality = plan.level.enables(OptimizationLevel::O1)
        || plan.quality_context_token_budget.is_some();

    let mut pipeline = Pipeline::new("frontend.hir_optimization");

    if needs_legality {
        pipeline.push_pass(AgentEffectLegalityPass);
    }
    if plan.level.enables(OptimizationLevel::O1) {
        pipeline.push_group(o1_pipeline());
    }
    if plan.level.enables(OptimizationLevel::O2) {
        pipeline.push_group(o2_pipeline());
    }
    if plan.level.enables(OptimizationLevel::O3) {
        pipeline.push_group(o3_pipeline());
    }
    if plan.quality_context_token_budget.is_some() {
        pipeline.push_pass(ApplyQualityContextBudgetPolicyPass);
    }

    pipeline
}
```

The main semantic pipeline inserts exactly one nested group:

```rust
.pass(AnalyzeLoopProgressPass)
.group(optimization_pipeline(plan))
.pass(VerifyInterpreterSupportPass)
```

`project_pipeline` and `candidate_body_pipeline` receive the same immutable
`OptimizationPlan`. Candidate compilation inherits the level and policy inputs
from its frozen parent checked program unless the public candidate API defines
an explicit, fingerprinted override.

### 7.2 Why In-Pass Gating Is Forbidden

Transforms must not all be scheduled and then execute this pattern:

```rust
if context.input.options.optimization_level >= required_level {
    // rewrite
}
```

Pipeline construction is the scheduling authority. This ensures:

- O0 has no optimization pass records or timing overhead;
- PassManager schedule text shows the real effective schedule;
- instrumentation and changed statistics remain truthful;
- a disabled pass cannot accidentally publish artifacts;
- missing pass dependencies fail during schedule execution instead of being
  hidden by local conditionals.

`PipelineConfig` pass filters remain a debugging and pass-bisection mechanism.
They may disable a pass already selected by the level, but they do not promote
a pass above the selected level in production.

### 7.3 Pass Result Contract

Every optimization pass continues to implement the ordinary
`Pass<ProjectContext>` contract:

- declare all facts read in `requires`;
- declare every artifact changed or created in `produces`;
- declare preserved artifacts accurately;
- return `PassResult::unchanged()` when no IR or semantic fact changed;
- return `PassResult::changed(...)` only when a rewrite or fact mutation
  occurred;
- never count optimization-report bookkeeping as an IR change;
- use the manager-provided current unit and execution controls.

Passes that reconcile replaced expressions currently read and mutate HIR, type
facts, and effect facts. Their descriptors therefore require at least
`HIR_OUTPUT`, `TYPE_OUTPUT`, `EFFECT_OUTPUT`, and
`HIR_OPTIMIZATION_LEGALITY`, and produce the updated artifacts they own. Manual
fact cleanup does not replace an honest PassManager artifact contract.

### 7.4 Unit Granularity

HIR-local passes operate on `Body` or a narrower semantic unit through
`Pipeline::for_each` and the existing `UnitTree`. They must not scan the entire
HIR block or expression arena to rediscover bodies.

The target schedule shape is:

```text
frontend.hir_optimization
  AgentEffectLegalityPass
  ForEach(Affected Body, stable order)
    frontend.hir_optimization.o1
      ...O1 body passes...
    frontend.hir_optimization.o2
      ...O2 body passes...
    frontend.hir_optimization.o3
      ...O3 body passes...
  ApplyQualityContextBudgetPolicyPass [only when explicitly configured]
```

A genuinely project-wide transform may remain global, but its descriptor and
design notes must explain the cross-body invariant that requires global scope.

Pass reports aggregate per-unit results deterministically by pass name and
stable unit order.

## 8. Reporting And Observability

`HirOptimizationReport` records:

```rust
pub struct HirOptimizationReport {
    pub requested_level: OptimizationLevel,
    pub effective_level: OptimizationLevel,
    pub passes: Vec<HirOptimizationPassReport>,
    pub policy_transforms: Vec<HirPolicyTransformReport>,
}
```

For every scheduled optimization pass, report:

- canonical pass name;
- optimization group (`O1`, `O2`, or `O3`);
- matched and rewritten counts;
- affected unit count;
- whether the pass changed HIR or semantic facts.

Pass timing, stop/failure control, skipped state, and generic artifact
invalidation remain sourced from `PassManager::PassRunRecord`; do not duplicate
those mechanisms in the optimization report.

At O0 the optimization pass list is empty. An explicitly configured policy
transform may still appear in `policy_transforms`, never in the O-level pass
list.

Text and JSON command output expose the effective level when compiler metadata
is requested. Diagnostics do not change wording solely because a different
optimization level was selected.

## 9. Fingerprints, Cache, And Incremental Compilation

The canonical compile-options fingerprint is versioned and includes:

```text
optimization-level:o0|o1|o2|o3
quality-context-policy:off|token-budget:<n>
```

The migration increments the existing options fingerprint version. Legacy
`hir-optimization-on/off` strings must not remain in the new canonical form.

Rules:

- Checked HIR and downstream artifacts produced at different levels are not
  interchangeable.
- An explicit policy input is a separate fingerprint component.
- Changing only the optimization level invalidates the optimization boundary
  and downstream checked artifacts.
- Source, parse, module, signature, type, effect, and other pre-optimization
  artifacts may be reused when their own fingerprints remain valid.
- Incremental sessions treat a level change as a pipeline-plan change even when
  no source file changed.
- Cache diagnostics and debug output include the effective level when reporting
  a hit or miss for an optimized artifact.

The initial implementation may conservatively invalidate more artifacts, but
it must never reuse checked HIR from another level.

## 10. Repository Ownership

The CLI contract is repository-neutral. Today the implemented passes are
typed-HIR-local and run in the frontend project pipeline. The long-term
architecture still assigns FIR, whole-flow, prompt/context, and AIR-oriented
optimization to `etas-optimizing`.

When that repository becomes the owner, it consumes the same effective
`OptimizationLevel` from the driver and builds its own nested Pipeline with the
same PassManager abstraction. Moving a pass between repositories must preserve
its minimum-level contract or be called out as a compatibility change.

No frontend-to-`etas-optimizing` dependency is introduced. The allowed
direction remains optimizer consuming checked frontend output.

## 11. Validation And Acceptance

### 11.1 Schedule Tests

For every level, assert the exact PassManager schedule and execution records:

- O0 contains no optimization prerequisite or transform.
- O1 contains only legality and O1 passes.
- O2 contains the complete O1 set followed by the O2 set.
- O3 contains the complete O2 set followed by the O3 set.
- policy transforms appear only when their explicit option is present.
- every schedule remains deterministic across repeated construction.

### 11.2 Semantic Tests

For all semantics-preserving fixtures:

- compile and execute at O0, O1, O2, and O3;
- assert equal values, errors, effects, traces, and externally visible order;
- assert HIR tree ownership after every transforming pass;
- assert no stale method, intrinsic, reflection, index, generic, or effect fact
  survives a replacement;
- assert a missing proof fact prevents the rewrite.

Policy-changing context-budget fixtures are tested separately. They assert the
configured policy result and must not be used as cross-level semantic
equivalence evidence.

### 11.3 CLI And Cache Tests

- Parse all short and long flag forms.
- Reject missing, conflicting, and out-of-range values.
- Verify the default is O2.
- Verify driver propagation into `CheckedProject::compile_options`.
- Verify fingerprints differ across all levels and policy inputs.
- Verify changing level cannot hit a downstream checked-HIR cache entry from
  another level.
- Verify pre-optimization artifacts can still be reused where supported.

### 11.4 Performance Tests

Record, but do not make correctness depend on:

- compile time by level;
- optimizer peak memory by level;
- interpreter steps, allocations, and elapsed time;
- checked-HIR size and optimized-out node count.

O3 remains opt-in if it regresses representative runtime workloads or creates
unbounded compile-time growth.

## 12. Engineering Work Items

The work should be delivered in dependency order.

### OPTLEVEL-01: Public option model

- Add public `OptimizationLevel` with O0 through O3, ordering, numeric
  conversion, display, parsing support, and O2 default.
- Replace `ProjectCompileOptions::optimize_hir` with `optimization_level`.
- Add the temporary deprecated bool builder only if compatibility requires it.
- Store the effective level in checked output and optimization reports.
- Unit-test enum ordering, default, and legacy mapping.

Acceptance: no production code has two independent optimization controls.

### OPTLEVEL-02: Fingerprint and incremental boundary

- Bump the compile-options fingerprint version.
- Encode optimization level and context policy as separate canonical parts.
- Detect a level change in a live session and invalidate optimization/downstream
  artifacts.
- Add cross-level cache isolation and pre-optimization reuse tests.

Acceptance: no checked HIR or downstream artifact is reused across levels.

### OPTLEVEL-03: Pipeline plan construction

- Add immutable `OptimizationPlan`.
- Extract `optimization_pipeline`, `o1_pipeline`, `o2_pipeline`, and
  `o3_pipeline` builders.
- Insert the nested optimization group at the existing semantic-pipeline
  boundary.
- Pass the same plan into project and candidate-body pipelines.
- Remove `optimize_hir` and level checks from individual pass bodies.
- Add exact schedule-text and PassRunRecord tests for all four levels.

Acceptance: PassManager records contain exactly the passes selected by the
effective level.

### OPTLEVEL-04: Split semantics-preserving optimization from policy

- Split `QualityAwareContextPolicyPass` into exact pruning and explicit budget
  policy passes.
- Assign exact pruning to O1.
- Schedule the budget pass only from `quality_context_token_budget`.
- Split optimization and policy reporting.

Acceptance: O3 without a policy option cannot truncate or approximate context.

### OPTLEVEL-05: Pass contracts and truthful results

- Audit every optimization descriptor's required, produced, and preserved
  artifacts.
- Include type/effect artifacts for passes that inspect or reconcile them.
- Return unchanged for zero-rewrite runs.
- Ensure report bookkeeping does not mark HIR changed.
- Add missing-proof and stale-fact regression tests.

Acceptance: manager artifact records describe every semantic fact mutation and
no no-op transform is counted as changed.

### OPTLEVEL-06: Unit-scoped and incremental optimization

- Convert HIR-local passes from whole-arena scans to Body/unit-scoped passes.
- Use stable UnitTree order and affected-unit selection.
- Aggregate per-unit reports deterministically.
- Document and test any pass that must remain project-global.

Acceptance: a one-body incremental edit does not scan or rewrite unrelated
bodies for a body-local pass.

### OPTLEVEL-07: CLI and driver propagation

- Add the shared CLI argument model for `-O0` through `-O3` and
  `--opt-level`.
- Apply it to check, run, HIR dump, and future AIR/build paths.
- Propagate the parsed level through `etas_driver` into the frontend input.
- Reject invalid/conflicting values and document O2 as the default.
- Include the effective level in requested structured compiler metadata.

Acceptance: a CLI integration test observes the requested level in the checked
artifact and its pass schedule.

### OPTLEVEL-08: Cross-level correctness suite

- Run every existing optimization fixture at O0, O1, O2, and O3.
- Compare interpreter values, errors, traces, effects, and order.
- Keep context-budget policy fixtures in a separate suite.
- Add tree ownership and stale-fact checks after each transforming pass.

Acceptance: all semantics-preserving fixtures are observationally equivalent
across levels.

### OPTLEVEL-09: Benchmarks and release documentation

- Extend frontend and interpreter benchmarks with an optimization-level axis.
- Report compile time, memory, interpreter steps, allocations, elapsed time,
  and HIR size.
- Add CLI help, migration notes, and release notes for the bool-to-level change.
- Keep O3 opt-in and record the evidence required before changing the default.

Acceptance: benchmark output identifies level and separates optimization gains
from explicit policy changes.

## 13. Definition Of Done

The optimization-level feature is complete when:

- one typed option flows from CLI through driver to compiler;
- O0 through O3 construct distinct nested Pipelines executed by the existing
  PassManager;
- pass membership is monotonic, deterministic, and covered by schedule tests;
- O-level passes are semantics-preserving and explicit policies remain
  separate;
- artifacts, changed results, fingerprints, and incremental invalidation are
  truthful;
- public checked output records the effective level;
- cross-level semantic and cache-isolation tests pass;
- no new optimizer runner or frontend-to-optimizer dependency exists.
