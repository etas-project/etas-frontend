# Etas Syntax Design

## 1. Purpose

This document defines the first implementation design for `etas_syntax`.

`etas_syntax` is the source-language entry crate. It owns lexing, parsing,
tokens, syntax-specific diagnostic production, and parsed AST node types. It
uses source, span, line-index, and shared diagnostic primitives from
`etas_core`. There is no separate `etas_ast` crate in the initial
architecture.

## 2. Boundary

`etas_syntax` owns:

- tokens and keywords;
- lexer;
- parser;
- syntax diagnostic codes and builders;
- parsed AST node types;
- parser recovery nodes;
- AST dump support for debugging and golden parser tests.

`etas_syntax` does not own:

- source file identity and text storage;
- byte ranges, spans, and line indexes;
- the shared diagnostic data model;
- name resolution;
- symbol ids;
- type checking;
- effect checking;
- trace-spec validation;
- AIR lowering;
- runtime execution;
- LSP UTF-16 position conversion;
- formatting.

The next semantic layer is `etas_hir`, which lowers parsed AST into a
resolution-friendly representation with symbol identities.

## 3. Crate Layout

Recommended file layout:

```text
crates/etas_syntax/
  Cargo.toml
  src/
    lib.rs
    token.rs
    trivia.rs
    lexer.rs
    cursor.rs
    parser.rs
    diagnostic.rs
    parse.rs
    dump.rs

    ast/
      mod.rs
      name.rs
      item.rs
      ty.rs
      expr.rs
      stmt.rs
      pattern.rs
      literal.rs

    grammar/
      mod.rs
      item.rs
      ty.rs
      expr.rs
      stmt.rs
      agent.rs
      spec.rs
      recovery.rs
```

Public users should import AST types through:

```rust
use etas_syntax::ast::{Program, Item, Expr, Stmt, TypeExpr};
```

## 4. Public API

The public API should stay independent from the parser implementation library.

```rust
pub fn lex(source: &SourceFile) -> TokenStream;

pub fn parse_program(source: SourceFile) -> Parse<ast::Program>;

pub fn dump_ast(program: &ast::Program, options: DumpOptions) -> String;
```

The implementation uses a hand-written parser internally, but public APIs should
not expose parser-internal types. This keeps the option open to move to a
rowan-backed lossless parser, a parser-combinator implementation, or a hybrid
parser later without forcing downstream crates to change.

AST dump is a syntax-layer feature. It serializes parsed AST shape, spans, and
error nodes for debugging and tests. It must not resolve names, infer types,
compute effects, or lower to AIR.

## 5. Parser Implementation Choice

The primary parser architecture is a hand-written, token-based recursive
descent parser with explicit recovery.

This is an intentional architecture choice for the first compiler frontend, not
a fallback implementation. Etas needs precise source spans, source-shaped AST
nodes, error nodes, delimiter-aware recovery, and diagnostics that remain useful
for CLI and future LSP edit states. Those requirements are easier to control
with a parser that owns its cursor, recovery boundaries, and diagnostic
construction directly.

The public syntax API remains parser-library independent:

```text
public API:
  lex(source) -> TokenStream
  parse_program(source) -> Parse<Program>
  dump_ast(program, options) -> String

internal implementation:
  lexer produces tokens and trivia
  cursor skips trivia for grammar parsing
  grammar modules consume a shared Parser context
  parser helpers normalize missing tokens, delimiter recovery, and diagnostics
```

`chumsky` is not the main parser contract. It may be used only for narrowly
scoped helper logic, such as local recovery scans or experimental diagnostics,
when that code is clearer than hand-written scanning. If no such helper remains,
the dependency should be removed. Using `chumsky` for a helper does not permit
exposing `chumsky` types from `etas_syntax`.

Do not expose parser-library types from `etas_syntax`:

```text
do not expose chumsky::Parser
do not expose chumsky::error::Rich
do not expose chumsky::Stream
do not expose parser-library span or token types
```

The parser must be organized by grammar domain. A large monolithic parser file
is not an acceptable long-term structure, even if the behavior is correct.
`parser.rs` owns parser state and shared mechanics; `grammar/` owns source
grammar productions.

Required internal split:

```text
parser.rs
  parse_program entry
  Parser context
  token cursor access
  shared expect/eat helpers
  delimiter stack
  missing-token diagnostics
  recovery dispatch helpers

grammar/item.rs
  module declarations
  imports
  top-level item dispatch
  type / enum / impl / effect / tool / protocol / flow declarations
  top-level let declarations

grammar/agent.rs
  agent declarations
  agent clauses
  agent config-row syntax entry points

grammar/spec.rs
  spec declarations
  spec entailment and satisfaction syntax
  callable-spec signatures
  trace-spec expressions and action patterns

grammar/ty.rs
  type expressions
  arrow types
  effect-row-bearing flow types
  handler effect-transformer types
  primitive/path/tuple/record types
  type parameters and type arguments

grammar/stmt.rs
  blocks
  let / var / assignment / expression statements
  if / match statements
  for / while / retry / resume / return / break / continue

grammar/expr.rs
  lambda expressions
  pipeline `~>` expressions
  stage composition `|`
  binary and unary precedence
  postfix call / method / field / index / `?`
  literals, records, lists, tuples, if/match expressions, perform, handle,
  handler literals

grammar/recovery.rs
  shared recovery helpers
  obsolete syntax scans
  recovery-boundary utilities
```

The grammar modules should be implementation modules, not public API modules.
They may add inherent methods on the shared `Parser` type or call free
functions that accept `&mut Parser`, but they should not define independent
parser states for each grammar family.

Reasons:

- `etas_hir`, `etas_intel`, `etas_lsp`, and tests should depend on Etas's
  syntax API, not on a parser library.
- Parser errors should be normalized into Etas diagnostics with stable
  diagnostic codes and Etas spans.
- Hand-written recovery must remain testable by grammar domain rather than
  hidden inside one large file.
- Future lossless syntax, rowan integration, or a parser-combinator rewrite
  should remain possible without changing downstream crate APIs.

## 6. Source and Span Types

Source and span primitives are defined in `etas_core`, not in
`etas_syntax`.

`etas_syntax` should import and use:

```rust
use etas_core::{
    SourceFile, SourceId, Span, TextRange, TextSize, LineIndex, LineCol,
};
```

Use UTF-8 byte offsets internally.

LSP clients use UTF-16 positions, but that conversion belongs in `etas_lsp` or
`etas_intel`, not in the parser.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
// Defined in etas_core.
pub struct SourceId(pub u32);

