use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use etas_core::{LineIndex, SourceId};
use etas_std::{StdImplementationRef, StdLibrarySourceId};
use etas_syntax::ast;
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{ADMITTED_STD_LIBRARY_SOURCES, PARSED_SOURCE_SET, SOURCE_SET};
use crate::{ProjectContext, SourceFile, SourceKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct AdmittedStandardLibrarySource {
    pub source: SourceId,
    pub library_source: StdLibrarySourceId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, etas_core::RetainedSize)]
pub struct AdmittedStandardLibrarySources {
    pub sources: Vec<AdmittedStandardLibrarySource>,
}

pub struct AdmitStandardLibrarySourcesPass;

impl Pass<ProjectContext> for AdmitStandardLibrarySourcesPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("AdmitStandardLibrarySourcesPass", PassKind::Transform)
            .requires(ArtifactSet::from([SOURCE_SET, PARSED_SOURCE_SET]))
            .produces(ArtifactSet::from([
                SOURCE_SET,
                ADMITTED_STD_LIBRARY_SOURCES,
            ]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let selected = select_sources(context);
        let mut admitted = AdmittedStandardLibrarySources::default();
        let Some(source_set) = context.sources.as_mut() else {
            return PassResult::failed("standard library admission requires a source set");
        };
        let source_set = Arc::make_mut(source_set);

        for library_source in selected {
            let source = virtual_source_id(library_source);
            if source_set.files.iter().any(|file| file.id == source) {
                return PassResult::failed(format!(
                    "reserved standard library source id {} conflicts with an input source",
                    source.0
                ));
            }
            let asset = etas_std::standard_library_source(library_source);
            let text: Arc<str> = Arc::from(asset.source);
            source_set.files.push(SourceFile {
                id: source,
                path: Some(PathBuf::from(asset.virtual_path)),
                text: text.clone(),
                line_index: LineIndex::new(&text),
                kind: SourceKind::VirtualStd {
                    source: library_source,
                },
            });
            admitted.sources.push(AdmittedStandardLibrarySource {
                source,
                library_source,
            });
        }

        context.admitted_std_library_sources = Some(Arc::new(admitted));
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::from([SOURCE_SET, ADMITTED_STD_LIBRARY_SOURCES]),
        )
    }
}

