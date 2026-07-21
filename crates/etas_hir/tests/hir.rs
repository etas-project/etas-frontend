use etas_core::{DiagnosticCode, NameDiagnosticCode, SourceFile, SourceId};
use etas_hir::{
    HirBodyKind, HirDumpOptions, HirEffectArg, HirEffectBody, HirElseBranch, HirExpr, HirFlowBody,
    HirImportKind, HirItem, HirNodeRef, HirOrigin, HirOwner, HirStmt, HirToolBody, HirTreeChild,
    HirTreeIndexDiagnostic, HirTreeNodeKind, HirTreeView, HirVisitor, ImportAliasOrigin,
    PartialResolutionReason, PatternBindingOwner, ResolveResult, ScopeOwner, SymbolDef, SymbolKind,
    Visibility, dump_hir, lower_program, walk_module,
};
use etas_syntax::parse_program;

fn source(text: &str) -> SourceFile {
    SourceFile::new(SourceId(1), None, text)
}

fn lower(text: &str) -> etas_hir::HirProgram {
    let parsed = parse_program(source(text));
    assert!(
        parsed.diagnostics.is_empty(),
        "parse diagnostics: {:#?}",
        parsed.diagnostics
    );
    lower_program(&parsed.value)
}

#[test]
fn lowers_item_annotations_to_hir_item_metadata() {
    let hir = lower(
        r#"
@model(adapter = "omlx-openai")
@tools(["Search"])
flow main(input: string) -> string {
  return input;
}
"#,
    );
    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let module = hir.modules[0];
    let item = hir.modules_arena[module].items[0];
    let annotations = hir
        .item_annotations
        .get(&item)
        .expect("annotation metadata should be attached to item");
    assert_eq!(annotations.len(), 2);
    assert_eq!(
        etas_hir::path_text(&annotations[0].path.syntax_path),
        "model"
    );
    assert_eq!(
        etas_hir::path_text(&annotations[1].path.syntax_path),
        "tools"
    );
    assert!(matches!(
        annotations[0].args[0],
        etas_hir::HirAnnotationArg::Named { .. }
    ));
    assert!(matches!(
        annotations[1].args[0],
        etas_hir::HirAnnotationArg::Positional { .. }
    ));

    let dump = dump_hir(&hir, HirDumpOptions::default());
    assert!(dump.contains("Annotation model"), "{dump}");
    assert!(dump.contains("NamedArg adapter"), "{dump}");
}

#[test]
fn lowers_bodyless_agent_without_primary_body() {
    let hir = lower(
        r#"
agent Reviewer;
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let (item_id, agent) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Agent(agent) => Some((id, agent)),
            _ => None,
        })
        .expect("expected agent");
    assert!(agent.params.is_empty());
    assert!(agent.output_type.is_none());
    assert!(matches!(agent.body, etas_hir::HirAgentBody::Decl { .. }));

    let view = HirTreeView::new(&hir);
    assert!(
        view.item(item_id)
            .expect("agent item should be indexed")
            .body()
            .is_none(),
        "bodyless agent must not expose a primary source body"
    );
}

#[test]
fn lowers_spec_method_selection_without_rewriting_to_method_call() {
    let hir = lower(
        r#"
type Prompt;
spec PromptEncode {
  flow encode(input: Prompt) -> string;
}

flow main(prompt: Prompt) -> unit {
  prompt::PromptEncode.encode();
  return;
}
"#,
    );
    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let dump = dump_hir(&hir, HirDumpOptions::default());
    assert!(dump.contains("SpecMethod encode"), "{dump}");
    assert!(dump.contains("spec=PromptEncode"), "{dump}");

    let has_spec_method = hir
        .exprs
        .iter()
        .any(|(_, expr)| matches!(expr, HirExpr::SpecMethodCall { .. }));
    assert!(has_spec_method, "{dump}");
}

#[test]
fn hir_tree_view_indexes_structural_owners_and_children() {
    let hir = lower(
        r#"
flow helper(input: i32) -> i32 {
  return input;
}

flow main() -> i32 {
  let value = helper(1);
  return value;
}
"#,
    );
    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

    let view = HirTreeView::new(&hir);
    let modules = view.modules().collect::<Vec<_>>();
    assert_eq!(modules.len(), hir.modules.len());
    let module = modules[0];
    assert_eq!(
        module.items().map(|item| item.id()).collect::<Vec<_>>(),
        hir.modules_arena[module.id()].items
    );

    let main = module
        .items()
        .find(|item| {
            matches!(item.data(), HirItem::Flow(flow)
                if hir.symbols.get(flow.symbol).is_some_and(|symbol| symbol.name == "main"))
        })
        .expect("main flow item should be visible through the tree view");
    let body = main.body().expect("flow body should be indexed");
    assert_eq!(body.id().kind, HirBodyKind::Flow);
    let blocks = body.blocks().collect::<Vec<_>>();
    assert_eq!(blocks.len(), 1);
    let block = blocks[0];
    let statements = block.statements().collect::<Vec<_>>();
    assert_eq!(statements.len(), 2);
    assert!(matches!(statements[0].data(), HirStmt::Let { .. }));
    assert_eq!(
        view.index().stmt_block.get(&statements[0].id()).copied(),
        Some(block.id())
    );
    assert_eq!(
        view.enclosing_scope(HirNodeRef::Stmt(statements[0].id())),
        Some(block.data().scope)
    );

    let HirStmt::Let { pat, value, .. } = statements[0].data() else {
        panic!("expected first statement to be let");
    };
    assert_eq!(
        view.index().pat_owner.get(&pat).copied(),
        Some(HirOwner::Stmt(statements[0].id()))
    );
    assert_eq!(
        view.index().expr_owner.get(&value).copied(),
        Some(HirOwner::Stmt(statements[0].id()))
    );
    let call = view.expr(*value).expect("let value expression view");
    assert!(
        call.children()
            .any(|child| matches!(child, HirTreeChild::Expr(_))),
        "call expression should expose child expressions"
    );

    assert_eq!(view.index().block_owner.len(), hir.blocks.len());
    assert_eq!(view.index().stmt_block.len(), hir.stmts.len());
    assert_eq!(view.index().expr_owner.len(), hir.exprs.len());
    assert_eq!(view.index().pat_owner.len(), hir.pats.len());
    assert!(
        module.scope().children().next().is_some(),
        "scope children should be indexed from ScopeTree"
    );
    assert_eq!(
        view.source_span(HirNodeRef::Item(main.id()))
            .expect("item span")
            .source,
        SourceId(1)
    );
}

