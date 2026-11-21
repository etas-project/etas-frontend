<!-- Historical recovery snapshot, archived 2026-11-21. Not a current specification, implementation status, or engineering task assignment. The current language SPEC and main implementation take precedence. -->

# Method Resolution Implementation Work List

Owner: compiler/interpreter engineer, with test engineer.
Architecture updated: 2026-11-03.
Status: implementation/acceptance tasks; unchecked does not mean all existing
infrastructure is absent. Reuse working solvers, HIR views and std kernels.
Architecture changes belong to Architect. Do not edit SPEC/EDK, commit or push
without direct user authorization.

## Required Reading

- [Unified method architecture](method-resolution-design.md).
- SPEC [02](../../../etas/docs/design/02-general-programming-constructs.md),
  [05](../../../etas/docs/design/05-type-system-and-errors.md),
  [09](../../../etas/docs/design/09-syntax-principles.md) and 06/15/18 for
  effects, package contracts and revision identity.
- Syntax/HIR/Types/Effects/frontend and interpreter designs. Keep the current
  run-scoped approval, residual enforcement and reflection tasks intact.

## MC01. Unify Postfix Parsing

Paths: etas_syntax/src/grammar/expr.rs and new expr/postfix.rs;
existing AST expression definitions/visitor/dump/format and syntax tests.

- [ ] Move the postfix tail loop and selected-spec suffix parsing into a cohesive
  module; call, dot-method, field, spec-selection, index/slice and ? resume the
  same loop after constructing each node. Do not add a parser per method family.
- [ ] Delete the early selected-call return in list_cons_expr_inner. Preserve
  its separate right-associative list-cons responsibility and syntactic lookahead
  discrimination; no symbol lookup or global reinterpretation of ::.
- [ ] Keep Call/MethodCall/SpecMethodCall distinct with accurate receiver/spec/
  method/argument spans. Preserve admitted generic syntax and named arguments.
- [ ] Cover `x::Step.step()::Step.step()`, `.method()` after a selected call,
  selected calls after ordinary methods and interaction with ?, field/index/
  slice, pipeline/composition and parentheses. No implicit Result unwrap.
- [ ] Update AST visitors/dumps/formatting and bounded parsing/recovery. A trailing
  malformed link must diagnose its real source without swallowing the next item.

## MC02. HIR Ownership And Method Bodies

Paths: etas_hir/src/lower/{expr,namespace_call}.rs, view/{model,index}.rs,
resolve/, hir/expr.rs; etas_hir_analysis/src/unit/.

- [ ] Lower all chain links in source shape; lexical module/std namespace calls
  stay ordinary calls and type-associated operations stay distinct from value
  receivers. HIR must not select implementations from a method's display name.
- [ ] Preserve typed/untyped self receiver declarations and impl/spec ownership;
  normalize both spec impl spellings to the existing semantic form.
- [ ] Reuse HirBodyRef(owner, ImplFlow(index)) and HirTreeView for every nested
  source method. Ensure source map, symbol -> declaration, BodyView and analysis
  units resolve the same body. No synthetic top-level wrapper or impl-wide body.
- [ ] Validate unique ownership and deterministic traversal across methods,
  nested closures/handlers and source module parts; retain deep-chain safety.

## MC03. Build A Shared Method Environment

Paths: etas_types/src/method/{model,index,evidence}.rs;
pipeline/signature/{state,materialize}.rs, collect/methods/, validate/methods.rs;
facts/{methods,specs,signatures}.rs and lower/std.rs.

- [ ] Add MethodSignatureFact/receiver-mode/owner identities using existing
  CallableSignature, canonical callable IDs and generic binder models. Bind
  untyped self from the owner, not call-site guesses; normalize accepted typed
  receiver forms too. Parameter names alone are not a method ABI.
- [ ] In real signature collection stages, collect inherent/spec/agent methods
  via HIR views, std methods via StdRegistry descriptors and external methods
  via validated dependency facts. Resolve/validate then materialize the index.
  Declare the corresponding stage requires/produces; no facade-only pass.
- [ ] Index by receiver type head/member and checked spec/impl identity. Do not
  scan every std symbol or impl for every expression. Alias normalization must
  not expose a nominal representation's methods/evidence.
- [ ] Validate visibility, duplicate method/impl identities, receiver mode and
  signatures/bounds. Preserve spec entailment/diamond evidence without treating
  distinct implementations as interchangeable. Do not export private methods.
- [ ] Publish the immutable method environment as explicit body input and include
  it in affected-unit/cache fingerprints. Its collectors cannot check bodies.

## MC04. Replace Split Collectors And First-Match Solving

Paths: pipeline/body/collect/{method_call,spec_method_call,std_member,agent_method}.rs;
constraint/ty.rs; solver/{method.rs,report.rs,callable.rs,overload.rs,spec_solver.rs};
new solver/method/{mod,candidates,instantiate,select}.rs.

