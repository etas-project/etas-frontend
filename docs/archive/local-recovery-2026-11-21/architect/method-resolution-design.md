<!-- Historical recovery snapshot, archived 2026-11-21. Not a current specification, implementation status, or engineering task assignment. The current language SPEC and main implementation take precedence. -->

# Method Resolution And Chained Calls

Status: Architecture target, not implementation acceptance.
Owner: Architect
Updated: 2026-11-03

## 1. Contract And Audit Baseline

Read SPEC [02, ordinary methods](../../../etas/docs/design/02-general-programming-constructs.md),
[05, type specs](../../../etas/docs/design/05-type-system-and-errors.md),
and [09, expressions and impl forms](../../../etas/docs/design/09-syntax-principles.md).
Both `impl Type { flow ... }` and behavioral `impl Type ~ Spec { flow ... }`
provide methods. `impl Spec for Type` remains accepted compatibility syntax.
Do not turn all free flows taking T into methods or add arbitrary UFCS sugar.

For each link in `value.first(args).second(args)`, solve the first call's actual
output type, then resolve the next method on that type. Links can change type;
they do not require one common receiver type or one common spec. Inherent
methods need no extra spec. Spec methods require checked implementation/bound
evidence. Explicit `value::Spec.method(args)` restricts selection to that spec.
Ordinary namespace calls and type-associated calls remain distinct.

The 2026-11-03 audit established these boundaries:

| Probe | Observed result, not target behavior |
| --- | --- |
| Std `Deque.new<i32>().push_back(2).push_front(1)` | Frontend passes; existing std-chain execution regression passes |
| Source inherent declaration without invocation | Frontend passes |
| Source inherent `x.bump()` / `x.bump().bump()` | UnknownField; declaration acceptance does not establish dispatch |
| Spec implementation with `x.step()` / `x.step().step()` | UnknownField |
| Explicit single `x::Step.step()` | Frontend and existing interpreter regression pass |
| `x::Step.step()::Step.step()` | Parser rejects the suffix |
| `(x::Step.step())::Step.step()` | Frontend passes; this probe did not verify execution |
| Explicit spec result followed by an inherent method | UnknownField |

These failures must become permanent regressions, not workarounds requiring
parentheses, std-only tests, renamed methods or altered nominal contracts.

One target acceptance fixture is the following, not a claim that it works today:

```etas
type Seed = { value: i32 };
type Output = { value: i32 };

impl Seed {
    public flow to_output(self) -> Output {
        return Output { value = self.value };
    }
}

spec Label {
    flow label(self) -> string;
}

impl Output ~ Label {
    flow label(self) -> string { return "ready"; }
}

flow main() -> string {
    return Seed { value = 7 }.to_output().label();
}

flow explicit_selection() -> string {
    return Seed { value = 7 }.to_output()::Label.label();
}
```

Both entries must return `ready`: the first method returns a different nominal
type, and the second method is selected using that type's spec evidence. Keeping
the original Seed type as the receiver of every link is incorrect.

## 2. Ownership And Layout

Keep the implementation in existing crates. Proposed paths extend their current
layers; create working modules, not empty facade directories:

```text
etas_syntax/src/grammar/
  expr.rs                         # expression precedence and list-cons composition
  expr/postfix.rs                 # one iterative call/member/spec/index/? tail loop
etas_hir/src/
  lower/expr.rs                   # source-shaped chain lowering
  view/{model,index}.rs           # existing ImplFlow body ownership
etas_types/src/
  method/
    model.rs                     # method identities, receiver mode, lookup selection
    index.rs                     # immutable owner/spec/member indexes
    evidence.rs                  # checked implementation/bound dispatch recipes
  facts/methods.rs                # checked method-call output contract
  pipeline/signature/collect/methods/
    source.rs                    # inherent/spec/agent declarations, shared owners
    standard.rs                  # StdRegistry adapters, no copied signatures
    external.rs                  # validated dependency declarations/impls
  pipeline/signature/validate/methods.rs
  pipeline/body/collect/method_call.rs
  pipeline/body/collect/spec_method_call.rs
  pipeline/body/materialize/methods.rs
  solver/method/
    mod.rs                       # method constraint solver entry
    candidates.rs                # indexed discovery and visibility
    instantiate.rs               # receiver/Self and kinded generic bindings
    select.rs                    # isolated probes, ambiguity, checked target
etas_frontend/src/package_metadata/projection/methods.rs
etas_effects/src/infer/semantics/method_calls.rs
etas_interpreter/src/
  plan/dispatch/methods.rs        # checked recipes -> engine call targets
  eval/method/checked.rs          # exactly-once evaluation and selected dispatch
```