#[test]
fn hir_tree_view_visitor_walks_in_stable_module_order() {
    let hir = lower(
        r#"
flow first() -> i32 {
  return 1;
}

flow second() -> i32 {
  let value = first();
  return value;
}
"#,
    );
    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

    let view = HirTreeView::new(&hir);
    let module = view.modules().next().expect("module").id();
    let expected_items = hir.modules_arena[module].items.clone();
    let mut visitor = RecordingVisitor::default();
    walk_module(&view, module, &mut visitor);

    assert_eq!(visitor.modules, vec![module]);
    assert_eq!(visitor.items, expected_items);
    assert!(
        visitor.blocks.len() >= 2,
        "visitor should enter flow body blocks"
    );
    assert!(
        visitor.exprs.len() >= 3,
        "visitor should enter return/call expressions"
    );
}

#[test]
fn hir_tree_walk_module_panics_on_missing_module_id() {
    let hir = lower(
        r#"
flow main() -> i32 {
  return 1;
}
"#,
    );
    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

    let view = HirTreeView::new(&hir);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut visitor = RecordingVisitor::default();
        walk_module(&view, etas_hir::HirModuleId(999), &mut visitor);
    }));

    assert!(result.is_err(), "walk_module must fail fast on invalid ids");
}

#[test]
fn hir_tree_view_indexes_nested_body_work_units() {
    let hir = lower(
        r#"
effect Approval {
  action request() -> unit;
}

effect Network;

flow Tokens(value: i32) -> i32 {
  return value;
}

spec Gate;

tool Search(q: string) -> string ![Network] ~ Gate;

@model(model = "local")
agent Writer(input: string) -> string {
  return input;
}

let reusable = handler {
  Approval.request() => {
    resume;
  }
};

flow main(input: string) -> string {
  let lambda = input => input;

  match input {
    _ => input,
  }

  handle {
    perform Approval.request();
  } with {
    Approval.request() => {
      resume;
    }
  };

  let result = input ~> Writer limit Tokens(1);
  return lambda(result);
}
"#,
    );
    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

    let view = HirTreeView::try_new(&hir).expect("complex HIR tree should be valid");
    let body_kinds = view
        .index()
        .item_bodies
        .values()
        .flatten()
        .map(|body| body.kind)
        .collect::<Vec<_>>();

    assert!(
        body_kinds
            .iter()
            .any(|kind| matches!(kind, HirBodyKind::Lambda(_))),
        "lambda bodies should be separate body work units: {body_kinds:?}"
    );
    assert!(
        body_kinds
            .iter()
            .any(|kind| matches!(kind, HirBodyKind::HandlerArm(_))),
        "handler arm bodies should be separate body work units: {body_kinds:?}"
    );
    assert!(
        body_kinds
            .iter()
            .any(|kind| matches!(kind, HirBodyKind::MatchArm { .. })),
        "match arm bodies should be separate body work units: {body_kinds:?}"
    );
    assert!(
        body_kinds
            .iter()
            .any(|kind| matches!(kind, HirBodyKind::StageLimit { .. })),
        "pipeline stage limits should be separate body work units: {body_kinds:?}"
    );
    let missing_scopes = view
        .index()
        .body_owner
        .keys()
        .filter(|body| {
            !matches!(body.kind, HirBodyKind::TopLevelLet)
                && view.body(**body).and_then(|body| body.scope()).is_none()
        })
        .copied()
        .collect::<Vec<_>>();
    assert!(
        missing_scopes.is_empty(),
        "all body work units with scopes should be queryable through BodyView: {missing_scopes:?}"
    );
}

#[test]
fn hir_tree_view_reports_missing_module_item_without_silent_skip() {
    let mut hir = lower(
        r#"
flow main() -> i32 {
  return 1;
}
"#,
    );
    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

    let module = hir.modules[0];
    hir.modules_arena
        .get_mut(module)
        .expect("module exists")
        .items
        .push(etas_hir::HirItemId(999));

    let error = match HirTreeView::try_new(&hir) {
        Ok(_) => panic!("missing item must fail index build"),
        Err(error) => error,
    };
    assert!(
        error.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            HirTreeIndexDiagnostic::MissingNode {
                kind: HirTreeNodeKind::Item,
                id: 999,
                ..
            }
        )),
        "missing item should be reported explicitly: {error:#?}"
    );
}

#[test]
fn hir_tree_view_reports_duplicate_expr_owner_without_release_overwrite() {
    let mut hir = lower(
        r#"
flow main() -> i32 {
  let value = 1;
  return value;
}
"#,
    );
    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

    let module = hir.modules[0];
    let main_item = hir.modules_arena[module].items[0];
    let HirItem::Flow(flow) = &hir.items[main_item] else {
        panic!("expected flow");
    };
    let block = flow.body.block();
    let first_stmt = hir.blocks[block].stmts[0];
    let HirStmt::Let { value, .. } = hir.stmts[first_stmt] else {
        panic!("expected let statement");
    };
    hir.blocks.get_mut(block).expect("block exists").final_expr = Some(value);

    let error = match HirTreeView::try_new(&hir) {
        Ok(_) => panic!("duplicate expr owner must fail index build"),
        Err(error) => error,
    };
    assert!(
        error.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            HirTreeIndexDiagnostic::DuplicateOwner {
                kind: HirTreeNodeKind::Expr,
                id,
                ..
            } if *id == value.0
        )),
        "duplicate owner should be reported explicitly: {error:#?}"
    );
}