fn select_sources(context: &ProjectContext) -> Vec<StdLibrarySourceId> {
    let implementations = context
        .std_registry
        .symbols()
        .filter_map(|symbol| match symbol.implementation {
            Some(StdImplementationRef::Source { target, .. }) => {
                Some((symbol.qualified_path.clone(), target.source))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let mut selected = BTreeSet::new();

    for parsed in &context.parsed_sources {
        if matches!(
            context
                .sources
                .as_ref()
                .and_then(|sources| sources.files.iter().find(|file| file.id == parsed.source))
                .map(|file| &file.kind),
            Some(SourceKind::VirtualStd { .. })
        ) {
            continue;
        }
        for import in &parsed.parse.value.imports {
            select_import(&import.tree, &implementations, &mut selected);
        }
        visit_program_paths(&parsed.parse.value, |path| {
            select_path(path, &implementations, &mut selected)
        });
    }

    let mut work = selected.iter().copied().collect::<Vec<_>>();
    while let Some(source) = work.pop() {
        for dependency in etas_std::standard_library_source(source).dependencies {
            if selected.insert(*dependency) {
                work.push(*dependency);
            }
        }
    }
    selected.into_iter().collect()
}

fn select_import(
    tree: &ast::ImportTree,
    implementations: &BTreeMap<Vec<String>, StdLibrarySourceId>,
    selected: &mut BTreeSet<StdLibrarySourceId>,
) {
    match tree {
        ast::ImportTree::Single { path, .. } => {
            select_path(path, implementations, selected);
            select_module_prefix(path, implementations, selected);
        }
        ast::ImportTree::Group { prefix, items, .. } => {
            for item in items {
                let mut path = prefix.segments.clone();
                path.push(item.name.clone());
                select_segments(
                    path.iter().map(|name| name.text.as_str()),
                    implementations,
                    selected,
                );
            }
        }
        ast::ImportTree::Wildcard { prefix, .. } => {
            select_module_prefix(prefix, implementations, selected);
        }
        ast::ImportTree::Error { .. } => {}
    }
}

fn select_path(
    path: &ast::Path,
    implementations: &BTreeMap<Vec<String>, StdLibrarySourceId>,
    selected: &mut BTreeSet<StdLibrarySourceId>,
) {
    select_segments(
        path.segments.iter().map(|name| name.text.as_str()),
        implementations,
        selected,
    );
}

fn select_segments<'a>(
    segments: impl Iterator<Item = &'a str>,
    implementations: &BTreeMap<Vec<String>, StdLibrarySourceId>,
    selected: &mut BTreeSet<StdLibrarySourceId>,
) {
    let path = segments.map(str::to_owned).collect::<Vec<_>>();
    if let Some(source) = implementations.get(&path) {
        selected.insert(*source);
    }
}

fn select_module_prefix(
    prefix: &ast::Path,
    implementations: &BTreeMap<Vec<String>, StdLibrarySourceId>,
    selected: &mut BTreeSet<StdLibrarySourceId>,
) {
    let prefix = prefix
        .segments
        .iter()
        .map(|name| name.text.as_str())
        .collect::<Vec<_>>();
    for (path, source) in implementations {
        if path.len() == prefix.len() + 1
            && path
                .iter()
                .zip(prefix.iter())
                .all(|(left, right)| left == right)
        {
            selected.insert(*source);
        }
    }
}

fn virtual_source_id(source: StdLibrarySourceId) -> SourceId {
    match source {
        StdLibrarySourceId::TextAssembly => SourceId(u32::MAX - 255),
        StdLibrarySourceId::TextFormat => SourceId(u32::MAX - 254),
    }
}

enum AstWork<'a> {
    Expr(&'a ast::Expr),
    Block(&'a ast::Block),
    Stmt(&'a ast::Stmt),
    IfStmt(&'a ast::IfStmt),
    Handler(&'a ast::HandlerArg),
}

fn visit_program_paths(program: &ast::Program, mut visit: impl FnMut(&ast::Path)) {
    let mut work = Vec::new();
    for item in &program.items {
        for annotation in &item.annotations {
            for argument in &annotation.args {
                work.push(AstWork::Expr(match argument {
                    ast::AnnotationArg::Positional(expr)
                    | ast::AnnotationArg::Named { value: expr, .. } => expr,
                }));
            }
        }
        seed_item_work(&item.item, &mut work);
    }
    while let Some(node) = work.pop() {
        match node {
            AstWork::Expr(expr) => visit_expr(expr, &mut work, &mut visit),
            AstWork::Block(block) => {
                work.extend(block.stmts.iter().map(AstWork::Stmt));
                if let Some(expr) = block.final_expr.as_deref() {
                    work.push(AstWork::Expr(expr));
                }
            }
            AstWork::Stmt(stmt) => visit_stmt(stmt, &mut work),
            AstWork::IfStmt(stmt) => visit_if_stmt(stmt, &mut work),
            AstWork::Handler(handler) => match handler {
                ast::HandlerArg::Expr(expr) => work.push(AstWork::Expr(expr)),
                ast::HandlerArg::Block(block) => {
                    for arm in &block.arms {
                        match &arm.body {
                            ast::HandlerArmBody::Stmt(stmt) => work.push(AstWork::Stmt(stmt)),
                            ast::HandlerArmBody::Block(block) => work.push(AstWork::Block(block)),
                        }
                    }
                }
            },
        }
    }
}

fn seed_item_work<'a>(item: &'a ast::Item, work: &mut Vec<AstWork<'a>>) {
    match item {
        ast::Item::Flow(flow) => seed_flow_work(flow, work),
        ast::Item::Tool(tool) => {
            if let ast::ToolBody::Source(body) = &tool.body {
                seed_flow_body_work(body, work);
            }
        }
        ast::Item::Agent(agent) => {
            if let ast::AgentBody::Source(body) = &agent.body {
                work.push(AstWork::Block(body));
            }
        }
        ast::Item::TopLevelLet(decl) => work.push(AstWork::Expr(&decl.value)),
        ast::Item::Impl(decl) => {
            for item in &decl.items {
                if let ast::ImplItem::Flow(flow) = item {
                    seed_flow_work(flow, work);
                }
            }
        }
        ast::Item::Alias(_)
        | ast::Item::Type(_)
        | ast::Item::Enum(_)
        | ast::Item::Spec(_)
        | ast::Item::Effect(_)
        | ast::Item::Protocol(_)
        | ast::Item::Error(_) => {}
    }
}

fn seed_flow_work<'a>(flow: &'a ast::FlowDecl, work: &mut Vec<AstWork<'a>>) {
    seed_flow_body_work(&flow.body, work);
    if let Some(handler) = &flow.trailing_handler {
        work.push(AstWork::Handler(handler));
    }
}

fn seed_flow_body_work<'a>(body: &'a ast::FlowBody, work: &mut Vec<AstWork<'a>>) {
    match body {
        ast::FlowBody::Block(block) => work.push(AstWork::Block(block)),
        ast::FlowBody::Expr { expr, .. } => work.push(AstWork::Expr(expr)),
    }
}

fn visit_stmt<'a>(stmt: &'a ast::Stmt, work: &mut Vec<AstWork<'a>>) {
    match stmt {
        ast::Stmt::Let(stmt) => work.push(AstWork::Expr(&stmt.value)),
        ast::Stmt::Var(stmt) => work.push(AstWork::Expr(&stmt.value)),
        ast::Stmt::Assign(stmt) => {
            work.push(AstWork::Expr(&stmt.target));
            work.push(AstWork::Expr(&stmt.value));
        }
        ast::Stmt::If(stmt) => visit_if_stmt(stmt, work),
        ast::Stmt::Match(stmt) => {
            work.push(AstWork::Expr(&stmt.scrutinee));
            for arm in &stmt.arms {
                match &arm.body {
                    ast::MatchArmBody::Expr(expr) => work.push(AstWork::Expr(expr)),
                    ast::MatchArmBody::Block(block) => work.push(AstWork::Block(block)),
                }
            }
        }
        ast::Stmt::For(stmt) => {
            work.push(AstWork::Expr(&stmt.iter));
            work.extend(stmt.limits.iter().map(AstWork::Expr));
            work.push(AstWork::Block(&stmt.body));
        }
        ast::Stmt::While(stmt) => {
            work.push(AstWork::Expr(&stmt.condition));
            work.extend(stmt.limits.iter().map(AstWork::Expr));
            work.push(AstWork::Block(&stmt.body));
        }
        ast::Stmt::Retry(stmt) => {
            work.extend(stmt.limits.iter().map(AstWork::Expr));
            work.push(AstWork::Block(&stmt.body));
        }
        ast::Stmt::Resume(stmt) => work.extend(stmt.value.as_ref().map(AstWork::Expr)),
        ast::Stmt::Finish(stmt) => work.push(AstWork::Expr(&stmt.value)),
        ast::Stmt::Return(stmt) => work.extend(stmt.value.as_ref().map(AstWork::Expr)),
        ast::Stmt::Expr(stmt) => work.push(AstWork::Expr(&stmt.expr)),
        ast::Stmt::Break(_) | ast::Stmt::Continue(_) | ast::Stmt::Error(_) => {}
    }
}

fn visit_if_stmt<'a>(stmt: &'a ast::IfStmt, work: &mut Vec<AstWork<'a>>) {
    work.push(AstWork::Expr(&stmt.condition));
    work.push(AstWork::Block(&stmt.then_branch));
    if let Some(branch) = &stmt.else_branch {
        match branch {
            ast::ElseBranch::If(stmt) => work.push(AstWork::IfStmt(stmt)),
            ast::ElseBranch::Block(block) => work.push(AstWork::Block(block)),
        }
    }
}

fn push_args<'a>(work: &mut Vec<AstWork<'a>>, args: &'a [ast::Arg]) {
    work.extend(args.iter().map(|arg| {
        AstWork::Expr(match arg {
            ast::Arg::Positional(expr) | ast::Arg::Named { value: expr, .. } => expr,
        })
    }));
}

