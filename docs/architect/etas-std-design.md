# Etas Standard Library Design

Status: `Draft`

Owner: `Architect`

Last updated: `2026-09-10`

## 1. Purpose

This document defines the architecture of `etas_std`.

`etas_std` is the standard language support surface for Etas. It provides
standard declarations, signatures, intrinsic descriptors, module metadata, and
documentation metadata used by the compiler, LSP, CLI, AIR lowering, runtime,
host adapters, and tests.

It is not the runtime. It must not execute model calls, tools, memory reads or
writes, approvals, commands, checkpoints, or network operations.

The contents below are derived from the PL design documents in the main Etas
repository at `etas/docs/design/`,
especially:

- `02-general-programming-constructs.md`;
- `03-agents-tools-prompts-memory.md`;
- `04-flows-human-gates-and-protocols.md`;
- `05-type-system-and-errors.md`;
- `06-effect-system-and-inference.md`;
- `08-formal-core-static-analyses-and-pl-context.md`;
- `11-agent-intermediate-representation.md`;
- `12-application-boundaries-and-framework-coverage.md`.

## 2. Position In The Architecture

Standard library declarations participate in multiple phases:

```text
etas_std
  -> name resolution sees std modules and prelude symbols
  -> type checker sees type, flow, method, schema, and support contracts
  -> effect checker sees core roots, effect actions, action footprints, and limits
  -> lowering recognizes intrinsic and runtime support signatures
  -> runtime dispatches pure/runtime intrinsics through checked descriptors
  -> host implements default handlers and reusable host services
  -> LSP reads docs, completions, and signature metadata
```

Dependency direction:

```text
etas_std
  -> etas_core

etas_hir       -> etas_std
etas_types     -> etas_std
etas_effects   -> etas_std
etas_air       -> etas_std
etas_lowering  -> etas_std
etas_analysis  -> etas_std
etas_runtime   -> etas_std
etas_host      -> etas_std
etas_intel     -> etas_std
etas_driver    -> etas_std
etas_test      -> etas_std
```

`etas_std` should expose data descriptors and registries, not depend on the
compiler or runtime crates.

## 2.1 Terminology Boundary

Use these terms consistently:

```text
std declaration
  A standard type, function, effect, constructor, method, or support contract
  recorded in etas_std.

intrinsic descriptor
  Metadata in etas_std saying that a standard declaration is recognized by
  compiler, interpreter, runtime, or host layers. It includes an intrinsic id,
  dispatch kind, type/effect signature, and lowering/runtime hints.

builtin kernel
  A pure deterministic Rust implementation in etas_builtin. It implements only
  pure intrinsic ids.

runtime intrinsic
  Runtime/host-authority behavior implemented by etas-runtime or host layers,
  not by etas_std and not by etas_builtin.
```

`etas_std` declares and describes. It does not execute.

## 3. Crate Layout

Recommended file layout:

```text
crates/etas_std/
  Cargo.toml
  src/
    lib.rs

    registry/
      mod.rs
      builder.rs
      module.rs
      prelude.rs
      lookup.rs

    decl/
      mod.rs
      type_decl.rs
      effect_decl.rs
      spec_decl.rs
      flow_decl.rs
      tool_decl.rs
      impl_decl.rs
      action_decl.rs
      trace_spec_decl.rs

    modules/
      mod.rs

      core/
        mod.rs
        primitives.rs
        collections.rs
        option_result.rs
        text.rs
        bytes.rs
        json.rs
        math.rs

      agent/
        mod.rs
        prompt.rs
        schema.rs
        message.rs
        session.rs
        group.rs

      runtime/
        mod.rs
        approval.rs
        error.rs
        limits.rs
        checkpoint.rs
        trace.rs
        time.rs
        budget.rs

      security/
        mod.rs
        trust.rs
        declassify.rs
        trace_spec.rs

      host/
        mod.rs
        sandbox.rs
        command.rs
        path.rs
        url.rs

      net/
        mod.rs
        tcp.rs

      stream.rs
      tls.rs
      fs.rs

      codec/
        mod.rs
        text.rs

      http/
        mod.rs
        codec.rs

      secret.rs
      crypto.rs

      browser/
        mod.rs
        protocol.rs

    intrinsic/
      mod.rs
      id.rs
      signature.rs
      pure.rs
      runtime.rs
      lowering.rs

    metadata/
      mod.rs
      docs.rs
      completion.rs
      manifest.rs

    materialize/
      mod.rs
      source_stub.rs
      hir_stub.rs
      uri.rs
```

The layout is intentionally layered:

- `registry` builds and queries the standard module table.
- `decl` defines reusable declaration, action, spec, and trace-spec descriptors.
- `modules` owns the standard library surface grouped by domain.
- `intrinsic` describes compiler/runtime-recognized operations.
- `metadata` serves LSP, docs, CLI explanation, and package manifests.
- `materialize` derives virtual source and HIR views from the registry.

## 4. Registry Layer

`registry` should provide:

- `StdRegistry`;
- `StdModule`;
- `StdSymbol`;
- `StdPrelude`;
- lookup by qualified path;
- lookup by prelude name;
- lookup by intrinsic id;
- iteration for LSP completion and documentation.

The compiler should not hardcode every standard symbol in name resolution.
Instead, `etas_hir` should preload or query `StdRegistry`.

The registry should be deterministic and versioned so package metadata and AIR
artifacts can record which standard-library contract was used.

## 4.1 Virtual Standard Modules And Stubs

`StdRegistry` is the canonical source of standard-library truth. The project
must not treat generated `.es` text as the authoritative standard library.

However, compiler tooling still needs source-shaped views of standard modules.
`etas_std` should therefore support materializing read-only virtual modules
from the registry:

```text
StdRegistry
  -> VirtualStdModule
  -> VirtualStdSourceStub
  -> VirtualStdHirStub
```

The generated stubs are derived views:

- `VirtualStdSourceStub` is synthetic `.es` text for display, dumps,
  diagnostics, documentation, and LSP navigation.
- `VirtualStdHirStub` is a declaration-only HIR-shaped view for symbol trees,
  signature help, and uniform resolver/type-checker queries.
- Neither stub is allowed to execute behavior.
- Neither stub is allowed to become the canonical input for standard-library
  signatures, escaping effects, requested-action footprints, intrinsic ids, or
  docs.

