<!-- Historical recovery snapshot, archived 2026-11-21. Not a current specification, implementation status, or engineering task assignment. The current language SPEC and main implementation take precedence. -->

# Agent Step Implementation Worklist

Owner: `Architect`

Last updated: `2026-11-11`

Status: implementation tasks derived from the current working-tree SPEC; not
implementation acceptance. Preserve unrelated engineer work.

## 1. Authoritative Contract

Read these current documents, including uncommitted designer edits:

- [Single-Step Agent Interaction](../../../etas/docs/design/19-standard-library.md#4-single-step-agent-interaction), including the complete controller example.
- [Agent operational intuition](../../../etas/docs/design/03-agents-tools-prompts-memory.md#13-operational-intuition).
- [Agentic action and default handling](../../../etas/docs/design/06-effect-system-and-inference.md#12-agentic-runtime-actions).
- [Shared JSON ADT](../../../etas/docs/design/19-standard-library.md#3-json-values).
- [Layer boundaries](../../../etas/docs/design/01-overview.md#53-feature-map-by-layer).

The target is:

```text
perform infer<O>(prompt)
  -> checked Agentic.infer<C, O>
  -> default full-loop controller OR an explicit user handler
       -> std.agent.step(conversation, schema)
            -> checked Agentic.step<C, O>
            -> one provider generation
            -> Final / CallTools / Stopped
       -> std.tool.invoke(call), only if the controller chooses to invoke
       -> pure append_step / append_results
       -> another explicit step or final return/resume
```

`infer` remains supported. `step` is not a new keyword and cannot call tools,
make a repair generation or run an automatic retry loop. Reuse existing crates,
pass infrastructure, checked HIR, provider adapters, action enforcement, budget,
replay and checkpoints. No new agent SDK crate, tool registry, public checked-call
wrapper, mandatory receipt API, JSON type, schema type or session abstraction.

Architectural constraints from prior work continue to apply: one compiler
pipeline, one machine evaluator, run-scoped approval, typed/lossless internal
checkpoint codecs and no source/body or string-based dispatch fallback.

## 2. Verified Current Gaps

| Current path | Evidence and required change |
|---|---|
| `etas_std/src/modules/agent/mod.rs` | Registers prompt/schema/message/session/group, not the new step protocol and conversation assembly APIs |
| `etas_std/src/modules/effects.rs` | Agentic declares infer only; step is missing |
| `etas_std/src/modules/core/json.rs` | JsonValue is still a Support declaration, not the complete public recursive ADT |
| `etas_types/src/facts/inference.rs` and `pipeline/body/collect/inference.rs` | Checked fact covers Prompt-based infer, not step/invoke/context propagation |
| `etas_effects/src/infer/semantics/inference.rs` | infer summary adds repeated exposed-tool summaries; this cannot become the step transfer unchanged |
| `etas_interpreter/src/eval/agent/inference.rs` | Default infer immediately creates the old PendingModel/full-loop path |
| `etas_interpreter/src/eval/machine/model.rs` | Owns automatic tool execution, argument repair feedback and typed-output repair generations |
| `etas_host/src/model/protocol.rs` | No general stop outcome or continuation metadata; tool calls have two copies; reported token fields are mandatory within ModelUsage |
| `etas_host/src/tool/protocol.rs` | ToolRef has display/qualified names and std ID but no implementation revision |
| `etas_interpreter/src/value/support.rs` | Conversation requires a session string; public model usage projection has no cost |
| `etas_interpreter/src/value/code/metadata.rs` | Reflection ToolRef is an opaque CodeMetadataPayload::Tool(DefinitionKey), not the new shared public record |

These are source-inspection findings. No live provider or new feature tests were
run for this worklist; do not interpret missing-symbol searches as a full audit
of every existing agent feature.

## 3. AS01: Complete Std Protocol Declarations

Extend files within `etas-core/crates/etas_std`:

```text
src/modules/agent/
  mod.rs
  protocol/
    mod.rs          # registration glue
    turn.rs         # AssistantTurn, MessageContent, Usage
    decision.rs     # StepResult, StepDecision, StopReason, StepDiagnostic
    tool.rs         # shared ToolRef, ToolCall, ToolResult declarations
  step.rs           # public step signature and primitive binding
  conversation.rs   # pure assembly declarations
src/modules/tool.rs # std.tool.reference / invoke registration
src/modules/effects.rs
src/intrinsic/{runtime,pure,completion,lowering}.rs
```

- [ ] Register every record field, variant and generic parameter exactly as
  SPEC section 4.2. These are ordinary source-visible types/enums with normal
  construction, matching and field access, not a new family of opaque tickets.
- [ ] Register all six APIs: step, reference, invoke, from_prompt, append_step,
  append_results, with the exact types in section 4.3. Do not replace them with
  private helpers that Etas source cannot call.
- [ ] Register `Agentic.step<C, O>(Conversation, Schema<O>) -> StepResult<C,O>`
  alongside infer. Use canonical identities/default handling/runtime requirement
  descriptors, not string checks for `step` or broad Agentic effect injection.
- [ ] Bind pure assembly separately from model/tool execution. An absent
  explicit source effect row does not authorize publishing a fabricated empty
  summary for step/invoke.
- [ ] Preserve `StepDiagnostic.code` as stable machine-readable categories,
  `path` as optional location and `message` as explanatory text. Tests and
  controllers must not identify error classes by matching message wording.
- [ ] Resolve the existing MessageContent/ToolRef declarations by canonical
  identity; re-export the same public type where needed. Do not create another
  incompatible compiler ToolRef or another JSON tree for this protocol.

## 4. AS02: Make Shared JSON And Tool Identity Real

Relevant existing files:

```text
etas_std/src/modules/core/json.rs
etas_std/src/modules/compiler/{types,reports}.rs
etas_code/src/descriptor/{definitions,fingerprint,validation}.rs
etas_interpreter/src/value/{json,code/metadata}.rs
etas_interpreter/src/plan/code/value_bridge/
etas_frontend/src/code/descriptor/
etas_host/src/tool/protocol.rs
```

- [ ] Expose JsonValue's six real variants and JsonError.InvalidJson; make
  construction/match/parse/stringify work through checked source. Reuse current
  optimized JSON storage and pure codecs, not an extra agent JSON enum.
- [ ] Respect SPEC Number(f64) precision/finite-number limits. Do not invent a
  lossless-large-integer promise or route engine identity, usage cost and ledger
  values through that public JSON representation. Internal typed/lossless ABI
  guarantees remain intact before/after model-facing projection.
- [ ] Derive public ToolRef.id from canonical package/module/item identity,
  using existing etas_code identities. revision binds the selected implementation
  snapshot/environment, including external package/tool bindings as applicable.
  Do not hash a display name alone or use a placeholder revision.
- [ ] `std.tool.reference` accepts a resolved native tool declaration, not a
  flow, raw string, fabricated record or an anonymous test ToolRef. It is pure
  description, not invocation or registration.
- [ ] Reflection publishes the same public `{id, revision}` ToolRef. Replace
  the public opaque projection; keep engine-only validated executable bindings
  private. Hand construction/editing of ToolRef remains legal and safe.
- [ ] Host routing may retain provider-facing names/std IDs internally, but
  validate their exact mapping to canonical identity/revision. Same-named tools
  from different packages or revisions cannot be confused.

## 5. AS03: Carry Checked Agent Context Across Handler Boundaries

Extend the existing type pipeline, not another inference checker:

```text
etas_types/src/facts/inference.rs
etas_types/src/constraint/ty.rs
etas_types/src/pipeline/body/collect/{inference,call,perform,handler}.rs
etas_types/src/solver/{agent,callable,handler}.rs
etas_types/src/pipeline/body/materialize/
etas_types/src/pipeline/finalize/{merge,remap}.rs
etas_interpreter/src/control/{signal,continuation}.rs
etas_interpreter/src/eval/{agent/inference,handler}.rs
```

- [ ] Reuse infer's checked agent/method identity and existing kinded/static
  parameter machinery. Give step/invoke checked context requirements and
  operation-specific facts with method/revision/tool-surface bindings. C is a
  checked agent-method contract index, not a user string or arbitrary data type
  that can acquire another agent's authority.
- [ ] Infer O from supplied Schema<O>/expected result. The step uses that checked
  schema, not a silently substituted agent return type or previous infer summary.
  A surrounding infer handler's final resume still has its original result type.
- [ ] A handler for infer<C,O> may call step/invoke using the intercepted
  request context, even when the handler is defined in ordinary flow code.
  Propagate these requirements through generic/helper calls; do not require
  every call to be textually inside an agent body.
- [ ] Extend PendingPerform's current inference-policy payload into a proper
  engine-owned request context carrying canonical C/revision and tool bindings.
  Keep it through handler execution, suspension, nested handlers and unwind.
  Do not reconstruct context by searching for a similarly named agent.
- [ ] Naming another C does not switch context; runtime verifies facts against
  the active request. With no applicable context, reject rather than choose a
  default agent/model or use protocol fields as context evidence.
- [ ] Context is invocation/scoped state, not a mutable global. Nested calls
  restore outer context; a captured context cannot revive a finished run or
  inherit expired approvals. Recheck current authority at actual boundaries.
- [ ] Keep grammar changes limited to necessary existing generic/path handling.
  No new `step` keyword, controller syntax or public continuation/multi-shot API.

## 6. AS04: Implement A True Single-Generation Model Boundary

Refactor existing host and interpreter boundaries:

```text
etas_host/src/model/{protocol,openai,anthropic,client}.rs
etas_host/src/context/trace/payload/model.rs
etas_interpreter/src/eval/agent/inference.rs
etas_interpreter/src/eval/boundary_model.rs
etas_interpreter/src/driver/dispatch/model.rs
etas_interpreter/src/eval/machine/model.rs
etas_interpreter/src/eval/agent/step/{request,decode,project}.rs  # focused new modules
```

- [ ] Build exactly one provider-generation request from Conversation plus
  checked agent context and Schema<O>. Retain normal policy, trust, budget,
  cancellation, trace and HostDispatch response-ID validation.
- [ ] Decode one explicit result: Final(O), non-empty CallTools, or Stopped.
  Final requires real schema/type validation. Tool names resolve against C to
  canonical ToolRefs. Unknown/malformed requests become InvalidToolCall without
  invoking a tool; invalid output becomes InvalidOutput without repair.
- [ ] Preserve truncation/refusal from provider finish/stop fields. Do not
  interpret every text-bearing response as a valid Final or catch malformed
  protocol content into an empty successful value.
- [ ] Preserve required provider continuation metadata in AssistantTurn's
  provider_data and encode it back in subsequent requests. Use versioned,
  provider-specific validation and reserved-field protection; arbitrary data
  must not override model, endpoint, tool set, credentials, policy or budgets.
- [ ] Keep ToolCall only in public StepDecision, not another independently
  editable copy in AssistantTurn. Normalize provider envelopes to one
  authoritative tool-call list; any legacy projection is derived from it.
- [ ] Preserve optional usage fields and cost. Absent means unknown, not zero.
  Update Host ModelUsage/projection/accounting together. Conservative budget
  admission may reject missing required evidence; it cannot treat unknown as
  free execution. Caller-built Usage never settles the engine ledger.
- [ ] Separate host request correlation IDs from optional provider/request
  identifiers exposed in the turn. Never fabricate an absent ID or use a
  model-provided ID as the trusted operation identity.
- [ ] A step cannot enter advance_tool_calls, typed-output repair, automatic
  required-tool repair, or another provider request. Audit provider/transport
  retry configuration too: it cannot hide a second potentially executed model
  generation behind one step. Normal transport failures retain failure/unwind.

## 7. AS05: Implement Pure Conversation Assembly

Reuse `Conversation` rather than create a second source type:

```text
etas_std/src/modules/agent/{conversation,session}.rs
etas_interpreter/src/value/{support,conversation/mod,conversation/payload}.rs
etas_interpreter/src/value/conversation/protocol/{state,validate,append}.rs
etas_builtin/src/agent/conversation.rs  # neutral pure rules if shared here
```

Do not move interpreter values into builtin. A shared helper accepts neutral
protocol descriptions; typed value projection remains in the engine. Ordinary
assembly/controller logic may be Etas library source in existing etas_std.

- [ ] from_prompt creates an in-memory conversation preserving channels and
  trust labels. Existing ConversationValue's mandatory session string must not
  force a fabricated SessionId, database access, or implicit session append/load.
  Add an internal sessionless protocol view/association while preserving real
  session/history/publication APIs and their fences/provenance.
- [ ] append_step returns a new value containing the whole assistant turn and
  its decision requests/metadata. Validate transitions: no empty CallTools,
  duplicate call IDs or a new turn over unresolved pending requests.
- [ ] append_results completes exactly the pending set once. Reject missing,
  duplicate, foreign IDs and results after Final/Stopped or before CallTools.
  Input result order may differ; assemble in pending/provider-required order.
- [ ] Revalidate the conversation before each provider request, including
  pending completion. Constructor correctness or a previous append is not
  sufficient proof for later caller-edited input.
- [ ] Fabricated ToolResult values are allowed for simulation; assembly does
  not write completed-operation ledger entries, approval events, usage evidence
  or trace-spec execution events. It cannot upgrade user-controlled trust.
- [ ] Pure assembly validates only protocol/data. It neither executes tools nor
  certifies their execution and cannot perform hidden I/O or model repairs.
- [ ] Bound turn/content/JSON nodes, depth and bytes before allocation. Preserve
  immutable value semantics and avoid copying whole history on every append
  when existing shared/persistent storage can be reused.

## 8. AS06: Expose One Checked Native Tool Invocation Path

Extract reusable execution from the existing automatic loop:

```text
etas_interpreter/src/eval/machine/model.rs
etas_interpreter/src/eval/agent/tool/{reference,resolve,decode,invoke,project}.rs
etas_interpreter/src/driver/dispatch/tool.rs
etas_interpreter/src/plan/dispatch/
etas_interpreter/src/eval/host_value.rs
etas_host/src/tool/{protocol,client,http,process,mcp}.rs
```

- [ ] Resolve ToolRef identity AND revision against registered tools exposed
  by active C, including imported tool metadata. Do not register or authorize
  caller-constructed references or fall back to global display-name lookup.
- [ ] Decode args against the actual checked declaration/schema. Validate
  concrete numeric widths, nominal identities, arity and record fields; schema
  claims in user/provider data are irrelevant.
- [ ] Enforce caller/tool boundaries and current trace specs/runtime policy,
  approval, sandbox, limits and cancellation. Then execute checked Etas HIR or
  its declared external binding. Source-bodied tools never fall through to Host.
- [ ] Validate returned output before encoding shared JsonValue feedback.
  Correlate ToolResult.call_id without treating that caller-visible string as
  the engine's trusted operation identity.
- [ ] Boundary rejections can return Err(StepDiagnostic) without execution.
  Define stable codes. Internal Error<E> effects remain ordinary escaping
  obligations unless explicit source handling translates them; do not catch
  every tool exception into ToolResult, Stopped or an empty JSON value.
- [ ] Operational policy termination, cancellation, exhaustion and uncertain
  external outcomes preserve existing failure/unwind semantics. Merely
  appending an error feedback record is not a substitute for enforcing them.
- [ ] Reuse current occurrence/ledger/trace infrastructure with active request,
  call correlation, arguments and exact revision. Duplicate/conflicting records
  follow replay/dedup or uncertainty rules, not automatic re-execution and not
  a claim of exactly-once external effects.
- [ ] Caller-built valid calls work under the same checks; do not require a
  new opaque checked-call token minted only by step. One private validated
  binding during dispatch is an implementation detail, not a second public API.
- [ ] Multiple calls are sequential by default unless explicit, checked
  concurrency satisfies dependency/order/resource rules. No parallel map simply
  because a provider returned an array.

## 9. AS07: Separate Step Effects From Invoke And Infer Summaries

Use the existing `etas_hir_analysis` interpreter and `etas_utils` pipeline,
fixpoint and graph facilities:

```text
etas_effects/src/infer/semantics/{inference,transfer,std_intrinsics,calls}.rs
etas_effects/src/effect/{registry,coverage}.rs
etas_effects/src/infer/semantics/agent/{context,step,invoke}.rs
etas_frontend/src/passes/analysis/effect_check.rs
etas_types/src/facts/inference.rs
```

- [ ] Step records Agentic.step<C,O> requested/default action and real model
  nondeterminism/runtime support. It does not add actual tool-execution events
  merely because CallTools is possible or returned. Preserve any declared
  ordinary errors separately from the default-handled agentic action.
- [ ] invoke<C> consumes checked C/tool-surface facts and conservatively joins
  exposed tools' escaping effects, requested/default actions, determinism,
  transitive requirements and temporal behavior. Missing external summaries
  fail closed; no empty summary or string-name allowlist.
- [ ] For temporal analysis use branch choice over tool summaries at an invoke,
  with repetition introduced by an actual controller loop. Do not invent a
  sequential trace that says every exposed tool ran, or use only an unordered
  may-action union to prove approval-before-write.
- [ ] infer's full controller composes step and invocation behavior; its public
  tool effects remain visible even though Agentic.infer and step are handled.
  Do not copy `agent_infer_summary` unchanged as the step implementation.
- [ ] Track Request/Invoke/Attempt/Complete at their real boundaries. A model
  request, fabricated ToolResult or conversation append cannot satisfy an
  approval/execution trace spec. Runtime enforcement observes actual dispatch.
- [ ] Preserve C and its bounded tool summary through generics/helper calls,
  package metadata, handler continuation context and frozen reflection contracts.
  Dynamic ToolRef selection narrows runtime execution, not the static contract
  unless soundly established by existing analysis.
- [ ] Stopped/ToolResult Err are value outcomes, not broad Error handlers.
  `?` retains existing single-error capture semantics and is not Result unwrap.

## 10. AS08: Rebuild Default Infer On Shared Primitives; Remove Old Authority

Existing authoritative paths to replace/remove after migration:

```text
etas_interpreter/src/eval/machine/model.rs:
  handle_model_response's automatic tool/repair orchestration
  advance_tool_calls as a second controller
  append_tool_result / invalid_tool_args_message as hidden protocol assembly
etas_interpreter/src/eval/machine/model_repair.rs:
  automatic repair generation policy owned only by Rust model frames
etas_interpreter/src/control/signal.rs:
  PendingModel fields that make every model request a full loop
```

- [ ] Implement the default full-loop controller using the same step, invoke
  and assembly operations exposed to source. Prefer real versioned Etas std
  source, e.g. `etas_std/library/std/agent/infer.es`, reusing the existing std
  implementation-source pipeline rather than adding another compiler/SDK.
- [ ] Keep one authoritative implementation of validation, dispatch and
  assembly. An optimized native controller may only implement the same checked
  operations/contracts and must be tested differentially, not remain the old
  loop behind a renamed `step` wrapper.
- [ ] Move repair/retry/round/required-tool choices into explicit controller
  policy under inherited limits. Step itself does not silently clear tools,
  switch choice, retry invalid output or inject a repair request. Preserve
  supported infer behavior deliberately and test the distinction.
- [ ] User-provided infer handlers bypass the default full controller and can
  implement SPEC section 4.5. Repeated steps are separate requests; final resume
  remains single-shot. Never implement this as multi-shot continuation reuse.
- [ ] Keep ordinary agent.run execution, pre/post-inference code, multiple
  infer sites, explicit step-only methods and no-inference pure agents working.
  Agent declaration itself still does not request model execution.
- [ ] Retain raw ModelResponse APIs only according to their existing contracts;
  they are not a substitute for StepResult or an authority bypass. Remove old
  code only when its legitimate users have migrated, not by disabling tests.

## 11. AS09: Update Recovery, Metadata And CLI Integration

Reuse existing locations:

```text
etas_interpreter/src/eval/machine/{frame,state,snapshot/model}.rs
etas_interpreter/src/eval/machine/snapshot/{continuation,value_capture,value_restore,validator}.rs
etas_interpreter/src/api/codec/{machine/model,value/encode_model}.rs
etas_interpreter/src/orchestration/{model_request,checkpoint}.rs
etas_frontend/src/package_metadata/projection/
etas-core/crates/etas_package_metadata/src/{model,proto/codec,container/mod}.rs
etas/crates/etas_driver/src/
etas/crates/etas_cli/src/command/interpreter/{host,host_config,host_tool}.rs
```

Paths under etas_interpreter are crate-relative; repository paths above refer
to the existing workspaces, not new crates.

- [ ] Snapshot the explicit controller state, C/revisions, conversation pending
  calls, provider_data, completed real invocations and the original handler
  continuation. Do not replay an already completed tool on resume or lose its
  call ID/order when the source handler is suspended.
- [ ] Serialize transparent data as data, not execution/approval certificates.
  Resume uses current authority; old Approved monitor/grants are not restored.
  Validate revisions, limits and context consistency before continuing.
- [ ] Bump affected checkpoint/package/artifact schema and fingerprints where
  necessary; incompatible artifacts fail explicitly. No hard-coded obsolete
  next-version number is mandated by this checklist.
- [ ] Export/import tool IDs/revisions, agent context requirements and full
  solved invoke summaries. Reflection tool references and package tools agree
  with runtime dispatch; metadata-only tool imports cannot omit schemas/bodies
  or use a package-private fallback.
- [ ] CLI/driver keep orchestration/configuration duties only. Use existing
  model/tool adapters and runtime configuration; do not add CLI-owned agent
  controllers, a separate tool registry, new required env variables or an
  application dependency in the compiler.
- [ ] Preserve bounded payload/JSON/schema validation and redaction through
  adapters, replay, trace and checkpoint paths, including provider_data.

## 12. AS10: Required Regression And End-To-End Matrix

Extend current core, frontend, interpreter, driver/CLI tests and
`etas_tests/fixtures/compiler/std_conformance/` categories. Do not add a test
crate or use an unrelated unresolved name as the expected negative result.

| Area | Required evidence |
|---|---|
| Source surface | All protocol records/enums construct, access and match; all six APIs resolve with correct generics and return/effect types |
| C/O | Handler in ordinary flow uses intercepted context; nested/reentrant contexts restore; unrelated C/no context fails; supplied Schema<O> drives Final decoding |
| One-step boundary | Exactly one model generation for Final, CallTools, invalid args/output, truncation/refusal; zero tool invocations and no hidden repair/retry |
| Protocol payload | Empty/unknown/duplicate tool requests; mixed content; provider metadata round trip and reserved-field injection rejection; one authoritative request list |
| Tool invocation | Source-bodied and external package tools work; wrong ID/revision, raw name, out-of-surface tool, bad args/output and nominal/numeric mismatch fail before unsafe execution |
| Value vs effect failure | Boundary Err feedback, Stopped, ordinary tool Error<E>, transport fault, cancellation, limit failure and policy termination remain distinct |
| Pure assembly | Pending set/order checks; missing/duplicate/foreign results; reordered valid results; empty CallTools; post-Final/Stopped transitions; no implicit session I/O |
| Simulated records | Hand-built ToolRef/ToolCall checked at invoke; fabricated ToolResult accepted only as feedback, never approval, trace satisfaction, ledger completion or usage evidence |
| Summaries | Step-only code has no executed-tool trace; invoke preserves conservative full transitive summaries; infer/controller wrappers and cross-package replay preserve them |
| Limits | Multiple steps share parent token/cost/time/cancellation limits; absent usage is unknown; reported cost survives; fake Usage cannot reset or settle budgets |
| Recovery | Checkpoint before/after tool and between step/results; source handler suspension; stale revision/changed authority rejection; no duplicate completed side effects |
| Existing APIs | Ordinary infer full loop, pure agent, multiple inference sites, native/source tools, typed output, explicit handlers and raw response behavior retain valid coverage |

- [ ] Add the SPEC section 4.5 example as a real project fixture, not just a
  parser sample. Assert final output, per-step model count, invoke count, IDs,
  conversation messages and handler single-shot behavior.
- [ ] Extend `etas_interpreter/src/testing/cases/{tool,agent_execution}.rs`
  with focused source-controller tests and new step tests. Add provider adapter
  tests in `etas_host` for finish reasons, continuation metadata and usage.
- [ ] Use deterministic scripted provider responses to verify ordering/counts
  and negative cases. They are test doubles, not a production implementation.
  Include ordinary checked source execution and public transparent-data paths,
  not only hand-made Rust pending frames.
- [ ] Run a real configured oMLX smoke for explicit controller and default infer,
  asserting actual tool dispatch and schema validation. Small model suffices;
  model noncompliance remains visible. Do not commit API keys, fabricate a
  response on failure or call an ignored live test completed.
- [ ] Provide exact command/results, old-entry-point search, API coverage and
  relevant strict Clippy/fmt/diff evidence. Preserve existing dirty changes.

## 13. Completion Gate

Suggested order: AS01/AS02, AS03, AS04/AS05/AS06, AS07, AS08, AS09 and AS10,
with tests alongside every change. This is one target, not a reduced language
subset. Engineer reports must distinguish declaration, compiled fact, executable
path, removed old authority and end-to-end evidence for each item.

Do not mark complete if step still calls the old whole model loop, if invoke
only returns an unverified ToolResult, if default infer has a second hidden
dispatcher, or if context/revision validation is replaced with a name check.
No empty fallback, fake approval/usage, swallowed ordinary Error effects,
mandatory opaque receipt/token or application-private host binding is acceptable.
