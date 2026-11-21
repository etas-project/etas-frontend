<!-- Historical recovery snapshot, archived 2026-11-21. Not a current specification, implementation status, or engineering task assignment. The current language SPEC and main implementation take precedence. -->

# Reflection And Checked Code Generation Work List

Owner: compiler/interpreter engineer, with language designer and test engineer.
Architecture updated: 2026-11-03.
This is the complete target work list, not evidence that proposed APIs already work.
Unchecked boxes are verification obligations, not a claim that existing code is
absent. Reuse working implementation; remove conflicting paths rather than
adding a parallel Agent-only or flow-only compiler.
Keep ongoing compiler-memory work intact; architecture/document changes belong to
Architect. Do not edit EDK or SPEC to make compiler tests pass.

## Required Reading

Read all of
[language chapter 18](../../../etas/docs/design/18-reflection-and-checked-code-generation.md),
[frontend reflection architecture](etas-reflection-design.md),
[shared code/service architecture](../../../etas-core/docs/architect/etas-code-design.md)
and the reflection sections in syntax/HIR/types/std/effects/frontend/interpreter/
Host/CLI designs. Chapters 03/05/06/09/14/15 of SPEC remain authoritative for
agent identity, type/spec rules, handlers, safety and packages.
Residual work follows the complete
[Residual Enforcement Implementation Work List](residual-checks-implementation-worklist.md),
not a string-only check/report placeholder. Its RC01-RC10 gates apply to normal
programs and reflected A/F publication alike.

## R00. Adopt The Current Source Contract

- [ ] Apply the SPEC matrix: AgentDef<A> -> A and CallableDef<F> -> F for
  editable/compilable definitions; bare CallableDef is read-only inspection.
- [ ] Preserve F's ordinary generic/effect-row typing and frozen behavioral
  contract. Do not require users to wrap flows as agents to revise them.
- [ ] Apply one-way typed-to-erased projection. Reject editing/compiling erased
  values and detaching reflected agent methods, including ^A.run.
- [ ] Implement the now-specified retention, invalidated-summary None values,
  frozen inclusion, report/errors and resource rules instead of waiting on the
  unresolved design gates. Unsupported behavior is an explicit implementation gap.
- [ ] Remove any public std.compiler.load path. Restoration saves source and
  locked environment, then validates/parses/compiles the statically known target.
- [ ] Do not invent DraftAgent/CompiledAgent source wrappers, Code.load/Code.parse
  actions, automatic source export or permission-granting annotations.

## R01. Shared Contracts In etas-core

Paths: crates/etas_code/Cargo.toml and src/identity/, descriptor/, syntax/,
definition/, contract/, diagnostic/, artifact/ as specified in etas-code-design.

- [ ] Keep this one crate in the core workspace and the relevant dependency maps/
  local path patch configurations. No parser/HIR/interpreter dependencies.
- [ ] Implement stable DefinitionKey/MethodId/RevisionKey with package and spec/
  impl ownership; canonical structured fingerprints and explicit versioning.
- [ ] Implement shared descriptor graphs, immutable snapshot/draft references,
  internal Checked/BaselineOnly/Unavailable analysis state and retention leases.
- [ ] Preserve recursive type/row binders, nominal constructors, annotations,
  default invocation and additional-method identity; no string-only effect rows.
- [ ] Carry distinct Agent/ordinary-flow/method origins and static A/F contract
  references. Typed and erased views share descriptors but not source operations;
  raw runtime TypeInfo data must not create typed compilation evidence.
- [ ] Define CodeSyntaxOperations and bounded artifact/report contracts.
  Structured syntax handles refer to real backend AST objects and cannot be
  recovered from arbitrary IDs/Any or user-created descriptor records.
- [ ] Integrate existing retained-size accounting with shared-storage deduplication.
- [ ] Test same spelling in different packages/specs, recursive descriptor graphs,
  malformed references, canonical digest stability and shared clone costs.

## R02. Lexer, AST And Body Fragments