#[derive(Default)]
struct RecordingVisitor {
    modules: Vec<etas_hir::HirModuleId>,
    items: Vec<etas_hir::HirItemId>,
    blocks: Vec<etas_hir::HirBlockId>,
    exprs: Vec<etas_hir::HirExprId>,
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for RecordingVisitor {
    fn enter_module(&mut self, module: etas_hir::ModuleView<'view, 'hir>) {
        self.modules.push(module.id());
    }

    fn enter_item(&mut self, item: etas_hir::ItemView<'view, 'hir>) {
        self.items.push(item.id());
    }

    fn enter_block(&mut self, block: etas_hir::BlockView<'view, 'hir>) {
        self.blocks.push(block.id());
    }

    fn enter_expr(&mut self, expr: etas_hir::ExprView<'view, 'hir>) {
        self.exprs.push(expr.id());
    }
}

#[test]
fn lowers_specs_effect_params_and_effect_row_tail() {
    let hir = lower(
        r#"
spec ByteStream;
type TlsStream = string;
spec Region;
spec Within<Root>;
spec RegionWithin<R, Root>;
type WorkspaceRoot;
type ReportsRoot;

spec PromptEncode<T ~ Schema + ResponseDecode, effect E> ~ ByteStream {
  flow encode(input: T) -> string ![Console.stdout_write, E];
}

impl TlsStream ~ ByteStream;
impl ReportsRoot ~ Region + Within<ReportsRoot> + RegionWithin<ReportsRoot, WorkspaceRoot>;
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    assert!(
        hir.symbols
            .iter()
            .any(|symbol| symbol.name == "ByteStream" && symbol.kind == SymbolKind::Spec)
    );
    assert!(
        hir.symbols
            .iter()
            .any(|symbol| symbol.name == "E" && symbol.kind == SymbolKind::EffectParam)
    );
    let multi_impl = hir
        .items
        .iter()
        .filter_map(|(_, item)| match item {
            HirItem::Impl(impl_decl) => Some(impl_decl),
            _ => None,
        })
        .find(|impl_decl| match &impl_decl.target {
            etas_hir::HirImplTarget::SpecSatisfaction { specs, .. } => specs.len() == 3,
            _ => false,
        })
        .expect("multi-spec impl should lower");
    let etas_hir::HirImplTarget::SpecSatisfaction { specs, .. } = &multi_impl.target else {
        panic!("expected spec impl target");
    };
    assert_eq!(specs.len(), 3);
    assert_eq!(
        specs
            .iter()
            .filter(|spec_ref| spec_ref.spec_args.is_empty())
            .count(),
        1
    );

    let spec_decl = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Spec(item)
                if hir
                    .symbols
                    .get(item.symbol)
                    .is_some_and(|symbol| symbol.name == "PromptEncode") =>
            {
                Some(item)
            }
            _ => None,
        })
        .expect("PromptEncode spec should lower");
    assert_eq!(spec_decl.bounds.len(), 1);
    let signature = spec_decl
        .items
        .iter()
        .find_map(|item| match item {
            etas_hir::HirSpecItem::FlowSignature(signature) => Some(signature),
            _ => None,
        })
        .expect("spec flow signature should lower");
    let effects = signature.effects.as_ref().expect("signature effects");
    assert_eq!(effects.effects.len(), 1);
    assert!(effects.tail.is_some(), "expected row tail for E: effect");

    let dump = dump_hir(&hir, HirDumpOptions::default());
    assert!(dump.contains("Spec"), "{dump}");
    assert!(dump.contains("SuperSpec ByteStream"), "{dump}");
    assert!(dump.contains("EffectRowTail"), "{dump}");
}

#[test]
fn lowers_trace_spec_expression() {
    let hir = lower(
        r#"
type PersonalAccount;
type WorkAccount;
effect Approval {
  action request() -> unit;
}
effect CompanyEmail {
  action send<Account>() -> unit;
}
spec SafeEmail: trace =
  +Approval.request
  & -CompanyEmail.send<PersonalAccount>
  & (Approval.request >> CompanyEmail.send<WorkAccount>);
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let spec = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Spec(item) => Some(item),
            _ => None,
        })
        .expect("trace spec should lower");
    assert!(matches!(spec.kind, etas_hir::HirSpecKind::TraceSpec));
    assert!(spec.trace.is_some());
    assert!(spec.callable.is_none());

    let dump = dump_hir(&hir, HirDumpOptions::default());
    assert!(dump.contains("kind=trace"), "{dump}");
    assert!(dump.contains("TraceAllow"), "{dump}");
    assert!(dump.contains("TraceDeny"), "{dump}");
    assert!(dump.contains("TraceBefore"), "{dump}");
}

#[test]
fn lowers_effect_row_generic_arguments() {
    let hir = lower(
        r#"
flow wrap<effect E>() -> unit ![E] {
  f<![Console.stdout_write, E]>();
  return;
}

flow f<effect E>() -> unit ![E] {
  return;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let dump = dump_hir(&hir, HirDumpOptions::default());
    assert!(dump.contains("GenericEffectRowArg"), "{dump}");
    assert!(dump.contains("EffectRowTail"), "{dump}");
}

#[test]
fn lowers_flow_spec_signature_and_flow_satisfaction() {
    let hir = lower(
        r#"
spec Pure<I, O> I => O ![];

flow Normalize(text: string) -> string ~ Pure {
  return text;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let spec_decl = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Spec(item) => Some(item),
            _ => None,
        })
        .expect("spec should lower");
    assert!(spec_decl.callable.is_some());
    let flow = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Flow(item) => Some(item),
            _ => None,
        })
        .expect("flow should lower");
    assert_eq!(flow.conformances.len(), 1);
    let dump = dump_hir(&hir, HirDumpOptions::default());
    assert!(dump.contains("SpecCallableSignature"), "{dump}");
    assert!(dump.contains("Conformance Pure"), "{dump}");
}

#[test]
fn lowers_effect_ref_argument_kinds_without_type_fallback() {
    let hir = lower(
        r#"
type ProjectMemory = string;

flow main() -> unit ![Memory.read<ProjectMemory>, Console.stdout_write<_>, Console.stdout_write<"/tmp/a">, Console.stdout_write<42>, Error<IOError>] {
  return;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let flow = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Flow(flow) => Some(flow),
            _ => None,
        })
        .expect("flow should lower");
    let effects = flow.effects.as_ref().expect("flow should carry effects");

    assert!(matches!(effects.effects[0].args[0], HirEffectArg::Path(_)));
    assert!(matches!(
        effects.effects[1].args[0],
        HirEffectArg::Wildcard { .. }
    ));
    assert!(matches!(
        effects.effects[2].args[0],
        HirEffectArg::String { .. }
    ));
    assert!(matches!(
        effects.effects[3].args[0],
        HirEffectArg::Int { .. }
    ));
    assert!(matches!(effects.effects[4].args[0], HirEffectArg::Path(_)));
}