fn visit_expr<'a>(
    expr: &'a ast::Expr,
    work: &mut Vec<AstWork<'a>>,
    visit: &mut impl FnMut(&ast::Path),
) {
    match expr {
        ast::Expr::Path(path) => visit(path),
        ast::Expr::Reflect(reflection) => match &reflection.target {
            ast::ReflectTarget::Path(path) => visit(path),
            ast::ReflectTarget::SpecMethod { receiver, spec, .. } => {
                visit(receiver);
                visit(spec);
            }
        },
        ast::Expr::Record(record) => {
            if let Some(path) = &record.path {
                visit(path);
            }
            for field in &record.fields {
                if let ast::FieldInit::Named { value, .. } = field {
                    work.push(AstWork::Expr(value));
                }
            }
        }
        ast::Expr::Tuple { elems, .. }
        | ast::Expr::Array { elems, .. }
        | ast::Expr::List { elems, .. }
        | ast::Expr::Set { elems, .. } => work.extend(elems.iter().map(AstWork::Expr)),
        ast::Expr::ListCons { head, tail, .. } => {
            work.push(AstWork::Expr(head));
            work.push(AstWork::Expr(tail));
        }
        ast::Expr::Map(map) => {
            for entry in &map.entries {
                work.push(AstWork::Expr(&entry.key));
                work.push(AstWork::Expr(&entry.value));
            }
        }
        ast::Expr::Range(range) => {
            work.push(AstWork::Expr(&range.start));
            work.push(AstWork::Expr(&range.end));
        }
        ast::Expr::Call(call) => {
            work.push(AstWork::Expr(&call.callee));
            push_args(work, &call.args);
        }
        ast::Expr::MethodCall(call) => {
            work.push(AstWork::Expr(&call.receiver));
            push_args(work, &call.args);
        }
        ast::Expr::SpecMethodCall(call) => {
            work.push(AstWork::Expr(&call.receiver));
            visit(&call.spec_path);
            push_args(work, &call.args);
        }
        ast::Expr::Perform(perform) => push_args(work, &perform.args),
        ast::Expr::Infer(infer) => push_args(work, &infer.args),
        ast::Expr::Handle(handle) => {
            work.push(AstWork::Expr(&handle.body));
            work.push(AstWork::Handler(&handle.handler));
        }
        ast::Expr::Handler(handler) => {
            for arm in &handler.block.arms {
                match &arm.body {
                    ast::HandlerArmBody::Stmt(stmt) => work.push(AstWork::Stmt(stmt)),
                    ast::HandlerArmBody::Block(block) => work.push(AstWork::Block(block)),
                }
            }
        }
        ast::Expr::StageCompose(compose) => {
            for stage in &compose.stages {
                work.push(AstWork::Expr(&stage.expr));
                work.extend(stage.limits.iter().map(AstWork::Expr));
            }
        }
        ast::Expr::Pipeline(pipeline) => {
            work.push(AstWork::Expr(&pipeline.input));
            for stage in &pipeline.stages {
                work.push(AstWork::Expr(&stage.expr));
                work.extend(stage.limits.iter().map(AstWork::Expr));
            }
        }
        ast::Expr::Field(field) => work.push(AstWork::Expr(&field.receiver)),
        ast::Expr::Index(index) => {
            work.push(AstWork::Expr(&index.receiver));
            work.push(AstWork::Expr(&index.index));
        }
        ast::Expr::Slice(slice) => {
            work.push(AstWork::Expr(&slice.receiver));
            work.push(AstWork::Expr(&slice.start));
            work.push(AstWork::Expr(&slice.end));
        }
        ast::Expr::Try(expr) => work.push(AstWork::Expr(&expr.expr)),
        ast::Expr::Unary(expr) => work.push(AstWork::Expr(&expr.expr)),
        ast::Expr::Binary(expr) => {
            work.push(AstWork::Expr(&expr.lhs));
            work.push(AstWork::Expr(&expr.rhs));
        }
        ast::Expr::If(expr) => {
            work.push(AstWork::Expr(&expr.condition));
            work.push(AstWork::Block(&expr.then_branch));
            work.push(AstWork::Block(&expr.else_branch));
        }
        ast::Expr::Match(expr) => {
            work.push(AstWork::Expr(&expr.scrutinee));
            for arm in &expr.arms {
                match &arm.body {
                    ast::MatchArmBody::Expr(expr) => work.push(AstWork::Expr(expr)),
                    ast::MatchArmBody::Block(block) => work.push(AstWork::Block(block)),
                }
            }
        }
        ast::Expr::Lambda(lambda) => match &lambda.body {
            ast::LambdaBody::Expr(expr) => work.push(AstWork::Expr(expr)),
            ast::LambdaBody::Block(block) => work.push(AstWork::Block(block)),
        },
        ast::Expr::Block(block) => work.push(AstWork::Block(&block.block)),
        ast::Expr::Literal(_)
        | ast::Expr::EmptyRecordOrMap { .. }
        | ast::Expr::EmptySequence { .. }
        | ast::Expr::Error(_) => {}
    }
}