Paths: etas_syntax/src/token.rs, lexer punctuation handling, ast/expr.rs,
ast/reflection.rs, grammar/expr.rs, grammar/reflection.rs,
grammar/body_fragment.rs, parse.rs, dump/ and format/.

- [ ] Add ^ punctuation and a dedicated ReflectExpr node/target grammar.
- [ ] Implement ^A.run versus (^A).body precedence. Reject reflection of calls
  and arbitrary expressions with precise recovery spans.
- [ ] Reuse existing path/member/spec-selection syntax where permitted; do not
  introduce a different :: spelling.
- [ ] Add parse_body_contents(SourceFile) -> Parse<ast::Block>, consuming EOF
  without requiring enclosing braces and without text-wrapping a dummy flow.
- [ ] Retain token/AST/source data in an immutable syntax object. Recovery AST
  with errors must not be returned as successful SyntaxBlock.
- [ ] Implement stable readable body formatting and AST/reflect dump output.
- [ ] Test malformed targets, incomplete editor input, body return/tail/block
  syntax, trailing garbage, Unicode locations and normalized format round trips.

## R03. HIR, Resolution And Tree Integration

Paths: etas_hir/src/hir/reflection.rs, lower/reflection.rs, resolve/reflection.rs,
hir/expr.rs, source_map/, view/, visitor/dump integration.

- [ ] Lower reflection as its own reference-bearing expression; do not translate
  it to a normal invocation, quote(Call), or intrinsic named from source text.
- [ ] Preserve target/default-invocation/impl-method identity and generated
  syntax origin through source maps and tree ownership.
- [ ] Make all relevant walkers/visitors/hash/fingerprint/retained-size routines
  cover the new expression and metadata dependencies.
- [ ] Resolve declaration targets and allowed agent/ordinary-flow bindings through
  ordinary import/visibility/member resolution; preserve spec-selected methods.
- [ ] Establish one original body slot for ^A and ^A.run, including source agents
  lowered into a nominal component/default method. Auxiliary methods stay distinct.
- [ ] Represent the generated default method as a reference to that canonical
  body owner; tree/unit collectors schedule it once. No duplicate block ownership
  or forwarding run method that calls the agent again.
- [ ] Migrate ergonomic agent bodies and impl agent methods to SPEC chapter 03's
  flow-like default/additional invocation model, preserving one source body.
  Remove Prompt-only/one-implicit-model-call assumptions from lowering.
- [ ] Test cross-file imports/re-exports, aliases, duplicate method names and
  inaccessible members; reflecting a definition must not execute its body.

## R04. Type Constraints And Checked Reflection Facts

Paths: etas_types/src/constraint/reflection.rs,
pipeline/body/collect/reflection.rs, solver/reflection.rs,
facts/reflection.rs and existing finalize/remap/metadata integration.

- [ ] Collect target/kind/visibility/generic constraints; solve them in the
  existing type pipeline, then materialize CheckedReflectionFact.
- [ ] Infer AgentDef<A> from declaration or dynamic agent value identity.
  with_body preserves A; compile returns A with A inferred from its argument.
- [ ] Infer CallableDef<F> from ordinary flow declarations and supported value
  origins. with_body preserves F; check/compile overloads infer F from input,
  not the expected return type or a signature string. Reject non-callable F.
- [ ] Implement compiler-known CallableDef<F> -> CallableDef projection only at
  a checked expected erased view, including heterogeneous arrays. No reverse
  conversion, general omitted-generic erasure, field-shape or Any fallback.
- [ ] Direct method reflection and method enumeration remain erased; preserve
  default/spec-selected identity without allowing method compilation/detachment.
- [ ] Check SyntaxBlock input to with_body. Reject string substitution, raw
  descriptor-as-type, descriptor-as-spec-proof and wrong nominal A.
- [ ] Bare AgentDef.signature/body are None. Callable descriptors have a
  signature. Reject compiling a bare-agent body without its entry contract;
  do not substitute unit -> unit or synthesize a callable signature.
- [ ] Check agent method result O normally; only explicit infer operands require
  Prompt and output-schema checking. Infer effects/determinism from the actual
  method, not the agent keyword or an obsolete universal Agent[I,O,E,C,G] tuple.