Virtual standard modules should use stable virtual URIs:

```text
etas-std://std/io.es
etas-std://std/effects.es
etas-std://std/agent/prompt.es
```

The URI, registry version, module path, and symbol ids must be stable enough for
diagnostics, LSP definition locations, fixture goldens, and build artifact
metadata.

### 4.1.1 Source Stub Generation

Source stubs should be generated directly from registry descriptors. The
generator should render declaration signatures and metadata, not implementation
bodies:

```etas
module std.io;

public flow println(text: string) -> unit ![Error[IOError]]
    intrinsic("std.io.println");
```

The registry descriptor, not the source stub syntax, records the action
footprint:

```text
std.io.println:
  escaping_effects          = [Error[IOError]]
  requested_actions         = [Console.stdout_write]
  handled_requested_actions = [Console.stdout_write]
```

If a standard flow is implemented by runtime or host authority, the stub should
show the public escaping-effect contract and intrinsic/runtime boundary, not fake
an ordinary user-space body. For example, `std.io.println` may be documented as
requesting and internally handling `Console.stdout_write`, but the stub must not pretend
that the frontend can execute that write as normal source code. `RuntimeCall` on
a `std.io` intrinsic means "recognized by the execution engine and lowered to a
checked action/default-handler boundary"; it does not mean the interpreter or
runtime may bypass `etas_host`.

Source stub generation must preserve:

- module declaration;
- public/private export shape;
- type, effect, flow, tool, and support declarations;
- parameter names and source-facing type syntax;
- escaping-effect rows and requested-action metadata;
- intrinsic ids or symbolic intrinsic paths;
- documentation comments and completion metadata;
- stable generated spans within the virtual file.

Generated spans should map back to `(StdRegistryVersion, StdModuleId,
StdSymbolId, field)` so diagnostics can point at virtual source text while
still retaining semantic identity.

### 4.1.2 HIR Stub Generation

HIR stubs should be built from the same registry descriptors without re-parsing
the generated source stub. Re-parsing can be used in tests to validate that the
displayed source is syntactically valid, but normal compilation should not
depend on parser success to load the standard library.

HIR stubs are declaration-only:

- module symbols;
- exported symbol table;
- flow/type/effect/tool signatures;
- importable local names and prelude names;
- source-map entries pointing to virtual stub spans;
- stable links back to `StdModuleId`, `StdSymbolId`, and `StdIntrinsicId`.

HIR stubs should not contain executable bodies unless the declaration is an
ordinary pure standard combinator that is intentionally authored as source in a
future phase. Even then, the registry remains canonical for the public contract.

### 4.1.3 Consumers

Virtual stubs serve different tasks:

- LSP: go to definition, hover, completion details, document symbols, semantic
  tokens, and signature help for `std.*` names.
- CLI: `etas dump std`, `etas dump hir --include-std`, and human-readable
  explanation of standard declarations.
- Diagnostics: stable virtual locations for missing members, ambiguous imports,
  unsupported runtime-only intrinsics, and effect/action/policy explanations.
- Tests: golden snapshots of standard module surfaces and regression checks
  that registry descriptors materialize deterministically.
- Frontend internals: optional uniform symbol/export views for resolver and
  type/effect facts, while avoiding ad hoc hardcoded standard names.

The ordinary project parser pipeline should not load virtual standard source
stubs as if they were user files. Project compilation should resolve standard
modules through the standard module provider, then consume registry descriptors
or HIR stubs as semantic metadata.

## 5. Declaration Layer

`decl` should model standard declarations without depending on parser AST
nodes:

- type declarations;
- enum declarations;
- effect declarations;
- effect action signatures;
- flow signatures;
- method signatures;
- tool signatures;
- requirement constructors;
- intrinsic descriptors;
- documentation metadata.

The declaration model should be semantic and compact. It should not reuse
`etas_syntax::ast` as its storage format.

## 6. Standard Modules

### 6.1 `std.core`

Owns primitive and universally available support:

- primitive types:
  - `bool`;
  - `i8`, `i16`, `i32`, `i64`, `i128`, `isize`;
  - `u8`, `u16`, `u32`, `u64`, `u128`, `usize`;
  - `f32`, `f64`;
  - `char`;
  - `string`;
  - `bytes`;
  - `unit`;
  - `never`;
- generic dynamic value support:
  - `Value`;
- assertions and unrecoverable failure:
  - `assert(condition: bool) -> unit`;
  - `abort(message: string) -> never`;
- comparison and boolean helpers if needed by lowering;
- standard spec constraints:
  - `Index`;
  - `ByteStream`;
  - `PromptEncode[T]`;
  - `Schema[T]`;
  - `ResponseDecode[T]`;
  - `Limit`.

The compiler may treat primitive types specially internally, but their public
names and documentation still belong to `etas_std`.

These are source-visible spec constraints, but some impl sets are compiler- or
std-owned. `Index` is a compiler-owned marker spec used by checked indexing;
ordinary user code must not create arbitrary `Index` impls unless the SPEC later
allows user-defined index semantics. `ByteStream` is a marker spec implemented
by opaque stream handles such as `TcpStream`, `TlsStream`, `FileStream`, and
`BrowserStream`. `PromptEncode`, `Schema`, and `ResponseDecode` are behavioral
or derivable specs used by prompt construction and structured model output.
`etas_std` declares their public metadata so diagnostics, LSP, dumps, type
checking, and package metadata can explain them, while `etas_types` owns
satisfaction and impl-coherence checking.

### 6.2 `std.collections`

Owns generic containers:

- `Array[T]`;
- `List[T]`;
- `Map[K, V]`;
- `Set[T]`;
- `Range[I]`;
- `Slice[T]`;
- `Deque[T]`;
- `Queue[T]`;
- `Stack[T]`;
- `PriorityQueue[T, P]`;
- `OrderedMap[K, V]`;
- `OrderedSet[T]`;
- collection methods such as:
  - `len`;
  - `is_empty`;
  - checked indexing helpers such as `at`;
  - slicing helpers where applicable;
  - `push`;
  - `get`;
  - `contains`;
  - `contains_key` for `Map[K, V]`;
  - `keys` and `values` for `Map[K, V]`;
  - `map`;
  - `filter`;
  - `fold`.