#[test]
fn lowers_modules_imports_symbols_scopes_and_local_resolution() {
    let hir = lower(
        r#"
module project.demo;
import host.fs as fs;

type FeatureBrief = string;
type DesignDoc = string;

agent ProductManager(input: FeatureBrief) -> DesignDoc {}
agent Architect(input: DesignDoc) -> DesignDoc {}

flow BuildFeature(brief: FeatureBrief) -> DesignDoc {
  let prd = brief ~> ProductManager;
  return prd ~> Architect;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    assert_eq!(hir.modules.len(), 1);

    let module = &hir.modules_arena[hir.modules[0]];
    assert_eq!(module.imports.len(), 1);
    assert_eq!(module.items.len(), 5);
    assert!(matches!(
        module.imports[0].target.resolution,
        ResolveResult::PartiallyResolved(ref partial)
            if partial.reason == PartialResolutionReason::PackageResolverRequired
    ));
    let import_alias = module.imports[0]
        .binding
        .as_ref()
        .expect("import binding")
        .symbol;
    let alias_symbol = hir.symbols.get(import_alias).expect("alias symbol");
    assert!(matches!(
        &alias_symbol.def,
        SymbolDef::ImportAlias { path, origin }
            if *origin == ImportAliasOrigin::SourceImport
                && path == &["host".to_owned(), "fs".to_owned()]
    ));

    let kinds = hir
        .symbols
        .iter()
        .map(|symbol| (&symbol.name, symbol.kind))
        .collect::<Vec<_>>();
    assert!(kinds.contains(&(&"FeatureBrief".to_string(), SymbolKind::Type)));
    assert!(kinds.contains(&(&"DesignDoc".to_string(), SymbolKind::Type)));
    assert!(kinds.contains(&(&"ProductManager".to_string(), SymbolKind::Agent)));
    assert!(kinds.contains(&(&"Architect".to_string(), SymbolKind::Agent)));
    assert!(kinds.contains(&(&"BuildFeature".to_string(), SymbolKind::Flow)));
    assert!(kinds.contains(&(&"brief".to_string(), SymbolKind::Param)));
    assert!(kinds.contains(&(&"prd".to_string(), SymbolKind::Local)));

    let flow = module
        .items
        .iter()
        .find_map(|item_id| match &hir.items[*item_id] {
            HirItem::Flow(flow) => Some(flow),
            _ => None,
        })
        .expect("expected flow");
    let body = &hir.blocks[flow.body.block()];
    let HirStmt::Let { value, .. } = &hir.stmts[body.stmts[0]] else {
        panic!("expected let statement");
    };
    let etas_hir::HirExpr::Pipeline { input, stages, .. } = &hir.exprs[*value] else {
        panic!("expected pipeline");
    };
    let etas_hir::HirExpr::Path(input_path) = &hir.exprs[*input] else {
        panic!("expected input path");
    };
    assert!(matches!(input_path.resolution, ResolveResult::Resolved(_)));
    let etas_hir::HirExpr::Path(stage_path) = &hir.exprs[stages[0].expr] else {
        panic!("expected stage path");
    };
    assert!(matches!(stage_path.resolution, ResolveResult::Resolved(_)));
}

#[test]
fn lowers_grouped_and_wildcard_import_trees() {
    let hir = lower(
        r#"
import std.io.{print, println as log, read_line,};
public import std.prelude.*;

flow main() -> unit {
  return;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let module = &hir.modules_arena[hir.modules[0]];
    assert_eq!(module.imports.len(), 4);

    assert_eq!(module.imports[0].kind, HirImportKind::GroupMember);
    assert_eq!(module.imports[1].kind, HirImportKind::GroupMember);
    assert_eq!(module.imports[2].kind, HirImportKind::GroupMember);
    assert_eq!(module.imports[3].kind, HirImportKind::Wildcard);
    assert_eq!(module.imports[3].visibility, Visibility::Public);
    assert!(module.imports[3].binding.is_none());

    let bindings = module
        .imports
        .iter()
        .filter_map(|import| import.binding.as_ref())
        .map(|binding| {
            (
                binding.local_name.as_str(),
                binding.is_alias,
                hir.symbols.get(binding.symbol).unwrap().kind,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        bindings,
        vec![
            ("print", false, SymbolKind::Import),
            ("log", true, SymbolKind::Import),
            ("read_line", false, SymbolKind::Import),
        ]
    );

    assert!(matches!(
        &hir.symbols
            .get(module.imports[1].binding.as_ref().unwrap().symbol)
            .unwrap()
            .def,
        SymbolDef::ImportAlias { path, origin }
            if *origin == ImportAliasOrigin::SourceImport
                && path == &["std".to_owned(), "io".to_owned(), "println".to_owned()]
    ));
}

#[test]
fn lowers_std_prelude_symbols_into_module_scope_without_hir_imports() {
    let hir = lower(
        r#"
flow main() -> string {
  return Some("hello");
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let module = &hir.modules_arena[hir.modules[0]];
    assert!(
        module.imports.is_empty(),
        "std prelude symbols must not become source HIR imports: {:#?}",
        module.imports
    );
    let some = hir
        .symbols
        .iter()
        .find(|symbol| {
            symbol.name == "Some"
                && symbol.kind == SymbolKind::StdPreludeAlias
                && matches!(
                    &symbol.def,
                    SymbolDef::ImportAlias { path, origin }
                        if *origin == ImportAliasOrigin::StdPrelude
                            && path
                                == &[
                                    "std".to_owned(),
                                    "option".to_owned(),
                                    "Some".to_owned(),
                                ]
                )
        })
        .expect("Some should come from the std registry prelude");
    assert!(
        hir.symbols
            .iter()
            .all(|symbol| !(symbol.name == "unwrap" && symbol.kind == SymbolKind::StdPreludeAlias)),
        "unwrap must not be injected through the std prelude"
    );
    let dump = dump_hir(&hir, HirDumpOptions::default());
    assert!(
        dump.contains(&format!(
            "s{} StdPreludeAlias Some origin=StdPrelude target=std.option.Some",
            some.id.0
        )),
        "{dump}"
    );
    assert!(!dump.contains("StdPreludeAlias unwrap"), "{dump}");
    assert!(
        !dump.contains("Import kind="),
        "prelude symbols must not be printed as source HIR imports:\n{dump}"
    );
    assert!(
        !hir.symbols
            .iter()
            .any(|symbol| symbol.name == "Option" && symbol.kind == SymbolKind::StdPreludeAlias),
        "unused std prelude symbols must not be materialized"
    );
}

#[test]
fn lowers_expression_bodied_flow_to_final_expr_block() {
    let hir = lower(
        r#"
flow add_one(value: i32) -> i32 = value + 1;
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let flow = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Flow(flow) => Some(flow),
            _ => None,
        })
        .expect("flow should lower");
    let HirFlowBody::Expr {
        expr,
        lowered_block,
        ..
    } = flow.body
    else {
        panic!("expected expression-bodied flow");
    };
    let body = &hir.blocks[lowered_block];
    assert!(body.stmts.is_empty());
    assert_eq!(body.final_expr, Some(expr));
}

#[test]
fn lowers_tool_source_body_and_declaration_body_distinctly() {
    let hir = lower(
        r#"
tool local_emit(message: string) -> unit {
  return;
}

tool remote_emit(message: string) -> unit ![Network];
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let tools = hir
        .items
        .iter()
        .filter_map(|(_, item)| match item {
            HirItem::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(tools.len(), 2);
    assert!(matches!(tools[0].body, HirToolBody::Source(_)));
    assert!(matches!(tools[1].body, HirToolBody::Decl { .. }));

    let dump = dump_hir(&hir, HirDumpOptions::default());
    assert!(dump.contains("ToolSourceBody"), "{dump}");
    assert!(dump.contains("ToolDeclBody"), "{dump}");
}

#[test]
fn lowers_effect_body_source_shape() {
    let hir = lower(
        r#"
effect Empty;
effect Block {}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let effects = hir
        .items
        .iter()
        .filter_map(|(_, item)| match item {
            HirItem::Effect(effect) => Some(effect),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(matches!(effects[0].body, HirEffectBody::Empty { .. }));
    assert!(matches!(effects[1].body, HirEffectBody::Block { .. }));
}

#[test]
fn reports_duplicate_and_unresolved_names() {
    let hir = lower(
        r#"
flow same() -> unit {
  let x = missing;
  let x = 1;
  return;
}

flow same() -> unit {
  return;
}
"#,
    );

    assert!(
        hir.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Name(NameDiagnosticCode::DuplicateSymbol)
        }),
        "{:#?}",
        hir.diagnostics
    );
    assert!(hir.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Name(NameDiagnosticCode::UnresolvedName)
            && diagnostic.message.contains("missing")
    }));
    assert!(hir.diagnostics.iter().any(|diagnostic| {
        diagnostic.labels.iter().any(|label| {
            label.message.contains("previous definition")
                || label.message.contains("previous definition is here")
        })
    }));
}

#[test]
fn reports_ambiguous_ordinary_name_resolution_after_duplicate_bindings() {
    let hir = lower(
        r#"
flow main() -> int {
  let x = 1;
  let x = 2;
  return x;
}
"#,
    );

    assert!(hir.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Name(NameDiagnosticCode::AmbiguousName)
            && diagnostic.message.contains("x")
    }));
}

#[test]
fn preserves_if_statement_else_shape_and_source_ids() {
    let hir = lower(
        r#"
flow main(ready: bool, other: bool) -> unit {
  if ready {
    return;
  }

  if ready {
    return;
  } else if other {
    return;
  }

  return;
}
"#,
    );

    let flow = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Flow(flow) => Some(flow),
            _ => None,
        })
        .expect("expected flow");
    let body = &hir.blocks[flow.body.block()];

    let HirStmt::If(first_if) = &hir.stmts[body.stmts[0]] else {
        panic!("expected if statement");
    };
    let HirExpr::If { else_branch, .. } = &hir.exprs[*first_if] else {
        panic!("expected if expression");
    };
    assert!(else_branch.is_none());

    let HirStmt::If(second_if) = &hir.stmts[body.stmts[1]] else {
        panic!("expected if statement");
    };
    let HirExpr::If {
        else_branch: Some(HirElseBranch::If(inner_if)),
        ..
    } = &hir.exprs[*second_if]
    else {
        panic!("expected else-if branch");
    };
    assert!(matches!(hir.exprs[*inner_if], HirExpr::If { .. }));

    assert!(
        hir.source_map
            .expr_sources
            .values()
            .all(|source| source.span().source == SourceId(1))
    );
}

#[test]
fn scopes_for_lambda_match_arms_and_handlers_are_owned_by_real_exprs() {
    let hir = lower(
        r#"
effect Approval {
  action request() -> unit;
}

flow main(input: string) -> string {
  let lambda = input => input;

  match input {
    _ => input,
  }

  handle {
    perform Approval.request();
  } with {
    Approval.request() => {
      resume;
    }
  };

  return input;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

    let mut saw_lambda = false;
    let mut saw_match_arm = false;
    let mut saw_handler = false;

    for scope in hir.scopes.iter() {
        match scope.owner {
            ScopeOwner::Lambda(expr) => {
                assert!(matches!(hir.exprs[expr], HirExpr::Lambda { .. }));
                saw_lambda = true;
            }
            ScopeOwner::MatchArm(expr) => {
                assert!(matches!(hir.exprs[expr], HirExpr::Match { .. }));
                saw_match_arm = true;
            }
            ScopeOwner::Handler(expr) => {
                assert!(matches!(hir.exprs[expr], HirExpr::Handler { .. }));
                saw_handler = true;
            }
            _ => {}
        }
    }

    assert!(saw_lambda);
    assert!(saw_match_arm);
    assert!(saw_handler);
}

#[test]
fn source_pattern_bindings_keep_their_owner_context() {
    let hir = lower(
        r#"
effect Approval {
  action request<T>(value: T) -> T;
}

flow main(input: string) -> string {
  match input {
    matched => matched,
  }

  for item in input {
    let loop_value = item;
  }

  handle {
    perform Approval.request<string>(input);
  } with {
    Approval.request<string>(handled) => {
      resume handled;
    }
  };

  return input;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

    let matched = hir
        .symbols
        .iter()
        .find(|symbol| symbol.name == "matched")
        .expect("match binding");
    assert!(matches!(
        matched.def,
        SymbolDef::PatternBinding {
            owner: PatternBindingOwner::MatchArm { .. },
            ..
        }
    ));

    let item = hir
        .symbols
        .iter()
        .find(|symbol| symbol.name == "item")
        .expect("for binding");
    assert!(matches!(
        item.def,
        SymbolDef::PatternBinding {
            owner: PatternBindingOwner::For { .. },
            initializer: Some(_),
            ..
        }
    ));

    let handled = hir
        .symbols
        .iter()
        .find(|symbol| symbol.name == "handled")
        .expect("handler binding");
    assert!(matches!(
        handled.def,
        SymbolDef::PatternBinding {
            owner: PatternBindingOwner::Handler { .. },
            ..
        }
    ));
    let handler_type_args = hir
        .exprs
        .iter()
        .filter_map(|(_, expr)| match expr {
            HirExpr::Handler { handlers, .. } => Some(handlers),
            _ => None,
        })
        .flat_map(|handlers| handlers.iter())
        .map(|handler| hir.handler_arms[*handler].generic_args.len())
        .collect::<Vec<_>>();
    assert_eq!(handler_type_args, vec![1]);
}

#[test]
fn qualified_partial_resolution_distinguishes_deferred_path_reasons() {
    let hir = lower(
        r#"
import host.fs as fs;

flow main(input: string) -> unit {
  let module_member = fs.missing;
  let package_path = std.io.fs.read;
  let unsupported_path = Unknown.Shape.Deep.More;
  return;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

    let reasons = hir
        .exprs
        .iter()
        .filter_map(|(_, expr)| match expr {
            HirExpr::Path(path) => {
                let ResolveResult::PartiallyResolved(partial) = &path.resolution else {
                    return None;
                };
                Some((
                    path.segments
                        .iter()
                        .map(|segment| segment.name.as_str())
                        .collect::<Vec<_>>()
                        .join("."),
                    partial.reason,
                ))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(reasons.iter().any(|(path, reason)| {
        path == "fs.missing" && *reason == PartialResolutionReason::ModuleMemberMissing
    }));
    assert!(reasons.iter().any(|(path, reason)| {
        path == "std.io.fs.read" && *reason == PartialResolutionReason::PackageResolverRequired
    }));
    assert!(reasons.iter().any(|(path, reason)| {
        path == "Unknown.Shape.Deep.More"
            && *reason == PartialResolutionReason::UnsupportedPathShape
    }));
}

#[test]
fn qualified_path_resolves_module_qualified_item_name() {
    let hir = lower(
        r#"
module project.demo;

type Draft = string;

flow main() -> unit {
  let qualified = project.demo.Draft;
  return;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let path = hir
        .exprs
        .iter()
        .find_map(|(_, expr)| match expr {
            HirExpr::Path(path)
                if path
                    .segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .collect::<Vec<_>>()
                    .join(".")
                    == "project.demo.Draft" =>
            {
                Some(path)
            }
            _ => None,
        })
        .expect("module-qualified item path should lower");

    let ResolveResult::Resolved(symbol) = path.resolution else {
        panic!(
            "expected module-qualified item path to resolve, got {:?}",
            path.resolution
        );
    };
    assert_eq!(
        hir.symbols.get(symbol).map(|symbol| symbol.kind),
        Some(SymbolKind::Type)
    );
}

#[test]
fn qualified_path_resolves_full_symbol_before_prefix_fallback() {
    let hir = lower(
        r#"
effect Approval {
  action request() -> unit;
}

flow main() -> unit {
  let action = Approval.request;
  return;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let resolved_action_path = hir
        .exprs
        .iter()
        .find_map(|(_, expr)| match expr {
            HirExpr::Path(path)
                if path
                    .segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .collect::<Vec<_>>()
                    .join(".")
                    == "Approval.request" =>
            {
                Some(path)
            }
            _ => None,
        })
        .expect("qualified action path should lower");

    let ResolveResult::Resolved(symbol) = resolved_action_path.resolution else {
        panic!(
            "expected fully resolved qualified path, got {:?}",
            resolved_action_path.resolution
        );
    };
    assert_eq!(
        hir.symbols.get(symbol).map(|symbol| symbol.kind),
        Some(SymbolKind::EffectAction)
    );
}

#[test]
fn effect_action_symbols_record_owning_effect() {
    let hir = lower(
        r#"
effect Approval {
  action request() -> unit;
}

impl Approval {
  action audit() -> unit;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let (effect_id, effect) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Effect(effect) => Some((id, effect)),
            _ => None,
        })
        .expect("effect item");
    let (impl_id, impl_action) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Impl(impl_decl) => impl_decl.items.iter().find_map(|item| match item {
                etas_hir::HirImplItem::Action(action) => Some((id, action)),
                _ => None,
            }),
            _ => None,
        })
        .expect("impl action");

    let effect_action = effect.body.actions().first().expect("effect action");
    let request_symbol = hir
        .symbols
        .get(effect_action.symbol)
        .expect("effect action symbol");
    assert!(matches!(
        request_symbol.def,
        SymbolDef::EffectAction {
            declaring_item,
            owner_effect,
            action_index: 0,
        } if declaring_item == effect_id && owner_effect == Some(effect.symbol)
    ));

    let audit_symbol = hir
        .symbols
        .get(impl_action.symbol)
        .expect("impl action symbol");
    assert!(matches!(
        audit_symbol.def,
        SymbolDef::EffectAction {
            declaring_item,
            owner_effect,
            action_index: 0,
        } if declaring_item == impl_id && owner_effect == Some(effect.symbol)
    ));
}

#[test]
fn qualified_path_reports_unresolved_when_no_prefix_or_package_rule_applies() {
    let hir = lower(
        r#"
flow main() -> unit {
  let missing = Unknown.Member;
  return;
}
"#,
    );

    assert!(hir.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Name(NameDiagnosticCode::UnresolvedName)
            && diagnostic.message.contains("Unknown.Member")
    }));
    let path = hir
        .exprs
        .iter()
        .find_map(|(_, expr)| match expr {
            HirExpr::Path(path)
                if path
                    .segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .collect::<Vec<_>>()
                    .join(".")
                    == "Unknown.Member" =>
            {
                Some(path)
            }
            _ => None,
        })
        .expect("qualified unresolved path should lower");
    assert!(matches!(path.resolution, ResolveResult::Unresolved));
}

#[test]
fn qualified_path_reports_ambiguous_full_symbol() {
    let hir = lower(
        r#"
effect Approval {
  action request() -> unit;
}

effect Approval {
  action request() -> unit;
}

flow main() -> unit {
  let action = Approval.request;
  return;
}
"#,
    );

    assert!(hir.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Name(NameDiagnosticCode::AmbiguousName)
            && diagnostic.message.contains("Approval.request")
    }));
    let path = hir
        .exprs
        .iter()
        .find_map(|(_, expr)| match expr {
            HirExpr::Path(path)
                if path
                    .segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .collect::<Vec<_>>()
                    .join(".")
                    == "Approval.request" =>
            {
                Some(path)
            }
            _ => None,
        })
        .expect("qualified ambiguous path should lower");
    assert!(matches!(path.resolution, ResolveResult::Ambiguous(_)));
}

#[test]
fn record_type_fields_are_not_inserted_into_lexical_scopes() {
    let hir = lower(
        r#"
type Draft = {
  title: string,
  body: string,
};
"#,
    );

    for scope in hir.scopes.iter() {
        for symbol in &scope.symbols {
            let symbol = hir.symbols.get(*symbol).expect("symbol should exist");
            assert_ne!(symbol.kind, SymbolKind::Field);
        }
    }
}

#[test]
fn symbol_defs_point_back_to_declaring_hir_nodes() {
    let hir = lower(
        r#"
type Draft = {
  title: string,
};

enum Maybe {
  Some(string);
}

flow main(input: Draft) -> Draft {
  let local: Draft = input;
  return local;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

    let input = hir
        .symbols
        .iter()
        .find(|symbol| symbol.name == "input")
        .expect("input param symbol");
    assert_eq!(input.kind, SymbolKind::Param);
    let SymbolDef::Param { owner, ty, .. } = input.def else {
        panic!("expected param def, got {:?}", input.def);
    };
    assert!(matches!(hir.items[owner], HirItem::Flow(_)));
    assert!(ty.is_some());
    assert_eq!(input.declared_type, ty);

    let local = hir
        .symbols
        .iter()
        .find(|symbol| symbol.name == "local")
        .expect("local symbol");
    let SymbolDef::Local {
        binding,
        pattern,
        ty,
        initializer,
    } = local.def
    else {
        panic!("expected local def, got {:?}", local.def);
    };
    assert!(matches!(hir.stmts[binding], HirStmt::Let { .. }));
    assert!(matches!(
        hir.pats[pattern],
        etas_hir::HirPat::Binding { .. }
    ));
    assert!(ty.is_some());
    assert!(initializer.is_some());

    let title = hir
        .symbols
        .iter()
        .find(|symbol| symbol.name == "title")
        .expect("field symbol");
    assert_eq!(title.kind, SymbolKind::Field);
    assert!(matches!(title.def, SymbolDef::Field { field_index: 0, .. }));

    let some = hir
        .symbols
        .iter()
        .find(|symbol| symbol.name == "Some" && symbol.kind == SymbolKind::EnumVariant)
        .expect("variant symbol");
    assert_eq!(some.kind, SymbolKind::EnumVariant);
    assert!(matches!(
        some.def,
        SymbolDef::EnumVariant {
            variant_index: 0,
            ..
        }
    ));
}

#[test]
fn lowers_literal_patterns_without_creating_bindings() {
    let hir = lower(
        r#"
flow classify(value: i32) -> string {
  return match value {
    0 => "zero",
    _ => "many",
    };
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let dump = dump_hir(&hir, HirDumpOptions::default());
    assert!(dump.contains("Literal Int"), "{dump}");
    assert!(
        hir.symbols.iter().all(|symbol| symbol.name != "0"),
        "{:#?}",
        hir.symbols
    );
}

#[test]
fn lowers_postfix_try_to_hir_try_expr() {
    let hir = lower(
        r#"
flow main(value: string) -> Result<string, string> {
  return value?;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let try_exprs = hir
        .exprs
        .iter()
        .filter(|(_, expr)| matches!(expr, HirExpr::Try { .. }))
        .count();
    let try_unaries = hir
        .exprs
        .iter()
        .filter(|(_, expr)| matches!(expr, HirExpr::Unary { .. }))
        .count();
    assert_eq!(try_exprs, 1);
    assert_eq!(try_unaries, 0);
    let dump = dump_hir(&hir, HirDumpOptions::default());
    assert!(dump.contains("Try"), "{dump}");
}

#[test]
fn lowers_block_postfix_try_without_desugaring() {
    let hir = lower(
        r#"
flow main(value: string) -> Result<string, string> {
  return {
    let local = value;
    local
  }?;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let try_operands = hir
        .exprs
        .iter()
        .filter_map(|(_, expr)| match expr {
            HirExpr::Try { expr, .. } => Some(*expr),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(try_operands.len(), 1);
    assert!(matches!(hir.exprs[try_operands[0]], HirExpr::Block(_)));
    assert!(
        !hir.exprs
            .iter()
            .any(|(_, expr)| matches!(expr, HirExpr::Handle { .. })),
        "postfix `?` must stay source-shaped HIR, not lower to `handle`"
    );
}

#[test]
fn lowers_handler_values_and_handle_args_to_first_class_hir() {
    let hir = lower(
        r#"
effect Approval {
  action request() -> unit;
}

flow main() -> unit {
  let reusable = handler {
    Approval.request() => {
      resume;
    }
  };

  handle {
    perform Approval.request();
  } with reusable;

  handle {
    perform Approval.request();
  } with {
    Approval.request() => finish "fallback";
  };
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let handler_values = hir
        .exprs
        .iter()
        .filter(|(_, expr)| matches!(expr, HirExpr::Handler { .. }))
        .count();
    let handle_expr_args = hir
        .exprs
        .iter()
        .filter(|(_, expr)| matches!(expr, HirExpr::Handle { .. }))
        .count();

    assert_eq!(handler_values, 2);
    assert_eq!(handle_expr_args, 2);
    assert_eq!(hir.handler_arms.len(), 2);
    for (handler_id, handler) in hir.handler_arms.iter() {
        assert!(
            hir.source_map.handler_arm_sources.contains_key(&handler_id),
            "missing source map for handler arm {handler_id:?}"
        );
        assert_direct_reverse(&hir, HirNodeRef::HandlerArm(handler_id));
        assert!(hir.blocks.get(handler.body).is_some());
    }
    assert!(
        hir.handler_arms
            .iter()
            .any(|(_, arm)| hir.blocks[arm.body].final_expr.is_none()
                && !hir.blocks[arm.body].stmts.is_empty()),
        "statement-bodied handler arm should lower to a block with statements and no final expr"
    );
    let dump = dump_hir(&hir, HirDumpOptions::default());
    assert!(dump.contains("HandlerValue"), "{dump}");
    assert!(dump.contains("Handler ha0"), "{dump}");
    assert!(dump.contains("Handler "), "{dump}");
    assert!(!dump.contains("InlineHandlerArg"), "{dump}");
}

#[test]
fn lowers_flow_trailing_handler_to_handle_expr() {
    let hir = lower(
        r#"
effect Approval {
  action request() -> string;
}

flow main() -> string {
  perform Approval.request();
} with {
  Approval.request() => finish "fallback";
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    assert!(
        hir.exprs
            .iter()
            .any(|(_, expr)| matches!(expr, HirExpr::Handle { .. })),
        "flow trailing handler should normalize to a handle expression"
    );
    assert_eq!(hir.handler_arms.len(), 1);
}

#[test]
fn lowers_top_level_handler_let_with_direct_origin() {
    let hir = lower(
        r#"
effect Approval {
  action request() -> unit;
}

let reusable = handler {
  Approval.request() => {
    resume;
  }
};

flow main() -> unit {
  handle {
    perform Approval.request();
  } with reusable;
  return;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);
    let (item_id, top_level_let) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::TopLevelLet(item)
                if hir
                    .symbols
                    .get(item.symbol)
                    .is_some_and(|symbol| symbol.name == "reusable") =>
            {
                Some((id, item))
            }
            _ => None,
        })
        .expect("top-level handler let should lower");
    assert!(matches!(
        hir.exprs[top_level_let.value],
        HirExpr::Handler { .. }
    ));

    let symbol = hir
        .symbols
        .get(top_level_let.symbol)
        .expect("top-level let symbol");
    assert!(matches!(
        symbol.def,
        SymbolDef::TopLevelLet {
            item,
            initializer,
            ..
        } if item == item_id && initializer == top_level_let.value
    ));
    assert_direct_reverse(&hir, HirNodeRef::Item(item_id));
    assert_direct_reverse(&hir, HirNodeRef::Symbol(top_level_let.symbol));
    assert_direct_reverse(&hir, HirNodeRef::Expr(top_level_let.value));
}

#[test]
fn source_map_records_symbol_origins_and_reverse_lookup() {
    let hir = lower(
        r#"
flow main(input: string) -> string {
  let local = input;
  return local;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

    let local = hir
        .symbols
        .iter()
        .find(|symbol| symbol.name == "local")
        .expect("local symbol");
    let symbol_origin = hir
        .source_map
        .symbol_sources
        .get(&local.id)
        .expect("local symbol source");
    assert_eq!(symbol_origin.span().source, SourceId(1));

    let syntax = symbol_origin
        .primary_syntax()
        .expect("local symbol should have direct syntax");
    let reverse = hir
        .source_map
        .syntax_to_hir
        .get(&syntax.id)
        .expect("reverse syntax lookup");
    assert!(reverse.contains(&HirNodeRef::Symbol(local.id)));
}

#[test]
fn source_map_links_core_hir_nodes_to_direct_syntax() {
    let hir = lower(
        r#"
flow main(input: string) -> string {
  let local: string = input;
  return local;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

    let (flow_id, flow) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow) => Some((id, flow)),
            _ => None,
        })
        .expect("flow should lower");
    assert_direct_reverse(&hir, HirNodeRef::Item(flow_id));
    assert_direct_reverse(&hir, HirNodeRef::Block(flow.body.block()));
    assert_direct_reverse(
        &hir,
        HirNodeRef::Type(flow.return_type.expect("return type source")),
    );

    let body = &hir.blocks[flow.body.block()];
    let let_stmt = body.stmts[0];
    assert_direct_reverse(&hir, HirNodeRef::Stmt(let_stmt));
    let HirStmt::Let {
        pat,
        type_annotation,
        value,
        ..
    } = hir.stmts[let_stmt]
    else {
        panic!("expected let statement");
    };
    assert_direct_reverse(&hir, HirNodeRef::Pat(pat));
    assert_direct_reverse(
        &hir,
        HirNodeRef::Type(type_annotation.expect("let type annotation source")),
    );
    assert_direct_reverse(&hir, HirNodeRef::Expr(value));

    let return_stmt = body.stmts[1];
    assert_direct_reverse(&hir, HirNodeRef::Stmt(return_stmt));
    let HirStmt::Return {
        value: Some(return_value),
        ..
    } = hir.stmts[return_stmt]
    else {
        panic!("expected return value");
    };
    assert_direct_reverse(&hir, HirNodeRef::Expr(return_value));
}

#[test]
fn source_map_does_not_create_synthetic_origins_for_normal_source_nodes() {
    let hir = lower(
        r#"
flow main(input: string) -> string {
  let local = input;
  return local;
}
"#,
    );

    assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

    for origin in hir
        .source_map
        .item_sources
        .values()
        .chain(hir.source_map.expr_sources.values())
        .chain(hir.source_map.handler_arm_sources.values())
        .chain(hir.source_map.stmt_sources.values())
        .chain(hir.source_map.pat_sources.values())
        .chain(hir.source_map.type_sources.values())
        .chain(hir.source_map.block_sources.values())
        .chain(hir.source_map.symbol_sources.values())
    {
        assert!(
            !matches!(origin, HirOrigin::Synthetic { .. }),
            "normal source node should not be synthetic: {origin:?}"
        );
        assert_eq!(origin.span().source, SourceId(1));
    }
}

#[test]
fn dumps_hir_with_symbols_scopes_resolutions_and_diagnostics() {
    let hir = lower(
        r#"
type Draft = string;
agent Writer(input: Draft) -> Draft {}

flow write(input: Draft) -> Draft {
  return input ~> Writer;
}
"#,
    );

    let dump = dump_hir(
        &hir,
        HirDumpOptions {
            include_spans: false,
            include_diagnostics: true,
            include_symbols: true,
            include_scopes: true,
            include_source_map: false,
        },
    );

    assert!(dump.contains("HirProgram"));
    assert!(dump.contains("Symbols"));
    assert!(dump.contains("Scopes"));
    assert!(dump.contains("Flow"));
    assert!(dump.contains("Path input(s"));
    assert!(dump.contains("Path Writer(s"));
    assert!(!dump.contains("Diagnostics\n"));
}

fn assert_direct_reverse(hir: &etas_hir::HirProgram, node: HirNodeRef) {
    let origin = match node {
        HirNodeRef::Item(id) => hir.source_map.item_sources.get(&id),
        HirNodeRef::Expr(id) => hir.source_map.expr_sources.get(&id),
        HirNodeRef::HandlerArm(id) => hir.source_map.handler_arm_sources.get(&id),
        HirNodeRef::Stmt(id) => hir.source_map.stmt_sources.get(&id),
        HirNodeRef::Pat(id) => hir.source_map.pat_sources.get(&id),
        HirNodeRef::Type(id) => hir.source_map.type_sources.get(&id),
        HirNodeRef::Block(id) => hir.source_map.block_sources.get(&id),
        HirNodeRef::Symbol(id) => hir.source_map.symbol_sources.get(&id),
    }
    .unwrap_or_else(|| panic!("missing source-map origin for {node:?}"));
    assert!(matches!(origin, HirOrigin::Direct(_)), "{origin:?}");
    assert_eq!(origin.span().source, SourceId(1));

    let syntax = origin
        .primary_syntax()
        .expect("direct origin should have syntax node");
    let reverse = hir
        .source_map
        .syntax_to_hir
        .get(&syntax.id)
        .unwrap_or_else(|| panic!("missing reverse lookup for {node:?}"));
    assert!(
        reverse.contains(&node),
        "reverse lookup for syntax {:?} did not contain {:?}: {:?}",
        syntax.id,
        node,
        reverse
    );
}

#[test]
fn reports_ambiguous_effect_action_resolution() {
    let hir = lower(
        r#"
effect Approval {
  action request() -> unit;
}

effect Approval {
  action request() -> unit;
}

flow main() -> unit {
  perform Approval.request();
  return;
}
"#,
    );

    assert!(hir.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Name(NameDiagnosticCode::AmbiguousName)
            && diagnostic.message.contains("Approval.request")
    }));

    let actions = hir
        .symbols
        .iter()
        .filter(|symbol| symbol.name == "Approval.request")
        .collect::<Vec<_>>();
    assert_eq!(actions.len(), 2);
    for action in actions {
        let SymbolDef::EffectAction {
            declaring_item,
            owner_effect,
            ..
        } = action.def
        else {
            panic!("expected action def, got {:?}", action.def);
        };
        assert!(owner_effect.is_some());
        assert!(matches!(hir.items[declaring_item], HirItem::Effect(_)));
    }
}

#[test]
fn obsolete_inline_policy_declaration_conformance_does_not_lower_to_hir_body() {
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
        parsed.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("inline conformance is obsolete")),
        "parse diagnostics should reject obsolete inline policy syntax: {:#?}",
        parsed.diagnostics
    );
    let hir = lower_program(&parsed.value);
    let flow = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Flow(flow) => Some(flow),
            _ => None,
        })
        .expect("flow should lower");
    assert!(flow.conformances.iter().all(|conformance| matches!(
        conformance.target,
        etas_hir::HirDeclarationConformanceTarget::Error { .. }
    )));
    let tool = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Tool(tool) => Some(tool),
            _ => None,
        })
        .expect("tool should lower");
    assert!(tool.conformances.iter().all(|conformance| matches!(
        conformance.target,
        etas_hir::HirDeclarationConformanceTarget::Error { .. }
    )));

    let view = HirTreeView::new(&hir);
    let conformance_bodies = view
        .modules()
        .flat_map(|module| module.items())
        .flat_map(|item| item.bodies())
        .count();
    assert_eq!(
        conformance_bodies, 1,
        "only the source flow body should remain; obsolete inline policy must not create bodies"
    );
}