- [ ] Include new facts in body deltas, TypeId relocation and package projection;
  avoid cloning the global signature/type universe per reflected expression.
- [ ] Test explicit/inferred binders, open effect rows, aliases, generics and
  false type/spec evidence built from ordinary descriptive values.

## R05. Definition Catalog And Retained Syntax

Paths: etas_frontend/src/code/identity/, definition/, descriptor/.

- [ ] Build a stable catalog from checked source/HIR/type/effect/annotation facts.
  Project runtime-facing DTOs; do not create another checker.
- [ ] Implement SignatureInfo, ParameterInfo, TypeInfo graph, AgentDef primary
  fields, erased CallableDef method list, ToolRef, SpecInfo and DependencyInfo.
- [ ] Project ordinary-flow typed and erased descriptors with revision and
  dependency fields; ^current selects the pinned current flow implementation.
- [ ] Project declared bound/escaping/requested/default-handled independently.
  Reuse existing temporal summaries and preserve unavailable-analysis states.
- [ ] Freeze definition lexical imports/generic binders/locked package/config
  identities. Store references, not live locals, credentials or connections.
- [ ] Validate owning-manifest retain_bodies/export_bodies selections; export
  requires a public owned item and implies retention. Both default to empty;
  runtime revisions inherit the target policy. Apply equally to agents/flows.
- [ ] Implement explicit body availability, with None for hidden/
  unavailable bodies. No implicit disk read or source download in ^.
- [ ] Share one immutable revision graph among reflected objects; integrate
  memory budgets and lease lifetimes without double-counting Arc backing.

## R06. Pure Code Operations And Draft Overlays

Paths: etas_frontend/src/code/service/syntax.rs, definition/objects/{edit,view}.rs,
draft/{overlay,revision}.rs; interpreter eval/code/ and value/code/.

- [ ] Implement pure parse/format/with_body/diff through CodeSyntaxOperations.
  They run in memory and introduce no compiler-service requested action.
- [ ] Register checked intrinsic dispatch; missing syntax backend is a readiness/
  operational error, not ParseError for valid text or a successful empty block.
- [ ] with_body creates a new AgentDef<A> or CallableDef<F> snapshot sharing
  authoritative target identity, signature, contract and permitted environment.
- [ ] Assign a fresh draft/content fingerprint; retain the base revision as a
  parent/reference, never as the new body's cache identity.
- [ ] Preserve declared_bound; set edited escaping/requested/default_handled,
  determinism and trace to None and invalidate dependent auxiliary summaries.
  Baseline data is internal contract evidence, never current edited analysis.
- [ ] Apply a structured AST overlay to the canonical agent-default/free-flow slot.
  No generated program assembled by string concatenation.
- [ ] Resolve new names in the frozen definition environment; prevent unrelated
  caller locals or unlocked packages from entering through text.
- [ ] Diff only available syntax/descriptor data; never claim semantic equivalence.

## R07. Candidate Compiler Pipeline And Diagnostics

Paths: etas_frontend/src/code/pipeline/{context,run}.rs and passes/,
code/service/compiler.rs, code/diagnostic/project.rs, session integration.

- [ ] Add ResolveReflectionTargetsPass, BuildDefinitionDescriptorsPass,
  FreezeDefinitionEnvironmentPass, ValidateCandidateContractPass and
  EmitCheckedCandidatePass with explicit requires/produces artifacts.
- [ ] Reuse normal parse/resolution/type/effect/handler/spec/trace/support passes.
  An outer compiler facade must not bypass or duplicate their real logic.
- [ ] Check the selected default invocation or ordinary flow plus its transitive
  dependencies. Do not union unused auxiliary methods or silently recompile
  callers to excuse a broader public replacement contract.
- [ ] Candidate target checking must not demand/invoke a synthetic main.
- [ ] Enforce frozen signature and full approved behavior contract, including
  requested actions and temporal/determinism obligations. Escaping row equality
  alone is insufficient.