Implementation may land in phases, but the declaration model should describe
the complete standard collection surface expected by the PL design. Unsupported
operations should be rejected by the relevant execution layer, not omitted from
the shared registry shape.

The standard declarations must preserve the language-level distinction between
sequence families:

- `Array[T]` is the default finite sequence literal type for `[a, b, c]` and the
  process entry argument type `Array[string]`.
- `List[T]` is the persistent/cons-oriented sequence type for `[a; b; c]` and
  `head :: tail`.
- `Slice[T]` is a view-like sequence result from slicing `Array[T]` or
  `Slice[T]`.
- `Range[I]` is an interval over an index-compatible integer type with explicit
  bound inclusivity.

`[]` and `{}` are not std constructors. They are syntax forms resolved by
`etas_types` using expected type. The std registry declares the target types
and their methods; it must not encode a default rule such as "empty sequence is
List".

`List[T].join(separator)` should not be declared as a fully generic helper until
the language has an explicit display/string-conversion spec such as `Display`
or `Format`.
Recommended standard surface:

- `std.text.join(parts: Array[string], separator: string) -> string`;
- `std.text.join_list(parts: List[string], separator: string) -> string` if list
  support needs a distinct overload before overload resolution is stable;
- optionally `Array[string].join(separator: string)` and
  `List[string].join(separator: string)` as method sugar;
- no implicit `Array[T]` or `List[T]` to string conversion for arbitrary `T`.

Algorithm fixtures that need to print `Array[i32]` or `List[i32]` should keep a
support helper such as `join_i32_array` / `join_i32_list` or explicitly map each
integer with `std.text.to_string_i32` before joining.

This text-joining helper is unrelated to the workflow combinator
`join([() => ..., () => ...])`. They may share an English name in source-facing
APIs, but their declarations, types, effects, and lowering paths must remain
separate.

`Store[K, V]` is not a basic in-memory collection. It belongs to typed
persistent-memory support and must be declared by the standard registry as a
runtime support type with read/write method signatures and region-sensitive
effects.

`MemoryRegion[S]` is also a standard support type. A source program declares
transparent schema abbreviations with `alias` when it wants no new type
identity, and binds immutable resource handles with top-level `let`:

```etas
alias ProjectMemorySchema = MemoryRegion[{
    Papers: Store[PaperId, PaperRecord],
}];

let ProjectMemory =
    std.memory.region[ProjectMemorySchema](
        stable_id = "project_memory",
        store = "project-main"
    );
```

If a program writes `type ProjectMemorySchema = MemoryRegion[...]`, that is a
nominal schema wrapper under the current SPEC and must not be treated as the
same type as `MemoryRegion[...]` by default. Standard declarations and fixtures
should use `alias` for pure abbreviations and reserve `type` for identities,
markers, and opaque handles.

`etas_std` owns the signatures and intrinsic ids for
`std.memory.region[...]`, `MemoryRegion[S]`, `Store[K, V]`, and store
operations. Execution belongs to the interpreter/runtime host layers.

### 6.2.1 Persistent Storage Contracts

The accepted [Storage Design](../../../etas-core/docs/architect/etas-storage-design.md)
defines the source API target. Keep existing `std.memory.get_entry` and bounded
`page` values. Add `prepare_put`, `prepare_delete`, `commit` and `reconcile` in
that module, rather than another versioned-read API or a generic storage crate.

`MemoryWriteIntent<K,V>` is an opaque immutable nominal value containing a typed
mutation, condition and runtime-issued operation reference. Provide a read-only
operation-reference accessor. Preparation does not mutate storage; it allocates
identity before dispatch and must not be classified as a deterministic pure
builtin. Intents/references support checked lossless checkpoint serialization,
not arbitrary source construction or serialized authority grants.

`commit` returns `WriteOutcome<MemoryWriteReceipt<K>, MemoryWriteRejection>`:
confirmed commit, confirmed non-commit with reason, or unknown with an operation
reference. `reconcile` returns a confirmed recorded outcome, unresolved, or
expired evidence and never resubmits a mutation. All outcomes use closed ADTs
whose variants are available to ordinary pattern matching. Preserve the
operation reference on uncertain convenience writes as well.

Declare scoped `Memory.write` for commit and scoped `Memory.read` for lookup,
reads and paging. Separate requested actions from escaping effects after wrapper
handling. Typed Error effects report validation/permission/lookup failures;
`Unknown` remains an inspectable result, not an Error captured or erased by `?`.
Existing convenience APIs may preserve their Error contract only if uncertain
write errors carry the recoverable operation reference.

These names and semantics are architecture-approved, not yet source-SPEC
declarations. Synchronize exact generic types, variant fields, signatures and
Error rows with the language designer before registration. Update declarations,
generated stubs, nominal identity, type/effect facts, package metadata and engine
codecs together. No special parser construct is needed. Acceptance requires
source-level matching, CAS conflict, unknown/reconcile and checkpoint/restore
tests through a real SQLite backend, not only Rust protocol tests.

### 6.3 `std.option`

Owns:

- `Option[T]`;
- constructors:
  - `Some(T)`;
  - `None`;
- common methods:
  - `is_some`;
  - `is_none`;
  - `unwrap_or`;
  - `map`;
  - `and_then`.

### 6.4 `std.result`

Owns:

- `Result[T, E]`;
- constructors:
  - `Ok(T)`;
  - `Err(E)`;
- helpers:
  - `is_ok`;
  - `is_err`;
  - `map`;
  - `map_err`;
  - `and_then`;
  - `unwrap_or`.

The `?` operator is language syntax/effect behavior, not a standard-library
function. It captures a typed `Error[E]` effect from its operand and converts
the value boundary to `Result[T, E]`:

```text
e  : T ! [Error[E], ...]
e? : Result[T, E] ! [...]
```

It must not be declared as an intrinsic, a helper flow, an `unwrap`, or a
Rust-style `Result` propagation operator.

The operator is only postfix and may be applied to any expression, including a
block expression. A semicolon terminates the surrounding statement, not `?`
itself:

```etas
let line = std.io.read_line()?;
{
    let text = fs.read(path);
    parse_report(text)
}?
```

### 6.4.1 `std.io` Error Boundary And Action Footprint

