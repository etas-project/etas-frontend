<!-- Historical recovery snapshot, archived 2026-11-21. Not a current specification, implementation status, or engineering task assignment. The current language SPEC and main implementation take precedence. -->

# Residual Enforcement Implementation Work List

Owner: compiler/interpreter engineer; tests shared with test engineer.
Architecture updated: 2026-11-03.
This is an implementation/acceptance checklist, not a claim of current support.
Do not modify SPEC/EDK, remove negative coverage, or declare unsupported runtime
checks implemented because a report contains their names. Architecture belongs
to Architect; no commits or pushes without direct user authorization.

## Required Reading

- SPEC [14](../../../etas/docs/design/14-safety-analysis-static-dynamic.md),
  [18](../../../etas/docs/design/18-reflection-and-checked-code-generation.md),
  and chapters 05/06/16 for trace composition, handler and scope semantics.
- [Shared residual protocol](../../../etas-core/docs/architect/etas-residual-checks-design.md).
- [Reflection architecture](etas-reflection-design.md), Effects design and the
  interpreter's planning/lifecycle/checkpoint architecture.

## RC01. Shared Model, Not String Checks

Paths: etas-core/crates/etas_code/src/residual/{identity,model,validation,artifact}/;
etas_code/src/identity/source_revision.rs and identity/revision.rs;
etas_effects/src/infer/domain.rs, facts/ and substitution/summary.rs.