- [ ] Make ordinary and selected-spec collectors emit the same MethodCall goal,
  differing only in the selection mode. Collect all arguments and provisional
  constraints; preserve contextual lambda/record/literal and named-argument work.
- [ ] Delete the authoritative std/agent-only selection branches from
  collect_method_call and its FieldAccess-only tail. Unknown methods cannot
  discard call arguments or return a field's type as the call result.
- [ ] Replace collect_spec_method_call's separate Callable-only path with the
  same goal, retaining call ExprId, explicit spec evidence and receiver identity.
  Do not keep call: None for a real selected method needing dispatch facts.
- [ ] Remove std_method_candidates/raw_std_method_candidates as global name-scan
  resolution authorities. Retain namespace/type-member helpers and typed std
  descriptor lowering where still needed; do not delete valid std APIs/kernels.
- [ ] Split solver/method.rs by the new responsibilities and remove the old
  first-success `for candidate ... break` selection path. Merely moving that
  loop into candidates.rs is not this refactor.
- [ ] Solve delayed receiver/call/argument constraints through the existing
  scheduler/worklist. Instantiate Self/owner and method generics with separate
  binder identities; reuse callable, spec, effect-row and overload solvers.
- [ ] Probe candidates transactionally. Commit only a unique legal operation;
  more than one applicable operation diagnoses unless SPEC defines ranking.
  No registration-order, inherent-first or expected-result-only tie-break.
- [ ] Select rigid generic bound/witness recipes without demanding a concrete T
  during generic-body checking; do not confuse a real bound with an unresolved
  inference variable or infer a bound from whichever concrete impls happen to be
  loaded. Preserve existing existential object-safety rules.
- [ ] Keep legitimate callable-field expressions on a fully checked
  FieldAccess + Callable path. They are not unknown-method fallback. Preserve
  index checks, Message.cast ABI and Secret.data restrictions as typed selected
  operations; never remove safety semantics while deleting string dispatch.

## MC05. Materialize One Checked Method Call Contract

Paths: facts/methods.rs, solver/report.rs, pipeline/body/materialize/methods.rs,
pipeline/finalize/{merge,remap}.rs; frontend body artifacts and fingerprints.

- [ ] Add checked_method_calls keyed by each MethodCall/SpecMethodCall ExprId.
  Record target/body or checked witness recipe, receiver ABI, argument-slot map,
  input/output types, kinded substitutions, evidence/access and source origin.
- [ ] Only materialization writes final facts. Collectors do not set checked
  targets; unresolved/failed calls produce no successful fact.
- [ ] Derive existing std/agent compatibility projections from that same selected
  operation and validate consistency. Delete independent selection paths after
  consumers migrate; do not maintain conflicting authoritative maps.
- [ ] Remap all embedded TypeIds and local symbols/body refs in body-delta merges,
  generic applications and revision bridges. Update size/fingerprint/codecs and
  negative tests for incomplete/forged checked facts.

## MC06. Connect Analysis, Effects And Reachability

Paths: etas_hir_analysis/src/unit/; etas_effects/src/infer/{unit,semantics/calls.rs},
new semantics/method_calls.rs and pipeline/; frontend passes/dependency/,
project/instantiation/ and session invalidation.

- [ ] Feed every source impl method into shared semantic-body collection by its
  real HirBodyRef. Do not map all impl members to EffectUnit::Item(impl_owner)
  when that owner has no executable body.
- [ ] Resolve graph edges from checked_method_calls for inherent/spec/agent/
  external/bound targets. Reuse generic HIR abstract interpretation and existing
  SCC/fixpoint solving, not another method effect checker.
- [ ] Evaluate/analyze receiver then explicit arguments in source order, then
  selected body. Instantiate complete escaping/requested/default/trace/latent
  behavior with the checked substitutions exactly once. A spec method with no
  known behavior is not automatically pure; preserve valid conservative contract
  templates or report missing facts, never an empty summary.
- [ ] Reachability/source overlay planning must follow those exact targets,
  including imported impl bodies and generic witness applications. No driver
  method-name scan, private Host fallback, whole-package body injection or
  second load/check to compensate for absent edges.
- [ ] Invalidate callers when receiver signatures, visible impl/spec evidence,
  overloads, method bodies or dependency artifacts change. Reuse unaffected
  bodies; a matching member name alone is not a valid cache dependency.

## MC07. Remove Runtime Method Resolution

Paths: etas_interpreter/src/plan/{pipeline,dispatch}.rs and dispatch/methods.rs;
eval/{expr,spec_method,method/dispatch,method/checked}.rs; control/call_target/,
eval/machine/ and api/codec/checkpoint/.

- [ ] Plan direct targets/checked witness recipes from method facts with exact
  source-body/std/agent/external identity and receiver/argument ABI.