Syntax does not resolve methods. HIR does not choose overloads. Types owns
lookup/evidence/substitutions. Effects owns interprocedural behavior; interpreter
planning/evaluation consume checked facts and must not perform type inference.
Package transport uses existing canonical callable/impl identities. Driver/CLI
do not search methods or add another source compilation pass.

## 3. One Postfix Grammar

Move selected-spec calls into the same iterative postfix tail processing as
ordinary methods, calls, fields, indexing/slicing and postfix `?`. Preserve the
AST distinction `Call`, `MethodCall`, `SpecMethodCall`, `Field` and `ListCons`.
Selected members bind as call/member postfix operations; use parentheses when
the intended receiver is an entire lower-precedence expression.

Required shapes include:

```text
make().first().second()
make().first()::Encode.encode().next()
value::Step.step()::Step.step()
(value::Step.step()).next()
value::Fallible.convert()?.next()
```

They are syntax shapes, not declarations of these example APIs. `?` still
captures Error into Result; a subsequent method must actually exist on Result,
not on its Ok payload. No implicit unwrap is introduced.

Retain the existing syntactic discriminator for `::Spec.method(...)` versus
right-associative list cons `head :: tail`. Parsing must not consult the symbol
table or treat all `::` as method selection. After a selected call, continue
parsing suffixes instead of returning from list_cons_expr_inner. Preserve
method/spec/argument spans and resource limits; malformed suffixes recover
without dropping later links. HIR lowering uses explicit chain iteration and
existing tree ownership rather than a separate flat-chain execution IR.

## 4. Declaration Model And Index

Normalize callable declarations to MethodSignatureFact:

```text
MethodSignatureFact
  MethodId / canonical callable and implementation origin
  Owner: inherent type head | selected behavioral spec | agent component
  Receiver: Value(parameter_slot) | Associated
  CallableSignature, owner/method generic binder environments
  visibility, defining module, source/metadata origin
  implementation body reference or external executable reference
```

Use the existing callable/signature models, not another callable type system.
`self` is bound by the owning impl/spec signature, including the SPEC's untyped
self form and already accepted explicitly typed receiver forms. Receiver metadata
is explicit once normalized; do not infer it from parameter names during calls
or classify an arbitrary first parameter of a free flow as a receiver. Preserve
parameter immutability/value semantics; no new reference/mutable-receiver syntax.

MethodIndex is built once from signature facts using HirTreeView. Index inherent
methods by canonical receiver type head/constructor and member name, spec
requirements by checked spec/member identity, and implementation evidence by
subject/spec. Std source-method metadata and agent declarations adapt to this
same model. Retain existing std intrinsic IDs, overloads, receiver modes and
checked index/message/security semantics. Do not synthesize std source impls.
Generic/standard receiver families use typed pattern buckets alongside concrete
type heads; bucket lookup is indexed by member/receiver constraints, not a fresh
scan of every registry symbol or hand-coded method-name/type-shape exceptions.

Transparent aliases normalize to their targets. Nominal representations are not
expanded to borrow another type's methods or spec evidence. Distinct nominal
types remain distinct even when runtime layout matches. Validate duplicate
members, impl coherence, target kind, receiver ABI and visibility. Re-exports
and diamond super-spec evidence deduplicate by canonical identity, not spelling.
Spec-call accessibility follows its permitted public spec/type surface and the
SPEC's implementation visibility rules. Exporting an implementation mapping
does not make its private body callable as a public free flow; do not confuse
body retention/import visibility with availability of a checked spec witness.