`std.io` exposes ordinary standard flows whose public signatures carry only
escaping typed errors. Console writes and reads are standard effect actions with
default handlers, recorded as requested-action metadata:

```text
std.io.read_line : unit -> string ! [Error[IOError]]
  requested_actions = [Console.stdin_read_line]

std.io.read_all  : unit -> string ! [Error[IOError]]
  requested_actions = [Console.stdin_read_all]

std.io.print     : string -> unit ! [Error[IOError]]
  requested_actions = [Console.stdout_write]

std.io.println   : string -> unit ! [Error[IOError]]
  requested_actions = [Console.stdout_write]

std.io.eprintln  : string -> unit ! [Error[IOError]]
  requested_actions = [Console.stderr_write]
```

`IOError` is a standard error type for process-standard-stream failures. Code
that wants value-level failure writes `std.io.read_line()?` and receives
`Result[string, IOError]`. The captured error is removed from
`escaping_effects`, but the requested action remains visible to trace-spec
checks, deployment grants, trace, and runtime mediation. The registry must not declare
`read_line` or `read_all` as returning `Result[string, _]`; that would turn `?`
into Rust-style value unwrapping and contradict the PL SPEC.

### 6.5 `std.text`

Owns deterministic text helpers used by examples and prompt builders:

- `len(string) -> usize`;
- `trim(string) -> string`;
- `lowercase(string) -> string`;
- `uppercase(string) -> string`;
- `contains(string, string) -> bool`;
- `lines(string) -> List[string]`;
- `split(string, separator: string) -> List[string]`;
- `join(parts: List[string], separator: string) -> string`;
- `starts_with`;
- `ends_with`;
- narrow formatting/display helpers such as `to_string_i32`, `to_string_i64`,
  `to_string_usize`, `to_string_bool`, and similar primitive conversions.

`toString` must not be the prompt serialization protocol. Prompt construction
uses `PromptEncode[T]`. Broad generic `to_string[T]` should wait until the
language has an explicit display/formatting spec. Phase 1 should
prefer concrete primitive conversion helpers or method sugar that resolves to
those concrete helpers.

Basic text parsing may be standard pure functionality:

- `parse_i32(string) -> Result[i32, ParseError]`;
- equivalent parse helpers for other concrete numeric widths where the standard
  surface needs them.

Fixture-specific formats should not become core standard-library functions.
`parse_i32_list(input: string) -> List[i32]`, for example, encodes an algorithm
fixture convention such as "trim, split on whitespace, parse every field, and
unwrap/report test failure". It should live in `tests.compiler.support`
fixtures or ordinary user code built from `trim`, `split`, and `parse_i32`.

### 6.6 `std.bytes`

Owns byte sequence support:

- `bytes`;
- conversion and length helpers;
- byte-safe slicing helpers as needed.

`std.bytes` is not the text codec layer. It owns byte container helpers. The
explicit byte/text boundary belongs to `std.codec.text`, where malformed input
behavior and typed codec errors are part of the signature.

### 6.7 `std.json`

Owns structured JSON support:

- `JsonValue` or `Value` integration;
- `json.parse`;
- `json.stringify`;
- schema-aware encode/decode hooks where appropriate.

JSON support must preserve structured data where possible. It should not become
the default representation for prompts or model output.

### 6.8 `std.math`

Owns deterministic numeric helpers and constructors as needed.

Money helpers such as `usd(2.00)` are better placed under runtime/budget
support because they are used by cost limits, but their underlying numeric
helpers may live here.

### 6.9 EDK-Facing Standard Substrate Modules

The language SPEC now accepts a low-level standard substrate required by EDK
default handlers and realistic local programs. These modules are still standard
library declarations, not high-level integration packages:

| Module | Action owner | Role |
|---|---|---|
| `std.net.tcp` | `Net extends Network` | TCP connect, stream identity, timeout and cancellation metadata |
| `std.stream` | `Stream` | Bounded byte-stream read, write, flush, close, EOF, timeout, and cancellation over typed stream handles |
| `std.tls` | `Tls extends Network` | TLS client session setup, server-name binding, certificate validation, TLS errors |
| `std.fs` | `Fs extends FileIO` | Project-root-scoped read, write, list, stat, canonicalize, and atomic replace |
| `std.http.codec` | none | Deterministic wire-level HTTP request/response encoding and parsing |
| `std.codec.text` | none | UTF-8 and charset encode/decode with explicit malformed-input behavior |
| `std.secret` | `Secret` | Opaque secret key/value reads and redaction-safe secret-backed operations |
| `std.crypto` | none for public deterministic operations; `Secret.use[K]` for secret-backed operations | Hashes, HMAC/signatures over opaque secrets, digest encoding, constant-time comparison |
| `std.browser.protocol` | `Browser extends Network` | Browser session attach/create, protocol transport, event receive, screenshot bytes, and session/origin binding |

Naming is deliberately split:

- source API paths are lowercase, such as `std.net.tcp.connect`;
- effect/action owners are uppercase and have no `Std` prefix, such as
  `Net.tcp_connect`, `Stream.read`, `Tls.handshake`, `Fs.write`, and
  `Browser.send`;
- high-level EDK package actions remain package-owned, such as
  `EdkHttp.request`, `EdkWorkspace.write`, and `EdkBrowser.navigate`.

Representative standard substrate declarations:

```text
std.net.tcp.connect(host: Host, port: Port, options: TcpOptions)
  -> TcpStream ![Net.tcp_connect[host, port], Error[NetworkError]]

std.stream.read[S ~ ByteStream](stream: S, max_bytes: usize, timeout: Timeout?)
  -> StreamRead ![Stream.read[stream], Error[StreamError]]

std.stream.read_until_limit[S ~ ByteStream](stream: S, limit: ByteLimit, timeout: Timeout?)
  -> bytes ![Stream.read[stream], Error[StreamError]]

std.stream.write_all[S ~ ByteStream](stream: S, body: bytes)
  -> unit ![Stream.write[stream], Error[StreamError]]

std.stream.flush[S ~ ByteStream](stream: S)
  -> unit ![Stream.flush[stream], Error[StreamError]]

std.stream.close[S ~ ByteStream](stream: S)
  -> unit ![Stream.close[stream], Error[StreamError]]

std.tls.connect(stream: TcpStream, server_name: Host, config: TlsConfig)
  -> TlsStream ![Tls.handshake[server_name], Error[TlsError]]

std.fs.read_bytes(path: WorkspacePath)
  -> bytes ![Fs.read[path], Error[IOError]]

std.fs.write_bytes(path: WorkspacePath, body: bytes)
  -> unit ![Fs.write[path], Error[IOError]]

std.secret.read[K](key: SecretKey[K])
  -> SecretValue[K] ![Secret.read[K], Error[SecretError]]

std.crypto.hmac_sha256[K](key: SecretValue[K], body: bytes)
  -> Digest ![Secret.use[K], Error[CryptoError]]
```

