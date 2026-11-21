<!-- Historical recovery snapshot, archived 2026-11-21. Not a current specification, implementation status, or engineering task assignment. The current language SPEC and main implementation take precedence. -->

# Text API Implementation Worklist

Owner: `Architect`

Last updated: `2026-11-04`

Status: approved implementation tasks, not a claim of implementation completion.

## 1. Goal And Constraints

Implement the complete string API in [SPEC chapter 19](../../../etas/docs/design/19-standard-library.md),
including all numeric widths, lazy traversal, Unicode, typed errors, limits,
package/incremental compatibility and checkpoint recovery. This is one complete
target; task order below is not a reduced subset or a staged language version.

Required architecture:

- [Std declarations and source implementations](etas-std-design.md#65-stdtext).
- [Shared kernels](../../../etas-core/docs/architect/etas-builtin-design.md#10-text-kernels-and-lazy-sequences).
- [Type checking](etas-types-design.md#text-standard-contracts-and-lazy-iterables).
- [Effect checking](etas-effects-design.md#text-apis-and-deterministic-error-effects).
- [Interpreter adapters and recovery](../../../etas-interpreter/docs/architect/phase1-interpreter-design.md#111-text-values-lazy-sequences-and-typed-errors).
- [Conformance matrix](etas-test-design.md#121-complete-text-api-conformance).

Use existing workspace crates only: `etas_std`, `etas_builtin`, `etas_core`,
`etas_utils`, `etas_hir`, `etas_types`, `etas_hir_analysis`, `etas_effects`,
`etas_frontend`, `etas_package_metadata`, `etas_interpreter`, and the existing
CLI/test crates where integration requires them. Do not create text, Unicode,
iterator, formatting or test crates. Do not add external dependency crates.
An existing workspace crate may be reused as a dependency where layering allows;
never introduce a builtin-to-frontend/interpreter dependency cycle.

Do not modify the language SPEC, EDK implementation, or unrelated engineer
changes. Do not commit/push without authorization. Architecture documents remain
the architect's responsibility. New files listed below are modules/assets inside
existing crates, not permission to create another subsystem or crate.

## 2. Verified Migration Baseline

Recheck current code before editing, but do not preserve these observed defects:

| Current file | Migration target |
|---|---|
| `etas-core/crates/etas_std/src/modules/core/text.rs` | Small registry, opaque text `ParseError`, eager `lines`, old formatting names |
| `etas-core/crates/etas_builtin/src/text/query.rs` | `Len` counts scalars; `ParseI32` returns a Rust error string inside Result |
| `etas-core/crates/etas_builtin/src/collections/count.rs` | A second scalar-counting string length path through generic collection methods |
| `etas-core/crates/etas_builtin/src/text/transform.rs` | `Lines` uses `TextOutput::Array` and can eagerly collect all lines |
| `etas-core/crates/etas_builtin/src/text/string.rs` | Old wrappers/formatters; `unary_integer_to_string` funnels widths through `i64` and casts |
| `etas-frontend/crates/etas_types/src/solver/index.rs` | Accepts direct indexing on string, contrary to chapter 19 |
| `etas-interpreter/crates/etas_interpreter/src/eval/index_access.rs` | Direct string indexing; returns a one-character string while type solver expects char |
| `etas-interpreter/crates/etas_interpreter/src/intrinsic/pure/text.rs` | Join maps combined allocation/size failure to NumericOverflow |
| `etas-frontend/crates/etas_hir_analysis/src/static_string.rs` | Evaluates trim/case using independent Rust helpers; must share pinned semantics |
| `etas/crates/etas_tests/fixtures/compiler/support/algorithms.es` | Imports old formatters and returns `lines(input)` as an Array |

These are code-inspection findings, not a claim that every existing path has
been executed. The tasks require both old-path removal and real regression tests.

## 3. TX01: Register The Entire Source API

Replace `etas-core/crates/etas_std/src/modules/core/text.rs` with this folder:

```text
etas_std/src/modules/core/
  text/
    mod.rs          # register text module and dispatch family registration
    errors.rs       # TextError enum and seven real constructors
    query.rs        # length/count/emptiness/ASCII declarations
    traversal.rs    # lazy chars/indices/lines declarations
    search.rs       # search/match declarations
    split.rs        # eager, lazy and once splitting declarations
    slice.rs        # Index-bounded access and checked/optional slices
    edit.rs         # trim/case/strip/replace declarations
    assembly.rs     # concat/join/repeat/pad/reverse declarations
    numeric.rs      # concrete parse/render families
    format.rs       # positional formatting declaration
    unicode.rs      # std.text.unicode and Normalization enum
  char.rs           # std.char scalar APIs
```

- [ ] Keep `core/mod.rs` as registration glue. Preserve canonical identities and
  validate duplicate exports through `registry/validation.rs`.
- [ ] Use existing `decl/std_type.rs`, `decl/type_decl.rs`,
  `decl/flow_decl.rs`, `registry/enum_constructor.rs` and the standard
  registration helpers. Declare real enum variants, not an opaque Support
  declaration with string-shaped errors.
- [ ] `TextError` has `InvalidIndex`, `InvalidRange`, `NotCharBoundary`,
  `ParseFailure`, `NumericOverflow`, `LengthOverflow`, `InvalidFormat`.
  `Normalization` has `NFC`, `NFD`, `NFKC`, `NFKD`.
- [ ] Extend `modules/core/collections.rs` with the standard `Iter<T>` contract
  and `.iterable_elements(0)` metadata using the existing descriptor mechanism.
  It is not an alias to Array/List, not publicly constructible from arbitrary
  runtime records, and does not imply a new public iterator protocol.
- [ ] Register every API in section 4. Use explicit generic `I ~ Index` bounds
  and exact result/effect declarations. `FlowDecl::pure` alone is not suitable
  for a checked text API that raises `Error<TextError>`.
- [ ] Keep UTF-8 declarations in existing `modules/codec/text.rs`. Do not
  re-export duplicate implementations under a second text codec module.
- [ ] No source keywords, generic ToString/Display spec, string builder,
  implicit method spelling or new Host action family is introduced.

## 4. TX02: Complete API-To-Implementation Matrix

Each name below needs registry, checked call, executable implementation and
positive/error evidence. A group is not complete when only one representative
works. Signatures/edge cases are those in chapter 19.

| Family | Required names | Implementation destination |
|---|---|---|
| Query | `len`, `char_count`, `is_empty`, `is_ascii` | builtin `text/query.rs` |
| Traversal | `chars`, `char_indices`, `lines` | builtin `text/iteration/`; engine adapter creates lazy value |
| Search | `contains`, `starts_with`, `ends_with`, `find`, `rfind`, `find_from`, `find_all`, `count` | builtin `text/search.rs`; shared cursor for all matches |
| Split | `split`, `split_iter`, `split_once`, `rsplit_once`, `splitn`, `split_whitespace` | builtin `text/split.rs` / shared scanner |
| Access | `char_get`, `byte_slice`, `char_slice`, `get_byte_slice`, `get_char_slice`, `is_char_boundary` | builtin `text/bounds.rs`, `text/slice.rs` |
| Edit | `trim`, `trim_start`, `trim_end`, `strip_prefix`, `strip_suffix`, `lowercase`, `uppercase`, `ascii_lowercase`, `ascii_uppercase`, `eq_ignore_ascii_case`, `replace`, `replace_n`, `replace_range` | builtin `text/transform.rs` and search/bounds kernels |
| Assembly | `concat`, `join`, `repeat`, `pad_start`, `pad_end`, `reverse_chars` | ordinary std assembly flows over shared bounded kernels; `text/join.rs` / `text/string.rs` |
| Numeric | `parse_N`, `from_N` for every numeric type below | builtin `text/parse.rs`, existing numeric value ABI |
| Other conversion | `parse_bool`, `from_bool`, `from_char`, `format` | parse/render primitives and ordinary std format flow |
| Unicode | `graphemes`, `grapheme_indices`, `grapheme_count`, `grapheme_slice`, `reverse_graphemes`, `normalize`, `is_normalized`, `casefold` | builtin `text/unicode/`, lazy sequence adapter or ordinary assembly |
| Character | `is_whitespace`, `is_alphabetic`, `is_alphanumeric`, `is_control`, `is_ascii_digit`, `to_u32`, `from_u32` | builtin `text/char.rs` with shared Unicode data |
| Codec | `utf8_encode`, `utf8_decode` | existing builtin `codec/text.rs` and checked ABI |

`N` means all fourteen types: `i8`, `i16`, `i32`, `i64`, `i128`, `isize`,
`u8`, `u16`, `u32`, `u64`, `u128`, `usize`, `f32`, `f64`. Implement twenty-eight
distinct numeric public flows, not one i32 implementation with narrowing casts.

Pure module APIs remain ordinary library contracts; a new public name does not
automatically require an intrinsic ID. Native implementations are shared kernels,
not per-name evaluator branches or private host bindings.

## 5. TX03: Link Real Std Bodies Without Display-Stub Execution

Files to extend/new modules within existing crates:

```text
etas-core/crates/etas_std/
  src/decl/implementation.rs              # typed source/primitive implementation reference
  src/library/mod.rs                     # release-bundled source asset catalog
  library/std/text/assembly.es            # ordinary assembly logic
  library/std/text/format.es              # template algorithm over checked primitives
etas-frontend/crates/etas_frontend/src/
  passes/source/standard_library.rs       # bundled implementation source admission
  passes/analysis/standard_library.rs     # checked body/decl compatibility
```

- [ ] Extend existing registry/module/declaration records with a typed
  implementation link: real library-body reference or explicit primitive
  binding. A source implementation and its optional native optimization share
  one public identity and contract, not two competing public symbols.
- [ ] Include immutable source assets in the existing `etas_std` distribution.
  Rust `etas_std` exposes bytes/descriptors; it does not depend on or run the
  parser, HIR checker or interpreter.
- [ ] Extend frontend `passes/source/mod.rs`, `passes/module/catalog.rs`,
  `passes/resolution/source_members.rs`, `project/context.rs` and existing
  `pipeline/{mod,passes,scheduler}.rs`. Select needed std bodies and their closure,
  give them stable virtual source origins, and admit them through normal
  parse/HIR/type/effect passes.
- [ ] Library-catalog registration is available before discovery. The admission
  pass uses the catalog, normal parsed import/path references and check goal
  to select source modules conservatively; it precedes parsing those admitted
  library bodies, not parsing the user's initial files. Selection need not wait
  for HIR, and never infers types by scanning strings. Reuse the session's source
  admission/unit scheduler for the added closure; do not restart the whole
  project parser or invoke `check_project` recursively. Later checked body
  reachability can narrow executable bodies within those modules.
- [ ] The compatibility pass consumes checked declaration/body facts. Register
  real requires/produces keys in `passes/artifacts.rs` and the existing pass
  manager; neither catalog presence nor an empty placeholder is a checked-body
  artifact. Compile/validate release-bundled library artifacts once per matching
  environment and reuse them, not once per public call.
- [ ] Bind source-body identity to the corresponding standard declaration.
  Do not create duplicate std modules/symbols or globally hide child metadata.
  Reuse module catalog identity and the existing source/body reachability model;
  conservative source import closure is allowed without compiling all std bodies.
- [ ] Validate inferred body contract against the declared public contract,
  including result types, errors, determinism and requested actions. Derive
  facts from the body; do not label a body pure without checking it.
- [ ] Generated LSP/source/HIR display stubs remain non-executable. Never parse
  an invented `intrinsic(...)` display body as the implementation.
- [ ] Session and existing `etas_cache` reuse checked library artifacts;
  invalidation includes source, std implementation and Unicode versions.
  Do not require a second driver load/check cycle or whole-std parse per call.

Ordinary source must implement real algorithms, not a one-line forwarding facade
added solely to count source coverage. A primitive requiring native storage or
Unicode support has an explicit typed binding and tested bounded semantics.
Do not add unapproved public helper APIs to expose internal storage primitives.

## 6. TX04: Share Position Validation And Matching Algorithms

Create/extend `etas_builtin/src/text/{bounds,search,split,slice}.rs`; use the
existing `text/mod.rs` exports. Reuse checked integer representation and range
rules instead of copying engine `NumericValue` or frontend `TypeId` into builtin.

- [ ] A neutral checked-position adapter preserves signedness and full integer
  width until validation; only then convert representable positions to usize.
  Negative/too-wide scalar access returns None; checked search start reports
  InvalidIndex. Invalid slice intervals report InvalidRange.
- [ ] Factor reusable interval normalization into an existing builtin domain
  if currently engine-only. `eval/slice_access.rs` and text adapters share the
  arithmetic rules; text applies its own required ordered/bounded policy.
  Do not reuse `value/range.rs`'s reversed-range-as-empty behavior as proof that
  an invalid text interval is legal. Preserve existing non-text contracts.
- [ ] `is_char_boundary` returns false for invalid positions; it does not raise
  a text error. Optional accessors and checked APIs share validation but retain
  their different public outcome mappings.
- [ ] Byte slice validation: normalize range, validate order and bounds, then
  validate UTF-8 scalar endpoints. An in-bounds interior byte is
  NotCharBoundary; it must not be rounded or repaired.
- [ ] Scalar/grapheme slice validation converts ordinal endpoints to byte
  endpoints through one progressing scan. Empty intervals at the valid end are
  allowed; char access at scalar count is absent. No hidden global scalar index
  or source-copy cache is required.
- [ ] Implement a shared progressing match cursor. Non-empty matches do not
  overlap. Empty patterns visit UTF-8 scalar boundaries including both ends,
  emit each empty match once, then terminate. Do not advance by one raw byte.
- [ ] `find_from` validates the byte boundary and returns original-source
  offsets. Search absence is None, not an error. `find_all` and count use the
  same match decisions as split/replace.
- [ ] Exact splitting preserves leading/trailing/adjacent empty segments;
  `split("ab", "")` is `["", "a", "b", ""]`; `splitn(..., 0)` is empty and
  `splitn(..., 1)` contains the original source once.
- [ ] Lines distinguish LF/CRLF from standalone CR, preserve internal empties,
  omit the extra final line, and produce no element for empty input.
- [ ] Use available standard string search primitives or a measured bounded
  matcher, not an O(n*m) substring-allocation loop. Account for consumed scan
  work and permit bounded polling even when finding no next element.

## 7. TX05: Implement Lazy Sequences Once

New modules stay inside `etas_builtin`:

```text
text/iteration/
  mod.rs
  plan.rs       # TextSequencePlan / TextSequenceKind
  cursor.rs     # validated operation-tagged TextCursorState
  step.rs       # one bounded next-element operation
```

- [ ] Implement the eight sequence kinds in the builtin architecture:
  Chars, CharIndices, Lines, Split, Whitespace, Matches, Graphemes,
  GraphemeIndices.
- [ ] A plan is immutable operation/configuration data. A cursor stores byte
  progress, terminal/empty-match phase and operation-specific scanner state.
  Elements are scalar, indexed scalar, segment, indexed segment or byte interval.
- [ ] `step` borrows source/configuration for bounded work and returns one
  element, end, or explicit pending scanner progress. It owns no engine value,
  frame, HIR/AIR ID, native borrowed iterator or Host service.
- [ ] Engine `SequenceValue` retains immutable StringValue/configuration and
  validated item type. Each consumer owns its cursor. Cloning the sequence
  descriptor does not consume or share another loop's mutable progress.
- [ ] Setup does not scan/copy all input and does not allocate a result array.
  Sequential traversal resumes saved state, not `chars().nth(n)` from zero.
  Early break produces/allocates no later segments.
- [ ] Do not implement lazy APIs by collecting an array and wrapping it with an
  Iter tag, or maintain a second text-only loop evaluator.
- [ ] `split`/`splitn` are intentionally eager arrays that consume shared
  scanners under limits. A one-step borrowed iterator inside Rust is not alone
  evidence that the public API is lazy.

## 8. TX06: Correct Rendering, Parsing, Editing And Assembly

Existing/new builtin modules: `text/query.rs`, `text/transform.rs`,
`text/join.rs`, `text/string.rs`, `text/parse.rs`, `text/format.rs`,
`numeric/*`, `codec/text.rs`.

- [ ] `TextQuery::Len` uses byte length in constant time. Add separate
  `char_count`; audit `collections/count.rs::evaluate_text` and interpreter
  `intrinsic/pure/count.rs` so any admitted string length entry uses the same
  byte semantics. Keep unrelated well-specified collection methods intact.
- [ ] Move `ParseI32` out of TextQuery. Implement every numeric width without
  passing through i64/f64. Parsing validates the entire token, sign, decimal
  syntax and representability, with no implicit trim or trailing suffix.
- [ ] Parsing invalid syntax is ParseFailure; out-of-range numeric input is
  NumericOverflow. Use structured parse outcomes, never Rust error.to_string()
  as an Etas error payload. Finite float render/parse round trips use matching
  f32/f64 semantics and release-stable locale-independent representation.
- [ ] `parse_bool` accepts exactly true/false. Rendering bool and char has no
  reflection or implicit callback. `from_u32` rejects surrogates/out-of-range
  codepoints with None, not a replacement character.
- [ ] Reuse/extend `join_projected` for checked total-size calculation, bounded
  reservation and bulk write without cloning a parts array. `concat` shares
  the assembly primitive with an empty separator; its source wrapper must still
  have a checked implementation reference, not string-based redispatch.
- [ ] Split `JoinError::OutputTooLarge` into representability, allocation and
  execution-limit failure as needed. Remove both NumericOverflow conversions
  in builtin `text/string.rs` and interpreter `intrinsic/pure/text.rs` that
  currently conflate allocation and arithmetic failures.
- [ ] Repeat checks length multiplication; overflow is LengthOverflow. Zero
  count and empty input return empty without iterating n times. Resource limits
  and allocation failures remain execution failures, not TextError variants.
- [ ] Pad by scalar count, never bytes/display width; avoid rebuilding the
  accumulated prefix. Reverse scalar order for reverse_chars; preserve source
  aliases for every transformation. Replace uses shared matches, and range
  replacement allows insertion/deletion at a checked byte interval.
- [ ] Formatting parses braces/indices, validates each placeholder with checked
  arithmetic and computes bounded output. Support repeated indices and escaped
  braces; missing indices/unsupported syntax are InvalidFormat. Unused arguments
  are allowed. No implicit formatting of arbitrary values or source evaluation.

## 9. TX07: Pin And Implement Unicode In The Existing Builtin

```text
etas_builtin/src/text/unicode/
  mod.rs
  version.rs
  segment.rs
  normalize.rs
  casefold.rs
  data/             # release-pinned compact data and provenance
etas_builtin/tools/generate_unicode.rs
etas_builtin/tests/unicode_data/
```

- [ ] Select and record one supported Unicode data release plus algorithm/data
  fingerprints. Reuse existing implementations only after verifying coverage
  and version. A transitive ICU normalizer is not proof of segmentation/full
  folding support, and no new dependency crate is authorized.
- [ ] Package missing release data and deterministic offline generation inside
  this crate, with source/checksum/license provenance. No runtime download,
  network-dependent build or platform-locale API.
- [ ] Implement default extended grapheme rules including the pinned release's
  required contextual state. Grapheme cursor progression preserves that state;
  slicing an arbitrary suffix must not forget the preceding context.
- [ ] NFC/NFD/NFKC/NFKD use the required decomposition, combining-class order
  and composition rules, including algorithmic cases. Bound work and scratch
  storage for adversarial combining runs; do not use an unbounded recursive
  decomposition or repeatedly copy long combining prefixes.
- [ ] Full default casefold is not lowercase. Unicode property APIs, trim,
  whitespace splitting and case conversion use compatible release data. If a
  Rust helper is reused, verify its Unicode version instead of assuming it
  matches. Runtime and static evaluation must share these semantics.
- [ ] Ordinary equality/search do not normalize/fold. Transformed offsets
  belong to transformed text. Grapheme reverse assembles reversed clusters,
  while scalar reverse intentionally may separate combining sequences.
- [ ] Run the pinned normalization/grapheme conformance vectors and specific
  casefold/property vectors. A few ASCII examples do not qualify as Unicode
  completion. Keep data generation/tests in this crate, not another workspace.

## 10. TX08: Materialize Typed Outcomes And Error Effects

Extend existing `etas_builtin/src/error.rs`, `value/data.rs` if needed, and
interpreter `intrinsic/pure/abi/{error,result,output,text}.rs`.

- [ ] Use a closed structured TextFailure taxonomy with a verified mapping to
  the seven checked TextError constructors. Reuse checked enum projection;
  neither a variant spelling alone nor value shape establishes nominal identity.
- [ ] Separate kernel value, declared text raise, and engine stop/fault outcomes.
  A parser/formatter's Err is itself a successful returned value. A byte slice
  failure instead enters the existing typed Error perform/handler continuation.
- [ ] Extend `etas_std/src/intrinsic/completion.rs` with accurate completion
  semantics for selected primitives. Checked text operations are
  ReturnsOrRaisesDeclaredErrors; Result/Option APIs return values. This does
  not declare operations immune to resource exhaustion or cancellation.
- [ ] Bridge errors at the real call span with checked error/action identity.
  Reuse the existing Error machinery; do not manufacture SourceId(0), string
  paths, IndexError payloads, unwrap or generic diagnostic-only failure instead
  of a catchable declared text error.
- [ ] `?` captures Error<TextError> only according to the existing rules.
  It does not unwrap Result, erase unrelated requested actions, or turn every
  native error into a source Result.
- [ ] UTF-8 decoding keeps its existing TextCodecError and explicit
  MalformedInput policy. Reject malformed strings before any normal string
  value exists; lossy replacement only occurs when explicitly selected.

## 11. TX09: Integrate Types Without A Second Checker

Modify the existing paths:

```text
etas_types/src/
  pipeline/signature/imports/std.rs
  pipeline/body/collect/{call,method_call,stmt,expr}.rs
  solver/{callable,method,iterable,index}.rs
  facts/symbols.rs
  lower/std.rs
```

- [ ] Standard signature imports populate actual enum, generic, method and
  iterable facts; ordinary calls solve normal Callable constraints.
- [ ] Existing Iterable constraints resolve Iter<T> through StdIterableFact.
  Char/tuple/string items are typed normally, including nested generic calls.
  Do not check callee names to infer a loop item.
- [ ] Preserve Index bounds and concrete integer types through generic solving
  and ABI. Do not add general usize coercions, unwrap nominal representations
  to borrow string APIs, or infer Iter as Array to satisfy expected types.
- [ ] Remove the String branch of `solver/index.rs::solve_index_access`.
  Direct string range slicing also stays unsupported. Accepted string access
  goes through chapter 19 functions; retain precise diagnostic suggestions.
- [ ] Update any body constraint collection and validation that pre-assumes
  string indexing. A rejected expression must not publish successful checked
  index facts. Do not preserve it behind a compatibility or fallback branch.
- [ ] Dot-method calls only use registered shared method facts, not automatic
  UFCS for every std.text export. Index syntax does not acquire a string spec
  impl merely because a text helper accepts Index arguments.
- [ ] Tests cover compile-time distinctions between Iter/Array/List,
  Option/Result/checked effect, nominal wrappers and all numeric widths.

## 12. TX10: Integrate Effect And Static String Analysis

Relevant files:

```text
etas_std/src/intrinsic/{completion,static_string}.rs
etas_effects/src/infer/semantics/{std_intrinsics,calls,static_string}.rs
etas_effects/src/pipeline/passes/build_registry.rs
etas_hir_analysis/src/static_string.rs
etas_frontend/src/passes/analysis/effect_check.rs
```

- [ ] Consume checked std contracts and actual ordinary-body summaries through
  the existing HIR abstract interpreter and effect pipeline. No new text pass
  duplicates inference; no name-based cleanup of an incorrect summary.
- [ ] Checked find_from/slices/range replacement/repeat carry Error<TextError>.
  Parsing/formatting/optional access do not manufacture escaping Error effects.
  Text iteration has no user callback or external Host action by itself.
- [ ] Preserve real Error attempts and handler/capture semantics separately
  from escaping rows. Do not invent Text/FileIO/Host requirements for pure text.
- [ ] Update std static-string descriptors for changed primitive identities.
  Replace independent trim/case implementations in HIR static evaluation with
  the shared pinned semantics through its existing oracle/helper boundary.
  Reuse an existing pure-kernel crate dependency where layering permits.
- [ ] The static evaluator remains conservative and bounded. Unsupported or
  too-costly evaluation yields its explicit unknown/error result, not an
  invented exact string, empty value or relaxed authority proof.
- [ ] Differential tests compare static-known text results with actual runtime
  results under the same compatibility descriptor. Changed Unicode semantics
  invalidate any cached exact-string/selector facts derived from them.

## 13. TX11: Adapt Checked Text Calls And Generic Iteration

Modify existing interpreter files, adding focused child modules if they grow:

```text
src/intrinsic/pure/{call,text}.rs
src/intrinsic/pure/abi/{borrowed,text,input,output,result,shape}.rs
src/intrinsic/dispatch.rs
src/plan/dispatch.rs
src/plan/dispatch/{checked_calls,checked_signatures}.rs
src/value/{mod,text,iteration,storage,codec}.rs
src/eval/{loop_control,index_access,continuation}.rs
```

- [ ] Bind operation, parameter/result types, iterable item type and error enum
  from checked facts during planning. Runtime never resolves methods/types from
  source spellings, guesses a TextError shape or scans std names for a handler.
- [ ] Reuse borrowed/shared string projections and existing StringValue COW.
  Do not clone all arguments into BuiltinValue to call an otherwise borrowed
  text primitive. Preserve concrete numeric and nominal ABI validation.
- [ ] Extend text output projection with validated sequence recipes, scalar,
  tuple and segment outcomes. Retain eager Array projection only for API results
  that actually have Array type.
- [ ] Extend generic IterationSource with a sequence/cursor arm. Refactor its
  implementation into `value/iteration/` if needed; keep a single loop/frame
  execution model shared with collections. `for` evaluates the input once and
  advances one element at a time.
- [ ] Remove the direct String arm in `eval/index_access.rs` and delete
  `scalar_at` plus its old string-index tests when no remaining legitimate
  caller exists. Migrate access tests to char_get/char_slice/byte_slice.
  Forged checked string-index facts must fail validation, not re-enable the API.
- [ ] `loop_control.rs` must not implement lazy progression by increasing an
  index and reconstructing/rescanning a sequence from zero. Save actual cursor
  progress before executing the loop body, so suspension does not consume twice.
- [ ] Strings remain immutable to aliases; segment copy/share policy is internal.
  No generic iterator-to-HostValue array conversion is added. An unsupported
  checked ABI is a precise internal/adapter failure, never empty iteration.

## 14. TX12: Bound Native Work And Preserve Recovery

Reuse existing scheduling, budgets, cancellation and snapshots:

```text
src/eval/safe_point.rs
src/eval/machine/{step,resume}.rs
src/control/{signal,continuation}.rs
src/orchestration/checkpoint.rs
src/eval/machine/snapshot/{value,value_capture,value_restore,continuation,validator}.rs
```

`control/signal.rs` already owns execution signals; extend that protocol instead
of creating a duplicate enum or a parallel text scheduler.

- [ ] Supply a narrow work-control port to native kernels: work polling,
  bounded output/scratch reservation and explicit stop reason. Core kernels do
  not own ExecutionBudget, HostContext or cancellation sources.
- [ ] Large scans, transforms, joining, formatting and Unicode operations yield
  at bounded work intervals, including a single huge scalar/grapheme run or
  search with no match. Checking only at the next Etas expression is insufficient.
- [ ] Keep resumable native work as owned typed state; borrows are recreated
  per step and never retained across scheduler yield. Integrate that state into
  the existing machine/continuation protocol, not recursive block_on or a new
  worker thread per text call. User cancellation is not a text error or retry.
- [ ] Snapshot lazy values, active cursors and any snapshot-visible pending
  native work using versioned discriminators and data. Preserve source,
  configuration, next-element phase, scan/assembly progress and checked item
  type. No Rust iterator pointers, native resources or collected remainder.
- [ ] Validate bytes/nodes/depth, indices, phase, Unicode version and operation
  consistency under current restore limits before use. Include cursor relation
  to retained source, not just `offset <= len`. Reject forged/incompatible state.
- [ ] Resume continues at the exact next logical operation; it cannot restart
  the loop/body, repeat completed Host actions or discard a half-scanned cursor.
  Advance checkpoint format when its schema changes, without hard-coding an
  obsolete next version from this checklist.
- [ ] Add same-executor cancellation, budget exhaustion and checkpoint/resume
  tests during large pure work. No partial immutable result is published on stop.

## 15. TX13: Version Artifacts And Package Contracts

Extend the existing compatibility chain rather than inventing a text cache:

```text
etas_std/src/{metadata/manifest,registry/module}.rs
etas_frontend/src/artifact/{fingerprint,kind}.rs
etas_frontend/src/session/snapshot/{type_environment,type_fingerprint}.rs
etas_frontend/src/package_metadata/projection.rs and typed projection children
etas_package_metadata/src/{model,container/mod,proto/schema,proto/codec}.rs
```

- [ ] Publish a canonical text compatibility descriptor: std/public contract,
  primitive ABI, source implementation and Unicode data/algorithm versions.
  Include target numeric-size/toolchain identity through existing environment
  compatibility where isize/usize or rendering depends on it.
- [ ] Incorporate it into cache dependencies, package requirements, frozen
  compilation environments and checkpoint verification. A changed descriptor
  invalidates affected facts; it must not silently hydrate old lines/ParseError
  contracts under new enum/iterator ABI.
- [ ] Export/replay Iter generic identity, enum variants, exact signatures and
  solved effect summaries through existing typed metadata. No string reparsing,
  fake empty external summaries or package-wide reanalysis as a hidden fallback.
- [ ] Incompatible disk compiler artifacts are explicit cache misses with safe
  recompilation according to existing cache policy. Incompatible execution
  checkpoints or malformed required package contracts are explicit rejections,
  not successful empty values. Preserve diagnostics and real source origins.
- [ ] Tests exercise fresh process/cache reuse, invalidation, external package
  re-export/alias use and unsupported old binary artifact versions.

## 16. TX14: Remove Old API And Implementation Paths

| Delete or replace | Required final state |
|---|---|
| Public `std.text.to_string_i32`, `to_string_usize` | Concrete `from_i32`, `from_usize` and the full `from_N` family; no compatibility exports |
| Text-only `std.text.ParseError` Support type | Real TextError enum; compiler ParseError remains unchanged |
| `TEXT_LINES`'s old eager contract | Lazy Iter result only; no Array/List hydration |
| Old `TextQuery::ParseI32` / Rust error strings | Shared typed parse kernel and checked enum ABI |
| `TextQuery::Len` and CountQuery scalar-length path | Byte length everywhere the admitted string len contract applies; separate char_count |
| `unary_integer_to_string` and narrowing `as` casts | Width-correct rendering for all numeric types |
| `TextTransform::Lines` / lines-to-TextOutput::Array | Shared lazy sequence recipe and per-consumer cursor |
| Direct string index solver/evaluator / scalar_at | Rejected direct syntax; explicit std access APIs |
| Combined join size/allocation -> NumericOverflow | Distinct representability/resource/allocator outcomes |
| Independent compile-time case/trim semantics | Same pinned semantics as runtime, or explicit unknown |

- [ ] Update `etas_std/src/intrinsic/pure.rs`, builtin
  `dispatch/{registry,call}.rs`, interpreter dispatch and static-string/completion
  tables together. The contains predicate cannot advertise a missing kernel.
- [ ] Retire the old numeric IDs for eager lines (currently 16), old formatters
  (19/20), and old parse ABI (21). Allocate non-conflicting new identities for
  changed operation contracts; never recycle retired numbers. Code constants
  for current operations may point to newly allocated IDs, but old encoded IDs
  cannot execute under the new meaning. Correct len implementation and text
  compatibility version together so old artifacts cannot preserve scalar len.
- [ ] Migrate `etas/crates/etas_tests/fixtures/compiler/support/algorithms.es`:
  use from_N; explicitly collect `lines` into Array where its helper promises
  Array; retain its local trimming/parsing convention as ordinary support code.
  Do not restore an eager std.lines to avoid editing this helper.
- [ ] Search all in-scope Rust tests, `.es` fixtures, goldens and examples for
  old imports/signatures. Update semantics and expected values, not only names.
  Preserve original assertions and negative coverage; add negative old-import
  tests proving removed exports cannot resolve.
- [ ] Inventory EDK/external consumers, report concrete required migrations to
  their owner and do not edit them or add compiler exceptions for their names.
  An obsolete external contract is not a reason to accept mismatched metadata.
- [ ] Avoid unrelated removal of compiler ParseError, std.option/std.result
  contracts, collection operations or type-system machinery.

## 17. TX15: Full Conformance And Regression Evidence

Use existing crates/harnesses; new tests may be focused files/folders inside
them. Update rather than delete old tests that encoded superseded semantics.

| Existing test location | Required additions/migration |
|---|---|
| `etas_std/tests/{std_registry,iterables,enum_constructors}.rs` | Full exported API/signature/variant matrix, retired names/IDs, registry consistency and compatibility fingerprints |
| `etas_builtin/src/text/{query,transform}/tests.rs`, `etas_builtin/tests/builtin.rs` | Byte len, typed parse/errors, no eager lines, complete family/width vectors |
| `etas_builtin/src/text/join.rs` and domain tests | Checked sizing, resource failure classification, non-copying assembly |
| `etas_builtin/tests/text/` and `tests/unicode_data/` | Per-domain edge cases, pinned conformance data, shared source/native reference vectors |
| `etas_types/tests/{types,std_iterables}.rs` | Real source calls, tuple iteration, Index bounds, no direct string index, nominal and type distinctions |
| `etas_effects/tests/effects.rs` and focused children | Error/Result/Option rows, capture semantics, no external text authority, static/runtime agreement |
| `etas_frontend/tests/` | Project/package/re-export use, source std body linkage, incremental invalidation and no duplicate modules |
| Interpreter `intrinsic/pure/*tests.rs` | Both regular/optimized paths, typed results and errors, full numeric widths and borrowed-input validation |
| Interpreter `testing/cases/{iteration,container_limits}.rs` and existing allocation harness | Lazy setup/early break, alias/cursor independence, linear work, limits and cancellation |
| Existing machine snapshot tests | Mid-loop/native-work recovery and forged cursor/version/type rejection |
| `etas_tests/fixtures/compiler/std_conformance/` | Existing syntax/hir/type/effect/interpreter positive and negative categories plus matrix.csv |

Minimum concrete behavior assertions include:

```text
len("é") == 2; char_count("é") == 1
char_get("é", 0) == Some('é'); char_get("é", 1) == None
byte_slice("é", [0, 2)) == "é"
byte_slice("é", [1, 2))? == Err(NotCharBoundary)
char_slice("é", [0, 1)) == "é"
split("ab", "") == ["", "a", "b", ""]
find_all("ab", "") yields (0,0), (1,1), (2,2), then ends
lines("") yields nothing; lines("a\r\n\n") yields "a", ""
replace("ab", "", "-") == "-a-b-"
parse_i32(" 42") == Err(ParseFailure)
parse_i32("2147483648") == Err(NumericOverflow)
from_u32(0xD800) == None
concat([]) == ""; join([], ",") == ""; repeat("", huge_n) == ""
```

These are semantic expectations, not copy-paste source syntax for undeclared
test helpers. Add actual checked source assertions and exact Error/Result enum
identity tests. Also cover every family/width, not just this sample list.

- [ ] Test different byte/scalar/grapheme counts, multibyte empty-pattern
  progress, LF/CRLF/CR, all range forms, invalid/too-wide endpoints and zero
  intervals. No UTF-8 rounding or accidental scalar/byte equivalence.
- [ ] Test format braces/indices, empty/repeated matches, absence, replacement,
  padding and enum matching. Include maximum/minimum widths and finite float
  round trips; the old wrong result must fail at least one focused regression.
- [ ] Test two sequential and nested consumers of the same Iter; suspend and
  resume in the body; checkpoint before/after empty matches and terminal lines.
- [ ] At multiple input sizes, measure bytes, allocations and scanned work;
  setup/first element cannot scale with all unvisited elements and sequential
  traversal/bulk assembly cannot repeatedly copy/rescan a growing prefix.
- [ ] Test cancellation on the same executor and budget failure during a large
  native operation, not only before invocation. Test that denied resource growth
  fails before full allocation. Checkpoint validation itself is bounded.
- [ ] Use independent expected values and official pinned Unicode vectors.
  Comparing a function with itself, only checking registry presence, hiding
  tests with ignore, accepting an unrelated unresolved import, or splitting a
  stress test to hide a crash is not valid evidence.

## 18. Completion And Delivery

Suggested implementation order: TX01/TX02/TX03, shared TX04/TX05,
TX06/TX07/TX08, consumer TX09/TX10/TX11, TX12/TX13, TX14 and full TX15.
Tests accompany each change; TX14 old-path cleanup is not deferred beyond
completion. All boxes remain required even if work is parallelized.

- [ ] Deliver a per-export coverage table: canonical path, exact signature,
  source/primitive implementation location, checked type/effect evidence,
  runtime and error-case test, laziness/limit evidence where applicable.
- [ ] Run relevant full suites in the existing core/frontend/interpreter/CLI
  workspaces, strict Clippy for changed crates, cargo fmt --check and git diff
  --check. Report exact commands/results and any environment-blocked tests.
  Do not delete pre-existing lockfiles or dirty changes to make status clean.
- [ ] Re-run existing algorithm/project/HTTP/agent fixtures affected by the
  shared text ABI and confirm original coverage remains. Live model/network
  access is unnecessary to prove pure text correctness; do not mark tests
  complete using fabricated external responses.
- [ ] Show old-entry-point search results and explain any retained occurrence
  (retired-ID constants, negative migration tests or historical docs only).
- [ ] No TODO implementation, success-shaped unsupported result, empty fallback,
  native-only mock surface, duplicated checker, private Host shortcut or
  unapproved API alias remains on the normal path.

Completion means every chapter 19 API works through a real checked source call,
with its specified values, errors, effects and applicable runtime guarantees.
An explicitly unsupported API is an honest intermediate state, not completion.