Use existing HirBodyRef(owner, ImplFlow(member_index)) for nested source method
bodies. Do not invent synthetic top-level flows or treat an entire impl as one
body. Type/effect body collectors and source-to-HIR maps must cover these methods.

## 5. Collect, Solve And Materialize

### Signature Pipeline

Extend the real signature stages: collect source/std/external method facts,
resolve aliases/spec entailment/import identities, validate members/coherence,
then materialize the immutable method environment/index. This environment is an
explicit input of body checking; it cannot be rebuilt by scanning all impls at
every call. Respect current visibility/import/conformance admission rules; do
not make all workspace methods globally visible merely because they are indexed.

### Body Constraints

The two syntax collectors emit one MethodCall goal with a selection mode:

```text
MethodCallGoal
  call_expr, receiver_expr, receiver_type, member and source origin
  selection: Unqualified | ExplicitSpec(checked_spec_ref, spec_arguments)
  explicit method generic arguments, lexical generic/spec evidence
  source-ordered argument expressions/names and output type variable
  caller module/access context
```

Collectors traverse receiver/arguments and emit constraints/provisional types;
they do not select an implementation, write final call facts or reinterpret
unknown methods as fields. Reuse normal contextual lambda/literal and named-
argument binding machinery; delayed receiver information must not force a
callback to be rejected before its parameter constraints become available.

### Solver Algorithm

1. Resolve known receiver substitutions. If an inference variable is unresolved,
   register the goal with the existing access/call worklist; do not fail early,
   choose a std method by name, or rescan the whole HIR.
2. Probe the relevant indexed inherent/standard/agent candidates and applicable
   behavioral spec methods. ExplicitSpec limits the search to that spec and its
   validated inherited members; it never falls back to an inherent/std name.
3. Instantiate each candidate in an isolated inference state. Bind owner/Self,
   fresh method generics, kinded type/effect-row arguments and spec obligations.
   Reuse callable/unification/assignability/overload solvers. Map named arguments
   to ABI slots without changing their source evaluation order.
4. Check receiver applicability, evidence, visibility, arity and all argument/
   output constraints. Failed probes cannot leak substitutions, facts or
   diagnostics into another candidate. Expected output helps inference but
   cannot fabricate an implementation or erase nominal identity.
5. Commit only a uniquely selected canonical operation, with its complete
   substitutions/evidence. Deduplicate equivalent evidence paths to the same
   operation, not distinct impls with identical signatures. Use any SPEC-defined
   ranking; absent such a rule, multiple applicable operations are ambiguous.
   Do not invent inherent-first priority or select the first successful probe.
   Explicit `::Spec` can disambiguate spec choices.
6. Feed the selected output type into dependent goals and continue until the
   normal solver worklist converges. A still-unresolved inference variable or
   missing required fact is a diagnostic, not dynamic runtime lookup.

For rigid `T ~ Spec`, resolution can select a checked bound method recipe without
choosing a concrete T. Generic instantiation supplies the validated witness and
substitutions. Existential spec values use the evidence established by checked
packing and existing object-safety rules, never runtime shape matching. Marker,
callable and trace specs do not acquire behavioral methods by name. No general
subtyping, implicit numeric conversion or representation coercion is introduced.
Generic-body lookup uses its declared bounds and their validated entailments,
not the set of concrete implementations currently loaded in the project. Do
not bind a rigid T to one convenient known implementation during candidate probes.

The existing solver stage produces a method-resolution report; only body
materialization publishes its final facts. This is real constraint solving,
not wrappers around recursive checker helpers or two parallel call solvers.

## 6. Checked Call Contract

Extend TypeFacts with checked_method_calls keyed by each call HirExprId:

```text
CheckedMethodCallFact
  receiver/input/output checked types and source-ordered argument binding map
  target:
    SourceMethod(body_ref, callable/impl identity)
    StdOperation(std identity, overload, intrinsic, checked ABI)
    AgentMethod(existing component/method identity)
    ExternalMethod(canonical package/callable identity)
    BoundSpecMethod(checked spec member, generic/existential witness recipe)
  Receiver mode and receiver ABI slot
  owner/method type and effect-row substitutions
  checked spec evidence, visibility/access provenance and source site
```

Reuse existing GenericInstantiationFact, checked std facts, agent facts and
canonical CallableId models where appropriate. One selected-call identity is
authoritative; compatibility projections must be derived and validated, not
independently selected. Body-delta merge, TypeId remapping, retained-size
accounting, fingerprints and serialization cover every embedded reference.

Missing facts are blocking for checked execution. A valid actual callable-field
expression, such as `(value.callback)(args)`, still uses FieldAccess plus Callable
constraints. That is not permission to convert an unknown MethodCall into a
FieldAccess and ignore its arguments. Preserve any other callable-field forms
already admitted by SPEC with full callable checking; unresolved collision rules
must diagnose rather than silently prefer one category.

## 7. Effects, Reachability And Runtime

Effect call targets and dependency edges come from checked_method_calls, including
source impl bodies and bound-method summaries. Reuse HirAbstractInterpreter,
the shared method body collector, SCC/worklist and normal effect substitutions.
Do not rediscover methods by name or discard behavior because a method has no
top-level HirItemId. Unknown/open behavior is not pure; use validated summary/
contract templates or report incomplete facts rather than an empty row/trace.

Evaluation order is receiver, explicit arguments in source order, then method
body/operation. Append each exactly once, retaining escaping effects, requested/
default actions, trace order and latent callable results. `?` has its existing
effect-to-Result meaning at its actual location. Argument collection is not a
second evaluation or a reason to duplicate requested actions.

Reachability and dependency source overlays follow these same checked edges.
Metadata-only calls require valid effect contracts; runtime execution of an
external method requires its checked body/executable or approved std substrate.
Do not make an external method a private Host fallback or force a second driver
load/check. Incremental invalidation covers receiver/impl/spec/import visibility,
generic contracts, external artifacts and selected target changes.

Interpreter plan preparation binds checked call facts to direct call targets or
validated witness dispatch recipes. Evaluation must not scan spec_impls, call
TypeUnifier, inspect display names to choose an impl, or guess from value shape.
Remove that selection from eval/spec_method.rs. Std kernels/adapters stay owned
by their existing implementations; only their target selection becomes uniform.

Use the explicit machine/continuation call path for source methods. Evaluate a
receiver once, bind its ABI slot once and preserve partial argument state across
Host suspension, handlers, finish/return and checkpoint. Checkpoint codecs and
revision bridges validate the selected method, generic/witness identity, source
origin, argument progress and ABI; they must not resolve again on restore.
Current authority/approval lifetime and residual checks remain unchanged.

## 8. Package And Diagnostics

Export public inherent/associated methods and behavioral requirements/impls with
canonical owner/member/body identities, receiver metadata, kinded generics,
visibility, spec evidence, type signatures and solved behavior contracts. A
consumer must distinguish type-spec membership from an executable method table.
Current external spec impl records without method information cannot stand in
for this data. Producer/consumer validation, artifact versions and cache keys
change together; package manager transports facts but does not infer methods.

Diagnostics distinguish no method, inaccessible member, missing/ambiguous spec
evidence, multiple operations, argument/generic mismatch and unresolved receiver.
Point at the failing link/member and related declarations/constraints. Preserve
upstream errors without cascades of fake UnknownField diagnostics. Never return
unit, Never, the receiver or an arbitrary candidate as a successful failed call.

## 9. Acceptance And Work List

The detailed [engineering work list](method-resolution-implementation-worklist.md)
defines deletion/migration tasks and tests. Completion requires source inherent,
unqualified spec, explicitly selected spec, heterogeneous/mixed/generic chains,
cross-package methods and interpreter execution, not only StdRegistry acceptance.
No code, fixture or SPEC change is made by this architecture document.