#[derive(Clone, Debug)]
pub struct SourceFile {
    pub id: SourceId,
    pub path: Option<PathBuf>,
    pub text: Arc<str>,
    pub line_index: LineIndex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextSize(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextRange {
    pub start: TextSize,
    pub end: TextSize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    pub source: SourceId,
    pub range: TextRange,
}
```

`LineIndex` must support byte offset to line/column conversion for diagnostics
and tooling. It is also defined in `etas_core`. Internally, offsets remain byte
offsets. Human-facing renderers may compute display columns separately.

```rust
pub struct LineIndex {
    line_starts: Vec<TextSize>,
}

pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

impl LineIndex {
    pub fn line_col(&self, offset: TextSize) -> LineCol;
    pub fn line_range(&self, line: u32) -> Option<TextRange>;
}
```

Column convention:

- `TextSize` and `TextRange` are UTF-8 byte offsets.
- `LineCol` is 0-based and derived from byte offsets.
- CLI diagnostic rendering may compute Unicode/display columns from source text.
- LSP UTF-16 positions are produced only by `etas_intel` or `etas_lsp`.

## 7. Token Types

Tokens preserve source spans and token kind. Trivia can be emitted by the lexer
so diagnostics and future formatter work have access to it, but the parser may
skip trivia by default.

```rust
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub struct TokenStream {
    pub source: SourceId,
    pub tokens: Vec<Token>,
}
```

```rust
pub enum TokenKind {
    Ident,
    IntLit,
    FloatLit,
    StringLit,
    CharLit,
    Keyword(Keyword),
    Punct(Punct),
    Whitespace,
    Comment(CommentKind),
    Unknown,
    Eof,
}
```

```rust
pub enum CommentKind {
    Line,
    Block,
}
```

`Whitespace` and `Comment(_)` are trivia tokens. They preserve spans and can be
retrieved from the token stream, but parser grammar must skip them by default.

Keywords should reflect surface syntax only:

```rust
pub enum Keyword {
    Module,
    Import,
    As,
    Alias,
    Type,
    Enum,
    Spec,
    Impl,
    Where,
    Private,
    Public,
    Flow,
    Effect,
    Extends,
    Action,
    Perform,
    Handle,
    With,
    Resume,
    Tool,
    Agent,
    Require,
    Deny,
    Before,
    Protocol,
    Let,
    Var,
    If,
    Else,
    Match,
    For,
    In,
    While,
    Limit,
    Retry,
    Return,
    Break,
    Continue,
    True,
    False,
}
```

Primitive type names may be lexed as identifiers initially and recognized by
the parser/type checker in type positions. If diagnostics or grammar
simplification later require primitive-specific tokens, add them deliberately.

Punctuation should include single-character and multi-character operators:

```rust
pub enum Punct {
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Dot,
    Colon,
    Semi,
    Arrow,
    FatArrow,
    Pipe,
    TildeArrow,
    Eq,
    EqEq,
    Bang,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    AmpAmp,
    PipePipe,
    Question,
}
```

### 7.1 Comments and Trivia

Comments are lexical trivia.

This follows the current PL design:

- line comments use `//` and run until the end of the line;
- block comments use `/* ... */` and may span multiple lines;
- block comments are not nested in the first syntax implementation;
- a `/*` inside an existing block comment is ordinary comment text until the
  first following `*/`;
- comments are ignored by parsing;
- comments do not enter typed AST, HIR, AIR, effect inference, trace-spec
  checking, traces, or runtime behavior;
- comments cannot express semantic constraints.

Examples:

```etas
// Line comment.
let draft = Writer.run(topic); // trailing comment

/*
Block comments can span multiple lines.
They may be used for human documentation near declarations.
*/
```

The lexer should produce comment trivia tokens:

```rust
pub struct Trivia {
    pub kind: TriviaKind,
    pub span: Span,
}

pub enum TriviaKind {
    Whitespace,
    LineComment,
    BlockComment,
}
```

Implementation options:

1. keep trivia as `TokenKind::Whitespace` and `TokenKind::Comment(...)` entries
   in `TokenStream`;
2. later attach leading/trailing trivia to significant tokens if a formatter or
   lossless parser needs it.

The first implementation should use option 1. It is enough for lexing tests,
diagnostics, AST dump options, and future formatter experiments without
committing to a lossless syntax tree.

Parser behavior:

```text
parser input = token stream with trivia skipped
AST nodes    = significant syntax only
AST spans    = spans of significant syntax, not surrounding comments
```

This means comments are not represented as AST nodes:

```rust
pub enum Item {
    Flow(FlowDecl),
    // no Comment item
}
```

Diagnostics:

- an unterminated block comment must produce a lexer diagnostic;
- the diagnostic primary span should cover the unterminated comment start or
  the comment range through EOF;
- a line comment never consumes the following newline as significant source
  text for statement parsing;
- comments should not suppress diagnostics for the surrounding code.

Recommended syntax diagnostic code addition:

```rust
pub enum SyntaxDiagnosticCode {
    // existing codes...
    UnterminatedBlockComment,
}
```

Semantic non-goals:

```etas
// This does not enforce a budget.
while needs_revision(draft) {
    draft = Rewriter.run(draft);
}

// This is the semantic form.
while needs_revision(draft)
    limit Iterations(3), Tokens(30_000)
{
    draft = Rewriter.run(draft);
}
```

Tooling guidance:

- AST dump may optionally include trivia in a separate token/trivia section;
- default AST dump should omit trivia to keep parser golden tests focused on
  parsed syntax;
- LSP hover/completion should not infer documentation from ordinary comments in
  the syntax layer;
- future documentation extraction may associate nearby block comments with
  declarations as tooling metadata, but that association must remain outside
  AST/HIR semantics and must not affect type/effect/runtime behavior.

## 8. Parse Result and Rich Diagnostics

Parsing should always return a value, even when the value contains error nodes.
This is required for LSP use because edited files are often incomplete.

```rust
pub struct Parse<T> {
    pub value: T,
    pub diagnostics: Vec<Diagnostic>,
    pub tokens: TokenStream,
}
```

Diagnostics should be rich enough for CLI rendering, LSP diagnostics, future
code actions, and golden diagnostic tests. `etas_syntax` produces only lexical
and parse diagnostics. The shared diagnostic data model lives in `etas_core`
and is reused by later compiler phases.

```rust
// Defined in etas_core.
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub phase: DiagnosticPhase,
    pub severity: Severity,
    pub message: String,
    pub primary: DiagnosticSpan,
    pub labels: Vec<DiagnosticLabel>,
    pub notes: Vec<String>,
    pub help: Option<String>,
    pub suggestions: Vec<Suggestion>,
}
```

```rust
pub struct DiagnosticSpan {
    pub span: Span,
    pub label: Option<String>,
}

pub struct DiagnosticLabel {
    pub span: Span,
    pub style: LabelStyle,
    pub message: String,
}

pub enum LabelStyle {
    Primary,
    Secondary,
}
```

`primary` is the main span the renderer should focus. `labels` can include both
the primary span and related spans. This supports diagnostics such as
"unclosed delimiter opened here" plus "expected matching delimiter before this
token".

Diagnostics must identify the compiler phase:

```rust
pub enum DiagnosticPhase {
    Lex,
    Parse,
    Lower,
    NameResolution,
    TypeCheck,
    EffectCheck,
    Analysis,
}
```

`etas_syntax` may only emit `Lex` and `Parse`, but downstream crates should
reuse the same `etas_core` structure where practical.

Diagnostic codes should be structured. The shared envelope lives in
`etas_core`; syntax-specific codes are owned by `etas_syntax` unless the
project chooses a single centralized code enum in `etas_core`.

```rust
pub enum DiagnosticCode {
    Syntax(SyntaxDiagnosticCode),
    Name(NameDiagnosticCode),
    Type(TypeDiagnosticCode),
    Effect(EffectDiagnosticCode),
    Analysis(AnalysisDiagnosticCode),
}
```

The syntax-level codes are:

```rust
pub enum SyntaxDiagnosticCode {
    UnexpectedToken,
    UnexpectedEof,
    MissingToken,
    UnclosedDelimiter,
    InvalidLiteral,
    UnterminatedString,
    UnterminatedBlockComment,
    InvalidItem,
    InvalidType,
    InvalidExpression,
    InvalidPattern,
}
```

`UnclosedDelimiter` must carry related spans through labels:

```text
primary: expected matching `}` before EOF
secondary: `{` opened here
```

Suggestions should be represented structurally even if the first LSP release
does not expose code actions:

```rust
pub struct Suggestion {
    pub title: String,
    pub edits: Vec<TextEdit>,
    pub applicability: Applicability,
}

pub struct TextEdit {
    pub range: TextRange,
    pub replacement: String,
}

pub enum Applicability {
    MachineApplicable,
    MaybeIncorrect,
    HasPlaceholders,
}
```

Examples:

- insert a missing `;`;
- replace `fun` with `flow` if a user writes an obsolete keyword;
- add a missing `}` placeholder;
- later phases may suggest adding a missing effect to an effect row.

`Severity` should support at least:

```rust
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}
```

Syntax diagnostics are normally `Error`, but lexer/parser recovery may also
emit `Warning` or `Hint` diagnostics for deprecated syntax after the language
evolves.

## 9. AST Design

AST nodes are parsed syntax nodes. They carry spans and source names, but no
semantic symbol ids.

Semantic identities belong in `etas_hir`.

### 9.1 Program and Items

```rust
pub struct Program {
    pub module: Option<ModuleDecl>,
    pub imports: Vec<ImportDecl>,
    pub items: Vec<Item>,
    pub span: Span,
}

pub enum Item {
    Alias(AliasDecl),
    Type(TypeDecl),
    Enum(EnumDecl),
    Spec(SpecDecl),
    Impl(ImplDecl),
    Effect(EffectDecl),
    Tool(ToolDecl),
    Agent(AgentDecl),
    Protocol(ProtocolDecl),
    Flow(FlowDecl),
    TopLevelLet(TopLevelLetDecl),
    Error(ErrorItem),
}
```

`ErrorItem` lets the parser recover after malformed top-level declarations.
The obsolete source-level `memory` declaration must be diagnosed as invalid
syntax and recovered as `ErrorItem`; it must not produce a `MemoryDecl` AST
node. Persistent memory is represented by ordinary type declarations,
compiler-known standard support types, top-level immutable resource handles,
and ordinary calls/member access.

`alias` is a distinct item, not a spelling variant of `type`. The parser must
preserve this distinction because the latest language SPEC makes `type`
nominal by default while `alias` is the only transparent abbreviation form.

The current source language also includes spec declarations and spec
implementations. `spec` is static evidence, not a parent class, runtime object
interface, or value-level supertype. The syntax layer must support the three
SPEC kinds:

- `type` specs for nominal/static evidence such as `ByteStream`,
  `Within<ReportsRoot>`, and `Index`;
- `callable` specs for flow/tool/agent callable shapes such as `Pure<I, O>`;
- `trace` specs for requested-action trace constraints.

The canonical SPEC syntax uses `~` for spec satisfaction, declaration
conformance, and spec entailment:

```etas
spec ReadWriteStream ~ ReadableStream + WritableStream;
flow read_all<S ~ ByteStream>(stream: S) -> bytes;
impl TlsStream ~ ByteStream;
flow Normalize(text: string) -> string ~ Pure { ... }
flow Publish(...) ~ SafeTrace { ... }
```

For compatibility, the parser may continue to accept the older
`impl Spec for Type` spelling and normalize it to the same AST/HIR
spec-satisfaction shape as `impl Type ~ Spec`. That compatibility must not
extend to using `:` for spec bounds: `:` remains a type-annotation token, while
`~` means spec satisfaction or spec entailment. The syntax layer must preserve
all spec declarations, spec satisfactions, marker impls, callable-spec clauses,
trace-spec conformances, and type/effect parameter bounds as source-shaped nodes
with spans. The old `follows` clause and source-level `policy { ... }`
declaration are not current syntax; they should recover with targeted
diagnostics rather than produce first-class AST nodes.

### 9.2 Modules and Imports

Module and import syntax must follow the current PL grammar:

```ebnf
Program       ::= ModuleDecl? ImportDecl* Item*
ModuleDecl    ::= "module" Path ";"
ImportDecl    ::= Visibility? "import" ImportTree ";"
ImportTree    ::= Path ImportTail?
ImportTail    ::= "as" Ident
                | ".*"
                | ".{" ImportTreeList "}"
ImportTreeList
              ::= ImportTreeEntry ("," ImportTreeEntry)* ","?
ImportTreeEntry
              ::= Ident ImportTail?
Visibility    ::= "public" | "private"
```

Supported source forms include:

```etas
import std.io;
import std.io as io;
import std.io.println;
import std.io.println as log;
import std.io.{print, println, eprintln};
import std.io.{println as log, read_line,};
import std.io.*;
public import std.io.{println, eprintln};
public import std.prelude.*;
```

`private` is the default visibility. `public import` is a re-export. Syntax
must preserve that visibility, but it must not decide which imported names are
available or whether a wildcard import is ambiguous. Those decisions belong to
HIR name resolution and later semantic phases.

The syntax AST must model import declarations as import trees, not as a flat
`Path + alias` pair:

```rust
pub struct ModuleDecl {
    pub path: Path,
    pub span: Span,
}

pub struct ImportDecl {
    pub visibility: Visibility,
    pub tree: ImportTree,
    pub span: Span,
}

pub enum Visibility {
    Private,
    Public,
}

pub enum ImportTree {
    Single {
        path: Path,
        alias: Option<Name>,
        span: Span,
    },
    Group {
        prefix: Path,
        items: Vec<ImportItem>,
        span: Span,
    },
    Wildcard {
        prefix: Path,
        star_span: Span,
        span: Span,
    },
    Error {
        span: Span,
    },
}

pub struct ImportItem {
    pub name: Name,
    pub alias: Option<Name>,
    pub span: Span,
}
```

Design rules:

- `Single` represents module imports, item imports, and single aliases such as
  `import std.io as io;` and `import std.io.println as log;`.
- `Group` represents `import std.io.{print, println};`. Each `ImportItem`
  records the member name, optional alias, and member-local span.
- `Wildcard` represents `import std.io.*;`. The syntax layer records the star
  and prefix only; imported names are not enumerated by the parser.
- `Error` lets the parser return a well-shaped `ImportDecl` after malformed
  import syntax.
- The `ImportDecl` span covers visibility if present, the `import` keyword,
  the full tree, and the terminating semicolon.
- `ImportTree::Group.span` covers the prefix, dot, braces, and group content.
  `ImportItem.span` covers only the member entry, including an alias if
  present.
- `ImportTree::Wildcard.star_span` points at `*` for precise diagnostics and
  future quick-fixes.

Parser requirements:

- `program()` must accept `public import` and `private import` in the import
  region before ordinary items.
- `import_decl()` must parse visibility before `import`, then call an
  `import_tree()` helper.
- `import_tree()` should parse a normal `Path`, then inspect the import tail:
  `as Ident`, `.*`, or `.{ ... }`.
- Grouped imports must allow a trailing comma.
- Grouped import recovery must be local to the braces. A malformed member
  should produce an error entry or diagnostic and continue to the next comma or
  `}` without turning the rest of the file into top-level `ErrorItem`s.
- `public import` and `private import` are import declarations, not ordinary
  top-level items.

Pass boundary:

- grouped imports, grouped aliases, trailing commas, and wildcard imports are
  not separate passes;
- they are source grammar forms and must be parsed directly into
  `ImportTree`;
- trailing comma is pure syntax tolerance and should not survive as semantic
  data;
- the first semantic pass that changes import shape is HIR lowering, where
  import trees are normalized for scopes and resolution.

HIR lowering guidance:

- HIR may either preserve an import tree or expand it into explicit import
  entries. For Phase 1, expanding grouped imports into member-level HIR imports
  is simpler for scopes and symbol tables.
- Single and grouped member imports should create import symbols for their
  effective local names.
- Wildcard imports should not create one symbol per member in the syntax/HIR
  lowering step. They should be recorded as wildcard import sources and resolved
  by the module/package resolver.
- Visibility must be carried into HIR so `public import` can become a re-export.
- The pass pipeline may expose this work as `HirLowerPass` followed by
  `ImportResolutionPass` or `NameResolutionPass`. It should not expose parser
  grammar details such as "grouped alias pass" or "trailing comma pass".

`flow` is the only user-defined callable declaration:

```rust
pub struct FlowDecl {
    pub visibility: Visibility,
    pub name: Name,
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub declared_effects: Option<EffectRow>,
    pub conformances: Vec<DeclarationConformance>,
    pub body: FlowBody,
    pub trailing_handler: Option<HandlerArg>,
    pub span: Span,
}

pub enum FlowBody {
    Block(Block),
    Expr {
        expr: Expr,
        eq_span: Span,
        semicolon_span: Option<Span>,
        span: Span,
    },
}
```

The return type and effect suffix are optional in source. When present, the
effect suffix belongs after the output type, for example
`flow Read(path: Path) -> Report ![FileIO] { ... }`. The type checker infers
the return type from explicit `return` statements and the block final
expression when the source omits `-> Type`. Expression-bodied flows use
`= Expr;?` and should be lowered as a block final expression, not executed at
module load.

Effect actions are signatures, not flow bodies:

```rust
pub struct EffectActionDecl {
    pub name: Name,
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Param>,
    pub return_type: TypeExpr,
    pub span: Span,
}
```

Effect actions may appear inside an `effect` block or an `impl` block whose
target resolves to an effect tag. Ordinary type `impl` blocks may contain only
`flow` methods. Spec impl blocks may contain only required spec `flow` methods,
and marker spec impls may use the short semicolon form. This target kind is
checked later by HIR/type checking, but the syntax AST should preserve all forms
and normalize both canonical and compatibility spec impl spellings:

```rust
pub enum ImplItem {
    Flow(FlowDecl),
    Action(EffectActionDecl),
    Error(Span),
}

pub struct ImplDecl {
    pub target: ImplTarget,
    pub items: Vec<ImplItem>,
    pub span: Span,
}

pub enum ImplTarget {
    Inherent {
        target: TypeExpr,
        span: Span,
    },
    SpecSatisfaction {
        self_type: TypeExpr,
        specs: Vec<SpecRef>,
        span: Span,
    },
    Error { span: Span },
}

pub struct SpecRef {
    pub spec_path: Path,
    pub spec_args: Vec<TypeExpr>,
    pub span: Span,
}
```

Spec declarations have explicit AST coverage:

```rust
pub struct SpecDecl {
    pub name: Name,
    pub type_params: Vec<TypeParam>,
    pub kind: SpecDeclKind,
    pub span: Span,
}

pub enum SpecDeclKind {
    TypeSpec {
        entails: Vec<SpecRef>,
        body: SpecBody,
    },
    CallableSpec {
        input: TypeExpr,
        output: TypeExpr,
        effects: CallableSpecEffectConstraint,
    },
    TraceSpec {
        expr: SpecExpr,
    },
}

pub enum SpecBody {
    Marker {
        semicolon_span: Span,
    },
    Block {
        items: Vec<SpecItem>,
        span: Span,
    },
}

pub enum CallableSpecEffectConstraint {
    Unconstrained,
    Closed(EffectRow),
}

pub enum SpecItem {
    FlowSignature(FlowSignature),
    Error(Span),
}

pub struct FlowSignature {
    pub name: Name,
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Param>,
    pub return_type: FlowReturnType,
    pub span: Span,
}
```

`public spec ByteStream;` and `public spec Index;` are marker type specs.
`PromptEncode`, `Schema`, and `ResponseDecode` may be behavioral type specs
with required flow signatures. Callable specs such as
`public spec Pure<I, O>: callable I => O ![];` are bodiless
computation-shape specs and must not be parsed as ordinary type specs with a
method body. Trace specs such as `spec Safe: trace = +Net.tcp_connect<_>;`
contain a `SpecExpr`, not a policy block. Syntax only preserves these
declarations; kind checking, satisfaction, coherence, callable-spec signature
inference, trace-spec operator kind checking, and impl completeness are
type-checker/effect-checker responsibilities.

Effect declarations have two valid source forms: `effect Name;` for an empty
tag and `effect Name { action ...; }` for a tag plus action signatures. The
detailed AST shape is listed in the declarations section below.

`impl` targets are not syntax-kind-checked. The parser preserves
`ImplItem::Flow`, `ImplItem::Action`, and spec-implementation forms;
HIR/type/effect checking must reject a type `impl` that contains `action`
entries, an effect `impl` that contains ordinary `flow` methods, and a spec
`impl` that adds methods outside the spec contract.

`IndexExpr` and `SliceExpr` are syntax only. The syntax layer records
`base[index]`, `base[start, end)`, and `base(start, end]` shapes and spans, but
it must not decide whether an index is sequence indexing, map lookup, or
invalid. `Index` spec satisfaction, checked index classification, and checked
slice classification belong to `etas_types`.

### 9.3 Names and Paths

```rust
pub struct Name {
    pub text: String,
    pub span: Span,
}

pub struct Path {
    pub segments: Vec<Name>,
    pub span: Span,
}
```

Names are source text only. They are not interned in the syntax layer. HIR may
intern names or assign symbol ids.

### 9.4 Types

```rust
pub enum TypeExpr {
    Handler(HandlerType),
    Arrow {
        effect: Option<EffectRow>,
        input: Box<TypeExpr>,
        output: Box<TypeExpr>,
        span: Span,
    },
    Primitive {
        kind: PrimitiveType,
        span: Span,
    },
    Path {
        path: Path,
        args: Vec<GenericArg>,
        span: Span,
    },
    Record(RecordType),
    Tuple {
        elems: Vec<TypeExpr>,
        span: Span,
    },
    Error(Span),
}
```

`TypeExpr::Error` allows downstream crates to continue and report additional
diagnostics without panicking on malformed source.

Arrow types are the source notation for flow values. The parser should preserve
them as `TypeExpr::Arrow`; later semantic layers normalize them to the internal
`Flow[I, O, E]` representation.

Examples:

```etas
Url -> Page
Url -> Page ![Network]
Topic -> Draft ![Web.search, Memory.read[ResearchMemory]]
(A, B) -> C
A -> B -> C
```

`A -> B -> C` is right-associative and means `A -> (B -> C)`.

Type parameters must preserve kinded bounds:

```rust
pub struct TypeParam {
    pub name: Name,
    pub bounds: Vec<TypeParamBound>,
    pub span: Span,
}

pub enum TypeParamBound {
    Spec(Path),
    EffectKind { span: Span },
    Error(Span),
}
```

`T ~ PromptEncode + Schema` constrains value-type parameters. `effect E`
declares an effect-row parameter. The parser records the shape and spans only;
spec satisfaction and effect-row instantiation belong to later phases. A parser
may diagnose old `T: Spec` bound spelling with a targeted recovery message, but
it must not treat `:` as an accepted spec-bound operator in new source.

Generic arguments must also preserve kinded shape in positions where the SPEC
allows both ordinary type arguments and effect-row arguments:

```rust
pub enum GenericArg {
    Type(TypeExpr),
    EffectRow(EffectRow),
    Error(Span),
}
```

The syntax parser does not decide whether an identifier inside `[...]` names a
type parameter or an effect-row parameter. It records the source shape and lets
HIR/type lowering check the generic parameter kind.

Handler type annotations are type expressions, not effect rows attached to a
flow arrow. They describe reusable handler values:

```rust
pub struct HandlerType {
    pub handled: EffectRow,
    pub produced: HandlerProducedEffects,
    pub result: Option<Box<TypeExpr>>,
    pub span: Span,
}

pub enum HandlerProducedEffects {
    Infer,
    Explicit(EffectRow),
}
```

Source forms:

```etas
![Approval]
![Approval.request => Console.stdout_write]
![Error[AppError] for Report]
![Error[AppError] => [] for Report]
```

`![...]` inside `-> Type ![...]` is an effect suffix. `![...]` as a standalone
type expression is a handler type. The parser should preserve the shape and let
later type/effect phases decide whether the position is valid.

There is no source-level `fun` type and no `flow` keyword in type expressions.
`flow` is only the declaration keyword for named callables.

### 9.4 Expressions

```rust
pub enum Expr {
    Literal(Literal),
    Path(Path),
    Record(RecordExpr),
    Tuple {
        elems: Vec<Expr>,
        span: Span,
    },
    Array {
        elems: Vec<Expr>,
        span: Span,
    },
    List {
        elems: Vec<Expr>,
        span: Span,
    },
    ListCons {
        head: Box<Expr>,
        tail: Box<Expr>,
        span: Span,
    },
    EmptySequence {
        span: Span,
    },
    Map(MapExpr),
    Set {
        elems: Vec<Expr>,
        span: Span,
    },
    Range(RangeExpr),
    Call(CallExpr),
    MethodCall(MethodCallExpr),
    Perform(PerformExpr),
    Handle(HandleExpr),
    Handler(HandlerExpr),
    StageCompose(StageComposeExpr),
    Pipeline(PipelineExpr),
    Field(FieldExpr),
    Index(IndexExpr),
    Slice(SliceExpr),
    Try(TryExpr),
    Unary(UnaryExpr),
    Binary(BinaryExpr),
    If(IfExpr),
    Match(MatchExpr),
    Lambda(LambdaExpr),
    Block(BlockExpr),
    Error(Span),
}
```

The syntax layer should preserve syntactic distinctions such as `Call`,
`MethodCall`, `StageCompose`, `Pipeline`, collection literal forms, ranges, and
slices. HIR can later normalize forms where appropriate, but it must not lose
the source distinction between `Array`, `List`, `Map`, `Set`, `Range`, and
`Slice`.

Collection literal syntax follows the PL SPEC exactly:

- `[a, b, c]` parses as `Array`.
- `[a; b; c]` parses as `List`.
- `a :: b :: []` parses as right-associative `ListCons`.
- `[]` parses as `EmptySequence`; the syntax layer must not default it to
  `Array` or `List`.
- `{ key => value }` parses as `Map`.
- `#{a, b, c}` parses as `Set`.
- `[start, end)` and `(start, end]` parse as `Range`.
- `xs[start, end)` and `xs(start, end]` parse as `Slice`.

Empty `[]`, `{}`, and other syntactically insufficient empty collection forms
remain ambiguous until contextual type checking. The parser should attach spans
precisely enough for diagnostics to underline the whole literal and the
delimiter or separator that caused disambiguation.

```rust
pub struct MapExpr {
    pub entries: Vec<MapEntry>,
    pub span: Span,
}

pub struct MapEntry {
    pub key: Expr,
    pub value: Expr,
    pub span: Span,
}

pub struct RangeExpr {
    pub start: Box<Expr>,
    pub end: Box<Expr>,
    pub bounds: RangeBounds,
    pub span: Span,
}

pub struct SliceExpr {
    pub base: Box<Expr>,
    pub start: Box<Expr>,
    pub end: Box<Expr>,
    pub bounds: RangeBounds,
    pub span: Span,
}

pub enum RangeBounds {
    ClosedOpen,
    OpenClosed,
}
```

`RecordExpr` and `MapExpr` share braces but not grammar:

- record fields use `name`, `name = expr`, or `Path { ... }`;
- map entries use `key => value`;
- `{}` is syntactically empty and must be resolved by expected type.

`Set` requires lexer support for `#{` either as a composite token or as `#`
followed by `{`; both choices must preserve one contiguous source span.

Stage composition and pipeline application should use dedicated AST nodes, not
plain `BinaryExpr`.

```rust
pub struct StageComposeExpr {
    pub stages: Vec<PipelineStage>,
    pub span: Span,
}

pub struct PipelineExpr {
    pub input: Box<Expr>,
    pub stages: Vec<PipelineStage>,
    pub span: Span,
}

pub struct PipelineStage {
    pub expr: Box<Expr>,
    pub limits: Vec<Expr>,
    pub span: Span,
}
```

Examples:

```etas
Researcher | Writer | Publisher
brief ~> ProductManager ~> Architect
design ~> Engineer limit Tokens(12_000)
```

The syntax layer records the surface structure only. HIR/lowering decides
whether a stage is an agent, flow, tool, composed flow, or invalid expression.

Postfix `?` must be represented as its own AST node, not as a generic unary
operator and not as a `Result` unwrapping call:

```rust
pub struct TryExpr {
    pub expr: Box<Expr>,
    pub question_span: Span,
    pub span: Span,
}
```

The parser only records the postfix source form. It must not decide whether the
operand raises `Error[E]`, whether the result type is `Result[T, E]`, or whether
the expression is invalid because it only has a value-level `Result[T, E]`.
Those are type/effect checks.

`?` is only postfix. Prefix forms such as `?value` are not part of the language
and must produce a syntax diagnostic. The operator is also not a statement
terminator: in `let r = read()?;` and `return Load(path)?;`, the semicolon
terminates the surrounding `let` or `return` statement. A block final expression
may be `expr?` without a semicolon, and a multi-step computation is captured by
placing postfix `?` on the whole block expression:

```etas
flow TryLoad(path: Path) -> Result[Report, AppError] {
    {
        let text = fs.read(path);
        parse_report(text)
    }?
}
```

Anonymous flow expressions use lambda syntax, not a `flow` expression keyword:

```etas
let normalize = (title: string) => trim(lowercase(title));
let branch = () => StepA(input);
```

```rust
pub struct LambdaExpr {
    pub params: LambdaParams,
    pub body: LambdaBody,
    pub span: Span,
}

pub enum LambdaParams {
    Ident(Name),
    ParamList(Vec<Param>),
}

pub enum LambdaBody {
    Expr(Box<Expr>),
    Block(Block),
}
```

Effect action invocation and handlers also need dedicated nodes:

```rust
pub struct PerformExpr {
    pub effect: EffectRef,
    pub action: Name,
    pub action_type_args: Vec<TypeExpr>,
    pub args: Vec<Arg>,
    pub span: Span,
}

pub struct HandleExpr {
    pub body: Box<Expr>,
    pub handler: HandlerArg,
    pub span: Span,
}

pub enum HandlerArg {
    Inline(HandlerBlock),
    Expr(Box<Expr>),
}

pub struct HandlerExpr {
    pub block: HandlerBlock,
    pub span: Span,
}

pub struct HandlerBlock {
    pub arms: Vec<HandlerArm>,
    pub span: Span,
}

pub struct HandlerArm {
    pub effect: EffectRef,
    pub action: Name,
    pub action_type_args: Vec<TypeExpr>,
    pub patterns: Vec<Pattern>,
    pub body: Block,
    pub span: Span,
}
```

`handle` is an expression, not a statement form. Its left operand is any
expression, including a block expression: `handle { ... } with Handler`.
The right side is either an inline handler block or an ordinary expression that
evaluates to a handler value. A standalone reusable handler must use the
`handler` expression keyword:

```etas
let HumanApproval: ![Approval] = handler {
    Approval.request(req) => {
        resume Accepted;
    }
};

handle Publish(doc) with HumanApproval;
handle Publish(doc) with {
    Approval.request(req) => {
        resume Accepted;
    }
};
```

The bare handler-arm block `{ Action(...) => ... }` is not a general expression;
it is only valid as the inline `with { ... }` arm of `handle`.

`CallExpr` and `MethodCallExpr` preserve ordinary call syntax. Agent calls,
tool calls, support-flow calls, and memory API calls are not distinguished in
syntax.

Source-level agent invocation uses ordinary method-call syntax such as
`Writer.run(topic)`. The parser must not create a special `AgentCallExpr` node
or treat `run` as a keyword. Whether a method call is an agent inference
boundary is resolved later from HIR symbols and type facts. Likewise, stage
application such as `draft ~> Reviewer` is syntax-level pipeline application;
later phases decide whether the stage lowers to `Reviewer.run(draft)`, a flow
call, a tool call, or an invalid stage.

```rust
pub struct CallExpr {
    pub callee: Box<Expr>,
    pub type_args: Vec<TypeExpr>,
    pub args: Vec<Arg>,
    pub span: Span,
}

pub struct MethodCallExpr {
    pub receiver: Box<Expr>,
    pub method: Name,
    pub type_args: Vec<TypeExpr>,
    pub args: Vec<Arg>,
    pub span: Span,
}

pub enum Arg {
    Positional(Expr),
    Named { name: Name, value: Expr },
}
```

Record expressions support field shorthand:

```rust
pub struct RecordExpr {
    pub path: Option<Path>,
    pub fields: Vec<FieldInit>,
    pub span: Span,
}

pub enum FieldInit {
    Shorthand(Name),
    Named { name: Name, value: Expr },
}
```

### 9.5 Statements

```rust
pub enum Stmt {
    Let(LetStmt),
    Var(VarStmt),
    Assign(AssignStmt),
    If(IfStmt),
    Match(MatchStmt),
    For(ForStmt),
    While(WhileStmt),
    Retry(RetryStmt),
    Resume(ResumeStmt),
    Return(ReturnStmt),
    Break(Span),
    Continue(Span),
    Expr(ExprStmt),
    Error(Span),
}
```

Blocks should preserve statement order and carry their own span.

```rust
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub final_expr: Option<Expr>,
    pub span: Span,
}
```

A trailing expression without `;` is the block value. The type checker uses
that value together with explicit `return` statements for flow return type
inference.

### 9.6 Patterns and Literals

```rust
pub enum Pattern {
    Ident(Name),
    Wildcard(Span),
    Literal(Literal),
    Tuple {
        elems: Vec<Pattern>,
        span: Span,
    },
    Record(RecordPattern),
    Variant(VariantPattern),
    Error(Span),
}
```

```rust
pub enum Literal {
    Bool {
        value: bool,
        span: Span,
    },
    Int {
        text: String,
        span: Span,
    },
    Float {
        text: String,
        span: Span,
    },
    String {
        value: String,
        span: Span,
    },
    Char {
        value: char,
        span: Span,
    },
}
```

Keep numeric literal text in syntax. Type-directed numeric interpretation
belongs in type checking.

Expression literals include booleans, integers, floats, strings, and chars.
`bytes` is a primitive type, but the current PL SPEC does not define a dedicated
bytes literal syntax; do not invent one in `etas_syntax` until the SPEC does.

Literal patterns are narrower than expression literals. MVP literal patterns
allow booleans, integers, strings, and chars. Floating-point literal patterns
must be rejected by pattern parsing or pattern checking because equality around
`NaN` and precision must not be hidden inside pattern syntax.

### 9.7 Declarations and Clauses

All source-level item forms in
`etas/docs/design/09-syntax-principles.md` should have explicit AST
coverage.

Types and enums:

```rust
pub struct AliasDecl {
    pub name: Name,
    pub type_params: Vec<TypeParam>,
    pub target: TypeExpr,
    pub span: Span,
}

pub struct TypeDecl {
    pub name: Name,
    pub type_params: Vec<TypeParam>,
    pub body: TypeDeclBody,
    pub span: Span,
}

pub enum TypeDeclBody {
    Bodyless,
    Representation(TypeExpr),
}

pub struct EnumDecl {
    pub name: Name,
    pub type_params: Vec<TypeParam>,
    pub variants: Vec<EnumVariant>,
    pub span: Span,
}

pub struct EnumVariant {
    pub name: Name,
    pub fields: Vec<TypeExpr>,
    pub span: Span,
}
```

`AliasDecl` covers `alias A = B;` and introduces no type identity.
`TypeDecl` covers both `type A = B;` and `type A;`; both are nominal source
declarations. A record declaration such as `type Review = { ... }` is parsed as
`TypeDeclBody::Representation(TypeExpr::Record(...))`, not as a transparent
record alias. Syntax does not decide assignability or constructor visibility;
it only preserves the source keyword and representation/bodyless shape.

Effects and actions:

```rust
pub struct EffectDecl {
    pub name: Name,
    pub type_params: Vec<TypeParam>,
    pub extends: Option<EffectRef>,
    pub body: EffectBody,
    pub span: Span,
}

pub enum EffectBody {
    Empty {
        semicolon_span: Span,
    },
    Block {
        actions: Vec<EffectActionDecl>,
        span: Span,
    },
}

pub struct EffectRow {
    pub effects: Vec<EffectRef>,
    pub tail: Option<Path>,
    pub span: Span,
}

pub struct EffectRef {
    pub path: Path,
    pub action: Option<Name>,
    pub args: Vec<EffectArg>,
    pub span: Span,
}

pub enum EffectArg {
    Type(TypeExpr),
    Literal(Literal),
    Path(Path),
    Wildcard(Span),
    Error(Span),
}
```

The parser should preserve the current SPEC shape:

```text
EffectTag
EffectTag "." ActionName
EffectTag "." ActionName "[" EffectArgList "]"
EffectRowVar
```

Examples:

```etas
![Network]
![Web.search]
![Web.fetch["github.com"]]
![Workspace.write["reports/**"]]
![Memory.read[ProjectMemory.Papers]]
![Console.stdout_write]
![Error[IOError]]
![Console.stdout_write, E]
```

`Web.*` and other namespace wildcard forms are not source syntax. `_` is only an
argument wildcard inside an action pattern, for example `Web.fetch[_]`.
`E` in `![Console.stdout_write, E]` is an effect-row tail variable declared by a
generic parameter such as `effect E`; syntax records it as `EffectRow.tail`.

Tools:

```rust
pub struct ToolDecl {
    pub visibility: Visibility,
    pub path: Path,
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Param>,
    pub return_type: TypeExpr,
    pub effects: Option<EffectRow>,
    pub conformances: Vec<DeclarationConformance>,
    pub body: ToolBody,
    pub span: Span,
}

pub enum DeclarationConformanceTarget {
    Path(SpecRef),
    InlineTraceSpec(SpecExpr),
    Error(Span),
}

pub enum ToolBody {
    Source(FlowBody),
    Bodyless { semicolon_span: Span },
    Error(Span),
}
```

`tool ... { ... }` and `tool ...;` are the two tool source forms. A tool is
model-callable; a flow is not directly listed in `@tools([...])`. A
bodyless `tool ...;` is a package-interface or compiler/runtime-metadata
signature for a standard-library primitive, host-provided binding, generated
API, or precompiled package item. It is not introduced by an `extern` keyword.
The parser records optional effect rows and declaration conformance clauses.
`etas_effects` enforces default-deny behavior for source-bodied tools that omit
both explicit effects and an allowing trace-spec conformance. Bodyless tool
signatures must carry explicit effect/action metadata through the source row or
imported package metadata because the compiler cannot inspect an implementation
body.

Ordinary project implementation files should not use `tool ...;` as a local
unresolved declaration. A bodyless tool used by user code must be resolved by
normal import/name resolution to one of:

- a package interface export;
- generated package metadata;
- compiler-known standard-library metadata;
- a precompiled package item;
- runtime binding metadata declared by the package manifest.

If a bodyless tool signature has no resolved provider and implementation
binding, frontend checking must reject it. It must not be treated as a
successfully declared local tool with an implicit host implementation.

Agents:

```rust
pub struct AgentDecl {
    pub visibility: Visibility,
    pub name: Name,
    pub params: Vec<Param>,
    pub output_type: Option<TypeExpr>,
    pub effects: Option<EffectRow>,
    pub conformances: Vec<DeclarationConformance>,
    pub body: AgentBody,
    pub span: Span,
}

pub enum AgentBody {
    Source(Block),
    Decl { semicolon_span: Span },
    Error(Span),
}
```

Agent runtime metadata is expressed with item annotations such as `@model`,
`@tools`, `@limits`, `@trace`, and `@optimization`. These annotations are
ordinary metadata and do not grant authority; effect, trace-spec, and host
readiness checks remain independent.

An `agent` declaration is a single model-inference boundary:

```text
@model(model = Models.local)
@tools([Search])
agent Name(input: I) -> O { body_returning_Prompt }
agent ImportedAgent;
```

The agent body is a context harness and prompt assembly block; it must produce
the Agent/runtime support type `Prompt`. It is not a general orchestration
body. Ordinary multi-step control flow, approval gates, high-impact tool calls,
persistent memory writes, and post-output validation belong in `flow`
declarations around the agent call.

There is no source-level `prompt` declaration and no source-level `msg` or
`message` declaration. Prompt construction is ordinary expression syntax using
support values such as `Prompt`, `PromptPart`, and `PromptEncode[T]`.
Agent-to-agent communication is expressed with typed support values such as
`Message[T]`, `SessionConfig`, and `Conversation`, not with syntax nodes.

Top-level `let`:

```rust
pub struct TopLevelLetDecl {
    pub visibility: Visibility,
    pub name: Name,
    pub type_annotation: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
}
```

Top-level `let` is an item, not a block statement. It declares an immutable
module-level binding. Later semantic passes classify the initializer as either
a compile-time deterministic/effect-free constant or a compiler-known runtime
resource handle, for example `std.memory.region[Schema](...)`. Top-level
`var` is invalid source syntax and should recover with a diagnostic.

There is no source-level `memory` declaration. A persistent memory schema that
is only a transparent abbreviation is ordinary alias syntax such as
`alias ProjectMemorySchema = MemoryRegion[...]`; `type ProjectMemorySchema =
MemoryRegion[...]` is a nominal wrapper under the current SPEC. The region
handle is an ordinary top-level `let`; reads and writes are ordinary
std/runtime API calls that later type and effect passes classify.

Trace specs:

```rust
pub struct TraceSpecDecl {
    pub name: Name,
    pub type_params: Vec<TypeParam>,
    pub expr: SpecExpr,
    pub span: Span,
}

pub enum SpecExpr {
    Ref(SpecRef),
    ActionPattern(ActionPattern),
    Allow(ActionPattern),
    Deny(ActionPattern),
    Before { guard: ActionPattern, target: ActionPattern },
    After { target: ActionPattern, obligation: ActionPattern },
    And(Box<SpecExpr>, Box<SpecExpr>),
    Or(Box<SpecExpr>, Box<SpecExpr>),
    Error(Span),
}
```

Accepted trace-spec source forms include:

```etas
spec Production: trace =
    +Web.search
    & +Workspace.write<"reports/**">
    & -Command.run<_>
    & (Approval.request >> Email.send<WorkAccount>)
    & (Audit.write << Payment.charge<BillingAccount>);

flow Publish(...) ~ Production { ... }
```

The parser does not evaluate trace specs. It preserves spec refs, action
patterns, operators, spans, and recovery nodes. `etas_effects` later normalizes
action patterns into monitor facts.

`policy` blocks, `allow` / `deny` / `require` keywords, `follows`, and policy
`where` predicates are not current source syntax. They should be rejected or
recovered as obsolete syntax with a diagnostic that points users to
`spec Name: trace = ...` and `~ Name`.

Protocols:

```rust
pub struct ProtocolDecl {
    pub name: Name,
    pub messages: Vec<ProtocolMsg>,
    pub span: Span,
}

pub struct ProtocolMsg {
    pub from: Path,
    pub to: Path,
    pub payload: TypeExpr,
    pub span: Span,
}
```

Common declaration helpers:

```rust
pub enum Visibility {
    Private,
    Public,
}

pub struct Param {
    pub name: Name,
    pub ty: TypeExpr,
    pub span: Span,
}

pub struct DeclarationConformance {
    pub target: DeclarationConformanceTarget,
    pub span: Span,
}

pub enum DeclarationConformanceTarget {
    Path(SpecRef),
    InlineTraceSpec(SpecExpr),
    Error(Span),
}
```

## 10. Parser Recovery

The parser must be error-tolerant.

Recovery rules:

- Malformed item: emit `Item::Error` and recover at the next top-level item
  keyword or closing brace.
- Malformed statement: emit `Stmt::Error` and recover at `;` or block boundary.
- Malformed expression: emit `Expr::Error` and recover at a caller-provided
  delimiter such as `,`, `;`, `)`, `]`, or `}`.
- Missing expected token: emit `MissingToken` and continue when a reasonable
  recovery point exists.
- Unterminated string: emit `UnterminatedString`, produce a token, and let the
  parser continue.
- Unterminated block comment: emit `UnterminatedBlockComment`, produce a block
  comment trivia token spanning through EOF, and let the parser continue from
  EOF.
- Unclosed delimiter: emit `UnclosedDelimiter` with a primary span at the point
  where the closing delimiter was expected and a secondary label on the opening
  delimiter.

The parser should maintain enough recovery context to produce useful labels:

```text
currently parsing item kind
currently parsing statement kind
delimiter stack
expected token set
last stable recovery point
```

Diagnostics should avoid generic messages when context is known. Prefer:

```text
expected `}` to close agent block
```

over:

```text
unexpected end of file
```

Recovery quality is part of the LSP user experience and should be tested with
incomplete edit states, not only complete invalid programs.

## 11. AST Dump

`etas_syntax` should provide a deterministic AST dump facility.

Primary use cases:

- parser development;
- parser golden tests;
- debugging error recovery;
- explaining syntax tree shape to later compiler phases;
- future CLI commands such as `etas parse` or `etas dump-ast`;
- LSP/debug tooling that needs to inspect parsed syntax without semantic
  lowering.

The dump should be deterministic across runs. It should include:

- node kind;
- important field names;
- source span;
- token text for names and literals where useful;
- error nodes;
- syntax diagnostics when requested.

It should not include:

- resolved symbol ids;
- inferred types;
- inferred effects;
- AIR node ids;
- runtime trace data.

Suggested API:

```rust
pub struct DumpOptions {
    pub include_spans: bool,
    pub include_tokens: bool,
    pub include_diagnostics: bool,
}

pub trait AstDump {
    fn dump(&self, f: &mut AstDumpWriter);
}

pub struct AstDumpWriter {
    // implementation detail
}
```

Suggested default text shape:

```text
Program @0..128
  ModuleDecl path=Example @0..15
  FlowDecl name=BuildFeature @17..128
    Param name=brief ty=Path(FeatureBrief) @35..54
    ReturnType Path(DesignDoc) @59..68
    Block @69..128
      LetStmt name=prd @73..105
        PipelineExpr @83..104
          input Path(brief) @83..88
          stage Path(ProductManager) @92..104
      ReturnStmt @109..126
        PipelineExpr @116..125
          input Path(prd) @116..119
          stage Path(Architect) @123..125
```

The exact formatting can evolve, but it must remain stable enough for golden
tests. If a human-readable dump and a machine-readable dump are both needed,
prefer two explicit modes rather than making one format serve both poorly.

Potential future extension:

```text
etas dump-ast file.es --format text
etas dump-ast file.es --format json
```

JSON dump can be added later. The first version should prioritize a readable
stable text dump for tests and architecture debugging.

## 12. Model Names and Runtime Values

The syntax layer should not add special lexer rules for provider model names.

Prefer:

```etas
@model(model = "gpt-5.5-thinking")
agent Reviewer(input: Draft) -> Review {
    return ReviewPrompt(input);
}
```

or an ordinary imported value:

```etas
@model(model = Models.gpt_5_5_thinking)
agent Reviewer(input: Draft) -> Review {
    return ReviewPrompt(input);
}
```

Avoid requiring bare tokens such as:

```etas
@model(model = gpt-5.5-thinking)
agent Reviewer(input: Draft) -> Review {
    return ReviewPrompt(input);
}
```

Bare model names make `-` and `.` ambiguous with ordinary punctuation and
operators. Runtime configuration values should not complicate core lexing.

## 13. Implementation Scope

`etas_syntax` should be designed around the complete source grammar described
by the PL design documents. Implementation may be delivered incrementally, but
AST node families, token kinds, diagnostics, and dump formats should be shaped
for the complete language rather than only the first implementation tasks.

The first `etas_syntax` implementation should support:

- depend on and use `etas_core` `SourceFile`, `Span`, `TextRange`, and
  `LineIndex`;
- lexer for identifiers, keywords, literals, punctuation, `//` line comments,
  non-nested `/* ... */` block comments, and trivia;
- AST node definitions under `etas_syntax::ast`;
- parser coverage for the core source item set described by the PL design;
- parser recovery for common incomplete edit states;
- syntax diagnostics with spans;
- deterministic AST dump for parser golden tests;
- tests for lexing, parsing, and recovery.

The first implementation should not support:

- lossless concrete syntax trees;
- formatter-preserving trivia APIs beyond token-level trivia;
- documentation comments as a semantic or AST feature;
- nested block comments;
- name resolution;
- type-directed parsing;
- LSP position conversion;
- AIR lowering.