- [ ] Delete resolve_checked_spec_method_target's runtime spec_impls scan and
  TypeUnifier selection; remove spec_type_ids_match/type_arg_lists_match when
  no checked non-selection use remains. Runtime cannot solve missing evidence.
- [ ] Replace method dispatch by raw method name/value shape with checked target
  dispatch. Existing std kernels/special ABIs remain reusable implementations,
  not a permission to select an unchecked target.
- [ ] Bind source impl bodies in plan tables; do not repeatedly search an impl's
  item list to rediscover the selected body. Preserve generic/witness and pinned
  revision identity at calls and cross-revision bridges.
- [ ] Use the existing explicit machine for method execution/recursion. Evaluate
  the receiver once and arguments in source order; bind receiver once, preserve
  partial calls across Host suspension/handlers/finish/checkpoint and check
  current effect/authority boundaries. No separate recursive Rust evaluator.
- [ ] Restore selected call/witness/ABI facts and source argument progress from
  validated snapshots; missing/mismatched targets reject, not runtime rediscovery
  or re-execution. Keep run-scoped approval and residual gates unchanged.

## MC08. Cross-Package Methods And Portable Facts

Paths: etas_package_metadata/src/{model,proto}/; etas_frontend/src/package_metadata/
projection/methods.rs and passes/analysis/external_metadata_validation/;
etas_types/src/pipeline/signature/imports/external/specs.rs.

- [ ] Export public inherent/associated methods, behavioral signatures and actual
  impl member mappings, receiver modes, binder/substitution info, canonical
  callable/body references, visibility and solved behavior contracts.
- [ ] Extend external spec facts beyond self_type/spec/args membership: a method
  call needs the member signature plus valid implementation/witness mapping.
  A marker proof is not an executable method table.
- [ ] Validate canonical references at the frontend environment boundary and
  consume them in the shared MethodIndex. Aliases/re-exports preserve identity;
  missing/private/ambiguous data cannot become a local empty implementation.
- [ ] Version artifacts/cache/facts/checkpoints where their wire contracts change.
  The package manager transports compiler facts; do not make it infer methods
  or require reanalysis of every dependency source for metadata-only checking.

## MC09. Required Regression Matrix

Put permanent source fixtures under the existing owning harness, with
inherent/spec/mixed and positive/negative categories. Do not rely on a /tmp
probe or std-only chain as proof of completion.

- [ ] Positive: inherent single/chain, untyped and admitted typed self, nominal
  constructor -> method, value -> different return type -> next method.
- [ ] Positive: unique unqualified spec methods, explicit single/repeated ::,
  and inherent/spec/std mixed chains without mandatory extra parentheses.
- [ ] Positive: generic T ~ Spec, method/owner generic binder separation,
  higher-order callbacks, kinded effect rows, inherited specs and validated
  existential receivers where the SPEC permits their methods.
- [ ] Positive: cross-file/module/package, aliases/re-exports, private access
  within its valid scope and associated constructors with explicit/inferred args.
- [ ] Negative: missing/private/ambiguous method, absent/wrong spec evidence,
  conflicting implementations, wrong argument name/arity/type/generic kind,
  unresolved receiver and nominal-representation method leakage.
- [ ] Parser negatives and precedence: :: selection vs right-associative cons,
  fields/index/slices/?, composition/pipeline, malformed suffix recovery. A ?
  result is Result, not an implicitly extracted Ok receiver.
- [ ] Runtime: assert exact values, receiver/argument evaluation counts and order,
  requested/escaping effects and trace multiplicity. Cross suspension/checkpoint,
  handler Error/finish paths, generic witness calls and pinned revisions.
- [ ] Tampering: wrong target/body/evidence/receiver slot/substitution in method
  facts, metadata or checkpoint rejects before any side effect.
- [ ] Scaling: permitted long chains are iterative; work scales with links and
  indexed candidates, not links times every method/impl in the project. Respect
  existing resource ceilings and diagnose over-limit input without stack overflow.
- [ ] New regressions reproduce the audit failures on old code and pass on the
  new path. Do not rename methods to registered std names, flatten the chain,
  wrap it in fake agents or weaken fixture expectations to obtain green tests.

## MC10. Closure Evidence

- [ ] Remove obsolete selection implementations/imports and duplicate facts;
  every target family reaches the same checked resolution/dispatch contract.
- [ ] Run owning syntax/HIR/types/HIR-analysis/effects/frontend/metadata/
  interpreter suites and relevant package/CLI fixtures offline when applicable.
  Report exact commands/counts, strict lint/format checks and any live-test gaps.
- [ ] Complete means real source -> checked facts/effects -> execution across
  both user method families and mixed chains, not parser acceptance or a thin
  wrapper around the previous std/agent-only resolver. No implementation commit
  or SPEC/EDK modification is authorized by this work list.