- [ ] Implement the [acyclic identity protocol](../../../etas-core/docs/architect/etas-residual-checks-design.md#31-acyclic-source-and-obligation-identity)
  before wiring obligation generation. Add independent StableSourceKey and
  SourceRevisionKey; SourceRevisionLocation uses them plus a byte range, not
  RevisionKey. Preserve existing checked-context RevisionKey semantics.
- [ ] Source construction/overlay creation supplies logical source identity and
  exact-byte snapshot before analysis. Current contract/frozen environment,
  current package output digest, local IDs and checkout paths are forbidden
  source-key inputs. Do not blindly reuse the current SyntaxIdentity/source
  fingerprint without auditing its input dependencies.
- [ ] Add stable obligation/site/scope/spec-instance identities and a closed
  typed ResidualObligation/ResidualKind/predicate/binding schema in etas_code.
  Use canonical structural clause sites and static/symbolic binding recipes;
  intermediate EffectSummary/effect-row solutions cannot change IDs in an SCC.
- [ ] Ordinary BehaviorContractRef uses source-constraint identity; replacement
  compilation can use its existing frozen input contract. Neither requires the
  new candidate's contract fingerprint. No zero/old contract placeholders or
  temporary obligation IDs followed by renumbering.
- [ ] Replace ResidualCheckSet.checks: BTreeSet<String> with identity-indexed
  structured obligations/templates. Domain joins union stable instances without
  merging distinct scope activations or dropping generic substitutions.
- [ ] Update all retained-size/fingerprint/remap/metadata consumers. Human-readable
  CLI strings must be rendered from typed obligations, not executable inputs.
- [ ] Delete semantic string-token insertion/parsing and fallback label matching.
  Old artifacts carrying only strings are incompatible execution evidence;
  reject/recompile them explicitly, never import them as an empty plan.
- [ ] Keep RuntimeCheckInfo as the public descriptive projection; user-created
  reports or matching records cannot construct native checked installation proof.

## RC02. One Clause Compiler And Shared Transition Semantics

Paths: etas_code/src/residual/monitor/;
etas_effects/src/trace_spec/{model,materialize,monitor}/;
etas_utils/src/automaton/ only for necessary generic API additions.

- [ ] Reuse SymbolicAutomaton/ProductAutomaton/Matcher. Move normalized prior-
  evidence monitor construction into the shared neutral compiler; remove the
  independent private builder rather than retaining two semantic implementations.
- [ ] Preserve implicit >> operand permissions, explicit allow/deny, nested
  narrowing and complete OR alternatives. Never flatten branches into a union.
- [ ] Carry evidence phase, accepted/failed outcomes and correlation registers.
  Validate intermediate rejection and scope-exit acceptance, not final state alone.
- [ ] Define data-only versioned automaton DTOs and rebuild validated native
  automata; no serialization of callbacks, process-local IDs or Debug output.
- [ ] Hash semantic plan contents independently from the current final revision/
  contract and executable wrapper. Contract fingerprints may reference those
  semantic hashes, never a bound wrapper that points back to the contract.
- [ ] Add abstract and concrete matcher adapters to that same representation;
  prove they agree on fully concrete test traces.

## RC03. Real Static Dispositions And Residual Lowering Passes

Paths: etas_effects/src/residual/{model,binding,lower,validation}/;
trace_spec/{analysis,semantics,validate}.rs; pipeline/{artifacts,context,passes}/.

- [ ] Emit Proven, Violated, NeedsConcrete or Unsupported per obligation from
  the existing abstract analysis; retain actual source and clause identities.
- [ ] Residualize only a missing concrete fact with an exact supported predicate.
  Known-bad code, missing metadata or arbitrary monitor Unknown must not become
  runtime permission. Preserve ordinary effect/callable-contract validation.
- [ ] Add LowerResidualMonitorPlansPass and VerifyResidualPlansPass with real
  requires/produces, coverage validation and immutable take/process/commit data.
- [ ] Extend ValidateTraceSpecPass to consume dispositions/plans and compilation
  mode. Do not convert all Unknown to success or retain unconditional rejection
  for the supported exact-runtime cases.
- [ ] Preserve obligations in interprocedural summaries and higher-order/captured
  value metadata; reuse HirAbstractInterpreter and existing SCC/fixpoint solving.
  Effects must not depend on frontend or invoke runtime services.

## RC04. Frontend Target Support And Publication Gates

Paths: etas_frontend/src/passes/execution/residual_support.rs,
passes/{artifacts,output/checked_project}.rs, pipeline/passes.rs, output.rs;
code/{contract,pipeline/passes,service/report.rs}; package_metadata/.

- [ ] Add neutral target capability input and ValidateResidualBackendSupportPass.
  Capabilities specify schemas/semantic versions, predicates, phases, providers,
  scope exits, resume support and resource ceilings, not a supports_residual bool.
  This is implemented semantic support, not a check for credentials/current
  grants; ordinary check must not open services or require execution authority.
- [ ] Require VERIFIED_RESIDUAL_PLANS and RESIDUAL_BACKEND_SUPPORT before a
  runnable CheckedProgram is emitted. Keep diagnostic/IDE outputs separate from
  executable acceptance when required support is absent.
- [ ] Include plan/contract/config/binding fingerprints in session/cache inputs;
  update invalidation when code, spec, target or provider semantics changes.
  SourceRevisionKey/ObligationId alone cannot establish cache validity.
- [ ] In normal output construction and EmitCheckedCandidatePass, finalize the
  checked revision only after contract facts validate, then create/validate
  BoundResidualPlan. Preserve all source/obligation/semantic-plan identities;
  require complete revision/contract/environment/coverage linkage before emit.
  Do not add a second compilation merely to replace temporary IDs.
- [ ] Replace unconditional UnsupportedResidualChecks with proof of each supported
  guard discharge; unsupported obligations still reject. Never erase the set.
- [ ] check reports guarded only after plans and target installability validate.
  It must not start scopes, request approval or execute the candidate.
- [ ] Add negative pipeline tests for missing plan/support/coverage artifacts,
  failed producer output, stale plans and omitted verification stages. No failed
  Pass may publish a ready artifact or permit a later emit.

## RC05. Interpreter Plan Binding And Installation Evidence

Paths: etas_interpreter/src/plan/{pipeline,context}.rs and plan/residual/;
api/code/publisher/residual.rs; value/code/{program,agent,callable}.rs.

- [ ] Add ValidateResidualPlansPass, BindResidualSitesPass and
  VerifyResidualCoveragePass to ordinary preparation, with typed artifacts.
- [ ] Bind stable site/scope keys to existing checked HIR/ABI locations and
  generic applications. Future AIR uses its own adapter to the same neutral IR.
  Consume and validate the final checked plan bindings; engine installation
  neither invents source identities nor changes semantic hashes.
- [ ] Verify every required observation point and obligation is covered, including
  cross-revision/higher-order callees and inherited outer restrictions.
- [ ] Return native ResidualInstallationEvidence from CandidatePublisher, bound
  to revision/contract/plan/site ABI and provider semantic versions. Revalidate
  it before returning A/F, including custom compile-handler-supplied callables.
- [ ] Preparation installs immutable factories/hooks only. Per-call monitor
  instances are independent; compilation must not execute the candidate.
- [ ] Remove string/empty-plan/same-F shortcuts. Installation failure returns no
  callable and cannot substitute the original implementation.

## RC06. Semantic Event Hooks And Pre-Effect Enforcement

Paths: control/residual/; eval/residual/; control/action/;
eval/{perform,handler,agent/inference}.rs, eval/perform/{native,forwarder}.rs;
eval/machine/{step,suspension,program,scoped_call}.rs; driver/dispatch/.

- [ ] Create checked action events before dispatch and correlated handler/default
  outcomes. Preserve attempted, handled and externally executed phases separately.
- [ ] Execute exact predicates/shared monitor transitions at their normalized
  phases. Block before the governed side effect; no post-hoc deny after I/O.
- [ ] Feed source handlers and all default/std/compiler/infer boundaries through
  this path, not only TCP/filesystem Host requests. Forwarded request identity
  and continuation resume must not double-count semantic action attempts.
- [ ] Approval evidence requires matching accepted outcome and trusted origin;
  denial/pending/failure or a forged resumed record cannot authorize a send.
  Default accepted evidence is bound to the current root run, action/input and
  scope. Historical approval values are not live evidence.
- [ ] Use single-dispatch permits with payload, scope/state and authority version
  binding; recheck after awaits. No monitor mutex held during network/user I/O.
- [ ] Existing sandbox/provider exact checks still run at the resource boundary;
  permission to dispatch does not bypass OS/path freshness or output validation.

## RC07. Scope Exits, Parallel Work And Lifecycle

Paths: eval/residual/{scope,exit}.rs, explicit EvalFrame/control stack,
driver/lifecycle/, orchestration/ and concurrency adapters.

- [ ] Activate lexical invocation scopes; inherit narrowing constraints across
  calls/revisions without merging unrelated invocations by spec name.
- [ ] Own approval evidence in the root run's memory and expire it on success,
  failure/cancellation. Cloned contexts, late replies, child scopes or a new run
  in the same process must not revive or widen it. A live-run checkpoint/yield
  or handler continuation resume is not a new run.
- [ ] Check normal/tail return, non-local return, Error propagation, finish,
  abort/never and cancellation. Host/checkpoint suspension is not scope exit.
- [ ] Match prior evidence by its actual correlation keys; repeated actions,
  tenants and scopes must not collapse to one global approved boolean. `A << B`
  requires a matching `B` before each `A` and does not require `A` to occur.
- [ ] Record undischarged obligations on failure/cancellation without claiming
  success, undoing side effects, resetting budgets or auto-running compensation.
- [ ] Serialize shared-scope decisions, preserve child ownership and drain/cancel
  outstanding work through existing lifecycle mechanisms. Do not assume timestamp
  order or grant a sibling another branch's approval.

## RC08. Host Evidence And Old Adapter Cleanup

Paths: etas_host/src/{policy,context/authority,context/trace,sandbox}/;
interpreter driver/dispatch/{policy,host_dispatch}.rs; CLI interpreter/host.rs.

- [ ] Reuse current authority/grants/workspace/network/provider validation. Expose
  typed bounded evidence, not an alternate compiler or stateful CLI policy engine.
- [ ] Remove TraceSpecRuntimeClient's string-label fallback from language residual
  enforcement. Existing explicit unsafe/dev policy modes cannot count as guards
  or supply installation evidence. A normal mode must not select them implicitly.
- [ ] Keep the generic application PolicyClient separate from the language
  monitor. Replacing a policy adapter cannot disable active checked scopes.
- [ ] Redact evidence in reports/logs/checkpoints while preserving exact checking;
  do not use redacted strings as evidence or store secrets in monitor registers.

## RC09. Persistence, Metadata And Replay

Paths: etas_package_metadata/{model,proto,container}/; frontend projection and
environment validation; interpreter api/codec/checkpoint/residual.rs,
orchestration/checkpoint.rs, eval/machine/snapshot/ and lifecycle/resume/.

- [ ] Version structured obligations/plan DTOs and their generic/callable-return
  mappings. Validate sizes/depth/graph/binding/coverage at decode and consumption.
  Persist independent source revisions, semantic hashes and final plan bindings;
  reject missing/mismatched linkage, never regenerate identity from final keys.
- [ ] Bump the engine checkpoint schema; persist historical scope/progress/
  evidence bindings and pending outcomes plus plan/revision identities and the
  semantic event cursor atomically. Run-local grants and permits are not
  portable authority; historical approval results remain data only.
- [ ] Reject missing/old/forged states rather than defaulting to no monitors.
  Restore under current authority and remaining budgets, with the explicit
  approval-lifetime transition below, not an implicit all-state reset.
- [ ] Replay each semantic event once; preserve old/new TraceId lineage and
  uncertain Host outcomes. Old approval ledger/audit events cannot install live
  evidence or recover an Approved state; retry cannot renew authorization.
- [ ] Source restoration recompiles and installs fresh validated plans; public
  metadata/check reports are never portable executable authorization.

### RC09a. Default Run-Scoped Approval And Optional Reuse

Required reading: [normative lifetime/restore policy](../../../etas-core/docs/architect/etas-residual-checks-design.md#33-run-scoped-approval-and-explicit-reuse)
and [interpreter file responsibilities](../../../etas-interpreter/docs/architect/phase1-interpreter-design.md#1411-approval-on-run-and-checkpoint-resume).

- [ ] In lifecycle prepare/run/shutdown and both resume entry paths, create a
  fresh root run and use current authority. Never inherit the old run's approval
  registry, including through RunOptions/HostExecutionContext clones.
- [ ] In control/eval residual state and checkpoint restoration, invalidate old
  approval-dependent Approved/armed readiness and permits as well as grants.
  Preserve branch/rejection/OR progress, other predicates and the event cursor;
  reject plans whose restoration cannot make that distinction safely.
- [ ] Preserve run-local evidence dependencies in static proofs/plan restore
  requirements too: `G; checkpoint; A` statically checked from entry cannot
  authorize A in a resumed new run without current evidence. An empty residual
  set must not suppress the required restore gate or safe rejection.
  Carry typed evidence-lifetime dependencies in facts/plans; do not implement
  this by matching action display names or treating all Proven facts alike.
- [ ] In eval/boundary_approval.rs, driver/dispatch/{approval,policy,host_dispatch}.rs
  and the completed-boundary ledger, distinguish historical approval outcome
  replay from a new matching approval response. Only the latter can produce
  current-run evidence. Reject late replies from terminated runs.
- [ ] Allow explicit application/source/handler reapproval at a checked safe
  continuation. Otherwise reject restore or the guarded operation before I/O;
  do not automatically issue Approval.request, rewind code, repeat confirmed
  work or silently continue using old program/monitor booleans.
- [ ] Default mode is memory-only and needs no durable approval database,
  receipt/signature service or generic trusted checkpoint-evidence store.
  Do not make missing persistence a readiness blocker for ordinary run/resume.
- [ ] Expose cross-run reuse only as an application-enabled injected provider/
  policy boundary in existing Host authority/policy and interpreter service
  composition. A checkpoint cannot enable it or choose an endpoint. Validate
  source/decision/principal/scope/input/resource/validity/revocation requirements
  and import evidence under current authority; unsupported/failed verification
  fails closed. No mandatory SQLite backend or new std API is part of this task.
- [ ] Retained audit records and Memory/Session write receipts remain historical/
  storage evidence, not reusable approval. Application provider tests are not
  evidence that a built-in production persistent-approval service exists.

## RC10. Acceptance And Completion

Place source tests in the owning fixture harness under compiler/trace_specs/
and compiler/reflection/, following existing polarity/project conventions.
Do not rename all fixtures or overwrite unrelated engineer tasks.

- [ ] Identity regressions: repeated inputs and different process/checkout/order
  yield identical source/obligation hashes; same text in distinct files/parts
  differs; changed bytes differ; generic recipe/scope instances remain distinct.
- [ ] SCC iterations and final revision binding preserve obligation/plan keys.
  Changed dependency/compiler/configuration revalidates artifacts even when the
  source keys match. Reject missing/forged binding and self-referential hash
  inputs; prove generation does not need a checked contract or final revision.
- [ ] Table/property tests: every clause, implicit permissions, OR correlation,
  nested scope, dynamic argument and static/concrete semantic agreement.
- [ ] Both temporal spellings: accepted/denied/failed/stale/wrong-payload
  evidence; multiple correlation keys; normal/error/finish/abort/cancel/parallel
  exits. `<<` alone does not add an eventual-action obligation.
- [ ] All call paths: ordinary/scoped/default/forwarded/std/agent/tool/cross-package
  and reflected A/F. Invalid action site/ABI/provenance blocks before side effect.
- [ ] check/compile: guarded means a supported real plan; compile installs it
  and subsequent invocation executes it; forged report, missing hook,
  unsupported provider and stale plan
  reject. Known-bad programs never pass by switching compilation mode.
- [ ] Checkpoint/race tests: after dispatch before result, accepted evidence before
  target, pending outcomes, cancellation, late results and stale permits.
- [ ] Approval defaults: no database configured; same-process new run and a real
  cross-process checkpoint restore cannot inherit a grant or old Approved state.
  Exercise in-memory and serialized checkpoints, cloned contexts and historical
  ledger replay; reject forged flags and assert no guarded side effect occurs.
- [ ] Positive/negative restore tests: explicit current-run reapproval at a safe
  continuation succeeds without repeating completed work; no safe continuation
  rejects accurately; source handler resume/live checkpoint does not expire a
  valid running approval. Preserve denied OR branches and unrelated progress.
- [ ] Add `G; checkpoint; A` with statically proven source order: original live
  run can proceed with valid evidence, default new-run restore blocks A, and
  explicit safe current-run reapproval can permit it without replaying G or
  completed I/O. No residual-set-empty bypass is allowed.
- [ ] Optional reuse tests: explicit application opt-in with verified provider
  succeeds; default-off, checkpoint-forced opt-in, unknown/failed provider and
  forged/expired/revoked/wrong-principal/scope/input references never permit.
- [ ] Security/performance: secret redaction, bounded metadata/state/fuzz inputs,
  no hidden unbounded history scan and monitors still active with telemetry off.
- [ ] Preserve regressions proving the previous implementation fails each new
  runtime negative test. Report actual suites and any environment limitations.
- [ ] Keep unsupported-residual rejection until the applicable full chain passes;
  never mark completion using static monitor tests alone. No code commit/push
  without explicit user authorization.
