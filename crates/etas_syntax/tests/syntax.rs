use etas_syntax::{
    CommentKind, DiagnosticCode, DumpOptions, Keyword, LabelStyle, Severity, SourceFile, SourceId,
    SyntaxDiagnosticCode, TokenKind, TriviaKind,
    ast::{
        EffectArg, EffectBody, Expr, FlowBody, ImportTree, Item, Stmt, TypeParamKind, Visibility,
    },
    dump_ast, dump_parse, lex, parse_program,
};

fn source(text: &str) -> SourceFile {
    SourceFile::new(SourceId(1), None, text)
}

#[test]
fn parses_item_annotations_as_structured_nodes() {
    let parsed = parse_program(source(
        r#"
@model(adapter = "omlx-openai", model = "local")
@tools([Search])
@limits([Tokens(128)])
agent Reviewer(input: string) -> string {
  return input;
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let item = &parsed.value.items[0];
    assert_eq!(item.annotations.len(), 3);
    assert_eq!(item.annotations[0].path.segments[0].text, "model");
    assert_eq!(item.annotations[1].path.segments[0].text, "tools");
    assert_eq!(item.annotations[2].path.segments[0].text, "limits");
    assert!(matches!(item.item, Item::Agent(_)));
    assert!(matches!(
        item.annotations[0].args[0],
        etas_syntax::ast::AnnotationArg::Named { .. }
    ));
    assert!(matches!(
        item.annotations[1].args[0],
        etas_syntax::ast::AnnotationArg::Positional(_)
    ));

    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("AnnotatedItem"), "{dump}");
    assert!(dump.contains("Annotation path=model"), "{dump}");
    assert!(dump.contains("AnnotationNamedArg"), "{dump}");
}

#[test]
fn parses_bodyless_agent_declaration() {
    let parsed = parse_program(source(
        r#"
agent Reviewer;
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Item::Agent(agent) = &parsed.value.items[0].item else {
        panic!("expected agent item");
    };
    assert!(agent.params.is_empty());
    assert!(agent.output_type.is_none());
    assert!(agent.effects.is_none());
    assert!(matches!(
        agent.body,
        etas_syntax::ast::AgentBody::Decl { .. }
    ));

    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("AgentDeclBody"), "{dump}");
}

#[test]
fn rejects_obsolete_agent_config_row() {
    let parsed = parse_program(source(
        r#"
agent Writer(input: string) -> string [model = "local"] {
  return input;
}
"#,
    ));

    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("agent config rows are obsolete")),
        "{:#?}",
        parsed.diagnostics
    );
    let Item::Agent(agent) = &parsed.value.items[0].item else {
        panic!("expected agent item");
    };
    assert!(matches!(agent.body, etas_syntax::ast::AgentBody::Source(_)));
}

#[test]
fn parses_spec_method_selection_without_breaking_list_cons() {
    let parsed = parse_program(source(
        r#"
flow main(prompt: Prompt) -> unit {
  prompt::PromptEncode.encode();
  let xs = 1 :: 2 :: [];
  return;
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("SpecMethodCallExpr"), "{dump}");
    assert!(dump.contains("spec=PromptEncode"), "{dump}");
    assert!(dump.contains("method=encode"), "{dump}");
    assert!(dump.contains("ListConsExpr"), "{dump}");
}

#[test]
fn parses_specs_spec_impls_and_effect_params() {
    let parsed = parse_program(source(
        r#"
spec ByteStream;

spec PromptEncode<T ~ Schema + ResponseDecode, effect E> ~ ByteStream {
  flow encode(input: T) -> string ![Console.stdout_write, E];
}

impl TlsStream ~ ByteStream;

impl ReportsRoot ~ Region + Within<ReportsRoot> + RegionWithin<ReportsRoot, WorkspaceRoot>;

impl MarkdownPrompt ~ PromptEncode<Message> {
  flow encode(input: MarkdownPrompt) -> string ![Console.stdout_write] {
    return "prompt";
  }
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(matches!(parsed.value.items[0].item, Item::Spec(_)));
    assert!(matches!(parsed.value.items[1].item, Item::Spec(_)));
    assert!(matches!(parsed.value.items[2].item, Item::Impl(_)));
    assert!(matches!(parsed.value.items[3].item, Item::Impl(_)));
    assert!(matches!(parsed.value.items[4].item, Item::Impl(_)));

    let Item::Spec(spec_decl) = &parsed.value.items[1].item else {
        panic!("expected spec declaration");
    };
    assert_eq!(spec_decl.type_params.len(), 2);
    assert_eq!(spec_decl.type_params[0].bounds.len(), 2);
    assert_eq!(spec_decl.type_params[1].kind, TypeParamKind::Effect);
    assert_eq!(spec_decl.bounds.len(), 1);

    let Item::Impl(marker_impl) = &parsed.value.items[2].item else {
        panic!("expected marker impl");
    };
    assert!(matches!(
        marker_impl.target,
        etas_syntax::ast::ImplTarget::SpecSatisfaction { .. }
    ));
    let Item::Impl(multi_impl) = &parsed.value.items[3].item else {
        panic!("expected multi-spec impl");
    };
    let etas_syntax::ast::ImplTarget::SpecSatisfaction { specs, .. } = &multi_impl.target else {
        panic!("expected spec impl target");
    };
    assert_eq!(specs.len(), 3);
    assert_eq!(specs[0].spec_path.segments[0].text, "Region");
    assert_eq!(specs[1].spec_path.segments[0].text, "Within");
    assert_eq!(specs[1].spec_args.len(), 1);

    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("SpecDecl"), "{dump}");
    assert!(dump.contains("SpecImplDecl"), "{dump}");
    assert!(dump.contains("ImplSpecRef"), "{dump}");
    assert!(
        dump.contains("type_params=<T~Schema+ResponseDecode,E ~effect>"),
        "{dump}"
    );
    assert!(dump.contains("bounds=<ByteStream>"), "{dump}");
}

#[test]
fn parses_legacy_impl_for_as_spec_satisfaction() {
    let parsed = parse_program(source(
        r#"
spec ByteStream;
spec Region;
spec Within<Root>;
type TlsStream = string;
type ReportsRoot;

impl ByteStream for TlsStream;
impl Region, Within<ReportsRoot> for ReportsRoot;
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Item::Impl(single_impl) = &parsed.value.items[5].item else {
        panic!("expected legacy single spec impl");
    };
    let etas_syntax::ast::ImplTarget::SpecSatisfaction { specs, .. } = &single_impl.target else {
        panic!("expected normalized spec satisfaction");
    };
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].spec_path.segments[0].text, "ByteStream");

    let Item::Impl(multi_impl) = &parsed.value.items[6].item else {
        panic!("expected legacy multi spec impl");
    };
    let etas_syntax::ast::ImplTarget::SpecSatisfaction { specs, .. } = &multi_impl.target else {
        panic!("expected normalized spec satisfaction");
    };
    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].spec_path.segments[0].text, "Region");
    assert_eq!(specs[1].spec_path.segments[0].text, "Within");
}

#[test]
fn parses_effect_row_generic_arguments_without_type_fallback() {
    let parsed = parse_program(source(
        r#"
flow wrap<effect E>() -> unit ![E] {
  f<![Console.stdout_write, E]>();
  return;
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Item::Flow(flow) = &parsed.value.items[0].item else {
        panic!("expected flow item");
    };
    let FlowBody::Block(block) = &flow.body else {
        panic!("expected block body");
    };
    let Stmt::Expr(expr) = &block.stmts[0] else {
        panic!("expected expression statement");
    };
    let Expr::Call(call) = &expr.expr else {
        panic!("expected call expression");
    };
    assert!(matches!(
        call.generic_args.as_slice(),
        [etas_syntax::ast::GenericArg::EffectRow(_)]
    ));

    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("EffectRowArg"), "{dump}");
}

#[test]
fn parses_flow_spec_signature_and_flow_satisfaction() {
    let parsed = parse_program(source(
        r#"
spec Pure<I, O> I => O ![];

flow Normalize(text: string) -> string ~ Pure {
  return text;
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Item::Spec(spec_decl) = &parsed.value.items[0].item else {
        panic!("expected spec declaration");
    };
    assert!(spec_decl.callable.is_some());
    let Item::Flow(flow) = &parsed.value.items[1].item else {
        panic!("expected flow declaration");
    };
    assert_eq!(flow.conformances.len(), 1);

    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("SpecCallableSignature"), "{dump}");
    assert!(dump.contains("DeclarationConformance"), "{dump}");
}

#[test]
fn parses_trace_spec_expression() {
    let parsed = parse_program(source(
        r#"
spec SafeEmail: trace =
  +Approval.request
  & -CompanyEmail.send<PersonalAccount>
  & (Approval.request >> CompanyEmail.send<WorkAccount>);
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Item::Spec(spec_decl) = &parsed.value.items[0].item else {
        panic!("expected spec declaration");
    };
    assert_eq!(spec_decl.kind, etas_syntax::ast::SpecKind::TraceSpec);
    assert!(spec_decl.trace.is_some());
    assert!(spec_decl.callable.is_none());

    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("kind=trace"), "{dump}");
    assert!(dump.contains("TraceAllow"), "{dump}");
    assert!(dump.contains("TraceDeny"), "{dump}");
    assert!(dump.contains("TraceBefore"), "{dump}");
}

#[test]
fn parses_effect_ref_argument_kinds_without_type_fallback() {
    let parsed = parse_program(source(
        r#"
flow main() -> unit ![Memory.read<ProjectMemory.Papers>, Command.run<DefaultCommandSandbox>, Console.stdout_write<_>, File.open<"/tmp/a">, Queue.publish<42>, Error<IOError>] {
  return;
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Item::Flow(flow) = &parsed.value.items[0].item else {
        panic!("expected flow item");
    };
    let effects = flow
        .declared_effects
        .as_ref()
        .expect("flow should carry effect suffix");

    assert!(matches!(effects.effects[0].args[0], EffectArg::Path(_)));
    assert!(matches!(effects.effects[1].args[0], EffectArg::Path(_)));
    assert!(matches!(
        effects.effects[2].args[0],
        EffectArg::Wildcard { .. }
    ));
    assert!(matches!(
        effects.effects[3].args[0],
        EffectArg::String { .. }
    ));
    assert!(matches!(effects.effects[4].args[0], EffectArg::Int { .. }));
    assert!(matches!(effects.effects[5].args[0], EffectArg::Path(_)));
}

#[test]
fn lexer_preserves_keywords_punctuation_and_trivia() {
    let file = source("/* block */\nflow main() -> unit { // comment\n  return; }\n");
    let token_stream = lex(&file);
    let tokens = &token_stream.tokens;

    assert!(
        tokens
            .iter()
            .any(|token| token.kind == TokenKind::Whitespace)
    );
    assert!(
        tokens
            .iter()
            .any(|token| token.kind == TokenKind::Comment(CommentKind::Line))
    );
    assert!(
        tokens
            .iter()
            .any(|token| token.kind == TokenKind::Comment(CommentKind::Block))
    );
    assert!(
        token_stream
            .trivia()
            .any(|trivia| trivia.kind == TriviaKind::BlockComment)
    );
    assert!(
        tokens
            .iter()
            .any(|token| token.kind == TokenKind::Keyword(Keyword::Flow))
    );
    assert!(
        tokens
            .iter()
            .any(|token| matches!(token.kind, TokenKind::Punct(_)))
    );
    assert!(
        lex(&source("#{x}"))
            .tokens
            .iter()
            .any(|token| token.kind == TokenKind::Punct(etas_syntax::Punct::Hash))
    );
    assert!(
        lex(&source("1 :: []"))
            .tokens
            .iter()
            .any(|token| token.kind == TokenKind::Punct(etas_syntax::Punct::ColonColon))
    );
}

#[test]
fn reports_unterminated_block_comment_as_lexer_diagnostic() {
    let parsed = parse_program(source(
        r#"
flow ok() -> unit { return; }
/* missing end
"#,
    ));

    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Syntax(SyntaxDiagnosticCode::UnterminatedBlockComment)
        })
        .expect("expected unterminated block comment diagnostic");

    assert_eq!(diagnostic.phase, etas_syntax::DiagnosticPhase::Lex);
    assert!(matches!(parsed.value.items[0].item, Item::Flow(_)));
    assert!(
        parsed
            .tokens
            .tokens
            .iter()
            .any(|token| { matches!(token.kind, TokenKind::Comment(CommentKind::Block)) })
    );
}

#[test]
fn parses_sequence_literals_without_defaulting_empty_sequence() {
    let parsed = parse_program(source(
        r#"
flow main() -> unit {
  let array = [1, 2, 3];
  let list = [1; 2; 3];
  let cons = 1 :: 2 :: [];
  let closed_open = [0, 10);
  let open_closed = (0, 10];
  let window = array[0, 2);
  let shifted = array(0, 2];
  let empty: Array<i32> = [];
  return;
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("ArrayExpr"), "{dump}");
    assert!(dump.contains("ListExpr"), "{dump}");
    assert_eq!(dump.matches("ListConsExpr").count(), 2, "{dump}");
    assert_eq!(dump.matches("RangeExpr").count(), 2, "{dump}");
    assert_eq!(dump.matches("SliceExpr").count(), 2, "{dump}");
    assert!(dump.contains("bounds=closed_open"), "{dump}");
    assert!(dump.contains("bounds=open_closed"), "{dump}");
    assert!(dump.contains("EmptySequenceExpr"), "{dump}");
}

#[test]
fn parses_postfix_try_as_dedicated_expr() {
    let parsed = parse_program(source(
        r#"
flow read_expr() -> Result<string, string> {
  let value = read()?;
  return value;
}

flow return_expr(path: string) -> Result<string, string> {
  return Load(path)?;
}

flow final_block_expr() -> Result<string, string> {
  read()?
}

flow whole_block_expr() -> Result<string, string> {
  return {
    let value = read();
    value
  }?;
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("TryExpr"), "{dump}");
    assert_eq!(dump.matches("TryExpr").count(), 4, "{dump}");
    assert!(dump.contains("BlockExpr"), "{dump}");
    assert!(!dump.contains("UnaryExpr op=?"), "{dump}");
}

#[test]
fn parses_expression_bodied_flow_as_source_shape() {
    let parsed = parse_program(source(
        r#"
flow add_one(value: i32) -> i32 = value + 1;
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let Item::Flow(flow) = &parsed.value.items[0].item else {
        panic!("expected flow item");
    };
    assert!(matches!(flow.body, FlowBody::Expr { .. }));
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("ExprBody"), "{dump}");
}

#[test]
fn rejects_prefix_question_as_invalid_syntax() {
    let parsed = parse_program(source(
        r#"
flow main() -> Result<string, string> {
  return ?read();
}
"#,
    ));

    assert!(
        !parsed.diagnostics.is_empty(),
        "prefix `?expr` must stay invalid syntax"
    );
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(!dump.contains("TryExpr"), "{dump}");
}

#[test]
fn parses_current_effect_suffixes_and_handler_types() {
    let parsed = parse_program(source(
        r#"
effect Approval;
effect Console extends FileIO;
effect Error<E> {
  action raise(error: E) -> never;
}

impl Approval {
  action request(message: string) -> bool;
}

type ApprovalHandler = ![Approval => [] for string];

tool web.search(q: string) -> string ![Network] ~ SearchPolicy;

agent Writer(input: string) -> string ![] {
  return input;
}

flow main() -> string ![Console.stdout_write, Error<IOError>] {
  return "";
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Item::Effect(console_effect) = &parsed.value.items[1].item else {
        panic!("expected console effect item");
    };
    let Item::Effect(approval_effect) = &parsed.value.items[0].item else {
        panic!("expected approval effect item");
    };
    let Item::Effect(error_effect) = &parsed.value.items[2].item else {
        panic!("expected error effect item");
    };
    assert!(matches!(approval_effect.body, EffectBody::Empty { .. }));
    assert!(matches!(console_effect.body, EffectBody::Empty { .. }));
    assert!(matches!(error_effect.body, EffectBody::Block { .. }));
    assert!(
        console_effect.extends.is_some(),
        "effect extends clause should be preserved"
    );
    let Item::Type(handler_ty) = &parsed.value.items[4].item else {
        panic!("expected handler type alias");
    };
    assert!(matches!(
        handler_ty.body,
        etas_syntax::ast::TypeDeclBody::Representation(etas_syntax::ast::TypeExpr::Handler(_))
    ));
    let Item::Tool(tool) = &parsed.value.items[5].item else {
        panic!("expected tool item");
    };
    assert!(
        tool.effects.is_some(),
        "tool effect suffix should be preserved"
    );
    let Item::Agent(agent) = &parsed.value.items[6].item else {
        panic!("expected agent item");
    };
    assert!(
        agent.effects.is_some(),
        "agent effect suffix should be preserved"
    );
    let Item::Flow(flow) = &parsed.value.items[7].item else {
        panic!("expected flow item");
    };
    assert!(
        flow.declared_effects.is_some(),
        "flow effect suffix should be preserved"
    );

    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("HandlerType"), "{dump}");
    assert!(dump.contains("EffectSuffix"), "{dump}");
    assert!(dump.contains("Produced"), "{dump}");
}

#[test]
fn parses_tool_source_body_and_declaration_body_distinctly() {
    let parsed = parse_program(source(
        r#"
tool local.echo(input: string) -> string {
  return input;
}

tool remote.echo(input: string) -> string ![Network];
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Item::Tool(local) = &parsed.value.items[0].item else {
        panic!("expected source tool");
    };
    assert!(matches!(local.body, etas_syntax::ast::ToolBody::Source(_)));
    let Item::Tool(remote) = &parsed.value.items[1].item else {
        panic!("expected declaration tool");
    };
    assert!(matches!(
        remote.body,
        etas_syntax::ast::ToolBody::Decl { .. }
    ));
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("ToolSourceBody"), "{dump}");
    assert!(dump.contains("ToolDeclBody"), "{dump}");
}

#[test]
fn parses_handler_transformer_type_forms() {
    let parsed = parse_program(source(
        r#"
type HandleOnly = ![Approval];
type HandleFor = ![Approval for string];
type Transform = ![Approval => Network];
type TransformFor = ![Approval => Network, Console.stdout_write for Result<string, IOError>];
type EmptyProduced = ![Approval => []];
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let handlers = parsed
        .value
        .items
        .iter()
        .filter_map(|item| match &item.item {
            Item::Type(ty) => match &ty.body {
                etas_syntax::ast::TypeDeclBody::Representation(
                    etas_syntax::ast::TypeExpr::Handler(handler),
                ) => Some(handler),
                other => panic!("expected handler type, got {other:?}"),
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(handlers.len(), 5);
    assert!(matches!(
        handlers[0].produced,
        etas_syntax::ast::HandlerProducedEffects::Infer
    ));
    assert!(handlers[0].result.is_none());
    assert!(handlers[1].result.is_some());
    assert!(matches!(
        handlers[2].produced,
        etas_syntax::ast::HandlerProducedEffects::Explicit(ref row)
            if row.effects.len() == 1
    ));
    assert!(matches!(
        handlers[3].produced,
        etas_syntax::ast::HandlerProducedEffects::Explicit(ref row)
            if row.effects.len() == 2
    ));
    assert!(handlers[3].result.is_some());
    assert!(matches!(
        handlers[4].produced,
        etas_syntax::ast::HandlerProducedEffects::Explicit(ref row)
            if row.effects.is_empty()
    ));
}

#[test]
fn rejects_obsolete_effect_clause_forms() {
    let parsed = parse_program(source(
        r#"
tool web.search(q: string) -> string
  effect [Network]
  require Capability("host.network.search");

agent Writer(input: string) -> string ![Agentic] {
  return input;
}

flow main() -> string
  effect [ConsoleIO]
{
  return "";
}
"#,
    ));

    let obsolete = parsed
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == DiagnosticCode::Syntax(SyntaxDiagnosticCode::UnexpectedToken)
                && diagnostic.message.contains("obsolete")
        })
        .count();
    assert_eq!(
        obsolete, 3,
        "old effect/capability clause forms should all be rejected: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_source_level_policy_declaration_as_obsolete_syntax() {
    let parsed = parse_program(source(
        r#"
policy Publish {
  require Web.post where tenant_allowed;
}
"#,
    ));

    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("`policy` declarations are obsolete")),
        "{:#?}",
        parsed.diagnostics
    );
    assert!(matches!(parsed.value.items[0].item, Item::Error(_)));
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(!dump.contains("PolicyDecl"), "{dump}");
    assert!(
        !dump.contains("WhereRequirement"),
        "obsolete policy blocks must not produce formal policy AST nodes: {dump}"
    );
}

#[test]
fn rejects_policy_pattern_bound_effect_arg_as_obsolete_policy_syntax() {
    let parsed = parse_program(source(
        r#"
spec Within<Root>;
type ReportsRoot;

effect FixtureFs {
  action read<R>() -> unit;
}

policy ReportsOnly {
  allow FixtureFs.read<R ~ Within<ReportsRoot>>;
}
"#,
    ));

    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("`policy` declarations are obsolete")),
        "{:#?}",
        parsed.diagnostics
    );
    let Item::Effect(effect) = &parsed.value.items[2].item else {
        panic!("expected effect item");
    };
    let etas_syntax::ast::EffectBody::Block { actions, .. } = &effect.body else {
        panic!("expected effect block");
    };
    assert_eq!(actions[0].type_params.len(), 1);
    assert!(matches!(parsed.value.items[3].item, Item::Error(_)));

    let dump = dump_ast(
        &parsed.value,
        DumpOptions {
            include_tokens: true,
            ..DumpOptions::default()
        },
    );
    assert!(!dump.contains("PolicyBoundEffectArg"), "{dump}");
}

#[test]
fn rejects_custom_policy_statements_as_unsupported_syntax() {
    let parsed = parse_program(source(
        r#"
policy Publish {
  audit TenantPolicy;
}
"#,
    ));

    assert!(
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Syntax(SyntaxDiagnosticCode::InvalidItem)
                && diagnostic
                    .message
                    .contains("`policy` declarations are obsolete")
        }),
        "custom policy statements are covered by obsolete policy recovery: {:#?}",
        parsed.diagnostics
    );
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(
        !dump.contains("CustomPolicyStmt"),
        "custom policy statements must not remain valid AST nodes: {dump}"
    );
}

#[test]
fn rejects_source_level_capability_requirement_as_obsolete_syntax() {
    let parsed = parse_program(source(
        r#"
tool shell(cmd: string) -> string ![Command]
  require Capability("host.shell.sandboxed");
"#,
    ));

    assert!(
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Syntax(SyntaxDiagnosticCode::UnexpectedToken)
                && diagnostic.message.contains("Capability")
                && diagnostic.message.contains("obsolete")
        }),
        "source-level Capability requirements must not remain valid syntax: {:#?}",
        parsed.diagnostics
    );
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(
        !dump.contains("ExprRequirement"),
        "`Capability(...)` must not lower as a valid requirement expression: {dump}"
    );
}

#[test]
fn rejects_source_level_sandbox_requirement_as_obsolete_syntax() {
    let parsed = parse_program(source(
        r#"
tool shell(cmd: string) -> string ![Command]
  require Sandbox(DefaultCommandSandbox);
"#,
    ));

    assert!(
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Syntax(SyntaxDiagnosticCode::UnexpectedToken)
                && diagnostic.message.contains("Sandbox")
                && diagnostic.message.contains("Command.run")
        }),
        "source-level Sandbox requirements must point users to action parameters: {:#?}",
        parsed.diagnostics
    );
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(
        !dump.contains("ExprRequirement"),
        "`Sandbox(...)` must not lower as a valid requirement expression: {dump}"
    );
}

#[test]
fn parses_handler_values_and_handle_arguments() {
    let parsed = parse_program(source(
        r#"
flow main() -> unit {
  let reusable: ![Approval] = handler {
    Approval.request(req) => {
      resume true;
    }
    Error<IndexError>.raise(err) => {
      finish "fallback";
    }
  };

  perform Approval.request("ship") with reusable;

  handle {
    perform Approval.request("ship");
  } with {
    Approval.request(req) => resume true;
  };

  perform Approval.request("ship") with {
    Approval.request(req) => resume true;
  };

  return;
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("HandlerExpr"), "{dump}");
    assert!(dump.contains("HandlerBlockArg"), "{dump}");
    assert!(dump.contains("HandlerArmStmtBody"), "{dump}");
    assert!(dump.contains("FinishStmt"), "{dump}");
    assert!(!dump.contains("InlineHandlerArg"), "{dump}");
    assert_eq!(dump.matches("HandleExpr").count(), 3, "{dump}");
}

#[test]
fn parses_handler_block_after_with() {
    let parsed = parse_program(source(
        r#"
flow main() -> unit {
  perform Approval.request("ship") with {
    Approval.request(req) => resume true;
  };
  return;
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("HandlerBlockArg"), "{dump}");
}

#[test]
fn parses_optional_semicolon_after_block_statements() {
    let parsed = parse_program(source(
        r#"
flow main(values: Array<string>) -> unit {
  for value in values limit Iterations(8) {
    if value == "" {
      return;
    };
  };
  while false {
    return;
  };
  retry limit Attempts(1) {
    return;
  };
  return;
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("ForStmt"), "{dump}");
    assert!(dump.contains("WhileStmt"), "{dump}");
    assert!(dump.contains("RetryStmt"), "{dump}");
}

#[test]
fn parses_flow_trailing_handler() {
    let parsed = parse_program(source(
        r#"
flow main() -> string {
  perform Approval.request("ship");
} with {
  Approval.request(req) => finish "fallback";
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("TrailingHandler"), "{dump}");
    assert!(dump.contains("HandlerArmStmtBody"), "{dump}");
}

#[test]
fn rejects_bare_handler_arm_block_expression() {
    let parsed = parse_program(source(
        r#"
flow main() -> unit {
  let invalid = {
    Approval.request(req) => {
      resume true;
    }
  };
  return;
}
"#,
    ));

    assert!(
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Syntax(SyntaxDiagnosticCode::UnexpectedToken)
                || diagnostic.code == DiagnosticCode::Syntax(SyntaxDiagnosticCode::MissingToken)
                || diagnostic.code
                    == DiagnosticCode::Syntax(SyntaxDiagnosticCode::InvalidExpression)
        }),
        "{:#?}",
        parsed.diagnostics
    );
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(!dump.contains("HandlerExpr"), "{dump}");
}

#[test]
fn rejects_prefix_try_expression() {
    let parsed = parse_program(source(
        r#"
flow main(value: string) -> unit {
  let invalid = ?value;
  return;
}
"#,
    ));

    assert!(
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Syntax(SyntaxDiagnosticCode::InvalidExpression)
        }),
        "{:?}",
        parsed.diagnostics
    );
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(!dump.contains("TryExpr"), "{dump}");
}

#[test]
fn map_entry_separator_does_not_consume_a_key_as_lambda_parameters() {
    let parsed = parse_program(source(
        r#"
flow main() {
    let entries = { key => 10, (left, right) => 20, key + 1 => 30,
        (x => x) => 40, key => x => x, key => { nested => 50 }, };
    let ordinary_lambda = x => x;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let Item::Flow(flow) = &parsed.value.items[0].item else {
        panic!("flow");
    };
    let Stmt::Let(binding) = &flow.body.stmts[0] else {
        panic!("binding");
    };
    let Expr::Map(map) = &binding.value else {
        panic!("map");
    };
    assert_eq!(map.entries.len(), 6);
    assert!(matches!(map.entries[0].key, Expr::Path(_)));
    assert!(matches!(map.entries[1].key, Expr::Tuple { .. }));
    assert!(matches!(map.entries[2].key, Expr::Binary { .. }));
    assert!(matches!(map.entries[3].key, Expr::Lambda(_)));
    assert!(matches!(map.entries[4].value, Expr::Lambda(_)));
    assert!(matches!(map.entries[5].value, Expr::Map(_)));
    let Stmt::Let(binding) = &flow.body.stmts[1] else {
        panic!("lambda binding");
    };
    assert!(matches!(binding.value, Expr::Lambda(_)));
}

#[test]
fn parses_map_set_and_empty_brace_literals_without_defaulting_empty_braces() {
    let parsed = parse_program(source(
        r#"
flow main() -> unit {
  let map = { "alice" => 10, "bob" => 8 };
  let set = #{1, 2, 3};
  let empty_map: Map<string, i32> = {};
  let empty_set: Set<i32> = #{};
  return;
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("MapExpr"), "{dump}");
    assert!(dump.contains("SetExpr"), "{dump}");
    assert!(dump.contains("EmptyRecordOrMapExpr"), "{dump}");
}

#[test]
fn parses_literal_patterns_and_rejects_float_patterns() {
    let parsed = parse_program(source(
        r#"
flow classify(value: i32) -> string {
  return match value {
    0 => "zero",
    1 => "one",
    _ => "many",
  };
}

flow boolean(value: bool) -> string {
  return match value {
    true => "yes",
    false => "no",
  };
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("LiteralPattern"), "{dump}");
    assert!(dump.contains("BoolLiteral value=true"), "{dump}");
    assert!(dump.contains("IntLiteral text=0"), "{dump}");

    let invalid = parse_program(source(
        r#"
flow invalid(value: f64) -> string {
  return match value {
    1.0 => "one",
    _ => "other",
  };
}
"#,
    ));

    assert!(
        invalid.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("floating-point literal patterns")),
        "{:#?}",
        invalid.diagnostics
    );
}

#[test]
fn parses_mvp_item_set_and_flow_body_forms() {
    let text = r#"
module project.demo;
import host.fs as fs;

type Url = string;
type Pair<T> = { public left: T, right: T };

enum Choice<T> {
  Some(T);
  None;
}

effect Approval extends Control {
  action request(req: ApprovalRequest) -> ApprovalDecision;
}

impl Approval {
  action audit(req: ApprovalRequest, decision: ApprovalDecision) -> unit;
}

let ProjectMemory: MemoryRegion<{
  Drafts: Store<string, Draft>
}> = std.memory.region<MemoryRegion<{
  Drafts: Store<string, Draft>
}>>(stable_id = "project_memory");

tool fs.write(path: Path, content: string) -> unit ![FileIO] ~ WorkspaceWritePolicy;

@model(model = "gpt-5.5")
agent Writer(input: { topic: string }) -> Draft ![] {
  return input.topic;
}

spec Publish;

protocol Review {
  Writer -> Reviewer: Draft;
}

flow normalize<T>(title: string) -> string ![] ~ Publish
{
  let normalize: string -> string = (value: string) => trim(value);
  var retries: u32 = 0;

  if retries < 3 {
    retries = retries + 1;
  }

  match title {
    _ => title,
  }

  for item in items limit Iterations(10) {
    continue;
  }

  while ready limit Iterations(1) {
    break;
  }

  retry limit Attempts(2) {
    resume title;
  }

  handle {
    perform Approval.request(req);
  } with {
    Approval.request(req) => {
      resume fallback;
    }
  };

  let pipeline = Researcher | Writer | Publisher;
  let result = brief ~> ProductManager ~> Engineer limit Tokens(12000);
  return normalize(title);
}
"#;

    let parsed = parse_program(source(text));
    assert!(
        parsed.diagnostics.is_empty(),
        "unexpected diagnostics: {:#?}",
        parsed.diagnostics
    );
    assert!(parsed.value.module.is_some());
    assert_eq!(parsed.value.imports.len(), 1);
    assert_eq!(parsed.value.items.len(), 11);

    assert!(matches!(parsed.value.items[0].item, Item::Type(_)));
    assert!(matches!(parsed.value.items[1].item, Item::Type(_)));
    assert!(matches!(parsed.value.items[2].item, Item::Enum(_)));
    assert!(matches!(parsed.value.items[3].item, Item::Effect(_)));
    assert!(matches!(parsed.value.items[4].item, Item::Impl(_)));
    assert!(matches!(parsed.value.items[5].item, Item::TopLevelLet(_)));
    assert!(matches!(parsed.value.items[6].item, Item::Tool(_)));
    assert!(matches!(parsed.value.items[7].item, Item::Agent(_)));
    assert!(matches!(parsed.value.items[8].item, Item::Spec(_)));
    assert!(matches!(parsed.value.items[9].item, Item::Protocol(_)));
    assert!(matches!(parsed.value.items[10].item, Item::Flow(_)));
    let Item::Agent(agent) = &parsed.value.items[7].item else {
        panic!("expected agent item");
    };
    assert!(
        agent.effects.is_some(),
        "agent effect suffix should be stored as the declaration effect row"
    );
}

#[test]
fn recovers_obsolete_memory_item_as_error() {
    let parsed = parse_program(source(
        r#"
memory ProjectMemory {
  Drafts: Store<string, Draft>;
}
"#,
    ));

    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("obsolete source syntax"))
    );
    assert!(matches!(parsed.value.items[0].item, Item::Error(_)));
}

#[test]
fn rejects_top_level_var_as_error_item() {
    let parsed = parse_program(source(
        r#"
var cache = 1;
"#,
    ));

    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("top-level `var` is invalid"))
    );
    assert!(matches!(parsed.value.items[0].item, Item::Error(_)));
}

#[test]
fn parses_import_trees_for_supported_module_forms() {
    let parsed = parse_program(source(
        r#"
import std.io;
import std.io as io;
import std.io.println;
import std.io.println as log;
import std.io.{print, println, eprintln};
import std.io.{println as log, read_line,};
import std.io.*;
public import std.io.{println, eprintln};
public import std.prelude.*;

flow main() -> unit {
  return;
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert_eq!(parsed.value.imports.len(), 9);
    assert_eq!(parsed.value.imports[0].visibility, Visibility::Private);
    assert_eq!(parsed.value.imports[7].visibility, Visibility::Public);

    assert!(matches!(
        &parsed.value.imports[0].tree,
        ImportTree::Single { path, alias: None, .. }
            if path.segments.iter().map(|segment| segment.text.as_str()).collect::<Vec<_>>()
                == ["std", "io"]
    ));
    assert!(matches!(
        &parsed.value.imports[1].tree,
        ImportTree::Single { alias: Some(alias), .. } if alias.text == "io"
    ));
    assert!(matches!(
        &parsed.value.imports[4].tree,
        ImportTree::Group { prefix, items, .. }
            if prefix.segments.iter().map(|segment| segment.text.as_str()).collect::<Vec<_>>()
                == ["std", "io"]
                && items.len() == 3
                && items[0].name.text == "print"
    ));
    assert!(matches!(
        &parsed.value.imports[5].tree,
        ImportTree::Group { items, .. }
            if items.len() == 2
                && items[0].name.text == "println"
                && items[0].alias.as_ref().is_some_and(|alias| alias.text == "log")
                && items[1].name.text == "read_line"
    ));
    assert!(matches!(
        &parsed.value.imports[6].tree,
        ImportTree::Wildcard { prefix, .. }
            if prefix.segments.iter().map(|segment| segment.text.as_str()).collect::<Vec<_>>()
                == ["std", "io"]
    ));
    assert!(matches!(
        &parsed.value.imports[8].tree,
        ImportTree::Wildcard { prefix, .. }
            if prefix.segments.iter().map(|segment| segment.text.as_str()).collect::<Vec<_>>()
                == ["std", "prelude"]
    ));
}

#[test]
fn parses_tool_clause_and_rejects_obsolete_policy_declaration() {
    let parsed = parse_program(source(
        r#"
tool web.search(q: string) -> List<WebPage> ![Network] ~ SearchPolicy;

policy Gate {
  require Approval before [Network];
}
"#,
    ));

    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("`policy` declarations are obsolete")),
        "{:#?}",
        parsed.diagnostics
    );
    let Item::Tool(tool) = &parsed.value.items[0].item else {
        panic!("expected tool item");
    };
    assert!(
        tool.effects.is_some(),
        "tool effect suffix should not be stored as a clause"
    );
    assert_eq!(tool.conformances.len(), 1);

    assert!(matches!(parsed.value.items[1].item, Item::Error(_)));
}

#[test]
fn keeps_error_nodes_and_reports_recovery_diagnostics() {
    let parsed = parse_program(source(
        r#"
flow broken() -> unit {
  let value = ;
  return;
"#,
    ));

    assert!(!parsed.diagnostics.is_empty());
    let Item::Flow(flow) = &parsed.value.items[0].item else {
        panic!("expected recovered flow item");
    };
    assert!(
        flow.body
            .stmts
            .iter()
            .any(|stmt| matches!(stmt, Stmt::Let(_)))
    );
}

#[test]
fn malformed_statement_recovers_as_stmt_error() {
    let parsed = parse_program(source(
        r#"
flow broken() {
  @;
  return;
}
"#,
    ));

    assert!(!parsed.diagnostics.is_empty());
    let Item::Flow(flow) = &parsed.value.items[0].item else {
        panic!("expected recovered flow item");
    };
    assert!(
        flow.body
            .stmts
            .iter()
            .any(|stmt| matches!(stmt, Stmt::Error(_)))
    );
    assert!(
        flow.body
            .stmts
            .iter()
            .any(|stmt| matches!(stmt, Stmt::Return(_)))
    );
}

#[test]
fn malformed_top_level_item_recovers_as_item_error() {
    let parsed = parse_program(source(
        r#"
???
flow next() { return; }
"#,
    ));

    assert!(matches!(parsed.value.items[0].item, Item::Error(_)));
    assert!(matches!(parsed.value.items[1].item, Item::Flow(_)));
}

#[test]
fn rejects_action_declaration_wildcard_selector() {
    let parsed = parse_program(source(
        r#"
effect EdkHttp {
  action request<_>(request: string) -> string;
}
"#,
    ));

    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("action selector declaration cannot use `_`")),
        "{:#?}",
        parsed.diagnostics
    );
    let Item::Effect(effect) = &parsed.value.items[0].item else {
        panic!("expected effect item");
    };
    assert!(
        effect.body.actions()[0].selector_params.is_empty(),
        "`action request<_>` must not lower `_` into an action selector parameter"
    );
}

#[test]
fn parses_record_tuple_list_lambda_and_pipeline_expressions() {
    let parsed = parse_program(source(
        r#"
flow expressions() {
  let record = Draft { topic, body = "text" };
  let tuple = (record, 1);
  let list = [tuple, tuple];
  let lambda = input => input;
  let typed = map<string>(list);
  let draft = Writer.run(topic);
  let out = topic ~> Researcher ~> Writer limit Tokens(4096);
  out
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Item::Flow(flow) = &parsed.value.items[0].item else {
        panic!("expected flow");
    };
    assert!(flow.body.final_expr.is_some());
    assert!(flow.body.stmts.iter().any(
        |stmt| matches!(stmt, Stmt::Let(let_stmt) if matches!(let_stmt.value, Expr::Lambda(_)))
    ));
    assert!(flow.body.stmts.iter().any(
        |stmt| matches!(stmt, Stmt::Let(let_stmt) if matches!(let_stmt.value, Expr::MethodCall(_)))
    ));
}

#[test]
fn parses_dotted_call_type_arguments_with_angle_brackets() {
    let parsed = parse_program(source(
        r#"
flow main(message: Message<string>) -> unit {
  message.cast<string>();
  return;
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(dump.contains("CallExpr"), "{dump}");
    assert!(dump.contains("PathExpr path=message.cast"), "{dump}");
    assert!(dump.contains("TypeArg"), "{dump}");
}

#[test]
fn rejects_pipeline_stage_with_options() {
    let parsed = parse_program(source(
        r#"
flow main(topic: string) -> string {
  topic ~> Writer with { model = Models.fast }
}
"#,
    ));

    assert!(
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Syntax(SyntaxDiagnosticCode::UnexpectedToken)
                || diagnostic.code
                    == DiagnosticCode::Syntax(SyntaxDiagnosticCode::InvalidExpression)
                || diagnostic.code == DiagnosticCode::Syntax(SyntaxDiagnosticCode::MissingToken)
        }),
        "{:#?}",
        parsed.diagnostics
    );
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(!dump.contains("StageOptions"), "{dump}");
}

#[test]
fn parses_assignment_statements_before_record_expression_recovery() {
    let parsed = parse_program(source(
        r#"
flow update() -> i32 {
  var low: i32 = 0;
  var high: i32 = 4;
  if low < high {
    low = low + 1;
  } else {
    high = high - 1;
  }
  return low;
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Item::Flow(flow) = &parsed.value.items[0].item else {
        panic!("expected flow");
    };
    let Stmt::If(if_stmt) = &flow.body.stmts[2] else {
        panic!("expected if statement");
    };
    assert!(matches!(
        if_stmt.then_branch.stmts.first(),
        Some(Stmt::Assign(_))
    ));
    let Some(etas_syntax::ast::ElseBranch::Block(else_branch)) = &if_stmt.else_branch else {
        panic!("expected else block");
    };
    assert!(matches!(else_branch.stmts.first(), Some(Stmt::Assign(_))));
}

#[test]
fn reports_unclosed_delimiter_with_related_opening_span_and_suggestion() {
    let parsed = parse_program(source(
        r#"
flow broken() -> unit {
  if ready {
    return;
"#,
    ));

    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.code == DiagnosticCode::Syntax(SyntaxDiagnosticCode::UnclosedDelimiter)
        })
        .expect("expected unclosed delimiter diagnostic");

    assert_eq!(diagnostic.severity, Severity::Error);
    assert!(diagnostic.labels.iter().any(|label| {
        label.style == LabelStyle::Secondary && label.message.contains("opened here")
    }));
    assert!(
        diagnostic
            .suggestions
            .iter()
            .any(|suggestion| { suggestion.edits.iter().any(|edit| edit.replacement == "}") })
    );
}

#[test]
fn rejects_obsolete_follows_flow_clause() {
    let parsed = parse_program(source(
        r#"
flow normalize(title: string) -> string
  follows Review
{
  return title;
}
"#,
    ));

    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("`follows` is obsolete")),
        "{:#?}",
        parsed.diagnostics
    );
    let Item::Flow(flow) = &parsed.value.items[0].item else {
        panic!("expected flow item");
    };
    assert!(matches!(
        flow.conformances.as_slice(),
        [etas_syntax::ast::DeclarationConformance {
            target: etas_syntax::ast::DeclarationConformanceTarget::Error(_),
            ..
        }]
    ));
}

#[test]
fn rejects_obsolete_follows_tool_clause() {
    let parsed = parse_program(source(
        r#"
tool Search(q: string) -> string
  follows policy {
    allow [Network];
  };
"#,
    ));

    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("`follows` is obsolete")),
        "{:#?}",
        parsed.diagnostics
    );
    let Item::Tool(tool) = &parsed.value.items[0].item else {
        panic!("expected tool item");
    };
    assert!(tool.conformances.is_empty());
}

#[test]
fn emits_structured_suggestion_for_obsolete_fun_keyword_scan() {
    let parsed = parse_program(source("fun main() -> unit { return; }"));

    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("obsolete"))
        .expect("expected obsolete fun diagnostic");
    assert!(diagnostic.suggestions.iter().any(|suggestion| {
        suggestion
            .edits
            .iter()
            .any(|edit| edit.replacement == "flow")
    }));

    let not_keyword = parse_program(source(
        r#"
// fun in a comment is trivia.
flow f() {
  let text = "fun";
  let value = refund;
  value
}
"#,
    ));
    assert!(
        !not_keyword
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("obsolete"))
    );
}

#[test]
fn dumps_ast_in_stable_text_form() {
    let parsed = parse_program(source(
        r#"
module project.demo;
import host.fs as fs;

flow build(brief: FeatureBrief) -> DesignDoc {
  let prd = brief ~> ProductManager;
  return prd ~> Architect;
}
"#,
    ));

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);

    let dump = dump_ast(
        &parsed.value,
        DumpOptions {
            include_spans: false,
            include_tokens: true,
            include_diagnostics: false,
        },
    );

    assert_eq!(
        dump,
        "\
Program
  ModuleDecl path=project.demo
  ImportDecl visibility=private
    ImportSingle path=host.fs alias=fs
  FlowDecl name=build
    Param name=brief
      PathType path=FeatureBrief
    ReturnType
      PathType path=DesignDoc
    Block
      LetStmt
        IdentPattern name=prd
        Value
          PipelineExpr
            Input
              PathExpr path=brief
            PipelineStage
              PathExpr path=ProductManager
      ReturnStmt
        PipelineExpr
          Input
            PathExpr path=prd
          PipelineStage
            PathExpr path=Architect
"
    );

    let structural_dump = dump_ast(
        &parsed.value,
        DumpOptions {
            include_spans: true,
            include_tokens: false,
            include_diagnostics: false,
        },
    );
    assert!(structural_dump.contains("Program @"));
    assert!(!structural_dump.contains("name=build"));
    assert!(!structural_dump.contains("path=project.demo"));
}

#[test]
fn dump_parse_can_include_syntax_diagnostics() {
    let parsed = parse_program(source(
        r#"
flow broken() -> unit {
  if ready {
    return;
"#,
    ));

    let dump = dump_parse(
        &parsed,
        DumpOptions {
            include_spans: true,
            include_tokens: true,
            include_diagnostics: true,
        },
    );

    assert!(dump.contains("Diagnostics"));
    assert!(dump.contains("Diagnostic code=Syntax(UnclosedDelimiter)"));
    assert!(dump.contains("Label style=secondary"));
    assert!(dump.contains("Suggestion title="));
}

#[test]
fn rejects_inline_policy_declaration_conformance_as_obsolete_syntax() {
    let parsed = parse_program(source(
        r#"
flow guarded() -> unit ![] ~ policy {
  allow [Console];
} {
  return;
}

tool Search() -> unit ~ policy {
  allow [Network];
};
"#,
    ));

    assert!(
        parsed
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .message
                .contains("`policy` inline conformance is obsolete"))
            .count()
            == 2,
        "{:#?}",
        parsed.diagnostics
    );
    let Item::Flow(flow) = &parsed.value.items[0].item else {
        panic!("expected flow item");
    };
    assert!(matches!(
        flow.conformances.as_slice(),
        [etas_syntax::ast::DeclarationConformance {
            target: etas_syntax::ast::DeclarationConformanceTarget::Error(_),
            ..
        }]
    ));
    let Item::Tool(tool) = &parsed.value.items[1].item else {
        panic!("expected tool item");
    };
    assert!(matches!(
        tool.conformances.as_slice(),
        [etas_syntax::ast::DeclarationConformance {
            target: etas_syntax::ast::DeclarationConformanceTarget::Error(_),
            ..
        }]
    ));

    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(!dump.contains("InlinePolicyConformance"), "{dump}");
}

#[test]
fn rejects_invalid_character_literal_without_fabricating_nul() {
    let parsed = parse_program(source(
        r#"
flow invalid() -> char {
  return 'ab';
}
"#,
    ));

    assert!(
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Syntax(SyntaxDiagnosticCode::InvalidExpression)
                && diagnostic
                    .message
                    .contains("character literal must contain exactly one valid character")
        }),
        "{:#?}",
        parsed.diagnostics
    );
    let dump = dump_ast(&parsed.value, DumpOptions::default());
    assert!(!dump.contains("value=\\0"), "{dump}");
}