`ByteStream` in these signatures is a spec bound, not a concrete supertype.
`std.tls.connect` returns `TlsStream`; `std.stream.read_until_limit` accepts that
value because the standard registry contains spec evidence
`impl TlsStream ~ ByteStream`. Compatibility metadata may accept older
`impl ByteStream for TlsStream` source spelling, but the canonical evidence fact
is still `TlsStream ~ ByteStream`. The frontend must not require a subtype
conversion from `TlsStream` to a concrete `ByteStream` type.

Pure substrate helpers must not introduce runtime effects:

```text
std.http.codec.encode_request(req: HttpWireRequest) -> bytes
std.http.codec.decode_response_head(bytes: bytes) -> Result[HttpWireResponseHead, HttpCodecError]

std.codec.text.utf8_decode(body: bytes, malformed: MalformedInput)
  -> Result[string, TextCodecError]
std.codec.text.utf8_encode(text: string) -> bytes

std.crypto.sha256(body: bytes) -> Digest
std.crypto.constant_time_eq(a: bytes, b: bytes) -> bool
```

`HttpWireRequest`, `HttpWireResponseHead`, header blocks, transfer-encoding
state, and body-limit values are std-owned wire-level types. They are not EDK's
high-level `edk.http.HttpRequest` or `edk.http.HttpResponse` records. EDK must
translate explicitly between user-facing HTTP records and these wire-level codec
types.

`StreamRead` distinguishes `Data(bytes)` from ordinary `Eof`. EOF is not an
error; timeout, cancellation, closed stream, interruption, body-limit overflow,
and host failure are typed `StreamError` failures. `Stream` is origin-indexed:
typed stream handles must carry provenance from the action that created them so
policy can still cover TCP/TLS streams through `Network` and future file streams
through `FileIO`.

`SecretValue[K]` is opaque and redaction-safe. Pure code cannot reveal its bytes.
Secret-backed operations such as HMAC are standard APIs, but they are effectful
through `Secret.use[K]`; public deterministic crypto over non-secret `bytes`,
such as hashing and constant-time comparison, remains pure.

`std.browser.protocol` is a standard substrate effect extending `Network`, not a
high-level browser automation API. It exposes narrow actions such as
`Browser.attach[profile]`, `Browser.send[session]`, `Browser.recv[session]`,
`Browser.screenshot[session]`, and `Browser.close[session]`. Page navigation,
clicking, DOM reading, and workflow-level browser behavior belong in EDK or user
packages.

`std.fs` is low-level substrate. Project-oriented ergonomic APIs such as
`edk.workspace.files.read`, `write_text`, `list`, and path-scoped policy
templates remain EDK package APIs. `std.fs` exists so EDK can implement those
APIs over a public, policy-visible runtime boundary instead of a private host
escape hatch.

## 7. Agent Support Modules

Agent support modules define the typed values used by `agent` declarations and
agent calls. They do not execute model calls. A source `agent` body constructs a
`Prompt`; the runtime later performs model inference, tool mediation, schema
validation, retries, and trace emission.

The standard library must keep these SPEC distinctions explicit:

- `Prompt` is the typed model-call input package;
- `PromptPart` and `PromptEncode[T]` describe how structured values enter
  prompt channels while preserving trust, provenance, and schema metadata;
- `Message[T]` is a typed communication/session value, not a prompt and not a
  source declaration;
- `SessionConfig` and `Conversation` are runtime support values used by stage
  options and runtime planning, not language keywords;
- model names, tools, limits, requirements, trace labels, and retention policy
  are typed config-row values, not syntax-specific declarations.

### 7.1 `std.agent.prompt`

Owns:

- `Prompt`;
- `PromptPart`;
- `PromptEncode[T]`;
- `Role`;
- `Prompt.new()`;
- `Prompt.system(...)`;
- `Prompt.user(...)`;
- `Prompt.assistant(...)`;
- `Prompt.data(...)`;
- `PromptPart.data(...)`;
- prompt channel metadata.

Prompt APIs must preserve trust labels, provenance, channel placement, and
schema information. `Prompt.system(x)` requires trusted instruction content.
`Prompt.user(x)` and `Prompt.data(x)` may accept untrusted data with
provenance.

Default `PromptEncode` support should be derivable for:

- primitive types;
- records;
- enums;
- `Array[T]`;
- `List[T]`;
- `Map[K, V]`;
- `Set[T]`;
- `Range[I]`;
- `Slice[T]`;
- `Option[T]`;
- `Message[T]`;
- `Trusted[T]`;
- `Untrusted[T]`;
- `Sanitized[T]`;
- `Public[T]`.

`Secret[T]` must not be prompt-encodable by default.

### 7.2 `std.agent.schema`

Owns:

- `Schema[T]`;
- `ResponseDecode[T]`;
- schema derivation metadata for primitives, records, enums, lists, maps, and
  options;
- schema validation support descriptors;
- model response decoding descriptors.

Runtime validation may lower to AIR nodes such as `ValidateSchema`.
`etas_std` describes the contract; runtime and AIR execute it.

### 7.3 `std.agent.message`

Owns typed communication support:

- `Message[T]`;
- `MessageId`;
- `Participant`;
- `AgentId`;
- `TraceId`;
- `Provenance`;
- `Role`;
- `Message.new(payload)`;
- `Message.cast[T]()` where safe and checked by type/protocol rules.

`Message[T]` is the semantic communication value for multi-agent systems.
It is not a source keyword.

### 7.4 `std.agent.session`