- [ ] Check conservative behavioral inclusion for each permitted abstract caller
  entry state, preserving exit outcomes and intermediate violations. Reuse the
  trace domain/automaton/fixpoint machinery; final-state-only comparison is not
  sufficient. Unproved inclusion rejects. Revalidate revision-specific inlining
  and caches independently from interface compatibility.
- [ ] Produce structured check status, diagnostics, expected/actual descriptors,
  constraint/call paths, optional inferred summaries and exact input fingerprint.
- [ ] Use status rejected/accepted/guarded; ParseError.diagnostics;
  CompileError.kind rejected/service plus diagnostics. Include revision,
  dependencies, snapshot_fingerprint and descriptive runtime_checks in reports.
- [ ] Register virtual generated source revisions and accurate line/column/
  related spans. No fake SourceId(0), call-site-only or message-string parsing.
- [ ] Follow RC01-RC10: typed obligations -> verified plans -> target support ->
  engine-owned installation evidence -> actual semantic event/exit enforcement.
  check may report guarded after proving installability, without running code;
  compile cannot return A/F until native preparation covers every obligation.
  Unsupported cases and unguardable compatibility failures remain rejected.
- [ ] Compile revalidates independently of check; only internally validated,
  fingerprint-matching results can be reused. A user report is never a permit.

### R07a. Static Contract Facts And Runtime Handoff