The current [Session SPEC](../../../etas/docs/design/03-agents-tools-prompts-memory.md#33-sessions-and-conversations)
defines this data substrate, not an automatic summarization service:

- `SessionId`, `SessionConfig { id, context, retention }`, typed messages and `Conversation`;
- atomic append/deduplication and bounded history/context pages;
- opaque session/history revision fences and expected context versions;
- `history_page` returning bounded history, cursor, `SessionHistoryFence` and optional published context;
- `prepare_context` binding caller-produced `SessionContextContent` and its fence to a `StorageOperationRef` before dispatch;
- `publish_context(session, fence, content, operation)` returning structured commit certainty and a `SessionContextReceipt` on commit;
- query-only `reconcile_context` using the same operation reference under current authority;
- execution of configured bounded retention/storage maintenance, without choosing what users should keep.

Publication accepts already-produced content. Its transaction validates history
and context versions, including concurrent appends, then stores content and
receipt atomically. Matching receipt replay returns the original outcome before
checking current versions. No model call, tokenization or implicit deletion is
part of publication. The detailed algorithm and bounds are in the
[Session contract](../../../etas-core/docs/architect/etas-storage-design.md#7-session-semantics).

`prepare_context` must bind content and provenance as well as target/fence;
changed requests under one operation reference are rejected. Receipts identify
the operation, session generation, published context version and actual
backend-confirmed durability. Context content preserves source/producer
provenance; publication does not certify accuracy or promote it to trusted
instruction content. Std types and codecs must retain these distinctions.

EDK/application flows own summary thresholds, tokenizer/model/provider selection,
prompts, validation and retry. They read history, call ordinary checked APIs and
conditionally publish the result. Etas still enforces cancellation, budgets,
authority and action trace on those calls. `LastTurns` selects bounded data;
`SummaryPlusRecent` selects existing context plus recent turns. If no summary
exists, it selects recent turns only with visible absence, never fabricated
summary data. Neither selection claims full history or deletes messages.
`ContextTokens` remains a budget bound: failure follows limit semantics rather
than causing a hidden inference call to shrink the context.

Remove implicit `CompactionPolicy`/`SummarizeWhen` model orchestration and its
configuration requirement; this removal is now backed by the current SPEC.
Delete `SessionConfig.compaction` from declarations/stubs and reject old options
explicitly. Keep retention, archival, deletion and storage compaction as separate
runtime operations, with provenance and replay limitations preserved. Session
operations stay in the existing
`std.agent.session` namespace, with no parallel `std.session` forwarding module.
They are ordinary APIs, not new keywords or a second meaning of `with`.

### 7.5 `std.agent.group`

Owns higher-level group and team combinators:

- `group.round_robin(...)`;
- future group-chat stop conditions such as `mentions("FINAL")`;
- quorum or voting helpers later.

These should lower to ordinary flows, loops, limits, memory access, and agent
calls. They should not require special source syntax.

Implementation can be scheduled later, but the module should be reserved
because the PL design identifies group chat combinators as a standard-library
direction.

## 8. Runtime Support Modules

### 8.1 `std.runtime.approval`

Owns:

- core effect `Approval`;
- effect action:
  - `Approval.request(req: ApprovalRequest) -> ApprovalDecision`;
- support types:
  - `ApprovalRequest`;
  - `ApprovalDecision`;
  - `Risk`;
- support flow:
  - `approve[T](title: string, content: T, risk: Risk) -> bool`
    with `![Approval]`.

`approve(...)` is parsed as an ordinary call. Typed lowering recognizes the
standard signature and lowers it to an approval AIR/runtime boundary.

### 8.2 `std.runtime.error`

Owns:

- core effect `Error[E]`;
- effect action:
  - `Error[E].raise(err: E) -> never`;
- standard runtime error kinds:
  - `SchemaError`;
  - `ValidationError`;
  - `ToolTimeout`;
  - `ToolError`;
  - `ToolDenied`;
  - `PolicyViolation`;
  - `PolicyDenied`;
  - `EffectBoundaryViolation`;
  - `SandboxViolation`;
  - `PromptInjectionRisk`;
  - `MissingCitation`;
  - `BudgetExceeded`;
  - `ProtocolViolation`;
  - `HumanRejected`.

Application-specific error types remain user declarations.

### 8.3 `std.runtime.limits`

Owns typed loop/retry/budget limit constructors:

- `Iterations(n)`;
- `Tokens(n)`;
- `ContextTokens(n)`;
- `Cost(money)`;
- `WallTime(duration)`;
- `Attempts(n)`;
- `LimitSet`.

Limits are typed runtime support values, not keywords. The syntax has `limit`
clauses, but the dimensions and constructors live in standard support.

### 8.4 `std.runtime.budget`

Owns:

- `Money`;
- `usd(amount)`;
- cost and budget helper types;
- token/context/cost/time budget descriptors.

Runtime owns actual budget accounting.

### 8.5 `std.runtime.time`

Owns:

- `Time`;
- `Duration`;
- duration constructors such as `seconds`, `minutes`, `hours`, and `days`;
- time-reading support signatures carrying `![Time]`.

Actual clock access is mediated by runtime/host.

### 8.6 `std.runtime.checkpoint`

Owns:

- `runtime.checkpoint(state)`;
- checkpoint label/state descriptor types;
- checkpoint metadata signatures.

The design documents state that checkpointing is an ordinary standard-library
call such as `runtime.checkpoint(state)`, not a core expression form. Runtime
may also insert automatic checkpoint boundaries.

### 8.7 `std.runtime.trace`

Owns trace-facing types:

- `TraceId`;
- trace label metadata;
- trace redaction descriptors;
- support types for trace export metadata.

Runtime owns trace event emission and persistence.

## 9. Security And Trace-Spec Support Modules

### 9.1 `std.security.trust`

Owns trust and provenance wrappers:

- `Trusted[T]`;
- `Untrusted[T]`;
- `Secret[T]`;
- `Public[T]`;
- `Sanitized[T]`.

These are type-level support wrappers used by prompt taint analysis, schema
checking, trace redaction, and trace-spec/runtime authority checks.

### 9.2 `std.security.declassify`

Owns support signatures for explicit declassification and sanitization:

- `sanitize(...)`;
- `declassify(secret, rule = ...)`;
- redaction/declassification rule hooks.

Actual safety rules are checked by `etas_analysis` and enforced by runtime
authority/trace-spec policy. `etas_std` only declares the support surface.

### 9.3 `std.security.trace_spec`

Owns documentation and descriptor support for trace-spec action patterns. These
are not source-level policy declarations and do not introduce a `policy`
keyword:

- `ActionPattern`;
- `TraceSpecClause`;
- `TraceSpecMonitor`;
- trust/prompt targets such as `PromptSystemWrite`;
- review gates such as `HumanReview`;
- publish targets such as `PublicPublish`.

Trace specs use source syntax such as `spec Safe: trace = +A & -B;`.
`etas_std` declares support value shapes and documentation metadata only;
`etas_effects` materializes trace-spec facts, and interpreter/runtime layers
enforce concrete admission decisions.

### 9.4 No `std.host.capability`

The latest PL SPEC removes source-level `Capability`. `etas_std` must not
declare `Capability(name: string)`, `RequirementKind::Capability`, or prelude
capability symbols.

Host/runtime authority is represented outside source syntax as deployment or
host grants over checked action patterns:

```text
grant Console.stdout_write
grant Fs.write["reports/**"]
grant EdkWorkspace.write["reports/**"]
grant Memory.read[ProjectMemory.Papers]
```

Those grants are consumed by interpreter/runtime/host mediation. They are not
standard-library source declarations and must not be imported by user programs.

### 9.5 `std.host.sandbox`

Owns command sandbox support:

- `SandboxProfile`;
- `Sandbox(profile)`;
- `DefaultCommandSandbox`;
- `AccessMode`;
- `PathPattern`.

Command execution is represented as `Command.run[S]`. The sandbox profile `S` is
part of the action instance and is checked by policy, deployment grants, and
host sandbox mediation. If no explicit sandbox profile is supplied by a standard
wrapper, the wrapper uses `DefaultCommandSandbox`.

### 9.6 `std.host.command`

Owns command support types:

- `Command`;
- `ShellResult`;
- command output/status descriptors.

`Command` itself is a typed value. Executing it is a tool/runtime operation
through `Command.run[S]`, with action-boundary checks, sandbox checks, policy
checks, limit checks, and trace emission.

### 9.7 `std.host.path` And `std.host.url`

Own common host-facing scalar types:

- `Path`;
- `PathPattern`;
- `Url`.

These are used in tools, memory-backed workflows, examples, and runtime support
contracts.

## 10. Core Effects

`etas_std` should declare the built-in effect roots:

- `Agentic`;
- `Network`;
- `FileIO`;
- `Command`;
- `Memory`;
- `Secret`;
- `Time`;
- `Human`;
- `Error[E]`.

`etas_std` must declare only the minimal standard effect/action surface from the
current Language Design SPEC. The registry still owns the core roots above, but
domain/platform operations belong to packages, host bindings, or project
metadata.

`etas_std` should declare these standard effect tags, actions, and default
handler metadata:

| Root / effect | SPEC relation | Minimal standard actions |
|---|---|---|
| `Console` | `FileIO` | `stdin_read_line`, `stdin_read_all`, `stdout_write`, `stderr_write` |
| `Approval` | `Human` | `request` |
| `Clock` | `Time` | `now`, `sleep` |
| `Memory` | core root | `read[R]`, `write[R]` |
| `Secret` | core root | `read[K]`, `use[K]` |
| `Command` | core root | `run[S]` |
| `Agentic` | core root | `infer[A]` |
| `Error[E]` | core root | `raise` |

The accepted low-level substrate adds standard tags such as `Net`, `Stream`,
`Tls`, `Fs`, and `Browser`. These are standard runtime boundaries, but they are
not high-level integration APIs. They exist so EDK and user packages can build
real implementations over public checked actions:

| Standard substrate tag | Relation | Representative actions |
|---|---|---|
| `Net` | `Network` | `tcp_connect[host, port]` |
| `Stream` | origin-indexed runtime byte stream support | `read[stream]`, `write[stream]`, `flush[stream]`, `close[stream]` |
| `Tls` | `Network` | `handshake[server_name]` |
| `Fs` | `FileIO` | `read[path]`, `write[path]`, `list[path]`, `stat[path]`, `atomic_replace[path]` |
| `Browser` | `Network` | `attach[profile]`, `send[session]`, `recv[session]`, `screenshot[session]`, `close[session]` |

HTTP clients, web search, workspace convenience APIs, databases, vector indexes,
browser page operations, email, calendars, queues, object stores, payments,
identity, deployment, package registries, observability, and business APIs are
still not core `std` vocabulary. Libraries and applications may declare tags
and actions such as `EdkHttp`, `EdkWorkspace`, `CompanyEmail`, or `Web.search`
that extend the core roots, but those actions must come from source
declarations, package metadata, host/project bindings, or precompiled package
items. User code should normally reach those signatures through `import`; a
bodyless tool that cannot be resolved to a std/package/precompiled provider and
runtime binding is invalid. The source language has no `extern` keyword.

Every standard action descriptor must include:

- owner tag and extension root;
- action name and parameter/result types;
- parameterized action arguments used by effect rows and trace-spec patterns;
- default-handler id when the runtime/std host supplies one;
- whether the action is resumable;
- whether the action returns `never`;
- host mediation kind used by interpreter/runtime readiness;
- documentation metadata for CLI/LSP explanation.

Standard flow descriptors must not collapse public escaping effects and action
footprints into one `effects` list. The target descriptor shape is:

```rust
pub struct FlowDecl {
    pub name: String,
    pub params: Vec<StdType>,
    pub output: StdType,
    pub public_effects: Vec<String>,
    pub requested_actions: Vec<String>,
}
```

`public_effects` feeds source-facing signatures and public effect contracts.
`requested_actions` feeds `etas_effects` action facts, trace-spec monitors,
deployment grants, trace, replay, and interpreter/runtime host mediation.

For example:

```text
std.io.println:
  public_effects    = [Error[IOError]]
  requested_actions = [Console.stdout_write]
```

The old shape where a standard flow declares
`[Console.stdout_write, Error[IOError]]` in one public effect list is obsolete.
Default-handler metadata belongs beside the requested action descriptor, not in
the source-facing escaping row.

## 11. Intrinsic Layer

The intrinsic layer describes operations recognized by compiler/lowering/runtime.

Intrinsic classes:

1. Pure deterministic intrinsics:
   - primitive arithmetic and comparison;
   - string/text helpers;
   - collection helpers;
   - option/result helpers;
   - schema derivation metadata queries.
2. Runtime intrinsics:
   - `approve`;
   - `runtime.checkpoint`;
   - `current_session`;
   - `std.io.read_all`;
   - `std.io.read_line`;
   - `std.io.print`;
   - `std.io.println`;
   - `std.io.eprintln`;
   - budget/limit construction;
   - trace metadata construction.
3. Host-backed support:
   - console/std-stream request signatures;
   - command execution signatures;
   - sandbox descriptors;
   - standard substrate signatures for `std.net.tcp`, `std.stream`, `std.tls`,
     `std.fs`, `std.secret`, and `std.browser.protocol`;
   - action footprint and default-handler descriptors.

`etas_std` should expose:

- intrinsic id;
- qualified path;
- type signature;
- effect signature;
- requested-action footprint;
- determinism classification;
- lowering hint;
- runtime dispatch kind;
- documentation metadata.

`etas_runtime` and `etas_interpreter` dispatch runtime-recognized intrinsics.
Any intrinsic that observes or mutates external host state must cross an
explicit checked action/default-handler boundary. For example, `std.io.println`
is a
source-level standard flow and a runtime-recognized intrinsic, but execution is a
host-backed `Console.stdout_write` console request, not filesystem I/O and not
a direct write from the compiler frontend. `etas_host` owns reusable host
protocol values, default-handler adapters, sandbox mediation, and low-level I/O.
`etas_std` only describes names, signatures, escaping effects, requested
actions, intrinsic ids, and lowering hints.

## 12. Prelude

The prelude should be conservative.

Recommended prelude:

- primitive type names;
- `Array`, `List`, `Map`, `Set`, `Range`, and `Slice`;
- `Option`, `Some`, `None`;
- `Result`, `Ok`, `Err`;
- `unit`;
- `assert`;
- `abort`;
- `Trusted`, `Untrusted`, `Secret`, `Public`, `Sanitized`;
- `Prompt`;
- `Message`;
- `Sandbox`;
- `DefaultCommandSandbox`;
- limit constructors:
  - `Iterations`;
  - `Tokens`;
  - `ContextTokens`;
  - `Cost`;
  - `WallTime`;
  - `Attempts`;
- `approve`;
- core effects:
  - `Agentic`;
  - `Network`;
  - `FileIO`;
  - `Command`;
  - `Memory`;
  - `Secret`;
  - `Time`;
  - `Human`;
  - `Error`.

Minimal standard action tags such as `Console`, `Approval`, and `Clock`, and
substrate tags such as `Net`, `Stream`, `Tls`, `Fs`, and `Browser`, may be
imported explicitly or re-exported by ergonomic modules. Package/project-defined
external tags such as `EdkWorkspace`, `Web`, or model-provider tags may also be
imported when supplied by package or host metadata. They should not all enter
the prelude by default.

Modules such as JSON, group combinators, detailed session policies, and host
path/url helpers may require explicit imports until the ergonomics are proven.

## 13. Compiler Responsibilities

Compiler crates consume `etas_std` as declarations:

- `etas_hir` loads std modules and prelude into resolution scopes.
- `etas_types` uses standard type constructors, standard function/support
  signatures, `Schema[T]`, `PromptEncode[T]`, `Result`, `Option`, and trust
  wrappers.
- `etas_effects` uses std effect tags, actions, requirements, and limit
  constructors.
- `etas_lowering` recognizes standard intrinsics and lowers them to AIR nodes
  or runtime intrinsic calls.
- `etas_analysis` recognizes trust wrappers, prompt channels, approval
  requirements, limits, and standard effect categories.

No compiler crate should hardcode standard-library behavior where a registry
descriptor is sufficient.

## 14. Runtime And Host Responsibilities

Runtime consumes `etas_std` descriptors but owns execution:

- pure intrinsic execution;
- approval handling;
- checkpoint handling;
- trace emission;
- budget accounting;
- time access;
- runtime-scoped effect action dispatch;
- handler and resume behavior.

Host consumes `etas_std` descriptors but owns external adapters:

- model provider adapters;
- tool registry;
- memory backends;
- console/stdin/stdout/stderr support;
- filesystem support;
- command sandbox implementation;
- network adapters;
- secret providers.

`etas_std` must not bypass runtime authority checks.

## 15. Complete Standard Surface

`etas_std` should be designed as the complete declaration surface for the PL
design, even when execution support is staged. It should include:

- primitive type registry;
- `List`, `Map`, `Set`;
- `Option`, `Result`;
- `Trusted`, `Untrusted`, `Secret`, `Public`, `Sanitized`;
- `Prompt`, `PromptPart`, `PromptEncode`;
- `Schema[T]` descriptors for ordinary Etas types;
- `Message[T]` and session/conversation identifiers;
- `MemoryRegion[S]`, `Store[K, V]`, and `std.memory.region[...]`;
- `Sandbox`, `SandboxProfile`, `DefaultCommandSandbox`;
- core effect roots;
- minimal standard effect/action declarations such as `Console`, `Clock`,
  `Memory.read/write`, `Approval.request`, and `Error[E].raise`;
- `approve`;
- `assert`;
- `abort`;
- loop/retry/budget limit constructors;
- `Path`, `Url`, `Command`, `ShellResult`;
- `Time`, `Duration`, `Money`, `usd`;
- `runtime.checkpoint` descriptor;
- documentation and completion metadata for the above.

Execution support may be staged for:

- full group chat combinators;
- rich `Conversation`;
- bounded context/history access and conditional context publication;
- complete persistent store backend execution;
- advanced JSON/schema customization;
- full trace redaction policy surface;
- package-level std version negotiation;
- provider-specific model catalogs.

Staged execution must not change the registry's type vocabulary. It only
affects which declarations the Phase 1 HIR interpreter can execute versus which
ones produce explicit runtime-required diagnostics.

Application summarization/tokenization policies are outside this execution
support list. Lack of an Etas-provided production summarizer is not a missing
standard-library/runtime feature.