Follow [the approved handoff protocol](etas-reflection-design.md#static-contract-facts-and-runtime-summary-handoff).
Paths: etas_types/src/facts/{reflection,expressions}.rs and normal call
collect/solve/materialize; etas_effects/src/facts/callable_values.rs,
infer/semantics/{latent,latent_source,returned_callable,callable_summary}/ and
interprocedural dependency integration; frontend code/definition/{contract,frozen_calls}/
and code/contract/; package callable-return metadata; interpreter publication.
Paths identify responsibilities; reuse existing types rather than parallel stores.

- [ ] Record a compile-result recipe: checked overload, descriptor input, A/F,
  generic/row substitutions and canonical/symbolic contract origin. Keep actual
  EffectSummary out of Types; no types -> effects/frontend dependency.
- [ ] Distinguish FrozenContract latent sources from CheckedRevision sources.
  Apply the frozen call transformer when the result is invoked, not when compile
  is requested. The caller must not wait for runtime-generated S_new.
- [ ] Resolve local contract dependencies with the existing SCC/worklist and
  freeze only converged checked contracts; no provisional bottom/empty fallback
  and no dependency cycle through the post-Effects descriptor catalog.
- [ ] Preserve origins through with_body, assignments, parameters, helper returns,
  higher-order calls, projections and ?'s Result.Ok payload. Substitute symbolic
  parameter/return templates, join all branch origins, and retain them through
  loop/recursion/widening and cross-package metadata replay.
- [ ] Reject finally unresolved/untrusted required contracts with anchored
  incomplete-facts diagnostics. No F-row-only summary or original-body fallback.
- [ ] Derive S_new independently, validate it against C_frozen and publish a
  matching contract/revision/interface/summary binding. Preserve that contract
  through successive revisions and custom-handler-supplied callable validation.
- [ ] Keep caller facts contract-based and immutable; ^candidate exposes actual
  S_new. An established old summary may back the contract, but must never be
  relabeled as the candidate's new analysis. Do not require summary equality.
- [ ] Prove narrower compatible behavior is accepted, while added handled
  actions with unchanged escaping rows and incompatible temporal behavior reject.
  Include generic returns, Result extraction, branch joins, recursive contract
  discovery, package replay, successive revisions and forged handler results.

## R08. Standard Declarations And Effects

Paths in core: etas_std/src/modules/compiler/{mod,types,syntax,service}.rs,
modules/effects.rs, intrinsic/{id,lowering,signature}.rs, registry/validation.rs.
Paths in frontend: etas_effects registry/semantics/materialization/support paths.

- [ ] Register std.compiler and the approved descriptor/support types.
  Reuse existing Option/Result/Error and generic/spec machinery.
- [ ] Declare typed CallableDef<F> and its dedicated erased CallableDef view;
  do not globally allow missing generic arguments. No new FlowDef wrapper.
- [ ] Register parse_body/format/diff/with_body as pure code operations using
  runtime code/syntax support, not PureBuiltin dependency on frontend.
- [ ] Register Code.check and Code.compile as registry-defined actions with
  type-directed AgentDef<A>/CallableDef<F> overloads under the same action/API
  identities. Standard check/compile wrappers really handle them; do not add
  duplicate symbols/IDs or interpreter-only name-based overload dispatch.
- [ ] compile returns A/F and both wrappers escape Error<CompileError>; ? captures that
  Error and preserves requested/default-handled Code.compile facts.
- [ ] Code.check rejected source is a normal report; compiler-service failure
  raises CompileError(kind=service). Compile rejection is kind=rejected.
  Authority denial/cancellation/limits keep their own enforcement semantics.
- [ ] Custom handler effects propagate normally. Compilation does not request
  the draft's model/tool effects; candidate invocation retains its contract.
- [ ] Validate actual A/F revision/frozen-contract provenance when a custom
  Code.compile handler returns an existing callable; matching type alone must
  not permit an unchecked or behaviorally incompatible implementation.
- [ ] Add readiness support for compiler service/code syntax operations through
  existing mechanisms. Do not require broad network/filesystem authority.
- [ ] Reject erased input even with an expected result type or explicit generic
  argument. Do not register std.compiler.load/Code.load or binary export APIs.
- [ ] Test registry/stubs, public signatures, effect propagation and support facts.

## R09. Host Protocol And Native Driver Adapter

Paths: etas_host/src/compiler/{protocol,service,validation,limits}/;
etas_driver/src/compiler/{mod,adapter,worker,publication}.rs.
Also interpreter/src/host/services.rs and CLI composition root.

- [ ] Implement typed Check/Compile request/response contracts and CompilerClient/
  CompilerService. Preserve request ID/authority/trace/cancellation/budget context.
- [ ] Preserve Agent/flow target family, canonical definition and frozen A/F
  contract through request, candidate publication and response validation.
- [ ] Implement bounded managed compiler work with queue, input/node/output/
  retention/deadline limits and explicit completion/cleanup accounting.
- [ ] Thread cancellation/resource checks through actual parser/pass/solver work;
  abandoning a Future or wrapping spawn_blocking alone is not cancellation.
- [ ] Charge queue wait and execution to active WallTime; do not reset parent
  limits on revisions/retries. Configure CPU/memory ceilings as durations/bytes,
  not model tokens; preserve BudgetExceeded and cancellation as such. Include
  bounded pure syntax operations in execution accounting.
- [ ] Compose frontend compilation with the engine-supplied CandidatePublisher.
  Driver must not depend on interpreter; no CLI private parser/checker.
- [ ] Keep engine publication on its owning executor; transfer immutable
  candidate data rather than InterpValue/Rc frames through compiler workers.
  No unsafe Send or unbounded thread workaround.
- [ ] Use pinned inputs/dependencies. No shelling out to etas, mutable-path
  reread, implicit package download or private untraced Host binding.
- [ ] Ensure check/compile do not execute candidate/model/tool code or change any
  global/current binding. Discard/account late results after cancellation.
- [ ] Test denial, absent adapter, queue exhaustion, cancellation races, failed
  checks and resource cleanup through real service wiring.

## R10. Interpreter Revision-Aware Agent And Flow Values

Paths: etas_interpreter/src/value/code/{mod,definition,agent,callable}.rs,
plan/code/{mod,targets,type_bridge}.rs, eval/reflection.rs,
eval/code/execution.rs, driver/dispatch/compiler.rs,
api/code/{mod,publisher}.rs, existing control/call_target/ and call dispatch.

- [ ] Introduce engine-owned ExecutableRevision and pinned revision references.
  Normalize static and compiled A/F targets to revision-aware dispatch.
- [ ] Return ordinary A or F from compile; preserve nominal identity for A and
  callable interface/generic/escaping-row bindings for F. Revision is metadata.
- [ ] CandidatePublisher prepares checked HIR using the existing plan pipeline
  and installs residual factories/hooks without evaluating the candidate.
  Validate plan/revision/contract/site/provider coverage evidence, including
  custom compile handlers; runtime invocations use independent scope state.
- [ ] Dispatch using each value/frame's own checked program and local symbol/
  type universe. Add a stable identity-based RevisionTypeBridge at boundaries.
- [ ] Never interpret a candidate TypeId/HirItemId in the original program or
  accept value shape as evidence. Include higher-order captures and methods.
- [ ] ^candidate reads that revision; ^A and ^A.run share the primary definition.
- [ ] Flow revisions use ordinary call dispatch. Preserve checked latent
  summaries and frozen behavior origin through assignment/arguments/returns;
  a declared F row cannot replace requested-action/trace evidence. No default
  agent, tool conversion, untyped closure or new public run wrapper.
- [ ] Execute the method's ordinary pre/post-inference computation. Remove the
  unconditional body-to-Prompt-then-one-model-call execution path; model
  requests occur only at explicit infer sites with selected method identity.
- [ ] Assignment updates one binding; aliases, existing frames and concurrent
  invocations retain their revisions and current authority/limits.
- [ ] Test two successive revisions, a paused old invocation, old alias calls,
  method identity and nominal inputs/outputs crossing revision boundaries.

## R11. Package Metadata And Internal Artifact Validation

Paths: etas_package_metadata/src/model.rs (split new code models into code/),
proto/{schema,codec,budget}.rs, container/;
frontend/src/package_metadata/ and code/artifact/;
driver compiler/package environment adapters.

- [ ] Add versioned neutral definition descriptors and optional retained-body
  sections, canonical IDs, retain/export visibility and frozen contract hashes.
- [ ] Preserve annotations/re-exports, hidden-body None and primary-method
  identity through producer -> binary -> manager -> consumer.
- [ ] Keep package manager as transport/integrity/materialization owner.
  It must not generate checked body definitions or default-handler fallbacks.
- [ ] Validate graph references, local-ID relocation, versions, limits, body
  availability and fingerprint consistency at the frontend environment boundary.
- [ ] Treat executable artifact provenance/validation separately from descriptive
  public metadata. Fingerprint equality alone does not establish trust.
- [ ] Preserve ordinary callable typing/binders separately from the erased
  descriptive view. Imported runtime-origin evidence must be validated rather
  than reconstructed from SignatureInfo or a shape-compatible record.
- [ ] Do not expose engine/cache binary artifacts as a public compiler load API.
  Reject forged/stale/incompatible internal artifacts at their owning boundary.
- [ ] Test metadata-only and explicitly retained source packages without silently
  overlaying hidden source to make reflection succeed.

## R12. Checkpoint, Replay And Persistence

Paths: interpreter orchestration/snapshot, eval/machine/snapshot/,
api/codec/, driver restore/preflight.

- [ ] Preserve pinned executable revision, target family/expected A or F, frozen contract, source/
  dependency identities and type relocation across checkpoints.
- [ ] Preserve drafts/syntax only under their approved serialization contract;
  opaque in-memory references are not portable artifacts by default.
- [ ] Implement public source restoration: authorized source/environment read,
  validate expected canonical target/fingerprints/locked surrounding declarations,
  typed reflection -> parse_body -> with_body -> compile under current authority.
  Saved summaries cannot replace checking; a body string alone is insufficient.
- [ ] Keep private validated engine-checkpoint decoding separate from public
  source restoration. No erased-to-typed cast or runtime-signature recovery of F.
- [ ] No original-agent fallback, implicit mutable-source recompilation or
  producer authority replay on missing/tampered candidates.
- [ ] Pending compiler operations obey normal replay/idempotency/unknown-outcome
  rules; cancellation does not fabricate a candidate.
- [ ] Apply [RC09a approval lifetime](residual-checks-implementation-worklist.md#rc09a-default-run-scoped-approval-and-optional-reuse)
  to ordinary and compiled A/F restore: current authority only, no inherited
  run-local grants, ledger-derived authorization or old monitor Approved state.
  Preserve completed work/trace progress; require explicit safe reapproval or
  reject. Persistent reuse requires application opt-in and a verifying provider;
  ordinary reflection/checkpoint/resume must not require an approval database.
- [ ] Test old/new revision coexistence, missing artifacts, schema corruption,
  stale dependencies and resumed current-authority denial.

## R13. Test Engineer Acceptance Matrix

Place source fixtures consistently under
etas/crates/etas_tests/fixtures/compiler/reflection/{positive,negative}/ and
runtime integration cases in the owning interpreter/CLI suites. Connect them to
the real fixture harness; no fixture is evidence merely because it exists.

- [ ] Syntax: caret/member precedence, call targets rejected without side effects,
  recovery, braces-free parsing, full-input consumption and format round trips.
- [ ] Reflection: agent/flow/extra method fields, ^A versus ^A.run, spec-selected
  identities, inferred output, generic/effect binders and unavailable summaries.
- [ ] Flow typing: ^declaration/^current -> CallableDef<F>, one-way erased view,
  mixed-signature Array<CallableDef>, rejected reverse cast/edit/check/compile,
  reflected methods read-only, generic row instantiation without first-class forall.
- [ ] Agent foundation: a pure default method runs without a model adapter;
  one/multiple infer methods preserve exact request count/identity, ordinary
  control flow and post-inference output validation.
- [ ] Visibility: same-name caller locals, aliases, imports, hidden bodies,
  forbidden captured state and locked dependency changes.
- [ ] Drafts: original immutability, body-only overlay, baseline analysis
  invalidation, wrong A and incompatible signature behavior.
- [ ] Checking: rejection/report versus operational failure, structured source
  locations, call-chain constraints and mixed unavailable/available facts.
- [ ] Effects: default/custom Code handlers, denied requests, ? preserving actions,
  new requested actions despite empty escaping row, trace/determinism violations.
- [ ] Execution: compile without prior check, compile does not run code, A/F
  returned directly, dynamic selection, old aliases/in-flight calls and type bridge.
- [ ] Residual checks: an accepted candidate actually enforces its installed
  obligations; unsupported checks reject.
- [ ] Artifacts: package identity, optional body data, forged reports/handles,
  schema/dependency/config tampering and locked source restoration.
- [ ] Lifecycle: cancellation during active compiler work, caller drop, bounded
  cleanup, queue saturation, repeated request handling and no leaked workers.
- [ ] Memory: 1/10/100 reflected refs share one graph; repeated draft creation
  stores overlays; releasing candidates/drafts reclaims unpinned revisions.
- [ ] End-to-end: deterministic Generator with a valid replacement returns the
  new output; invalid replacement retains the application's old binding.
- [ ] End-to-end ordinary flow: retained normalize -> two body edits -> F calls;
  old aliases/in-flight calls retain revisions. Reject new IO/requested actions,
  wrong result types, reordered trace obligations and absent body/contract data.
- [ ] Optional oMLX example exercises real rewriting; pin a small model through
  existing config. It supplements, not replaces, deterministic acceptance.

## R14. Definition Of Done

- [ ] All applicable chapter 18 requirements have evidence for both A and F;
  no obsolete source gate is used to excuse missing specified behavior.
- [ ] Every supported API has source-level parsing/checking/execution tests;
  unsupported cases are listed honestly, not represented as working stubs.
- [ ] Run targeted syntax/HIR/types/effects/metadata/Host tests, interpreter and
  CLI integration tests, existing package/handler/checkpoint regressions, and
  relevant strict lint/format checks.
- [ ] Supply before/after evidence for rejected inputs, not only new happy paths.
- [ ] Report actual allocation/retention, cancellation and artifact guarantees.
- [ ] No shadow checker, fake empty body, pure compile shortcut, string-based
  target dispatch, descriptor-to-proof coercion or automatic global promotion.
- [ ] EDK owns evaluation/scoring/publication; no compiler-owned RSI controller.
